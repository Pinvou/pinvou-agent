$ErrorActionPreference = "Stop"

$release = Join-Path $PSScriptRoot "..\src-tauri\target\release"
if (-not (Test-Path -LiteralPath $release)) {
  exit 0
}

# Tauri's NSIS bundler treats extra executables in target/release as external
# binaries and installs them into $INSTDIR. Keep ASR runtime files only under
# the configured asr/ resource directory, and keep dev helper binaries out of
# production installers.
$paths = @(
  "pinvou-asr.exe",
  "llama-funasr-sensevoice.exe",
  "fsmn-vad.gguf",
  "dump_system_prompt.exe",
  "dump_system_prompt.pdb",
  "dump_system_prompt.d",
  "7zip",
  "asr",
  "node",
  "pandoc",
  "poppler",
  "python",
  "tesseract"
)

foreach ($relative in $paths) {
  $target = Join-Path $release $relative
  Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction SilentlyContinue
}
