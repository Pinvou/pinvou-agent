$ErrorActionPreference = "Stop"

$tauriRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..\..\..")).Path
$release = Join-Path $tauriRoot "target\release"
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
  if (Test-Path -LiteralPath $target) {
    throw "Failed to remove a stale NSIS-bundled entry (it may be locked or running): $target"
  }
}
