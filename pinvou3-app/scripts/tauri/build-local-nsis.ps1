[CmdletBinding(DefaultParameterSetName = "Build")]
param(
  [Parameter(DontShow = $true)]
  [string]$AppRoot,

  [Parameter(ParameterSetName = "Prepare")]
  [switch]$PrepareOnly,

  [Parameter(ParameterSetName = "Verify")]
  [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

# Windows PowerShell 5.1 evaluates parameter default expressions before
# $PSScriptRoot is populated. Resolve the default from the script body so the
# public npm entry works without callers having to pass the hidden AppRoot.
if ([string]::IsNullOrWhiteSpace($AppRoot)) {
  $AppRoot = Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")
}

function Resolve-AbsolutePath {
  param([Parameter(Mandatory = $true)][string]$Path)

  $fullPath = [System.IO.Path]::GetFullPath($Path)
  $rootPath = [System.IO.Path]::GetPathRoot($fullPath)
  if ([string]::Equals($fullPath, $rootPath, [System.StringComparison]::OrdinalIgnoreCase)) {
    return $fullPath
  }
  return $fullPath.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
}

$appRootPath = Resolve-AbsolutePath -Path $AppRoot
$bundleRoot = Resolve-AbsolutePath -Path (Join-Path $appRootPath "src-tauri\target\release\bundle")
$outputPath = Resolve-AbsolutePath -Path (Join-Path $bundleRoot "nsis")
$expectedOutputPath = Resolve-AbsolutePath -Path (
  Join-Path $appRootPath "src-tauri\target\release\bundle\nsis"
)
$outputParent = Resolve-AbsolutePath -Path ([System.IO.DirectoryInfo]$outputPath).Parent.FullName
$comparison = [System.StringComparison]::OrdinalIgnoreCase

if (-not [string]::Equals($outputPath, $expectedOutputPath, $comparison) -or
    -not [string]::Equals($outputParent, $bundleRoot, $comparison) -or
    -not [string]::Equals([System.IO.Path]::GetFileName($outputPath), "nsis", $comparison)) {
  throw "Refusing to clean an unexpected local NSIS output path: $outputPath"
}

$markerPath = Join-Path $bundleRoot ".pinvou-local-nsis-build-start.json"

function Prepare-LocalNsisOutput {
  $startedAtUtc = [DateTime]::UtcNow
  if (Test-Path -LiteralPath $outputPath) {
    Write-Output "Removing previous local NSIS output: $outputPath"
    Remove-Item -LiteralPath $outputPath -Recurse -Force
  }
  New-Item -ItemType Directory -Path $bundleRoot -Force | Out-Null
  @{
    outputPath = $outputPath
    startedAtUtc = $startedAtUtc.ToString("o")
  } | ConvertTo-Json | Set-Content -LiteralPath $markerPath -Encoding UTF8
  Write-Output "Local NSIS output is ready for a fresh build: $outputPath"
}

function Confirm-LocalNsisOutput {
  if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    throw "Local NSIS freshness marker is missing: $markerPath"
  }
  $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
  if (-not [string]::Equals(
      (Resolve-AbsolutePath -Path ([string]$marker.outputPath)),
      $outputPath,
      $comparison
    )) {
    throw "Local NSIS freshness marker points to an unexpected output directory."
  }
  $startedAtUtc = [DateTime]::Parse(
    [string]$marker.startedAtUtc,
    [System.Globalization.CultureInfo]::InvariantCulture,
    [System.Globalization.DateTimeStyles]::RoundtripKind
  ).ToUniversalTime()
  $artifacts = @(
    Get-ChildItem -LiteralPath $outputPath -File -Filter "*.exe" -ErrorAction SilentlyContinue
  )
  if ($artifacts.Count -eq 0) {
    throw "Local NSIS build produced no installer in: $outputPath"
  }
  foreach ($artifact in $artifacts) {
    if ($artifact.LastWriteTimeUtc -lt $startedAtUtc.AddSeconds(-2)) {
      throw "Local NSIS artifact predates this build: $($artifact.FullName)"
    }
    Write-Output (
      "Fresh local NSIS artifact: {0} ({1} bytes, {2:o})" -f `
        $artifact.FullName,
        $artifact.Length,
        $artifact.LastWriteTimeUtc
    )
  }
  Remove-Item -LiteralPath $markerPath -Force
}

if ($PrepareOnly) {
  Prepare-LocalNsisOutput
  exit 0
}
if ($VerifyOnly) {
  Confirm-LocalNsisOutput
  exit 0
}

Prepare-LocalNsisOutput
Push-Location $appRootPath
try {
  & npm.cmd run build:nsis:stage:local
  if ($LASTEXITCODE -ne 0) {
    throw "Local NSIS compile stage failed with exit code $LASTEXITCODE"
  }
  & npm.cmd run bundle:nsis:local
  if ($LASTEXITCODE -ne 0) {
    throw "Local NSIS bundle stage failed with exit code $LASTEXITCODE"
  }
} finally {
  Pop-Location
}
Confirm-LocalNsisOutput
