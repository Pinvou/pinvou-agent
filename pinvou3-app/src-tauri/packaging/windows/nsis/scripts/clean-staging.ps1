$ErrorActionPreference = "Stop"

$tauriRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..\..\..")).Path
$release = Join-Path $tauriRoot "target\release"
if (-not (Test-Path -LiteralPath $release)) {
  exit 0
}

# Defensive hygiene for target/release: remove stale copies of the private
# runtime payload and dev helper binaries left by older layouts so a bundle
# can never pick them up. The pinned bundler only packages Cargo/externalBin
# binaries plus configured resources, but keeping this list clean costs
# nothing and makes the staging tree unambiguous.
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
