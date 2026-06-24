# Windows ASR runtime

pinvou3 resolves the optional offline voice recognition runtime from:

```text
{install_dir}/asr/pinvou-asr.exe
```

This directory contains the Windows offline ASR runtime used by the MSI build:

```text
asr/
  pinvou-asr.exe
  llama-funasr-sensevoice.exe
  models/
    sensevoice-small-q8.gguf
    fsmn-vad.gguf
```

`llama-funasr-sensevoice.exe` is built from the FunASR llama.cpp runtime with
portable x64 CPU flags for the project test machine. The upstream prebuilt
binary can fail on older CPUs if it was compiled with unsupported native
instructions.

The executable must support the stable pinvou command shape:

```text
pinvou-asr.exe asr --model sensevoice-q8 --lang zh --input <wav>
```

It should print the recognized text to stdout.

This repository provides the `pinvou-asr` wrapper source at
`src/bin/pinvou-asr.rs`.

Alternative model locations accepted by `pinvou-asr.exe`:

```text
asr/
  gguf/
    sensevoice-small-q8.gguf
    fsmn-vad.gguf

asr/
  runtime/
    llama-funasr-sensevoice.exe
    models/
      sensevoice-small-q8.gguf
      fsmn-vad.gguf
```

Useful development overrides:

```powershell
$env:PINVOU3_ASR_CMD="E:\path\to\pinvou-asr.exe"
$env:PINVOU3_ASR_BACKEND="E:\path\to\llama-funasr-sensevoice.exe"
$env:PINVOU3_SENSEVOICE_MODEL="E:\path\to\sensevoice-small-q8.gguf"
$env:PINVOU3_SENSEVOICE_VAD="E:\path\to\fsmn-vad.gguf"
pinvou-asr.exe asr --input sample.wav
```

Legacy PaddleSpeech-compatible backends are still accepted as a fallback by
setting `PINVOU3_ASR_BACKEND_KIND=paddlespeech`.
