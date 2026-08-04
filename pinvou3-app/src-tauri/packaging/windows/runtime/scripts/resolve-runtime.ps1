param(
  [ValidateSet("Validate", "Stage")]
  [string]$Mode = "Stage",
  [string]$RuntimeRoot = "",
  [string]$LockFile = "",
  [switch]$Force
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression.FileSystem

$tauriRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")).Path
$appRoot = (Resolve-Path (Join-Path $tauriRoot "..")).Path
$repoRoot = (Resolve-Path (Join-Path $appRoot "..")).Path
$defaultLockFile = Join-Path $tauriRoot "config\platforms\windows\runtime\x86_64.lock.json"
$generatedConfigPath = Join-Path $tauriRoot "target\windows-runtime\tauri.generated.conf.json"
$runtimeDescriptorPath = Join-Path $tauriRoot "target\windows-runtime\runtime-descriptor.json"
$stagingParent = Join-Path $tauriRoot "target\windows-runtime"

if ([string]::IsNullOrWhiteSpace($LockFile)) {
  $LockFile = $defaultLockFile
}
if (-not (Test-Path -LiteralPath $LockFile -PathType Leaf)) {
  throw "Windows runtime lock manifest not found: $LockFile"
}

$lock = Get-Content -LiteralPath $LockFile -Raw -Encoding UTF8 | ConvertFrom-Json
if ([int]$lock.schemaVersion -ne 2 -or [string]$lock.source.type -ne "git-submodule") {
  throw "Unsupported Windows runtime lock schema or source type."
}
$vcMinimumVersionText = [string]$lock.vcRedist.minimumVersion
if ($vcMinimumVersionText -notmatch '^\d+\.\d+\.\d+\.\d+$') {
  throw "Windows runtime lock must declare vcRedist.minimumVersion as a four-part version."
}
try {
  $vcMinimumVersion = [System.Version]$vcMinimumVersionText
} catch {
  throw "Windows runtime lock contains an invalid VC++ minimum version: $vcMinimumVersionText"
}
if ([string]::IsNullOrWhiteSpace($RuntimeRoot)) {
  $runtimeRootFromEnvironment = [Environment]::GetEnvironmentVariable("PINVOU3_WINDOWS_RUNTIME_ROOT")
  if (-not [string]::IsNullOrWhiteSpace($runtimeRootFromEnvironment)) {
    $RuntimeRoot = $runtimeRootFromEnvironment
  } else {
    $RuntimeRoot = Join-Path $repoRoot ([string]$lock.source.path).Replace('/', '\')
  }
}
$RuntimeRoot = [System.IO.Path]::GetFullPath($RuntimeRoot)
$stageId = ([string]$lock.source.commit).Substring(0, 12) + "-" + ([string]$lock.manifest.sha256).Substring(0, 12)
$stagingRoot = Join-Path $stagingParent $stageId

function Write-Utf8WithoutBom {
  param([string]$Path, [string]$Content)
  $parent = Split-Path -Parent $Path
  if ($parent) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
  }
  $encoding = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Assert-ChildPath {
  param([string]$Root, [string]$Path)
  if ([string]::IsNullOrWhiteSpace($Root) -or [string]::IsNullOrWhiteSpace($Path)) {
    throw "Expected non-empty root and child paths. Root='$Root' Path='$Path'"
  }
  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
  $pathFull = [System.IO.Path]::GetFullPath($Path)
  if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use a path outside the expected root: $Path"
  }
}

function Get-Sha256 {
  param([string]$Path)
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-RelativeStagePath {
  param([string]$Root, [string]$Path)
  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
  $pathFull = [System.IO.Path]::GetFullPath($Path)
  if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Path is outside the Windows runtime staging root: $Path"
  }
  return $pathFull.Substring($rootFull.Length).Replace('\', '/')
}

function Get-VerificationMarkerContent {
  $state = [ordered]@{
    schemaVersion = 2
    sourceCommit = ([string]$lock.source.commit).ToLowerInvariant()
    manifestSha256 = ([string]$lock.manifest.sha256).ToLowerInvariant()
    lockFileSha256 = Get-Sha256 -Path $LockFile
    target = [string]$lock.target
  }
  return (($state | ConvertTo-Json -Depth 4) + "`n")
}

function Test-VerificationMarker {
  param([string]$Path)

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    return $false
  }
  $actual = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
  return $actual -eq (Get-VerificationMarkerContent)
}

function Get-GitOutput {
  param([string[]]$Arguments)
  $output = & git -C $RuntimeRoot @Arguments 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to inspect Windows runtime submodule: $($output -join ' ')"
  }
  return ($output -join "`n").Trim()
}

