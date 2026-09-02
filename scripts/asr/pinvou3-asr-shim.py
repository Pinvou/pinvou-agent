#!/usr/bin/env python3
"""pinvou3 本地语音识别适配 shim（SenseVoice.cpp 后端）

pinvou3 后端通过 PINVOU3_ASR_CMD 指向本脚本，按契约调用：
    <this> asr --model <name> --lang <zh|auto> --input <wav>

本脚本做三件 pinvou3 后端不便做的事：
  1. 用 ffmpeg 把输入音频统一转 16k 单声道 PCM（浏览器 getUserMedia 录音多为
     48k/立体声，sense-voice 只吃 16k mono，不转会加载失败）。
  2. 调 SenseVoice.cpp 的 sense-voice-main 做识别。
  3. 剥掉 `[start-end]` 时间戳前缀 + 清洗 `<|zh|><|NEUTRAL|>` 等控制标记，
     输出纯文字（pinvou3 的 parse_local_asr_text 会跳过以 `[` 开头的行，
     且控制标记会污染写入输入框的内容）。

默认引擎/模型装在 ~/.pinvou3/asr/（见 scripts/asr/setup-sensevoice.sh），
可用环境变量覆盖：
    SV_ENGINE  sense-voice-main 可执行路径
    SV_MODEL   sense-voice gguf 模型路径
诊断日志写 ~/.pinvou3/asr/shim.log（每次调用追加：参数/路径/引擎输出/退出码）。
"""
import datetime
import os
import re
import subprocess
import sys
import tempfile

ASR_DIR = os.path.join(os.path.expanduser("~"), ".pinvou3", "asr")
ENGINE = os.environ.get("SV_ENGINE", os.path.join(ASR_DIR, "sense-voice-main"))
MODEL = os.environ.get("SV_MODEL", os.path.join(ASR_DIR, "sense-voice-small-q4_k.gguf"))
LOG = os.path.join(ASR_DIR, "shim.log")


def log(msg):
    try:
        os.makedirs(ASR_DIR, exist_ok=True)
        with open(LOG, "a") as f:
            f.write(f"{datetime.datetime.now().isoformat()} {msg}\n")
    except Exception:
        pass


log("=" * 60)
log(f"argv={sys.argv[1:]}")
log(f"HOME={os.environ.get('HOME')} cwd={os.getcwd()}")
log(f"ENGINE={ENGINE} exists={os.path.isfile(ENGINE)}")
log(f"MODEL={MODEL} exists={os.path.isfile(MODEL)}")

# 解析 --input（--model/--lang 不影响，模型固定走 SV_MODEL）
args = sys.argv[1:]
wav = None
for i, a in enumerate(args):
    if a == "--input" and i + 1 < len(args):
        wav = args[i + 1]
if not wav:
    log("ERROR missing --input")
    print("error: missing --input <wav>", file=sys.stderr)
    sys.exit(2)
log(f"input={wav} exists={os.path.isfile(wav)}")

if not os.path.isfile(ENGINE):
    log("ERROR engine missing")
    print(f"error: ASR engine not found: {ENGINE}（请先跑 scripts/asr/setup-sensevoice.sh）", file=sys.stderr)
    sys.exit(1)
if not os.path.isfile(MODEL):
    log("ERROR model missing")
    print(f"error: ASR model not found: {MODEL}（请先跑 scripts/asr/setup-sensevoice.sh）", file=sys.stderr)
    sys.exit(1)

# ffmpeg 统一转 16k 单声道（兼容浏览器录音的任意采样率/声道/容器）
norm_wav = wav
tmp = None
if subprocess.run(["which", "ffmpeg"], capture_output=True).returncode == 0:
    tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
    tmp.close()
    ff = subprocess.run(
        ["ffmpeg", "-y", "-i", wav, "-ar", "16000", "-ac", "1", "-f", "wav", tmp.name],
        capture_output=True, text=True,
    )
    log(f"ffmpeg rc={ff.returncode}")
    if ff.returncode == 0 and os.path.getsize(tmp.name) > 44:
        norm_wav = tmp.name
    else:
        log("ffmpeg failed, fall back to raw input")
else:
    log("ffmpeg not found, using raw input（建议 apt install ffmpeg）")

proc = subprocess.run(
    [ENGINE, "-m", MODEL, norm_wav, "-t", "4", "-l", "auto", "-itn"],
    capture_output=True, text=True,
)
log(f"engine rc={proc.returncode}")
log(f"engine stdout={proc.stdout.strip()[:500]}")
log(f"engine stderr={proc.stderr.strip()[-500:]}")

if tmp:
    try:
        os.unlink(tmp.name)
    except Exception:
        pass

# 剥时间戳前缀，拼接多段
parts = []
for line in proc.stdout.splitlines():
    line = line.strip()
    m = re.match(r"^\[[\d.\-\s]+\]\s*(.+)$", line)
    if m:
        parts.append(m.group(1).strip())
text = "".join(parts).strip()
# 清洗 SenseVoice 控制标记：<|zh|><|NEUTRAL|><|Speech|><|withitn|> 等偶发泄漏
text = re.sub(r"<\|[^|]*\|>", "", text).strip()
log(f"parsed_text={text!r}")

if not text:
    sys.stderr.write(proc.stderr)
    print("error: engine returned no text", file=sys.stderr)
    sys.exit(1)

print(text)
