param(
  [string]$NodeZip = "",
  [string]$PythonZip = "",
  [string]$PandocZip = "",
  [string]$OnnxRuntimeZip = ""
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression.FileSystem

$appRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$resourcesRoot = Join-Path $appRoot "src-tauri\resources\windows"
$defaultNodeZip = Join-Path $resourcesRoot "node-v24.18.0-win-x64.zip"
$defaultPythonZip = Join-Path $resourcesRoot "python-3.13.14-embed-amd64.zip"
$defaultPandocZip = Join-Path $resourcesRoot "pandoc-3.10-windows-x86_64.zip"
$defaultOnnxRuntimeZip = Join-Path $resourcesRoot "onnxruntime-win-x64-1.20.0-runtime.zip"
$sevenZipExe = Join-Path $resourcesRoot "7zip\7z.exe"
$pythonTarget = Join-Path $resourcesRoot "python"
$nodeTarget = Join-Path $resourcesRoot "node"
$pandocTarget = Join-Path $resourcesRoot "pandoc"
$onnxRuntimeTarget = Join-Path $resourcesRoot "onnxruntime"

if ([string]::IsNullOrWhiteSpace($NodeZip)) {
  $NodeZip = $defaultNodeZip
}
if ([string]::IsNullOrWhiteSpace($PythonZip)) {
  $PythonZip = $defaultPythonZip
}
if ([string]::IsNullOrWhiteSpace($PandocZip)) {
  $PandocZip = $defaultPandocZip
}
if ([string]::IsNullOrWhiteSpace($OnnxRuntimeZip)) {
  $OnnxRuntimeZip = $defaultOnnxRuntimeZip
}

function Assert-ReadableZip {
  param(
    [string]$Path,
    [string]$Label
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "$Label runtime archive not found: $Path"
  }

  try {
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Path)
    $zip.Dispose()
  } catch {
    throw "$Label runtime archive is not a readable zip: $Path ($($_.Exception.Message))"
  }
}

function Assert-ChildPath {
  param(
    [string]$Root,
    [string]$Path
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
  $pathFull = [System.IO.Path]::GetFullPath($Path).TrimEnd('\') + '\'
  if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to write outside resource root: $Path"
  }
}

function Reset-Directory {
  param([string]$Path)

  Assert-ChildPath -Root $resourcesRoot -Path $Path
  Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $Path | Out-Null
}

function Expand-Zip {
  param(
    [string]$ZipPath,
    [string]$Destination
  )

  if (Test-Path -LiteralPath $Destination) {
    Remove-Item -LiteralPath $Destination -Recurse -Force
  }
  New-Item -ItemType Directory -Path $Destination | Out-Null

  if (Test-Path -LiteralPath $sevenZipExe -PathType Leaf) {
    & $sevenZipExe x $ZipPath "-o$Destination" -y | Out-Null
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to extract zip with bundled 7z.exe. Exit code: $LASTEXITCODE. Archive: $ZipPath"
    }
    return
  }

  [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $Destination)
}

function Copy-DirectoryContents {
  param(
    [string]$Source,
    [string]$Destination
  )

  Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
  }
}

function Prepare-RuntimeFromZip {
  param(
    [string]$ZipPath,
    [string]$TempDirectory,
    [string]$TargetDirectory,
    [string]$RequiredFileName,
    [string]$Label
  )

  Expand-Zip -ZipPath $ZipPath -Destination $TempDirectory

  # Archives differ by layout: Python files are at zip root, while Node/Pandoc
  # use a versioned top-level directory. Find the required executable first,
  # then copy its parent directory contents so the final target path is stable.
  $requiredFile = Get-ChildItem -LiteralPath $TempDirectory -Recurse -File -Filter $RequiredFileName |
    Select-Object -First 1
  if ($null -eq $requiredFile) {
    throw "$Label runtime archive does not contain ${RequiredFileName}: $ZipPath"
  }

  Reset-Directory -Path $TargetDirectory
  Copy-DirectoryContents -Source $requiredFile.DirectoryName -Destination $TargetDirectory

  $targetFile = Join-Path $TargetDirectory $RequiredFileName
  if (-not (Test-Path -LiteralPath $targetFile -PathType Leaf)) {
    throw "Prepared $Label runtime is missing $RequiredFileName at $TargetDirectory"
  }

  return (Get-ChildItem -LiteralPath $TargetDirectory -Recurse -File | Measure-Object Length -Sum).Sum
}

