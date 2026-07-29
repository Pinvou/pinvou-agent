param(
  [switch]$ForceLfsPull,
  [switch]$CacheKeyOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..\..\..")).Path
$runtimePath = "private-runtimes/windows"
$runtimeRoot = Join-Path $repoRoot $runtimePath

function Invoke-Git {
  param(
    [string]$WorkingDirectory,
    [string[]]$Arguments,
    [string]$FailureMessage
  )

  $output = & git -C $WorkingDirectory @Arguments 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "$FailureMessage $($output -join ' ')"
  }
  return @($output)
}

function Get-GitlinkCommit {
  $lines = Invoke-Git -WorkingDirectory $repoRoot -Arguments @("ls-files", "--stage", "--", $runtimePath) -FailureMessage "Unable to inspect the Windows runtime gitlink."
  $line = ($lines -join "`n").Trim()
  if ($line -notmatch "^160000 ([0-9a-fA-F]{40}) 0`t") {
    throw "Windows runtime gitlink is missing or unresolved: $runtimePath"
  }
  return $Matches[1].ToLowerInvariant()
}

function Get-CurrentRuntimeCommit {
  if (-not (Test-Path -LiteralPath (Join-Path $runtimeRoot ".git"))) {
    return ""
  }
  $output = & git -C $runtimeRoot rev-parse HEAD 2>$null
  if ($LASTEXITCODE -ne 0) {
    return ""
  }
  return ($output -join "`n").Trim().ToLowerInvariant()
}

function Test-LfsPointer {
  param([string]$Path)

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    return $true
  }
  $item = Get-Item -LiteralPath $Path
  if ($item.Length -gt 1024) {
    return $false
  }
  $firstLine = Get-Content -LiteralPath $Path -TotalCount 1 -ErrorAction SilentlyContinue
  return [string]$firstLine -eq "version https://git-lfs.github.com/spec/v1"
}

function Get-LfsPointerPaths {
  $trackedPaths = Invoke-Git -WorkingDirectory $runtimeRoot -Arguments @("lfs", "ls-files", "-n") -FailureMessage "Unable to list Windows runtime Git LFS files."
  return @(
    $trackedPaths |
      ForEach-Object { [string]$_ } |
      Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
      Where-Object {
        $fullPath = Join-Path $runtimeRoot $_.Replace('/', '\')
        Test-LfsPointer -Path $fullPath
      }
  )
}

$expectedCommit = Get-GitlinkCommit
$cacheKey = "pinvou3-windows-runtime-$expectedCommit"
if ($CacheKeyOnly) {
  Write-Output $cacheKey
  exit 0
}

$currentCommit = Get-CurrentRuntimeCommit
if ($currentCommit -ne $expectedCommit) {
  Invoke-Git -WorkingDirectory $repoRoot -Arguments @(
    "-c", "submodule.$runtimePath.update=checkout",
    "submodule", "update", "--init", "--checkout", "--", $runtimePath
  ) -FailureMessage "Unable to initialize the private Windows runtime submodule. Confirm repository access and retry." | Out-Null

  $currentCommit = Get-CurrentRuntimeCommit
  if ($currentCommit -ne $expectedCommit) {
    throw "Windows runtime submodule did not reach the expected gitlink commit. Expected $expectedCommit, found $currentCommit."
  }
  Write-Host "Updated private Windows runtime submodule: $expectedCommit"
} else {
  Write-Host "Reused private Windows runtime submodule checkout: $expectedCommit"
}

$pointerPaths = @(Get-LfsPointerPaths)
if ($ForceLfsPull -or $pointerPaths.Count -gt 0) {
  $lfsArguments = @("lfs", "pull")
  if (-not $ForceLfsPull -and $pointerPaths.Count -gt 0) {
    $lfsArguments += "--include=$($pointerPaths -join ',')"
    $lfsArguments += "--exclude="
  }
  Invoke-Git -WorkingDirectory $runtimeRoot -Arguments $lfsArguments -FailureMessage "Unable to materialize Git LFS objects for the private Windows runtime submodule." | Out-Null

  $remainingPointers = @(Get-LfsPointerPaths)
  if ($remainingPointers.Count -gt 0) {
    throw "Git LFS objects remain unmaterialized: $($remainingPointers -join ', ')"
  }
  Write-Host ("Materialized Windows runtime Git LFS files: {0}" -f $pointerPaths.Count)
} else {
  Write-Host "Reused materialized Windows runtime Git LFS files; git lfs pull was skipped."
}

Write-Host "Windows runtime Jenkins cache key: $cacheKey"
Write-Output "PINVOU3_WINDOWS_RUNTIME_CACHE_KEY=$cacheKey"
