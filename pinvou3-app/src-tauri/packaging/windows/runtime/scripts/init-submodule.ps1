$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..\..\..")).Path
$runtimePath = "private-runtimes/windows"

& git -C $repoRoot submodule update --init --checkout -- $runtimePath
if ($LASTEXITCODE -ne 0) {
  throw "Unable to initialize the private Windows runtime submodule. Confirm repository access and retry."
}

& git -C (Join-Path $repoRoot $runtimePath) lfs pull
if ($LASTEXITCODE -ne 0) {
  throw "Unable to materialize Git LFS objects for the private Windows runtime submodule."
}

Write-Host "Initialized private Windows runtime submodule on demand: $runtimePath"
