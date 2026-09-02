$ErrorActionPreference = "Stop"

function Get-CanonicalWindowsRuntimeManifestPath {
  param([string]$Path)

  if (
    [string]::IsNullOrWhiteSpace($Path) -or
    $Path.StartsWith('/') -or
    $Path.StartsWith('\') -or
    $Path -match '^[A-Za-z]:' -or
    $Path.Contains('\')
  ) {
    throw "Windows runtime manifest path must be a canonical relative path: $Path"
  }

  $segments = @($Path.Split('/'))
  foreach ($segment in $segments) {
    if (
      [string]::IsNullOrEmpty($segment) -or
      $segment -eq '.' -or
      $segment -eq '..' -or
      $segment -ne $segment.Trim() -or
      $segment.EndsWith('.') -or
      $segment.Contains(':')
    ) {
      throw "Windows runtime manifest path contains a non-canonical segment: $Path"
    }
  }

  return $Path.Normalize([System.Text.NormalizationForm]::FormC)
}

function Get-WindowsRuntimeRelativePath {
  param([string]$Root, [string]$Path)

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([char[]]@('\', '/')) + '\'
  $pathFull = [System.IO.Path]::GetFullPath($Path)
  if (-not $pathFull.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Windows runtime staged file is outside the selected lifecycle root: $Path"
  }
  return Get-CanonicalWindowsRuntimeManifestPath -Path $pathFull.Substring($rootFull.Length).Replace('\', '/')
}

function Assert-WindowsRuntimeStagedFilesDeclared {
  param($Manifest)

  if ([int]$Manifest.schemaVersion -lt 2) {
    return
  }
  if ($null -eq $Manifest.stagedFiles -or @($Manifest.stagedFiles).Count -eq 0) {
    throw "Windows runtime manifest schema 2 must contain stagedFiles."
  }
}

function Assert-WindowsRuntimeStagedFilesExact {
  param($Manifest, [string]$StageRoot)

  if ([int]$Manifest.schemaVersion -lt 2) {
    return
  }
  Assert-WindowsRuntimeStagedFilesDeclared -Manifest $Manifest
  if (-not (Test-Path -LiteralPath $StageRoot -PathType Container)) {
    throw "Windows runtime stagedFiles lifecycle root is missing: $StageRoot"
  }

  $entries = @($Manifest.stagedFiles)

  $expected = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::OrdinalIgnoreCase)
  foreach ($entry in $entries) {
    $canonicalPath = Get-CanonicalWindowsRuntimeManifestPath -Path ([string]$entry.path)
    if ($expected.ContainsKey($canonicalPath)) {
      throw "Windows runtime stagedFiles contains a duplicate canonical path: $canonicalPath"
    }
    $expected.Add($canonicalPath, $entry)
  }

  $actual = [System.Collections.Generic.Dictionary[string, System.IO.FileInfo]]::new([System.StringComparer]::OrdinalIgnoreCase)
  foreach ($file in Get-ChildItem -LiteralPath $StageRoot -File -Recurse -Force) {
    $canonicalPath = Get-WindowsRuntimeRelativePath -Root $StageRoot -Path $file.FullName
    if ($actual.ContainsKey($canonicalPath)) {
      throw "Windows runtime lifecycle contains a duplicate canonical file path: $canonicalPath"
    }
    $actual.Add($canonicalPath, $file)
  }

  foreach ($canonicalPath in $expected.Keys) {
    if (-not $actual.ContainsKey($canonicalPath)) {
      throw "Windows runtime staged file is missing: $canonicalPath"
    }
  }
  foreach ($canonicalPath in $actual.Keys) {
    if (-not $expected.ContainsKey($canonicalPath)) {
      throw "Windows runtime lifecycle contains an extra file: $canonicalPath"
    }
  }

  foreach ($canonicalPath in $expected.Keys) {
    $entry = $expected[$canonicalPath]
    $file = $actual[$canonicalPath]
    $sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([long]$file.Length -ne [long]$entry.bytes -or $sha256 -ne [string]$entry.sha256) {
      throw "Windows runtime staged file failed verification: $canonicalPath"
    }
  }
}