function Get-SuperprojectGitlinkCommit {
  $sourcePath = [string]$lock.source.path
  $output = & git -C $repoRoot ls-files --stage -- $sourcePath 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to inspect the main-repository Windows runtime gitlink: $($output -join ' ')"
  }
  $line = ($output -join "`n").Trim()
  if ($line -notmatch "^160000 ([0-9a-fA-F]{40}) 0`t") {
    throw "Windows runtime submodule gitlink is missing or unresolved in the main-repository index: $sourcePath"
  }
  return $Matches[1].ToLowerInvariant()
}

function Assert-RuntimeIdentity {
  $actualCommit = Get-GitOutput -Arguments @("rev-parse", "HEAD")
  $expectedCommit = ([string]$lock.source.commit).ToLowerInvariant()
  $gitlinkCommit = Get-SuperprojectGitlinkCommit
  if ($gitlinkCommit -ne $expectedCommit) {
    throw "Windows runtime gitlink does not match the main-repository lock. Expected $expectedCommit, found $gitlinkCommit."
  }
  if ($actualCommit -ne $expectedCommit) {
    throw "Windows runtime submodule commit mismatch. Expected $expectedCommit, found $actualCommit."
  }
  $actualUrl = (Get-GitOutput -Arguments @("remote", "get-url", "origin")).TrimEnd('/')
  $expectedUrl = ([string]$lock.source.url).TrimEnd('/')
  if ($actualUrl -ne $expectedUrl) {
    throw "Windows runtime submodule origin URL mismatch. Expected $expectedUrl, found $actualUrl."
  }
  $dirty = Get-GitOutput -Arguments @("status", "--porcelain", "--untracked-files=no")
  if (-not [string]::IsNullOrWhiteSpace($dirty)) {
    throw "Windows runtime submodule contains tracked local changes; commit and update the main-repository lock first."
  }
  return $actualCommit
}

