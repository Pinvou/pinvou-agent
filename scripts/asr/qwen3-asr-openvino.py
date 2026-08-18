#!/usr/bin/env python3
"""Qwen3-ASR OpenVINO adapter for pinvou's stable local-ASR CLI.

The desktop invokes this program as:

    qwen3-asr-openvino asr --model <name> --lang <lang> --input <wav>

Inference is deliberately pinned to an OpenVINO GPU device.  There is no
CPU, AUTO, or NPU fallback: a missing GPU is reported as an error so an
installation cannot silently violate its acceleration policy.
"""

from __future__ import annotations

import argparse
from array import array
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import re
import secrets
import socket
import sys
import time
from typing import Iterator
import wave
import xml.etree.ElementTree as ET


MODEL_DIR_ENV = "PINVOU3_QWEN3_ASR_MODEL_DIR"
MODEL_LABEL_ENV = "PINVOU3_QWEN3_ASR_MODEL_LABEL"
DEVICE_ENV = "PINVOU3_QWEN3_ASR_DEVICE"
CACHE_DIR_ENV = "PINVOU3_QWEN3_ASR_CACHE_DIR"
CONTEXT_ENV = "PINVOU3_QWEN3_ASR_CONTEXT"
KV_CACHE_PRECISION_ENV = "PINVOU3_QWEN3_ASR_KV_CACHE_PRECISION"
DEFAULT_DEVICE = "GPU"
DEFAULT_MAX_NEW_TOKENS = 256
CACHE_LOCK_TIMEOUT_SECONDS = 15.0
CACHE_CLEAN_EXIT_MARKER = ".pinvou-qwen3-asr-clean-exit"
SERVICE_ENDPOINT_FILE = ".pinvou-qwen3-asr-service.endpoint"
SERVICE_STATUS_FILE = "service.status.json"
PERFORMANCE_FILE = "latest-performance.json"
PERFORMANCE_HISTORY_FILE = "performance-history.json"
SERVICE_PROTOCOL = "pinvou-qwen3-asr-v1"
SERVICE_AUDIO_PROTOCOL = "pinvou-qwen3-asr-audio-v3"
SERVICE_AUDIO_REQUEST = "AUDIO3"
SERVICE_WARMUP_PROTOCOL = "pinvou-qwen3-asr-warmup-v1"
SERVICE_WARMUP_REQUEST = "WARM2"
SERVICE_WARMUP_IDLE_SECONDS = 30.0
SERVICE_WARMUP_BUCKET_SECONDS = 10
SERVICE_REQUEST_LIMIT = 64 * 1024
SERVICE_AUDIO_LIMIT = 2 * 1024 * 1024
SERVICE_CONTEXT_LIMIT = 32 * 1024
SERVICE_SOCKET_TIMEOUT_SECONDS = 240.0
SERVICE_AUDIO_BUCKET_SECONDS = (10, 20, 40, 60)
SAMPLE_RATE = 16_000
MODEL_MARKERS = (
    "config.json",
    "preprocessor_config.json",
    "openvino_encoder_model.xml",
    "openvino_encoder_model.bin",
    "openvino_decoder_model.xml",
    "openvino_decoder_model.bin",
    "openvino_tokenizer.xml",
    "openvino_tokenizer.bin",
    "openvino_detokenizer.xml",
    "openvino_detokenizer.bin",
)
LANGUAGE_NAMES = {
    "zh": "Chinese",
    "zh-cn": "Chinese",
    "zh-hans": "Chinese",
    "chinese": "Chinese",
    "en": "English",
    "en-us": "English",
    "english": "English",
    "ja": "Japanese",
    "ja-jp": "Japanese",
    "japanese": "Japanese",
    "ko": "Korean",
    "ko-kr": "Korean",
    "korean": "Korean",
}


class AdapterError(RuntimeError):
    """A user-actionable ASR adapter failure."""


def configure_stdio() -> None:
    """Keep redirected Windows output readable by Rust's UTF-8 pipes."""

    for stream in (sys.stdout, sys.stderr):
        if stream is None:
            continue
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8", errors="replace")


def emit_stderr(message: str) -> None:
    if sys.stderr is not None:
        print(message, file=sys.stderr, flush=True)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def summarize_milliseconds(raw_values) -> dict[str, object]:
    values_ms = [float(value) for value in raw_values]
    if not values_ms:
        return {"count": 0, "values_ms": []}
    ordered = sorted(values_ms)

    def percentile(fraction: float) -> float:
        position = (len(ordered) - 1) * fraction
        lower = int(position)
        upper = min(lower + 1, len(ordered) - 1)
        weight = position - lower
        return ordered[lower] * (1.0 - weight) + ordered[upper] * weight

    return {
        "count": len(values_ms),
        "total_ms": round(sum(values_ms), 6),
        "mean_ms": round(sum(values_ms) / len(values_ms), 6),
        "p50_ms": round(percentile(0.50), 6),
        "p95_ms": round(percentile(0.95), 6),
        "max_ms": round(max(values_ms), 6),
        "values_ms": [round(value, 6) for value in values_ms],
    }


def summarize_microseconds(raw_values) -> dict[str, object]:
    return summarize_milliseconds(float(value) / 1000.0 for value in raw_values)


def summarize_timestamp_intervals(raw_values) -> dict[str, object]:
    timestamps = [float(value) for value in raw_values]
    # Python bindings expose PerfMetrics' clock timestamps in milliseconds,
    # while individual duration collections are expressed in microseconds.
    return summarize_milliseconds(
        [timestamps[index] - timestamps[index - 1] for index in range(1, len(timestamps))]
    )