function Prepare-OnnxRuntimeFromZip {
  param(
    [string]$ZipPath,
    [string]$TempDirectory,
    [string]$TargetDirectory
  )

  Expand-Zip -ZipPath $ZipPath -Destination $TempDirectory

  $runtimeDll = Get-ChildItem -LiteralPath $TempDirectory -Recurse -File -Filter "onnxruntime.dll" |
    Select-Object -First 1
  if ($null -eq $runtimeDll) {
    throw "ONNX Runtime archive does not contain onnxruntime.dll: $ZipPath"
  }

  Reset-Directory -Path $TargetDirectory
  Copy-Item -LiteralPath $runtimeDll.FullName -Destination $TargetDirectory -Force

  $sharedProviderDll = Join-Path $runtimeDll.DirectoryName "onnxruntime_providers_shared.dll"
  if (Test-Path -LiteralPath $sharedProviderDll -PathType Leaf) {
    Copy-Item -LiteralPath $sharedProviderDll -Destination $TargetDirectory -Force
  }

  $unexpectedDml = Get-ChildItem -LiteralPath $TargetDirectory -Recurse -File |
    Where-Object { $_.Name -ieq "DirectML.dll" -or $_.Name -ieq "onnxruntime_providers_dml.dll" }
  if ($unexpectedDml) {
    throw "ONNX Runtime CPU runtime target unexpectedly contains DirectML files."
  }

  $targetFile = Join-Path $TargetDirectory "onnxruntime.dll"
  if (-not (Test-Path -LiteralPath $targetFile -PathType Leaf)) {
    throw "Prepared ONNX Runtime is missing onnxruntime.dll at $TargetDirectory"
  }

  return (Get-ChildItem -LiteralPath $TargetDirectory -Recurse -File | Measure-Object Length -Sum).Sum
}

Assert-ReadableZip -Path $PythonZip -Label "Python"
Assert-ReadableZip -Path $NodeZip -Label "Node"
Assert-ReadableZip -Path $PandocZip -Label "Pandoc"
Assert-ReadableZip -Path $OnnxRuntimeZip -Label "ONNX Runtime"

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou3-runtimes-" + [System.Guid]::NewGuid().ToString("N"))
$pythonTmp = Join-Path $tmpRoot "python"
$nodeTmp = Join-Path $tmpRoot "node"
$pandocTmp = Join-Path $tmpRoot "pandoc"
$onnxRuntimeTmp = Join-Path $tmpRoot "onnxruntime"

try {
  $pythonBytes = Prepare-RuntimeFromZip -ZipPath $PythonZip -TempDirectory $pythonTmp -TargetDirectory $pythonTarget -RequiredFileName "pythonw.exe" -Label "Python"
  $nodeBytes = Prepare-RuntimeFromZip -ZipPath $NodeZip -TempDirectory $nodeTmp -TargetDirectory $nodeTarget -RequiredFileName "node.exe" -Label "Node"
  $pandocBytes = Prepare-RuntimeFromZip -ZipPath $PandocZip -TempDirectory $pandocTmp -TargetDirectory $pandocTarget -RequiredFileName "pandoc.exe" -Label "Pandoc"
  $onnxRuntimeBytes = Prepare-OnnxRuntimeFromZip -ZipPath $OnnxRuntimeZip -TempDirectory $onnxRuntimeTmp -TargetDirectory $onnxRuntimeTarget
  Write-Host ("Prepared Python runtime: {0} ({1:N2} MiB)" -f $pythonTarget, ($pythonBytes / 1MB))
  Write-Host ("Prepared Node runtime: {0} ({1:N2} MiB)" -f $nodeTarget, ($nodeBytes / 1MB))
  Write-Host ("Prepared Pandoc runtime: {0} ({1:N2} MiB)" -f $pandocTarget, ($pandocBytes / 1MB))
  Write-Host ("Prepared ONNX Runtime: {0} ({1:N2} MiB)" -f $onnxRuntimeTarget, ($onnxRuntimeBytes / 1MB))
} finally {
  Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}