function Get-RuntimeManifestPath {
  $manifestPath = Join-Path $RuntimeRoot ([string]$lock.manifest.path).Replace('/', '\')
  Assert-ChildPath -Root $RuntimeRoot -Path $manifestPath
  return $manifestPath
}

function Read-CompatibleRuntimeManifest {
  param([string]$ManifestPath)

  $manifest = Get-Content -LiteralPath $ManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ([int]$manifest.schemaVersion -ne 1 -or [string]$manifest.target -ne [string]$lock.target) {
    throw "Windows runtime submodule manifest schema or target is incompatible."
  }
  return $manifest
}

function Test-LfsPointer {
  param([string]$Path)
  $item = Get-Item -LiteralPath $Path
  if ($item.Length -gt 1024) { return $false }
  $firstLine = Get-Content -LiteralPath $Path -TotalCount 1 -ErrorAction SilentlyContinue
  return [string]$firstLine -eq "version https://git-lfs.github.com/spec/v1"
}

function Get-VerifiedManifest {
  if (-not (Test-Path -LiteralPath $RuntimeRoot -PathType Container)) {
    throw "Windows runtime submodule is not initialized: $RuntimeRoot. Run 'npm run runtime:windows:init' from pinvou3-app."
  }

  $actualCommit = Assert-RuntimeIdentity

  $manifestPath = Get-RuntimeManifestPath
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Windows runtime submodule manifest is missing: $manifestPath"
  }
  if ((Get-Sha256 -Path $manifestPath) -ne [string]$lock.manifest.sha256) {
    throw "Windows runtime submodule manifest SHA-256 does not match the main-repository lock."
  }

  $manifest = Read-CompatibleRuntimeManifest -ManifestPath $manifestPath
  foreach ($entry in $manifest.files) {
    $sourcePath = Join-Path $RuntimeRoot ([string]$entry.path).Replace('/', '\')
    Assert-ChildPath -Root $RuntimeRoot -Path $sourcePath
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
      throw "Locked Windows runtime file is missing: $($entry.path)"
    }
    if (Test-LfsPointer -Path $sourcePath) {
      throw "Git LFS object is not materialized: $($entry.path). Run 'git -C private-runtimes/windows lfs pull'."
    }
    $item = Get-Item -LiteralPath $sourcePath
    if ([long]$item.Length -ne [long]$entry.bytes) {
      throw "Locked Windows runtime file size mismatch: $($entry.path)"
    }
    if ((Get-Sha256 -Path $sourcePath) -ne [string]$entry.sha256) {
      throw "Locked Windows runtime file SHA-256 mismatch: $($entry.path)"
    }
  }

  $vcEntries = @($manifest.files | Where-Object { [string]$_.component -eq "vc_redist" })
  if ($vcEntries.Count -ne 1) {
    throw "Windows runtime manifest must contain exactly one VC++ redistributable."
  }
  $vcSourcePath = Join-Path $RuntimeRoot ([string]$vcEntries[0].path).Replace('/', '\')
  $vcVersionInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($vcSourcePath)
  $vcActualVersion = [System.Version]::new(
    $vcVersionInfo.FileMajorPart,
    $vcVersionInfo.FileMinorPart,
    $vcVersionInfo.FileBuildPart,
    $vcVersionInfo.FilePrivatePart
  )
  if ($vcActualVersion -lt $vcMinimumVersion) {
    throw "Locked VC++ redistributable version $vcActualVersion is older than required version $vcMinimumVersion."
  }

  Write-Host ("Validated Windows runtime submodule: {0} files at {1}" -f $manifest.files.Count, $actualCommit)
  return $manifest
}

function Find-ComponentArchive {
  param([string]$PayloadRoot, [string]$Pattern, [string]$Label)
  $matches = @(Get-ChildItem -LiteralPath $PayloadRoot -File -Filter $Pattern)
  if ($matches.Count -ne 1) {
    throw "Expected exactly one $Label component archive matching '$Pattern', found $($matches.Count)."
  }
  return $matches[0].FullName
}

function Expand-FlattenedRuntime {
  param(
    [string]$ZipPath,
    [string]$Destination,
    [string]$RequiredFile,
    [switch]$OnnxOnly
  )
  $temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou3-runtime-expand-" + [System.Guid]::NewGuid().ToString("N"))
  try {
    [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $temporary)
    $required = Get-ChildItem -LiteralPath $temporary -File -Recurse -Filter $RequiredFile | Select-Object -First 1
    if ($null -eq $required) {
      throw "Runtime archive does not contain ${RequiredFile}: $ZipPath"
    }
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    if ($OnnxOnly) {
      Copy-Item -LiteralPath $required.FullName -Destination $Destination -Force
      $shared = Join-Path $required.DirectoryName "onnxruntime_providers_shared.dll"
      if (Test-Path -LiteralPath $shared -PathType Leaf) {
        Copy-Item -LiteralPath $shared -Destination $Destination -Force
      }
    } else {
      Get-ChildItem -LiteralPath $required.DirectoryName -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
      }
    }
  } finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
  }
}

