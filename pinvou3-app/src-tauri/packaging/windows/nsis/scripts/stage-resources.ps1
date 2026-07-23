param(
  [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"

$tauriRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")).Path
$configPath = Join-Path $tauriRoot "tauri.conf.json"
$runtimeConfigPath = Join-Path $tauriRoot "target\windows-runtime\tauri.generated.conf.json"
$releaseRoot = Join-Path $tauriRoot "target\release"
$mainBinaryPath = Join-Path $releaseRoot "pinvou3-tauri.exe"
$stagingRoot = Join-Path $releaseRoot "nsis-stage"
$stagingResourcesRoot = Join-Path $stagingRoot "resources"
$stagingConfigPath = Join-Path $stagingRoot "tauri.nsis-stage.conf.json"
$manifestPath = Join-Path $stagingRoot "manifest.json"

function Assert-ChildPath {
  param(
    [string]$Root,
    [string]$Path
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
  $pathFull = [System.IO.Path]::GetFullPath($Path).TrimEnd('\') + '\'
  if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use a path outside the staging root: $Path"
  }
}

function Write-Utf8WithoutBom {
  param(
    [string]$Path,
    [string]$Content
  )

  $encoding = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Get-StagedRelativeFiles {
  if (-not (Test-Path -LiteralPath $stagingResourcesRoot -PathType Container)) {
    return @()
  }

  $prefix = [System.IO.Path]::GetFullPath($stagingResourcesRoot).TrimEnd('\') + '\'
  return @(
    Get-ChildItem -LiteralPath $stagingResourcesRoot -Recurse -File -Force |
      ForEach-Object {
        $_.FullName.Substring($prefix.Length).Replace('\', '/')
      } |
      Sort-Object
  )
}

function Copy-DirectoryContents {
  param(
    [string]$Source,
    [string]$Destination
  )

  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  & robocopy.exe $Source $Destination /E /COPY:DAT /DCOPY:DAT /R:2 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
  if ($LASTEXITCODE -gt 7) {
    throw "Failed to stage resource directory (robocopy exit code $LASTEXITCODE): $Source"
  }
}

if ($ValidateOnly) {
  foreach ($requiredPath in @($mainBinaryPath, $stagingResourcesRoot, $stagingConfigPath, $manifestPath)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
      throw "NSIS staging is incomplete. Run 'npm run build:nsis:stage' first. Missing: $requiredPath"
    }
  }

  $manifestJson = [System.IO.File]::ReadAllText($manifestPath, [System.Text.Encoding]::UTF8)
  $manifest = ConvertFrom-Json -InputObject $manifestJson
  $expectedFiles = @($manifest.files | Sort-Object)
  $actualFiles = @(Get-StagedRelativeFiles)
  $difference = @(Compare-Object -ReferenceObject $expectedFiles -DifferenceObject $actualFiles)
  if ($difference.Count -ne 0) {
    $preview = ($difference | Select-Object -First 10 | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }) -join "; "
    throw "NSIS staging file set changed after collection. Run 'npm run build:nsis:stage' again. Differences: $preview"
  }

  Write-Host ("Validated NSIS staging: {0} resource files" -f $actualFiles.Count)
  exit 0
}

if (-not (Test-Path -LiteralPath $mainBinaryPath -PathType Leaf)) {
  throw "Release executable not found. Run the Tauri --no-bundle build first: $mainBinaryPath"
}

$configJson = [System.IO.File]::ReadAllText($configPath, [System.Text.Encoding]::UTF8)
$config = ConvertFrom-Json -InputObject $configJson
if (-not (Test-Path -LiteralPath $runtimeConfigPath -PathType Leaf)) {
  throw "Windows runtime staging config is missing. Run src-tauri/packaging/windows/runtime/scripts/stage-runtime.ps1 first: $runtimeConfigPath"
}
$runtimeConfigJson = [System.IO.File]::ReadAllText($runtimeConfigPath, [System.Text.Encoding]::UTF8)
$runtimeConfig = ConvertFrom-Json -InputObject $runtimeConfigJson
$resourceProperties = @($config.bundle.resources.PSObject.Properties) + @($runtimeConfig.bundle.resources.PSObject.Properties)
if ($resourceProperties.Count -eq 0) {
  throw "No bundle.resources mappings were found in $configPath"
}

Assert-ChildPath -Root $releaseRoot -Path $stagingRoot
Remove-Item -LiteralPath $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $stagingResourcesRoot -Force | Out-Null

$resourceOverrides = [ordered]@{}
$mappingManifest = @()

foreach ($property in $resourceProperties) {
  $sourceSpec = [string]$property.Name
  $destinationSpec = [string]$property.Value

  if ($sourceSpec -match '[*?\[]') {
    throw "Glob resource mappings are not supported by the NSIS staging script: $sourceSpec"
  }

  $sourcePath = [System.IO.Path]::GetFullPath((Join-Path $tauriRoot $sourceSpec))
  if (-not (Test-Path -LiteralPath $sourcePath)) {
    throw "Configured bundle resource does not exist: $sourceSpec ($sourcePath)"
  }

  $destinationPath = [System.IO.Path]::GetFullPath((Join-Path $stagingResourcesRoot $destinationSpec))
  Assert-ChildPath -Root $stagingResourcesRoot -Path $destinationPath

  if (Test-Path -LiteralPath $sourcePath -PathType Container) {
    Copy-DirectoryContents -Source $sourcePath -Destination $destinationPath
  } else {
    $destinationParent = Split-Path -Parent $destinationPath
    New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
    Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
  }

  # Tauri merges --config values into tauri.conf.json. Null removes each original
  # source entry so the bundler can only consume the collected staging tree.
  $resourceOverrides[$sourceSpec] = $null
  $mappingManifest += [ordered]@{
    source = $sourceSpec
    destination = $destinationSpec
  }
}

$resourceOverrides["target/release/nsis-stage/resources/"] = ""
$stagingConfig = [ordered]@{
  bundle = [ordered]@{
    resources = $resourceOverrides
  }
}
Write-Utf8WithoutBom -Path $stagingConfigPath -Content ($stagingConfig | ConvertTo-Json -Depth 8)

$stagedFiles = @(Get-StagedRelativeFiles)
$stagedBytes = (
  Get-ChildItem -LiteralPath $stagingResourcesRoot -Recurse -File -Force |
    Measure-Object -Property Length -Sum
).Sum

$manifest = [ordered]@{
  schemaVersion = 1
  generatedAtUtc = [DateTime]::UtcNow.ToString("o")
  appVersion = [string]$config.version
  mainBinary = "target/release/pinvou3-tauri.exe"
  resourceFileCount = $stagedFiles.Count
  resourceBytes = [long]$stagedBytes
  mappings = $mappingManifest
  files = $stagedFiles
}
Write-Utf8WithoutBom -Path $manifestPath -Content ($manifest | ConvertTo-Json -Depth 8)

Write-Host ("Collected NSIS resources: {0} files ({1:N2} MiB)" -f $stagedFiles.Count, ($stagedBytes / 1MB))
Write-Host "Staging directory: $stagingRoot"
Write-Host "Main executable: $mainBinaryPath"
