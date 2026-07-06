param(
  [string]$NodeZip = "C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip",
  [string]$PythonZip = "C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip"
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression.FileSystem

$appRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$resourcesRoot = Join-Path $appRoot "src-tauri\resources\windows"
$pythonTarget = Join-Path $resourcesRoot "python"
$nodeTarget = Join-Path $resourcesRoot "node"

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

Assert-ReadableZip -Path $PythonZip -Label "Python"
Assert-ReadableZip -Path $NodeZip -Label "Node"

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou3-runtimes-" + [System.Guid]::NewGuid().ToString("N"))
$pythonTmp = Join-Path $tmpRoot "python"
$nodeTmp = Join-Path $tmpRoot "node"

try {
  Expand-Zip -ZipPath $PythonZip -Destination $pythonTmp
  Expand-Zip -ZipPath $NodeZip -Destination $nodeTmp

  $pythonw = Get-ChildItem -LiteralPath $pythonTmp -Recurse -File -Filter "pythonw.exe" | Select-Object -First 1
  if ($null -eq $pythonw) {
    throw "Python runtime archive does not contain pythonw.exe: $PythonZip"
  }

  $nodeExe = Get-ChildItem -LiteralPath $nodeTmp -Recurse -File -Filter "node.exe" | Select-Object -First 1
  if ($null -eq $nodeExe) {
    throw "Node runtime archive does not contain node.exe: $NodeZip"
  }

  Reset-Directory -Path $pythonTarget
  Reset-Directory -Path $nodeTarget

  # Python embeddable zip stores files at archive root. Copy the directory that
  # owns pythonw.exe so the final path is resources/windows/python/pythonw.exe.
  Copy-DirectoryContents -Source $pythonw.DirectoryName -Destination $pythonTarget

  # Official Node zip has a versioned top-level directory. Copy the node.exe
  # parent contents so the final path is resources/windows/node/node.exe.
  Copy-DirectoryContents -Source $nodeExe.DirectoryName -Destination $nodeTarget

  if (-not (Test-Path -LiteralPath (Join-Path $pythonTarget "pythonw.exe") -PathType Leaf)) {
    throw "Prepared Python runtime is missing pythonw.exe at $pythonTarget"
  }
  if (-not (Test-Path -LiteralPath (Join-Path $nodeTarget "node.exe") -PathType Leaf)) {
    throw "Prepared Node runtime is missing node.exe at $nodeTarget"
  }

  $pythonBytes = (Get-ChildItem -LiteralPath $pythonTarget -Recurse -File | Measure-Object Length -Sum).Sum
  $nodeBytes = (Get-ChildItem -LiteralPath $nodeTarget -Recurse -File | Measure-Object Length -Sum).Sum
  Write-Host ("Prepared Python runtime: {0} ({1:N2} MiB)" -f $pythonTarget, ($pythonBytes / 1MB))
  Write-Host ("Prepared Node runtime: {0} ({1:N2} MiB)" -f $nodeTarget, ($nodeBytes / 1MB))
} finally {
  Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}