function Expand-ComponentRuntime {
  param(
    [string]$ZipPath,
    [string]$Destination,
    [string]$RequiredFile
  )
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $Destination)
  $requiredPath = Join-Path $Destination $RequiredFile
  if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
    throw "Component archive is missing required runtime file '${RequiredFile}': $ZipPath"
  }
}

function Test-ManagedArchiveExpansion {
  param(
    $Manifest,
    [string]$ArchiveManifestPath,
    [string]$Destination
  )
  $archiveLocks = @($Manifest.managedArchives | Where-Object { [string]$_.archive -eq $ArchiveManifestPath })
  if ($archiveLocks.Count -ne 1) {
    throw "Managed component archive lock is missing or duplicated: $ArchiveManifestPath"
  }
  $archiveLock = $archiveLocks[0]
  $actualFiles = @(Get-ChildItem -LiteralPath $Destination -File -Recurse -Force)
  if ($actualFiles.Count -ne [int]$archiveLock.files) {
    throw "Managed component archive file count mismatch after extraction: $ArchiveManifestPath"
  }
  foreach ($entry in $archiveLock.entries) {
    $entryPath = Join-Path $Destination ([string]$entry.path).Replace('/', '\')
    Assert-ChildPath -Root $Destination -Path $entryPath
    if (-not (Test-Path -LiteralPath $entryPath -PathType Leaf)) {
      throw "Managed component file is missing after extraction: $ArchiveManifestPath -> $($entry.path)"
    }
    $item = Get-Item -LiteralPath $entryPath
    if ([long]$item.Length -ne [long]$entry.bytes -or (Get-Sha256 -Path $entryPath) -ne [string]$entry.sha256) {
      throw "Managed component file failed verification after extraction: $ArchiveManifestPath -> $($entry.path)"
    }
  }
}

function Get-RuntimeDescriptorContent {
  param($Manifest, [string]$StageId)
  $relativeRoot = "target/windows-runtime/$StageId"
  $vcRedistPath = Join-Path $stagingRoot "expanded\vc_redist\VC_redist.x64.exe"
  if (-not (Test-Path -LiteralPath $vcRedistPath -PathType Leaf)) {
    throw "Staged VC++ runtime is missing while generating the runtime descriptor: $vcRedistPath"
  }
  $descriptor = [ordered]@{
    schemaVersion = 1
    target = [string]$lock.target
    stageId = $StageId
    configPath = "target/windows-runtime/tauri.generated.conf.json"
    nodeExecutable = "$relativeRoot/expanded/node/node.exe"
    npmExecPath = "$relativeRoot/expanded/node/node_modules/npm/bin/npm-cli.js"
    onnxRuntimeDylib = "$relativeRoot/expanded/onnxruntime/onnxruntime.dll"
    vcRedist = [ordered]@{
      source = "$relativeRoot/expanded/vc_redist/VC_redist.x64.exe"
      bytes = [long](Get-Item -LiteralPath $vcRedistPath).Length
      sha256 = Get-Sha256 -Path $vcRedistPath
    }
    asrModel = [ordered]@{
      delivery = "download-on-first-use"
      bundled = $false
      filename = "sensevoice-small-q8.gguf"
    }
  }
  return (($descriptor | ConvertTo-Json -Depth 8) + "`n")
}

function Write-RuntimeDescriptor {
  param($Manifest, [string]$StageId)
  Write-Utf8WithoutBom -Path $runtimeDescriptorPath -Content (Get-RuntimeDescriptorContent -Manifest $Manifest -StageId $StageId)
}

function Get-StageInventoryContent {
  param([string]$StageRoot)
  $expandedRoot = Join-Path $StageRoot "expanded"
  $files = @(
    Get-ChildItem -LiteralPath $expandedRoot -File -Recurse -Force |
      Sort-Object FullName |
      ForEach-Object {
        [ordered]@{
          path = Get-RelativeStagePath -Root $StageRoot -Path $_.FullName
          bytes = [long]$_.Length
          sha256 = Get-Sha256 -Path $_.FullName
        }
      }
  )
  $inventory = [ordered]@{
    schemaVersion = 1
    stageId = $stageId
    files = $files
  }
  return (($inventory | ConvertTo-Json -Depth 8) + "`n")
}

function Test-StageInventory {
  param([string]$StageRoot)
  $inventoryPath = Join-Path $StageRoot ".verified-stage.json"
  if (-not (Test-Path -LiteralPath $inventoryPath -PathType Leaf)) {
    return $false
  }

  try {
    $inventory = Get-Content -LiteralPath $inventoryPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$inventory.schemaVersion -ne 1 -or [string]$inventory.stageId -ne $stageId) {
      return $false
    }
    $entries = @($inventory.files)
    $actualFiles = @(Get-ChildItem -LiteralPath (Join-Path $StageRoot "expanded") -File -Recurse -Force)
    if ($entries.Count -ne $actualFiles.Count) {
      return $false
    }
    $seenPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $entries) {
      $relativePath = ([string]$entry.path).Replace('\', '/')
      if (-not $relativePath.StartsWith("expanded/", [System.StringComparison]::OrdinalIgnoreCase)) {
        return $false
      }
      if (-not $seenPaths.Add($relativePath)) {
        return $false
      }
      $entryPath = Join-Path $StageRoot $relativePath.Replace('/', '\')
      Assert-ChildPath -Root $StageRoot -Path $entryPath
      if (-not (Test-Path -LiteralPath $entryPath -PathType Leaf)) {
        return $false
      }
      $item = Get-Item -LiteralPath $entryPath
      if (
        [long]$item.Length -ne [long]$entry.bytes -or
        (Get-Sha256 -Path $entryPath) -ne [string]$entry.sha256
      ) {
        return $false
      }
    }
    return $true
  } catch {
    return $false
  }
}

function Get-TauriOverlayContent {
  param([string]$StageId)
  $relativeRoot = "target/windows-runtime/$StageId"
  $resources = [ordered]@{
    "$relativeRoot/expanded/poppler/" = "runtime/poppler"
    "$relativeRoot/expanded/pandoc/" = "runtime/pandoc"
    "$relativeRoot/expanded/tesseract/" = "runtime/tesseract"
    "$relativeRoot/expanded/python/" = "runtime/python"
    "$relativeRoot/expanded/node/" = "runtime/node"
    "$relativeRoot/expanded/onnxruntime/" = "runtime/onnxruntime"
    "$relativeRoot/expanded/asr/README.md" = "runtime/asr/README.md"
    "$relativeRoot/expanded/asr/pinvou-asr.exe" = "runtime/asr/pinvou-asr.exe"
    "$relativeRoot/expanded/asr/llama-funasr-sensevoice.exe" = "runtime/asr/llama-funasr-sensevoice.exe"
    "$relativeRoot/expanded/asr/models/fsmn-vad.gguf" = "runtime/asr/models/fsmn-vad.gguf"
    "$relativeRoot/expanded/7zip/" = "runtime/7zip"
  }
  $config = [ordered]@{ bundle = [ordered]@{ resources = $resources } }
  return (($config | ConvertTo-Json -Depth 8) + "`n")
}

function Write-TauriOverlay {
  param([string]$StageId)
  Write-Utf8WithoutBom -Path $generatedConfigPath -Content (Get-TauriOverlayContent -StageId $StageId)
}

function Test-VerifiedStageReusable {
  param($Manifest)

  $stageMarkerPath = Join-Path $stagingRoot ".verified-lock"
  if (-not (Test-VerificationMarker -Path $stageMarkerPath)) {
    return $false
  }

  if (-not (Test-StageInventory -StageRoot $stagingRoot)) {
    return $false
  }

  if (-not (Test-Path -LiteralPath $generatedConfigPath -PathType Leaf)) {
    return $false
  }
  $actualOverlay = [System.IO.File]::ReadAllText($generatedConfigPath, [System.Text.Encoding]::UTF8)
  if ($actualOverlay -ne (Get-TauriOverlayContent -StageId $stageId)) {
    return $false
  }

  if (-not (Test-Path -LiteralPath $runtimeDescriptorPath -PathType Leaf)) {
    return $false
  }
  $actualDescriptor = [System.IO.File]::ReadAllText($runtimeDescriptorPath, [System.Text.Encoding]::UTF8)
  if ($actualDescriptor -ne (Get-RuntimeDescriptorContent -Manifest $Manifest -StageId $stageId)) {
    return $false
  }

  return $true
}

function Stage-Submodule {
  param($Manifest)
  $markerPath = Join-Path $stagingRoot ".verified-lock"
  $expectedMarker = Get-VerificationMarkerContent
  Assert-ChildPath -Root (Join-Path $tauriRoot "target") -Path $stagingRoot

  $markerValid = Test-VerificationMarker -Path $markerPath
  $stageContentValid = $markerValid -and (Test-StageInventory -StageRoot $stagingRoot)
  if ($Force -or -not $stageContentValid) {
    New-Item -ItemType Directory -Path $stagingParent -Force | Out-Null
    $temporaryRoot = Join-Path $stagingParent (".tmp-" + [System.Guid]::NewGuid().ToString("N"))
    $stageContext = [pscustomobject]@{
      TemporaryRoot = $temporaryRoot
      PayloadRoot = Join-Path $temporaryRoot "payload"
      ExpandedRoot = Join-Path $temporaryRoot "expanded"
    }
    Assert-ChildPath -Root $stagingParent -Path $stageContext.TemporaryRoot
    try {
      foreach ($entry in $Manifest.files) {
        $sourcePath = Join-Path $RuntimeRoot ([string]$entry.path).Replace('/', '\')
        $destinationPath = Join-Path $stageContext.TemporaryRoot ([string]$entry.path).Replace('/', '\')
        Assert-ChildPath -Root $stageContext.TemporaryRoot -Path $destinationPath
        New-Item -ItemType Directory -Path (Split-Path -Parent $destinationPath) -Force | Out-Null
        Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
      }

      $payloadRoot = $stageContext.PayloadRoot
      $expandedRoot = $stageContext.ExpandedRoot
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "7zip-runtime.zip" -Label "7-Zip") -Destination (Join-Path $expandedRoot "7zip") -RequiredFile "7z.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "asr-runtime.zip" -Label "ASR") -Destination (Join-Path $expandedRoot "asr") -RequiredFile "pinvou-asr.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "poppler-runtime.zip" -Label "Poppler") -Destination (Join-Path $expandedRoot "poppler") -RequiredFile "pdftotext.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "tesseract-runtime.zip" -Label "Tesseract") -Destination (Join-Path $expandedRoot "tesseract") -RequiredFile "tesseract.exe"
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/7zip-runtime.zip" -Destination (Join-Path $expandedRoot "7zip")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/asr-runtime.zip" -Destination (Join-Path $expandedRoot "asr")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/poppler-runtime.zip" -Destination (Join-Path $expandedRoot "poppler")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/tesseract-runtime.zip" -Destination (Join-Path $expandedRoot "tesseract")

      # The main ASR model is downloaded and verified on first use. Keep only the
      # engine and the small VAD model in installer staging.
      $bundledAsrModelPath = Join-Path $expandedRoot "asr\models\sensevoice-small-q8.gguf"
      if (Test-Path -LiteralPath $bundledAsrModelPath -PathType Leaf) {
        Remove-Item -LiteralPath $bundledAsrModelPath -Force
      }
      if (Test-Path -LiteralPath $bundledAsrModelPath) {
        throw "Download-on-first-use ASR model remained in Windows runtime staging: $bundledAsrModelPath"
      }

      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "python-*-embed-amd64.zip" -Label "Python") -Destination (Join-Path $expandedRoot "python") -RequiredFile "pythonw.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "node-*-win-x64.zip" -Label "Node.js") -Destination (Join-Path $expandedRoot "node") -RequiredFile "node.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "pandoc-*-windows-x86_64.zip" -Label "Pandoc") -Destination (Join-Path $expandedRoot "pandoc") -RequiredFile "pandoc.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "onnxruntime-win-x64-*-runtime.zip" -Label "ONNX Runtime") -Destination (Join-Path $expandedRoot "onnxruntime") -RequiredFile "onnxruntime.dll" -OnnxOnly
      $expandedRoot = $stageContext.ExpandedRoot
      Write-Host ("Expanded all Windows runtime components: {0}" -f $expandedRoot)

      # Resolver 只把 VC++ 组件放入通用 staging；是否供 NSIS 使用由 installer adapter 决定。
      Write-Host "Preparing descriptor-owned VC++ runtime component."
      $vcSourcePath = Join-Path $stageContext.TemporaryRoot "payload\vc_redist\VC_redist.x64.exe"
      $vcStagedPath = Join-Path $expandedRoot "vc_redist\VC_redist.x64.exe"
      New-Item -ItemType Directory -Path (Split-Path -Parent $vcStagedPath) -Force | Out-Null
      Copy-Item -LiteralPath $vcSourcePath -Destination $vcStagedPath -Force
      $vcActualBytes = [long](Get-Item -LiteralPath $vcStagedPath).Length
      $vcActualSha256 = Get-Sha256 -Path $vcStagedPath
      if (
        $vcActualBytes -ne [long](Get-Item -LiteralPath $vcSourcePath).Length -or
        $vcActualSha256 -ne (Get-Sha256 -Path $vcSourcePath)
      ) {
        throw "Staged VC++ runtime differs from its verified source: $vcStagedPath"
      }

      # Component archives have served their only purpose after extraction and
      # verification. Removing them keeps the reusable stage minimal and avoids
      # retaining a second copy of the first-download ASR model.
      Assert-ChildPath -Root $stageContext.TemporaryRoot -Path $stageContext.PayloadRoot
      Remove-Item -LiteralPath $stageContext.PayloadRoot -Recurse -Force
      if (Test-Path -LiteralPath $stageContext.PayloadRoot) {
        throw "Windows runtime payload cleanup failed: $($stageContext.PayloadRoot)"
      }

      Write-Utf8WithoutBom -Path (Join-Path $stageContext.TemporaryRoot ".verified-stage.json") -Content (Get-StageInventoryContent -StageRoot $stageContext.TemporaryRoot)
      Write-Utf8WithoutBom -Path (Join-Path $stageContext.TemporaryRoot ".verified-lock") -Content $expectedMarker

      if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
      }
      Move-Item -LiteralPath $stageContext.TemporaryRoot -Destination $stagingRoot
    } finally {
      Remove-Item -LiteralPath $stageContext.TemporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
  }

  Write-TauriOverlay -StageId $stageId
  Write-RuntimeDescriptor -Manifest $Manifest -StageId $stageId
  Write-Host ("Staged Windows runtime submodule: {0}" -f $stagingRoot)
  Write-Host ("Generated Tauri overlay: {0}" -f $generatedConfigPath)
  Write-Host ("Generated runtime descriptor: {0}" -f $runtimeDescriptorPath)
}

$verifiedManifest = Get-VerifiedManifest
if (-not $Force) {
  if ($Mode -eq "Stage" -and (Test-VerifiedStageReusable -Manifest $verifiedManifest)) {
    Write-Host ("Reused verified Windows runtime staging: {0}" -f $stagingRoot)
    Write-Host ("Reused Tauri overlay: {0}" -f $generatedConfigPath)
    Write-Host ("Reused runtime descriptor: {0}" -f $runtimeDescriptorPath)
    exit 0
  }
}

if ($Mode -eq "Stage") {
  Stage-Submodule -Manifest $verifiedManifest
}
