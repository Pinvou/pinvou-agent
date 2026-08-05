param(
  [string]$RuntimeRoot = "",
  [switch]$Force
)

$ErrorActionPreference = "Stop"

$resolver = Join-Path $PSScriptRoot "resolve-runtime.ps1"
& $resolver -Mode StageOnnx -RuntimeRoot $RuntimeRoot -Force:$Force
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}