def write_json_atomic(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    try:
        temporary.write_text(
            json.dumps(payload, ensure_ascii=False, sort_keys=True, indent=2),
            encoding="utf-8",
        )
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def write_json_best_effort(path: Path, payload: dict[str, object]) -> None:
    try:
        write_json_atomic(path, payload)
    except OSError as exc:
        emit_stderr(f"[qwen3-asr] telemetry_write_failed={exc}")


def append_performance_history(directory: Path, payload: dict[str, object]) -> None:
    path = directory / PERFORMANCE_HISTORY_FILE
    history: list[dict[str, object]] = []
    try:
        if path.is_file():
            parsed = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(parsed, list):
                history = [item for item in parsed if isinstance(item, dict)]
    except (OSError, ValueError):
        history = []
    history.append(payload)
    write_json_best_effort(path, history[-50:])


def adapter_root() -> Path:
    return Path(__file__).resolve().parent


def model_dir() -> Path:
    configured = os.environ.get(MODEL_DIR_ENV, "").strip()
    return Path(configured).expanduser() if configured else adapter_root() / "model"


def cache_dir() -> Path:
    configured = os.environ.get(CACHE_DIR_ENV, "").strip()
    return Path(configured).expanduser() if configured else adapter_root() / "cache"


def try_lock(handle) -> bool:
    if os.name == "nt":
        import msvcrt

        handle.seek(0)
        try:
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            return True
        except OSError as exc:
            if exc.errno in {13, 36}:
                return False
            raise

    import fcntl

    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except BlockingIOError:
        return False


def unlock(handle) -> None:
    if os.name == "nt":
        import msvcrt

        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        return

    import fcntl

    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def quarantine_incomplete_cache_files(directory: Path) -> list[Path]:
    quarantined: list[Path] = []
    clean_exit_marker = directory / CACHE_CLEAN_EXIT_MARKER
    try:
        clean_exit_time_ns = clean_exit_marker.stat().st_mtime_ns
    except FileNotFoundError:
        clean_exit_time_ns = None
    for pattern in ("*.cl_cache", "*.blob"):
        for candidate in directory.glob(pattern):
            try:
                metadata = candidate.stat()
                if not candidate.is_file() or metadata.st_size != 0:
                    continue
                if (
                    clean_exit_time_ns is not None
                    and metadata.st_mtime_ns <= clean_exit_time_ns
                ):
                    continue
                destination = candidate.with_name(f"{candidate.name}.incomplete")
                candidate.replace(destination)
                quarantined.append(destination)
            except FileNotFoundError:
                continue
    return quarantined


@contextmanager
def exclusive_cache_session(directory: Path) -> Iterator[list[Path]]:
    directory.mkdir(parents=True, exist_ok=True)
    lock_path = directory / ".pinvou-qwen3-asr.lock"
    with lock_path.open("a+b") as handle:
        handle.seek(0, os.SEEK_END)
        if handle.tell() == 0:
            handle.write(b"\0")
            handle.flush()
        deadline = time.monotonic() + CACHE_LOCK_TIMEOUT_SECONDS
        while not try_lock(handle):
            if time.monotonic() >= deadline:
                raise AdapterError(
                    "another Qwen3-ASR process still owns the GPU cache lock"
                )
            time.sleep(0.1)
        try:
            yield quarantine_incomplete_cache_files(directory)
        finally:
            unlock(handle)


def requested_device() -> str:
    device = os.environ.get(DEVICE_ENV, DEFAULT_DEVICE).strip().upper()
    if not re.fullmatch(r"GPU(?:\.\d+)?", device):
        raise AdapterError(
            f"{DEVICE_ENV} must name an OpenVINO GPU device (GPU or GPU.<index>); got {device!r}"
        )
    return device


def optional_kv_cache_precision() -> str | None:
    """Resolve the opt-in KV-cache experiment without changing stable defaults."""

    precision = os.environ.get(KV_CACHE_PRECISION_ENV, "").strip().lower()
    if not precision:
        return None
    if precision not in {"f16", "u8"}:
        raise AdapterError(
            f"{KV_CACHE_PRECISION_ENV} must be empty, f16, or u8; got {precision!r}"
        )
    return precision


def validate_model(directory: Path) -> None:
    missing = [name for name in MODEL_MARKERS if not (directory / name).is_file()]
    if missing:
        raise AdapterError(
            f"Qwen3-ASR OpenVINO model is incomplete at {directory}: missing {', '.join(missing)}"
        )
    try:
        config = json.loads((directory / "config.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise AdapterError(f"cannot read Qwen3-ASR config.json: {exc}") from exc
    if config.get("model_type") != "qwen3_asr":
        raise AdapterError(
            f"expected config.json model_type=qwen3_asr, got {config.get('model_type')!r}"
        )


def read_weight_compression(model_xml: Path) -> dict[str, object]:
    """Read the export-time NNCF compression metadata from an OpenVINO IR."""

    try:
        root = ET.parse(model_xml).getroot()
    except (OSError, ET.ParseError) as exc:
        raise AdapterError(f"cannot inspect OpenVINO model metadata in {model_xml}: {exc}") from exc

    compression = root.find("./rt_info/nncf/weight_compression")
    if compression is None:
        return {"present": False}

    metadata: dict[str, object] = {"present": True}
    for key in ("mode", "backup_mode", "group_size", "ratio", "awq"):
        node = compression.find(key)
        if node is None or "value" not in node.attrib:
            continue
        value: object = node.attrib["value"]
        if key == "group_size":
            try:
                value = int(str(value))
            except ValueError:
                pass
        elif key == "ratio":
            try:
                value = float(str(value))
            except ValueError:
                pass
        elif key == "awq":
            value = str(value).lower() == "true"
        metadata[key] = value
    return metadata


def inspect_model_identity(directory: Path) -> dict[str, object]:
    """Return cheap, deterministic model identity fields for A/B telemetry."""

    components: dict[str, object] = {}
    for component in ("encoder", "decoder"):
        xml_path = directory / f"openvino_{component}_model.xml"
        bin_path = directory / f"openvino_{component}_model.bin"
        components[component] = {
            "bin_bytes": bin_path.stat().st_size,
            "weight_compression": read_weight_compression(xml_path),
        }

    configured_label = os.environ.get(MODEL_LABEL_ENV, "").strip()
    decoder_compression = components["decoder"]["weight_compression"]
    if configured_label:
        label = configured_label
    elif isinstance(decoder_compression, dict) and decoder_compression.get("mode"):
        mode = str(decoder_compression["mode"]).upper().replace("_", "-")
        ratio = decoder_compression.get("ratio")
        group_size = decoder_compression.get("group_size")
        suffix = ""
        if ratio not in (None, 1, 1.0):
            suffix += f"-R{ratio}"
        if group_size not in (None, -1):
            suffix += f"-G{group_size}"
        label = f"Qwen3-ASR-0.6B-{mode}{suffix}"
    else:
        label = "Qwen3-ASR-0.6B-UNKNOWN"

    return {
        "label": label,
        "total_core_bin_bytes": sum(
            int(component["bin_bytes"])
            for component in components.values()
            if isinstance(component, dict)
        ),
        "components": components,
    }


def inspect_gpu(device: str):
    try:
        import openvino as ov
    except ImportError as exc:
        raise AdapterError(
            "OpenVINO is not installed; install the pinned openvino-genai runtime"
        ) from exc

    core = ov.Core()
    available = tuple(str(item).upper() for item in core.available_devices)
    if not any(item == "GPU" or item.startswith("GPU.") for item in available):
        raise AdapterError(
            f"OpenVINO exposes no GPU device (available: {', '.join(available) or 'none'})"
        )
    try:
        full_name = str(core.get_property(device, "FULL_DEVICE_NAME"))
    except Exception as exc:
        raise AdapterError(f"OpenVINO cannot resolve requested device {device}: {exc}") from exc
    if not full_name.strip():
        raise AdapterError(f"OpenVINO returned an empty FULL_DEVICE_NAME for {device}")
    return core, full_name


def read_pcm16_wav_source(source, label: str) -> list[float]:
    try:
        with wave.open(source, "rb") as wav:
            channels = wav.getnchannels()
            sample_width = wav.getsampwidth()
            sample_rate = wav.getframerate()
            compression = wav.getcomptype()
            frame_count = wav.getnframes()
            payload = wav.readframes(frame_count)
    except (OSError, EOFError, wave.Error) as exc:
        raise AdapterError(f"cannot read input WAV {label}: {exc}") from exc

    if compression != "NONE" or sample_width != 2:
        raise AdapterError("input WAV must be uncompressed 16-bit PCM")
    if channels < 1:
        raise AdapterError("input WAV contains no audio channel")
    if sample_rate != SAMPLE_RATE:
        raise AdapterError(
            f"input WAV must be {SAMPLE_RATE} Hz; pinvou records at 16 kHz but received {sample_rate} Hz"
        )

    samples = array("h")
    samples.frombytes(payload)
    if sys.byteorder == "big":
        samples.byteswap()
    if channels > 1:
        samples = array(
            "h",
            (
                round(sum(samples[index : index + channels]) / channels)
                for index in range(0, len(samples), channels)
            ),
        )
    if not samples:
        raise AdapterError("input WAV contains no audio samples")
    return [sample / 32768.0 for sample in samples]


def read_pcm16_wav(path: Path) -> list[float]:
    return read_pcm16_wav_source(str(path), str(path))


def read_pcm16_wav_bytes(payload: bytes) -> list[float]:
    return read_pcm16_wav_source(io.BytesIO(payload), "from resident client bytes")


def bucket_service_audio(
    audio: list[float],
) -> tuple[list[float], int | None, int]:
    """Pad resident-service input to a pre-warmed shape without truncation."""

    for bucket_seconds in SERVICE_AUDIO_BUCKET_SECONDS:
        target_samples = bucket_seconds * SAMPLE_RATE
        if len(audio) <= target_samples:
            padding_samples = target_samples - len(audio)
            if padding_samples == 0:
                return audio, bucket_seconds, 0
            return audio + [0.0] * padding_samples, bucket_seconds, padding_samples
    return audio, None, 0


def language_name(raw: str) -> str | None:
    normalized = raw.strip().lower().replace("_", "-")
    if normalized in {"", "auto", "none"}:
        return None
    return LANGUAGE_NAMES.get(normalized, raw.strip())


def check_runtime() -> dict[str, object]:
    directory = model_dir().resolve()
    validate_model(directory)
    identity = inspect_model_identity(directory)
    device = requested_device()
    kv_cache_precision = optional_kv_cache_precision()
    _, full_name = inspect_gpu(device)
    return {
        "backend": "qwen3-asr-openvino",
        "model": identity["label"],
        "model_identity": identity,
        "model_dir": str(directory),
        "device": device,
        "full_device_name": full_name,
        "kv_cache_precision": kv_cache_precision or "openvino-default",
    }


def load_pipeline(runtime: dict[str, object], compiled_cache: Path):
    try:
        import openvino_genai
    except ImportError as exc:
        raise AdapterError(
            "openvino-genai is not installed in the configured Python runtime"
        ) from exc

    load_started = time.perf_counter()
    pipeline_options = {"CACHE_DIR": str(compiled_cache)}
    if runtime["kv_cache_precision"] != "openvino-default":
        pipeline_options["KV_CACHE_PRECISION"] = runtime["kv_cache_precision"]
    pipeline = openvino_genai.ASRPipeline(
        Path(str(runtime["model_dir"])),
        str(runtime["device"]),
        **pipeline_options,
    )
    return pipeline, time.perf_counter() - load_started


def generate_text(
    pipeline,
    audio: list[float],
    lang: str,
    max_new_tokens: int,
    *,
    require_text: bool = True,
    timing: dict[str, object] | None = None,
    context_override: str | None = None,
) -> tuple[str, float]:
    config = pipeline.get_generation_config()
    config.max_new_tokens = max_new_tokens
    forced_language = language_name(lang)
    if forced_language:
        config.language = forced_language
    context = (
        os.environ.get(CONTEXT_ENV, "")
        if context_override is None
        else context_override
    ).strip()
    # GenerationConfig objects may be reused by the resident pipeline. Always
    # assign the request snapshot, including the empty string, so an older
    # context cannot leak into a later recognition request.
    config.context = context

    if timing is not None:
        timing["gpu_inference_started_at"] = utc_now()
    infer_started = time.perf_counter()
    result = pipeline.generate(audio, config)
    infer_seconds = time.perf_counter() - infer_started
    if timing is not None:
        timing["gpu_inference_completed_at"] = utc_now()
        timing["gpu_inference_seconds"] = round(infer_seconds, 6)
    texts = list(result.texts)
    text = texts[0].strip() if texts else ""
    if timing is not None:
        perf = result.perf_metrics

        def metric_mean(name: str) -> float | None:
            try:
                return round(float(getattr(perf, name)().mean), 6)
            except (AttributeError, RuntimeError, TypeError, ValueError):
                return None

        def metric_count(name: str) -> int | None:
            try:
                return int(getattr(perf, name)())
            except (AttributeError, RuntimeError, TypeError, ValueError):
                return None

        def raw_values(container, name: str) -> list[float]:
            try:
                return [float(value) for value in getattr(container, name)]
            except (AttributeError, RuntimeError, TypeError, ValueError):
                return []

        generated_tokens = metric_count("get_num_generated_tokens")
        timing["model_metrics"] = {
            "text_chars": len(text),
            "input_tokens": metric_count("get_num_input_tokens"),
            "generated_tokens": generated_tokens,
            "features_extraction_ms": metric_mean("get_features_extraction_duration"),
            "encoder_inference_ms": metric_mean("get_encode_inference_duration"),
            "decoder_inference_ms": metric_mean("get_decode_inference_duration"),
            "ttft_ms": metric_mean("get_ttft"),
            "tpot_ms": metric_mean("get_tpot"),
            "throughput_tokens_per_second": metric_mean("get_throughput"),
            "tokenization_ms": metric_mean("get_tokenization_duration"),
            "detokenization_ms": metric_mean("get_detokenization_duration"),
            "sampling_ms": metric_mean("get_sampling_duration"),
            "max_new_tokens_reached": generated_tokens == max_new_tokens,
        }
        raw = perf.raw_metrics
        asr_raw = perf.asr_raw_metrics
        timing["raw_model_metrics"] = {
            "features_extraction": summarize_microseconds(
                raw_values(asr_raw, "features_extraction_durations")
            ),
            "encoder_inference": summarize_microseconds(
                raw_values(asr_raw, "encode_inference_durations")
            ),
            "decoder_inference_per_step": summarize_microseconds(
                raw_values(asr_raw, "decode_inference_durations")
            ),
            "token_inference_per_step": summarize_microseconds(
                raw_values(raw, "token_infer_durations")
            ),
            "new_token_intervals": summarize_timestamp_intervals(
                raw_values(raw, "m_new_token_times")
            ),
            "sampling_per_step": summarize_microseconds(
                raw_values(raw, "sampling_durations")
            ),
            "tokenization": summarize_microseconds(
                raw_values(raw, "tokenization_durations")
            ),
            "detokenization": summarize_microseconds(
                raw_values(raw, "detokenization_durations")
            ),
            "time_to_first_token": summarize_microseconds(
                raw_values(raw, "m_times_to_first_token")
            ),
            "batch_sizes": [
                int(value) for value in raw_values(raw, "m_batch_sizes")
            ],
        }
    if require_text and not text:
        raise AdapterError("Qwen3-ASR returned an empty transcription")
    return text, infer_seconds


def performance_payload(
    *,
    mode: str,
    runtime: dict[str, object],
    audio: list[float],
    inference_audio: list[float] | None = None,
    max_new_tokens: int,
    load_seconds: float,
    infer_seconds: float,
    total_seconds: float,
    started_at: str,
) -> dict[str, object]:
    inference_samples = len(inference_audio) if inference_audio is not None else len(audio)
    return {
        "schema": 1,
        "mode": mode,
        "process_id": os.getpid(),
        "started_at": started_at,
        "completed_at": utc_now(),
        "audio_seconds": round(len(audio) / SAMPLE_RATE, 3),
        "inference_audio_seconds": round(inference_samples / SAMPLE_RATE, 3),
        "max_new_tokens": max_new_tokens,
        "model": runtime["model"],
        "model_dir": runtime["model_dir"],
        "model_identity": runtime["model_identity"],
        "device": runtime["device"],
        "full_device_name": runtime["full_device_name"],
        "kv_cache_precision": runtime["kv_cache_precision"],
        "load_seconds": round(load_seconds, 3),
        "infer_seconds": round(infer_seconds, 3),
        "total_seconds": round(total_seconds, 3),
    }


def transcribe(input_path: Path, lang: str, max_new_tokens: int) -> str:
    started_at = utc_now()
    total_started = time.perf_counter()
    runtime = check_runtime()
    audio = read_pcm16_wav(input_path)
    compiled_cache = cache_dir().resolve()

    with exclusive_cache_session(compiled_cache) as quarantined:
        if quarantined:
            emit_stderr(
                "[qwen3-asr] quarantined_incomplete_cache="
                + ",".join(path.name for path in quarantined)
            )
        pipeline, load_seconds = load_pipeline(runtime, compiled_cache)
        text, infer_seconds = generate_text(
            pipeline,
            audio,
            lang,
            max_new_tokens,
        )
        (compiled_cache / CACHE_CLEAN_EXIT_MARKER).touch()

    total_seconds = time.perf_counter() - total_started
    write_json_best_effort(
        compiled_cache / PERFORMANCE_FILE,
        performance_payload(
            mode="one-shot",
            runtime=runtime,
            audio=audio,
            max_new_tokens=max_new_tokens,
            load_seconds=load_seconds,
            infer_seconds=infer_seconds,
            total_seconds=total_seconds,
            started_at=started_at,
        ),
    )
    emit_stderr(
        "[qwen3-asr] "
        f"device={runtime['device']} full_device_name={runtime['full_device_name']} "
        f"load_seconds={load_seconds:.3f} infer_seconds={infer_seconds:.3f} "
        f"total_seconds={total_seconds:.3f}"
    )
    return text


def service_status_path(compiled_cache: Path) -> Path:
    return compiled_cache / SERVICE_STATUS_FILE


def service_endpoint_path(compiled_cache: Path) -> Path:
    return compiled_cache / SERVICE_ENDPOINT_FILE


def encode_protocol_field(value: str) -> str:
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def decode_protocol_field(value: str) -> str:
    try:
        return base64.b64decode(value.encode("ascii"), validate=True).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as exc:
        raise AdapterError("invalid UTF-8/base64 field in service request") from exc


def receive_protocol_line_with_remainder(
    connection: socket.socket,
) -> tuple[str, bytes]:
    """Read one bounded UTF-8 header without discarding coalesced body bytes."""

    payload = bytearray()
    while len(payload) <= SERVICE_REQUEST_LIMIT:
        chunk = connection.recv(min(4096, SERVICE_REQUEST_LIMIT + 1 - len(payload)))
        if not chunk:
            break
        payload.extend(chunk)
        newline = payload.find(b"\n")
        if newline >= 0:
            header = bytes(payload[:newline])
            remainder = bytes(payload[newline + 1 :])
            try:
                return header.decode("utf-8"), remainder
            except UnicodeDecodeError as exc:
                raise AdapterError("service request is not valid UTF-8") from exc
    if len(payload) > SERVICE_REQUEST_LIMIT:
        raise AdapterError("service request exceeds the size limit")
    if not payload:
        raise AdapterError("empty service request")
    raise AdapterError("service request header is not newline terminated")


def receive_protocol_line(connection: socket.socket) -> str:
    line, _ = receive_protocol_line_with_remainder(connection)
    return line


def receive_exact_payload(
    connection: socket.socket,
    expected_bytes: int,
    initial: bytes = b"",
) -> bytes:
    if expected_bytes < 0 or expected_bytes > SERVICE_AUDIO_LIMIT:
        raise AdapterError("service audio payload exceeds the size limit")
    if len(initial) > expected_bytes:
        raise AdapterError("service audio payload contains trailing bytes")
    payload = bytearray(initial)
    while len(payload) < expected_bytes:
        chunk = connection.recv(min(64 * 1024, expected_bytes - len(payload)))
        if not chunk:
            raise AdapterError(
                f"service audio payload ended early ({len(payload)}/{expected_bytes} bytes)"
            )
        payload.extend(chunk)
    return bytes(payload)


def read_service_endpoint(compiled_cache: Path) -> dict[str, str]:
    endpoint_path = service_endpoint_path(compiled_cache)
    try:
        endpoint = dict(
            line.split("=", 1)
            for line in endpoint_path.read_text(encoding="utf-8").splitlines()
            if "=" in line
        )
        if endpoint.get("protocol") != SERVICE_PROTOCOL:
            raise ValueError("unsupported protocol")
        port = int(endpoint["port"])
        if port < 1 or port > 65535:
            raise ValueError("invalid port")
        if not endpoint["token"]:
            raise ValueError("empty token")
        return endpoint
    except (OSError, KeyError, ValueError) as exc:
        raise AdapterError(
            f"cannot read resident service endpoint {endpoint_path}: {exc}"
        ) from exc


def parse_service_response(response: str, client_seconds: float) -> dict[str, object]:
    fields = response.split("\t")
    if len(fields) >= 2 and fields[0] == "ERR":
        raise AdapterError(decode_protocol_field(fields[1]))
    if len(fields) < 5 or fields[0] != "OK":
        raise AdapterError("resident service returned an invalid response")
    return {
        "text": decode_protocol_field(fields[1]),
        "load_seconds": float(fields[2]),
        "infer_seconds": float(fields[3]),
        "service_seconds": float(fields[4]),
        "client_seconds": round(client_seconds, 6),
    }


def run_service_client(input_path: Path, lang: str, max_new_tokens: int) -> dict[str, object]:
    """Call a specific resident service for repeatable A/B benchmarks."""

    compiled_cache = cache_dir().resolve()
    endpoint = read_service_endpoint(compiled_cache)
    port = int(endpoint["port"])
    token = endpoint["token"]

    request = "\t".join(
        (
            token,
            encode_protocol_field(str(input_path.resolve())),
            encode_protocol_field(lang),
            str(max_new_tokens),
        )
    ) + "\n"
    started = time.perf_counter()
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=2.0) as connection:
            connection.settimeout(SERVICE_SOCKET_TIMEOUT_SECONDS)
            connection.sendall(request.encode("utf-8"))
            response = receive_protocol_line(connection)
    except OSError as exc:
        raise AdapterError(f"resident service request failed: {exc}") from exc
    client_seconds = time.perf_counter() - started
    return parse_service_response(response, client_seconds)


def run_service_audio_client(
    input_path: Path,
    lang: str,
    max_new_tokens: int,
) -> dict[str, object]:
    """Send WAV bytes directly, matching the optimized Rust desktop path."""

    compiled_cache = cache_dir().resolve()
    endpoint = read_service_endpoint(compiled_cache)
    if endpoint.get("audio_protocol") != SERVICE_AUDIO_PROTOCOL:
        raise AdapterError("resident service does not advertise the audio byte protocol")
    try:
        payload = input_path.read_bytes()
    except OSError as exc:
        raise AdapterError(f"cannot read input WAV {input_path}: {exc}") from exc
    if len(payload) < 44 or len(payload) > SERVICE_AUDIO_LIMIT:
        raise AdapterError(
            f"input WAV byte length must be between 44 and {SERVICE_AUDIO_LIMIT}"
        )

    header = "\t".join(
        (
            SERVICE_AUDIO_REQUEST,
            endpoint["token"],
            encode_protocol_field(lang),
            str(max_new_tokens),
            encode_protocol_field(os.environ.get(CONTEXT_ENV, "").strip()),
            str(len(payload)),
        )
    ) + "\n"
    started = time.perf_counter()
    try:
        with socket.create_connection(
            ("127.0.0.1", int(endpoint["port"])), timeout=2.0
        ) as connection:
            connection.settimeout(SERVICE_SOCKET_TIMEOUT_SECONDS)
            connection.sendall(header.encode("utf-8"))
            connection.sendall(payload)
            response = receive_protocol_line(connection)
    except OSError as exc:
        raise AdapterError(f"resident audio service request failed: {exc}") from exc
    return parse_service_response(response, time.perf_counter() - started)


def run_service_prewarm_client(bucket_seconds: int) -> dict[str, object]:
    compiled_cache = cache_dir().resolve()
    endpoint = read_service_endpoint(compiled_cache)
    if endpoint.get("warmup_protocol") != SERVICE_WARMUP_PROTOCOL:
        raise AdapterError("resident service does not advertise guarded prewarm")
    if bucket_seconds not in SERVICE_AUDIO_BUCKET_SECONDS:
        raise AdapterError("unsupported warmup audio bucket")
    request = (
        f"{SERVICE_WARMUP_REQUEST}\t{endpoint['token']}\t{bucket_seconds}\n"
    )
    started = time.perf_counter()
    try:
        with socket.create_connection(
            ("127.0.0.1", int(endpoint["port"])), timeout=2.0
        ) as connection:
            connection.settimeout(15.0)
            connection.sendall(request.encode("utf-8"))
            response = receive_protocol_line(connection)
    except OSError as exc:
        raise AdapterError(f"resident prewarm request failed: {exc}") from exc
    fields = response.split("\t")
    if fields[0] == "ERR":
        message = decode_protocol_field(fields[1]) if len(fields) > 1 else "unknown"
        raise AdapterError(message)
    if fields[0] not in ("WARMED", "SKIPPED"):
        raise AdapterError("invalid resident prewarm response")
    return {
        "warmed": fields[0] == "WARMED",
        "status": fields[0].lower(),
        "detail": fields[1:] if len(fields) > 1 else [],
        "client_seconds": round(time.perf_counter() - started, 3),
    }


def service_warmup_audio() -> list[float] | None:
    sample_path = adapter_root() / "samples" / "qwen3-asr-official-zh.wav"
    if not sample_path.is_file():
        return None
    return read_pcm16_wav(sample_path)


def normalize_service_process_priority() -> str:
    """Undo Task Scheduler's BelowNormal default for latency-sensitive ASR."""

    if os.name != "nt":
        return "platform-default"
    try:
        import ctypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetCurrentProcess.restype = ctypes.c_void_p
        kernel32.SetPriorityClass.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel32.SetPriorityClass.restype = ctypes.c_int
        normal_priority_class = 0x00000020
        if not kernel32.SetPriorityClass(
            kernel32.GetCurrentProcess(), normal_priority_class
        ):
            raise OSError(ctypes.get_last_error(), "SetPriorityClass failed")
        return "normal"
    except Exception as exc:
        emit_stderr(f"[qwen3-asr] warning=process_priority_normalization_failed error={exc}")
        return "unchanged"


def serve_prewarm_request(
    connection: socket.socket,
    *,
    accepted_at: str,
    accepted_perf: float,
    fields: list[str],
    buffered_payload: bytes,
    token: str,
    pipeline,
    runtime: dict[str, object],
    warmup_audio: list[float] | None,
    seconds_since_inference: float,
) -> dict[str, object]:
    """Refresh GPU clocks/compiled state while the user is still recording."""

    if len(fields) != 3:
        raise AdapterError("invalid Qwen3-ASR warmup request")
    _, supplied_token, raw_bucket_seconds = fields
    if buffered_payload:
        raise AdapterError("warmup request contains unexpected trailing bytes")
    if not secrets.compare_digest(token, supplied_token):
        raise AdapterError("Qwen3-ASR service authentication failed")
    try:
        bucket_seconds = int(raw_bucket_seconds)
    except ValueError as exc:
        raise AdapterError("invalid warmup audio bucket") from exc
    if bucket_seconds not in SERVICE_AUDIO_BUCKET_SECONDS:
        raise AdapterError("unsupported warmup audio bucket")

    metrics: dict[str, object] = {
        "schema": 1,
        "mode": "resident-service-prewarm",
        "started_at": accepted_at,
        "device": runtime["device"],
        "audio_bucket_seconds": bucket_seconds,
        "idle_before_request_seconds": round(seconds_since_inference, 3),
    }
    if seconds_since_inference < SERVICE_WARMUP_IDLE_SECONDS:
        metrics["skipped"] = "recent-inference"
        metrics["completed_at"] = utc_now()
        metrics["connection_seconds"] = round(
            time.perf_counter() - accepted_perf,
            6,
        )
        connection.sendall(
            f"SKIPPED\trecent-inference\t{seconds_since_inference:.3f}\n".encode(
                "utf-8"
            )
        )
        return metrics

    target_samples = bucket_seconds * SAMPLE_RATE
    inference_audio = (warmup_audio or [])[:target_samples]
    if len(inference_audio) < target_samples:
        inference_audio += [0.0] * (target_samples - len(inference_audio))
    generate_started = time.perf_counter()
    _, infer_seconds = generate_text(
        pipeline,
        inference_audio,
        "zh",
        DEFAULT_MAX_NEW_TOKENS,
        require_text=False,
    )
    metrics["infer_seconds"] = round(infer_seconds, 3)
    metrics["generate_seconds"] = round(
        time.perf_counter() - generate_started,
        6,
    )
    metrics["completed_at"] = utc_now()
    metrics["connection_seconds"] = round(
        time.perf_counter() - accepted_perf,
        6,
    )
    connection.sendall(
        (
            f"WARMED\t{infer_seconds:.3f}\t"
            f"{float(metrics['connection_seconds']):.3f}\n"
        ).encode("utf-8")
    )
    return metrics


def serve_connection(
    connection: socket.socket,
    *,
    accepted_at: str,
    accepted_perf: float,
    token: str,
    pipeline,
    runtime: dict[str, object],
    compiled_cache: Path,
    load_seconds: float,
    warmup_audio: list[float] | None,
    seconds_since_inference: float,
) -> tuple[str, dict[str, object]]:
    timing: dict[str, object] = {
        "connection_accepted_at": accepted_at,
        "request_receive_started_at": accepted_at,
    }
    connection.settimeout(SERVICE_SOCKET_TIMEOUT_SECONDS)
    request_line, buffered_payload = receive_protocol_line_with_remainder(connection)
    fields = request_line.split("\t")
    timing["request_receive_header_completed_at"] = utc_now()
    timing["request_receive_header_seconds"] = round(
        time.perf_counter() - accepted_perf,
        6,
    )

    if fields and fields[0] == SERVICE_WARMUP_REQUEST:
        return (
            "prewarm",
            serve_prewarm_request(
                connection,
                accepted_at=accepted_at,
                accepted_perf=accepted_perf,
                fields=fields,
                buffered_payload=buffered_payload,
                token=token,
                pipeline=pipeline,
                runtime=runtime,
                warmup_audio=warmup_audio,
                seconds_since_inference=seconds_since_inference,
            ),
        )

    input_path: Path | None = None
    wav_payload: bytes | None = None
    request_context: str | None = None
    if len(fields) == 6 and fields[0] == SERVICE_AUDIO_REQUEST:
        (
            _,
            supplied_token,
            encoded_lang,
            raw_max_new_tokens,
            encoded_context,
            raw_audio_bytes,
        ) = fields
        timing["request_transport"] = SERVICE_AUDIO_PROTOCOL
        if not secrets.compare_digest(token, supplied_token):
            raise AdapterError("Qwen3-ASR service authentication failed")
        try:
            audio_bytes = int(raw_audio_bytes)
        except ValueError as exc:
            raise AdapterError("invalid service audio byte length") from exc
        if audio_bytes < 44 or audio_bytes > SERVICE_AUDIO_LIMIT:
            raise AdapterError(
                f"service audio byte length must be between 44 and {SERVICE_AUDIO_LIMIT}"
            )
        request_context = decode_protocol_field(encoded_context).strip()
        if len(request_context.encode("utf-8")) > SERVICE_CONTEXT_LIMIT:
            raise AdapterError(
                f"service context exceeds {SERVICE_CONTEXT_LIMIT} UTF-8 bytes"
            )
        wav_payload = receive_exact_payload(connection, audio_bytes, buffered_payload)
    elif len(fields) == 4:
        supplied_token, encoded_input, encoded_lang, raw_max_new_tokens = fields
        timing["request_transport"] = SERVICE_PROTOCOL
        if buffered_payload:
            raise AdapterError("path service request contains unexpected trailing bytes")
        if not secrets.compare_digest(token, supplied_token):
            raise AdapterError("Qwen3-ASR service authentication failed")
        input_path = Path(decode_protocol_field(encoded_input)).resolve()
    else:
        raise AdapterError("invalid Qwen3-ASR service request")

    timing["request_receive_completed_at"] = utc_now()
    timing["request_receive_seconds"] = round(time.perf_counter() - accepted_perf, 6)
    validation_started = time.perf_counter()
    timing["request_validation_started_at"] = utc_now()
    lang = decode_protocol_field(encoded_lang)
    try:
        max_new_tokens = int(raw_max_new_tokens)
    except ValueError as exc:
        raise AdapterError("invalid max_new_tokens in service request") from exc
    if max_new_tokens <= 0 or max_new_tokens > 256:
        raise AdapterError("service max_new_tokens must be between 1 and 256")
    if input_path is not None and not input_path.is_file():
        raise AdapterError(f"input WAV does not exist: {input_path}")
    timing["request_validation_completed_at"] = utc_now()
    timing["request_validation_seconds"] = round(
        time.perf_counter() - validation_started,
        6,
    )

    started_at = utc_now()
    total_started = time.perf_counter()
    timing["wav_read_started_at"] = utc_now()
    wav_read_started = time.perf_counter()
    audio = (
        read_pcm16_wav_bytes(wav_payload)
        if wav_payload is not None
        else read_pcm16_wav(input_path)
    )
    timing["wav_read_completed_at"] = utc_now()
    timing["wav_read_seconds"] = round(time.perf_counter() - wav_read_started, 6)
    timing["audio_bucket_started_at"] = utc_now()
    audio_bucket_started = time.perf_counter()
    inference_audio, audio_bucket_seconds, padding_samples = bucket_service_audio(audio)
    timing["audio_bucket_completed_at"] = utc_now()
    timing["audio_bucket_seconds"] = round(
        time.perf_counter() - audio_bucket_started,
        6,
    )
    timing["input_audio_seconds"] = round(len(audio) / SAMPLE_RATE, 3)
    timing["inference_audio_seconds"] = round(len(inference_audio) / SAMPLE_RATE, 3)
    timing["selected_audio_bucket_seconds"] = audio_bucket_seconds
    timing["audio_padding_seconds"] = round(padding_samples / SAMPLE_RATE, 3)
    timing["generate_started_at"] = utc_now()
    generate_started = time.perf_counter()
    text, infer_seconds = generate_text(
        pipeline,
        inference_audio,
        lang,
        max_new_tokens,
        timing=timing,
        context_override=request_context,
    )
    timing["generate_completed_at"] = utc_now()
    timing["generate_seconds"] = round(time.perf_counter() - generate_started, 6)
    timing["cache_marker_started_at"] = utc_now()
    cache_marker_started = time.perf_counter()
    (compiled_cache / CACHE_CLEAN_EXIT_MARKER).touch()
    timing["cache_marker_completed_at"] = utc_now()
    timing["cache_marker_seconds"] = round(
        time.perf_counter() - cache_marker_started,
        6,
    )
    total_seconds = time.perf_counter() - total_started
    metrics = performance_payload(
        mode="resident-service",
        runtime=runtime,
        audio=audio,
        inference_audio=inference_audio,
        max_new_tokens=max_new_tokens,
        load_seconds=load_seconds,
        infer_seconds=infer_seconds,
        total_seconds=total_seconds,
        started_at=started_at,
    )
    metrics["timing"] = timing
    metrics["context_chars"] = len(request_context or "")
    timing["response_encode_started_at"] = utc_now()
    response_encode_started = time.perf_counter()
    response = "\t".join(
        (
            "OK",
            encode_protocol_field(text),
            f"{load_seconds:.3f}",
            f"{infer_seconds:.3f}",
            f"{total_seconds:.3f}",
        )
    ) + "\n"
    timing["response_encode_completed_at"] = utc_now()
    timing["response_encode_seconds"] = round(
        time.perf_counter() - response_encode_started,
        6,
    )
    timing["response_send_started_at"] = utc_now()
    response_send_started = time.perf_counter()
    connection.sendall(response.encode("utf-8"))
    timing["response_send_completed_at"] = utc_now()
    timing["response_send_seconds"] = round(
        time.perf_counter() - response_send_started,
        6,
    )
    timing["connection_completed_at"] = utc_now()
    timing["connection_seconds"] = round(time.perf_counter() - accepted_perf, 6)
    write_json_best_effort(compiled_cache / PERFORMANCE_FILE, metrics)
    append_performance_history(compiled_cache, metrics)
    return "transcribe", metrics


def serve_forever() -> int:
    process_priority = normalize_service_process_priority()
    runtime = check_runtime()
    compiled_cache = cache_dir().resolve()
    compiled_cache.mkdir(parents=True, exist_ok=True)
    endpoint_path = service_endpoint_path(compiled_cache)
    status_path = service_status_path(compiled_cache)
    try:
        endpoint_path.unlink()
    except FileNotFoundError:
        pass

    status: dict[str, object] = {
        "schema": 1,
        "protocol": SERVICE_PROTOCOL,
        "audio_protocol": SERVICE_AUDIO_PROTOCOL,
        "warmup_protocol": SERVICE_WARMUP_PROTOCOL,
        "warmup_bucket_seconds": SERVICE_WARMUP_BUCKET_SECONDS,
        "max_audio_bytes": SERVICE_AUDIO_LIMIT,
        "process_priority": process_priority,
        "status": "starting",
        "process_id": os.getpid(),
        "started_at": utc_now(),
        "model": runtime["model"],
        "model_dir": runtime["model_dir"],
        "model_identity": runtime["model_identity"],
        "device": runtime["device"],
        "full_device_name": runtime["full_device_name"],
        "kv_cache_precision": runtime["kv_cache_precision"],
        "request_count": 0,
    }
    write_json_atomic(status_path, status)

    server: socket.socket | None = None
    try:
        with exclusive_cache_session(compiled_cache) as quarantined:
            if quarantined:
                status["quarantined_cache_files"] = [path.name for path in quarantined]
            pipeline, load_seconds = load_pipeline(runtime, compiled_cache)
            status["load_seconds"] = round(load_seconds, 3)

            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", 0))
            server.listen(4)
            port = int(server.getsockname()[1])
            token = secrets.token_urlsafe(32)
            endpoint = (
                f"protocol={SERVICE_PROTOCOL}\n"
                f"audio_protocol={SERVICE_AUDIO_PROTOCOL}\n"
                f"warmup_protocol={SERVICE_WARMUP_PROTOCOL}\n"
                f"warmup_bucket_seconds={SERVICE_WARMUP_BUCKET_SECONDS}\n"
                f"max_audio_bytes={SERVICE_AUDIO_LIMIT}\n"
                f"port={port}\n"
                f"token={token}\n"
                f"process_id={os.getpid()}\n"
            )
            temporary = endpoint_path.with_name(f"{endpoint_path.name}.{os.getpid()}.tmp")
            temporary.write_text(endpoint, encoding="utf-8")
            os.replace(temporary, endpoint_path)

            status["status"] = "warming"
            status["port"] = port
            write_json_atomic(status_path, status)
            warmup_audio = service_warmup_audio()
            if warmup_audio is not None:
                warmup_started = time.perf_counter()
                warmup_buckets: list[dict[str, object]] = []
                for bucket_seconds in SERVICE_AUDIO_BUCKET_SECONDS:
                    target_samples = bucket_seconds * SAMPLE_RATE
                    bucket_audio = warmup_audio[:target_samples]
                    if len(bucket_audio) < target_samples:
                        bucket_audio += [0.0] * (target_samples - len(bucket_audio))
                    bucket_started = time.perf_counter()
                    _, warmup_infer_seconds = generate_text(
                        pipeline,
                        bucket_audio,
                        "zh",
                        DEFAULT_MAX_NEW_TOKENS,
                        require_text=False,
                    )
                    warmup_buckets.append(
                        {
                            "audio_seconds": bucket_seconds,
                            "infer_seconds": round(warmup_infer_seconds, 3),
                            "total_seconds": round(
                                time.perf_counter() - bucket_started,
                                3,
                            ),
                        }
                    )
                (compiled_cache / CACHE_CLEAN_EXIT_MARKER).touch()
                status["warmup_source_audio_seconds"] = round(
                    len(warmup_audio) / SAMPLE_RATE,
                    3,
                )
                status["warmup_buckets"] = warmup_buckets
                status["warmup_total_seconds"] = round(
                    time.perf_counter() - warmup_started,
                    3,
                )
            else:
                status["warmup_skipped"] = "sample_missing"
            status["status"] = "ready"
            status["ready_at"] = utc_now()
            write_json_atomic(status_path, status)
            last_inference_perf = time.perf_counter()

            while True:
                connection, _ = server.accept()
                accepted_at = utc_now()
                accepted_perf = time.perf_counter()
                with connection:
                    try:
                        request_kind, metrics = serve_connection(
                            connection,
                            accepted_at=accepted_at,
                            accepted_perf=accepted_perf,
                            token=token,
                            pipeline=pipeline,
                            runtime=runtime,
                            compiled_cache=compiled_cache,
                            load_seconds=load_seconds,
                            warmup_audio=warmup_audio,
                            seconds_since_inference=(
                                time.perf_counter() - last_inference_perf
                            ),
                        )
                        if request_kind == "prewarm":
                            status["warmup_request_count"] = (
                                int(status.get("warmup_request_count", 0)) + 1
                            )
                            status["last_prewarm"] = metrics
                            if "skipped" not in metrics:
                                last_inference_perf = time.perf_counter()
                        else:
                            status["request_count"] = int(status["request_count"]) + 1
                            status["last_request"] = metrics
                            last_inference_perf = time.perf_counter()
                        status.pop("last_error", None)
                    except Exception as exc:
                        status["last_error"] = str(exc)
                        try:
                            error = "ERR\t" + encode_protocol_field(str(exc)) + "\n"
                            connection.sendall(error.encode("utf-8"))
                        except OSError:
                            pass
                    status["status"] = "ready"
                    write_json_best_effort(status_path, status)
    except Exception as exc:
        status["status"] = "failed"
        status["failed_at"] = utc_now()
        status["error"] = str(exc)
        write_json_best_effort(status_path, status)
        raise
    finally:
        if server is not None:
            server.close()
        try:
            endpoint_path.unlink()
        except FileNotFoundError:
            pass


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="pinvou Qwen3-ASR OpenVINO GPU adapter")
    parser.add_argument("--version", action="version", version="qwen3-asr-openvino 1.0")
    subparsers = parser.add_subparsers(dest="command", required=True)

    asr = subparsers.add_parser("asr", help="transcribe a 16 kHz PCM16 WAV file")
    asr.add_argument("--input", "-i", required=True, type=Path)
    asr.add_argument("--model", "-m", default="qwen3-asr-0.6b-int8-openvino")
    asr.add_argument("--lang", "-l", default="zh")
    asr.add_argument("--max-new-tokens", type=int, default=DEFAULT_MAX_NEW_TOKENS)

    client = subparsers.add_parser(
        "client",
        help="send a transcription request to the resident service selected by the cache env",
    )
    client.add_argument("--input", "-i", required=True, type=Path)
    client.add_argument("--lang", "-l", default="zh")
    client.add_argument("--max-new-tokens", type=int, default=DEFAULT_MAX_NEW_TOKENS)
    client.add_argument(
        "--transport",
        choices=("path", "audio"),
        default="path",
        help="path keeps launcher compatibility; audio sends WAV bytes directly",
    )

    subparsers.add_parser("check", help="validate the model and required GPU device")
    subparsers.add_parser(
        "serve",
        help="keep the OpenVINO pipeline resident and accept loopback requests",
    )
    prewarm = subparsers.add_parser(
        "prewarm",
        help="guardedly refresh the resident GPU pipeline after an idle period",
    )
    prewarm.add_argument(
        "--bucket-seconds",
        type=int,
        choices=SERVICE_AUDIO_BUCKET_SECONDS,
        default=SERVICE_WARMUP_BUCKET_SECONDS,
    )
    return parser


def main() -> int:
    configure_stdio()
    args = build_parser().parse_args()
    try:
        if args.command == "check":
            print(json.dumps(check_runtime(), ensure_ascii=False, sort_keys=True))
            return 0
        if args.command == "serve":
            return serve_forever()
        if args.command == "prewarm":
            print(
                json.dumps(
                    run_service_prewarm_client(args.bucket_seconds),
                    ensure_ascii=False,
                    sort_keys=True,
                ),
                flush=True,
            )
            return 0
        if args.max_new_tokens <= 0 or args.max_new_tokens > 256:
            raise AdapterError("--max-new-tokens must be between 1 and 256")
        if not args.input.is_file():
            raise AdapterError(f"input WAV does not exist: {args.input}")
        if args.command == "client":
            client_fn = (
                run_service_audio_client
                if args.transport == "audio"
                else run_service_client
            )
            print(
                json.dumps(
                    client_fn(args.input, args.lang, args.max_new_tokens),
                    ensure_ascii=False,
                    sort_keys=True,
                ),
                flush=True,
            )
            return 0
        text = transcribe(args.input.resolve(), args.lang, args.max_new_tokens)
        # `text:` is understood by both the installed 0.8 client and current main.
        print(f"text: {text}", flush=True)
        return 0
    except AdapterError as exc:
        emit_stderr(f"error: {exc}")
        return 2
    except Exception as exc:
        emit_stderr(f"error: Qwen3-ASR OpenVINO inference failed: {exc}")
        return 3


def run_entrypoint() -> int:
    exit_code = main()
    for stream in (sys.stdout, sys.stderr):
        if stream is not None:
            stream.flush()
    if os.name == "nt":
        # Some Intel GPU/OpenVINO combinations can spend tens of seconds in
        # Python interpreter teardown after the final transcript is ready.
        # The adapter owns no persistent state after its cache lock is released,
        # so let Windows reclaim the one-shot worker immediately.
        os._exit(exit_code)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(run_entrypoint())
