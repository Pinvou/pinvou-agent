param(
  [ValidateSet("Validate", "Stage")]
  [string]$Mode = "Stage",
  [string]$RuntimeRoot = "",
  [string]$LockFile = "",
  [switch]$Force
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression.FileSystem

$tauriRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$appRoot = (Resolve-Path (Join-Path $tauriRoot "..")).Path
$repoRoot = (Resolve-Path (Join-Path $appRoot "..")).Path
$defaultLockFile = Join-Path $tauriRoot "config\platforms\windows\runtime\x86_64.lock.json"
$generatedConfigPath = Join-Path $tauriRoot "tauri.windows-runtime.generated.conf.json"

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
if ([string]::IsNullOrWhiteSpace($RuntimeRoot)) {
  $runtimeRootFromEnvironment = [Environment]::GetEnvironmentVariable("PINVOU3_WINDOWS_RUNTIME_ROOT")
  if (-not [string]::IsNullOrWhiteSpace($runtimeRootFromEnvironment)) {
    $RuntimeRoot = $runtimeRootFromEnvironment
  } else {
    $RuntimeRoot = Join-Path $repoRoot ([string]$lock.source.path).Replace('/', '\')
  }
}
$RuntimeRoot = [System.IO.Path]::GetFullPath($RuntimeRoot)

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

function Test-LfsPointer {
  param([string]$Path)
  $item = Get-Item -LiteralPath $Path
  if ($item.Length -gt 1024) { return $false }
  $firstLine = Get-Content -LiteralPath $Path -TotalCount 1 -ErrorAction SilentlyContinue
  return [string]$firstLine -eq "version https://git-lfs.github.com/spec/v1"
}

function Get-VerifiedManifest {
  if (-not (Test-Path -LiteralPath $RuntimeRoot -PathType Container)) {
    throw "Windows runtime submodule is not initialized: $RuntimeRoot. Run 'git submodule update --init -- private-runtimes/windows'."
  }

  $actualCommit = Get-GitOutput -Arguments @("rev-parse", "HEAD")
  $expectedCommit = [string]$lock.source.commit
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

  $manifestPath = Join-Path $RuntimeRoot ([string]$lock.manifest.path).Replace('/', '\')
  Assert-ChildPath -Root $RuntimeRoot -Path $manifestPath
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Windows runtime submodule manifest is missing: $manifestPath"
  }
  if ((Get-Sha256 -Path $manifestPath) -ne [string]$lock.manifest.sha256) {
    throw "Windows runtime submodule manifest SHA-256 does not match the main-repository lock."
  }

  $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ([int]$manifest.schemaVersion -ne 1 -or [string]$manifest.target -ne [string]$lock.target) {
    throw "Windows runtime submodule manifest schema or target is incompatible."
  }
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

function Write-TauriOverlay {
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
  Write-Utf8WithoutBom -Path $generatedConfigPath -Content (($config | ConvertTo-Json -Depth 8) + "`n")
}

function Stage-Submodule {
  param($Manifest)
  $stageId = ([string]$lock.source.commit).Substring(0, 12) + "-" + ([string]$lock.manifest.sha256).Substring(0, 12)
  $stagingParent = Join-Path $tauriRoot "target\windows-runtime"
  $stagingRoot = Join-Path $stagingParent $stageId
  $markerPath = Join-Path $stagingRoot ".verified-lock"
  $expectedMarker = ([string]$lock.source.commit + "`n" + [string]$lock.manifest.sha256)
  Assert-ChildPath -Root (Join-Path $tauriRoot "target") -Path $stagingRoot

  $markerValid = (Test-Path -LiteralPath $markerPath -PathType Leaf) -and ((Get-Content -LiteralPath $markerPath -Raw).Trim() -eq $expectedMarker)
  if ($Force -or -not $markerValid) {
    New-Item -ItemType Directory -Path $stagingParent -Force | Out-Null
    $temporaryRoot = Join-Path $stagingParent (".tmp-" + [System.Guid]::NewGuid().ToString("N"))
    Assert-ChildPath -Root $stagingParent -Path $temporaryRoot
    try {
      foreach ($entry in $Manifest.files) {
        $sourcePath = Join-Path $RuntimeRoot ([string]$entry.path).Replace('/', '\')
        $destinationPath = Join-Path $temporaryRoot ([string]$entry.path).Replace('/', '\')
        Assert-ChildPath -Root $temporaryRoot -Path $destinationPath
        New-Item -ItemType Directory -Path (Split-Path -Parent $destinationPath) -Force | Out-Null
        Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
      }

      $payloadRoot = Join-Path $temporaryRoot "payload"
      $expandedRoot = Join-Path $temporaryRoot "expanded"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "7zip-runtime.zip" -Label "7-Zip") -Destination (Join-Path $expandedRoot "7zip") -RequiredFile "7z.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "asr-runtime.zip" -Label "ASR") -Destination (Join-Path $expandedRoot "asr") -RequiredFile "pinvou-asr.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "poppler-runtime.zip" -Label "Poppler") -Destination (Join-Path $expandedRoot "poppler") -RequiredFile "pdftotext.exe"
      Expand-ComponentRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "tesseract-runtime.zip" -Label "Tesseract") -Destination (Join-Path $expandedRoot "tesseract") -RequiredFile "tesseract.exe"
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/7zip-runtime.zip" -Destination (Join-Path $expandedRoot "7zip")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/asr-runtime.zip" -Destination (Join-Path $expandedRoot "asr")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/poppler-runtime.zip" -Destination (Join-Path $expandedRoot "poppler")
      Test-ManagedArchiveExpansion -Manifest $Manifest -ArchiveManifestPath "payload/tesseract-runtime.zip" -Destination (Join-Path $expandedRoot "tesseract")
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "python-*-embed-amd64.zip" -Label "Python") -Destination (Join-Path $expandedRoot "python") -RequiredFile "pythonw.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "node-*-win-x64.zip" -Label "Node.js") -Destination (Join-Path $expandedRoot "node") -RequiredFile "node.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "pandoc-*-windows-x86_64.zip" -Label "Pandoc") -Destination (Join-Path $expandedRoot "pandoc") -RequiredFile "pandoc.exe"
      Expand-FlattenedRuntime -ZipPath (Find-ComponentArchive -PayloadRoot $payloadRoot -Pattern "onnxruntime-win-x64-*-runtime.zip" -Label "ONNX Runtime") -Destination (Join-Path $expandedRoot "onnxruntime") -RequiredFile "onnxruntime.dll" -OnnxOnly
      Write-Utf8WithoutBom -Path (Join-Path $temporaryRoot ".verified-lock") -Content ($expectedMarker + "`n")

      if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
      }
      Move-Item -LiteralPath $temporaryRoot -Destination $stagingRoot
    } finally {
      Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
  }

  Write-TauriOverlay -StageId $stageId
  Write-Host ("Staged Windows runtime submodule: {0}" -f $stagingRoot)
  Write-Host ("Generated Tauri overlay: {0}" -f $generatedConfigPath)
}

$verifiedManifest = Get-VerifiedManifest
if ($Mode -eq "Stage") {
  Stage-Submodule -Manifest $verifiedManifest
}
