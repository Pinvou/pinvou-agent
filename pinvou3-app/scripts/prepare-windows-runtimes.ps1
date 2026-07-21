param(
  [string]$RuntimeRoot = "",
  [switch]$Force
)

$ErrorActionPreference = "Stop"

$resolver = Join-Path $PSScriptRoot "windows-runtime-submodule.ps1"
& $resolver -Mode Stage -RuntimeRoot $RuntimeRoot -Force:$Force
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}
