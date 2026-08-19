$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "runtime-manifest-contract.ps1")

function Write-FixtureFile {
  param([string]$Root, [string]$RelativePath, [string]$Content)
  $path = Join-Path $Root $RelativePath.Replace('/', '\')
  New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
  [System.IO.File]::WriteAllText($path, $Content, [System.Text.Encoding]::UTF8)
  return $path
}

function New-FixtureEntry {
  param([string]$Root, [string]$RelativePath)
  $path = Join-Path $Root $RelativePath.Replace('/', '\')
  return [pscustomobject]@{
    path = $RelativePath
    bytes = [long](Get-Item -LiteralPath $path).Length
    sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
  }
}

function Assert-Throws {
  param([string]$Name, [string]$MessagePattern = "", [scriptblock]$Action)
  $thrown = $false
  try {
    & $Action
  } catch {
    $thrown = $true
    if (-not [string]::IsNullOrEmpty($MessagePattern) -and $_.Exception.Message -notmatch $MessagePattern) {
      throw "Fixture '$Name' failed for an unexpected reason: $($_.Exception.Message)"
    }
  }
  if (-not $thrown) {
    throw "Expected fixture '$Name' to fail."
  }
}

$fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou-runtime-manifest-fixture-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
try {
  $schema1Root = Join-Path $fixtureRoot "schema1"
  Write-FixtureFile -Root $schema1Root -RelativePath "unmanaged/legacy.txt" -Content "legacy" | Out-Null
  Assert-WindowsRuntimeStagedFilesExact `
    -Manifest ([pscustomobject]@{ schemaVersion = 1 }) `
    -StageRoot $schema1Root

  $schema2Root = Join-Path $fixtureRoot "schema2"
  Write-FixtureFile -Root $schema2Root -RelativePath "payload/archive.zip" -Content "archive" | Out-Null
  Write-FixtureFile -Root $schema2Root -RelativePath "expanded/asr/pinvou-asr.exe" -Content "wrapper" | Out-Null
  $schema2Entries = @(
    New-FixtureEntry -Root $schema2Root -RelativePath "payload/archive.zip"
    New-FixtureEntry -Root $schema2Root -RelativePath "expanded/asr/pinvou-asr.exe"
  )
  $schema2Manifest = [pscustomobject]@{ schemaVersion = 2; stagedFiles = $schema2Entries }
  Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root

  $missingEntry = [pscustomobject]@{
    path = "expanded/asr/missing.exe"
    bytes = 1
    sha256 = ("0" * 64)
  }
  Assert-Throws -Name "schema2 missing" -MessagePattern "staged file is missing" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest ([pscustomobject]@{ schemaVersion = 2; stagedFiles = @($schema2Entries + $missingEntry) }) `
      -StageRoot $schema2Root
  }

  $extraPath = Write-FixtureFile -Root $schema2Root -RelativePath "expanded/asr/extra.dll" -Content "extra"
  Assert-Throws -Name "schema2 extra" -MessagePattern "lifecycle contains an extra file" -Action {
    Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root
  }
  [System.IO.File]::Delete($extraPath)

  $caseDuplicate = [pscustomobject]@{
    path = "Payload/ARCHIVE.zip"
    bytes = [long]$schema2Entries[0].bytes
    sha256 = [string]$schema2Entries[0].sha256
  }
  Assert-Throws -Name "schema2 case duplicate" -MessagePattern "duplicate canonical path" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest ([pscustomobject]@{ schemaVersion = 2; stagedFiles = @($schema2Entries + $caseDuplicate) }) `
      -StageRoot $schema2Root
  }

  $composed = "expanded/caf$([char]0x00E9).txt"
  $decomposed = "expanded/cafe$([char]0x0301).txt"
  $unicodeDuplicateEntries = @(
    [pscustomobject]@{ path = $composed; bytes = 1; sha256 = ("0" * 64) }
    [pscustomobject]@{ path = $decomposed; bytes = 1; sha256 = ("0" * 64) }
  )
  Assert-Throws -Name "schema2 normalized duplicate" -MessagePattern "duplicate canonical path" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest ([pscustomobject]@{ schemaVersion = 2; stagedFiles = $unicodeDuplicateEntries }) `
      -StageRoot $schema2Root
  }

  foreach ($invalidPath in @(
    "/absolute.txt",
    "C:/absolute.txt",
    "payload//archive.zip",
    "payload/./archive.zip",
    "payload/../archive.zip",
    "payload\archive.zip"
  )) {
    Assert-Throws -Name "schema2 non-canonical path $invalidPath" -MessagePattern "canonical" -Action {
      Assert-WindowsRuntimeStagedFilesExact `
        -Manifest ([pscustomobject]@{
          schemaVersion = 2
          stagedFiles = @([pscustomobject]@{ path = $invalidPath; bytes = 1; sha256 = ("0" * 64) })
        }) `
        -StageRoot $schema2Root
    }
  }

  [System.IO.Directory]::Delete((Join-Path $schema2Root "payload"), $true)
  Assert-Throws -Name "schema2 after payload cleanup" -MessagePattern "staged file is missing" -Action {
    Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root
  }
} finally {
  Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Windows runtime manifest lifecycle fixture: ok"
