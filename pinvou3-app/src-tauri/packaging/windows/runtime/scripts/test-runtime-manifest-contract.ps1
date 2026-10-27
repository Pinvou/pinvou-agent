# Windows runtime manifest contract suite. Scope note: production already
# consumes the contract library directly -- resolve-runtime.ps1 runs
# Assert-WindowsRuntimeStagedFilesDeclared on every manifest read and
# Assert-WindowsRuntimeStagedFilesExact inside Stage-Submodule, and that
# production wiring is itself pinned by tests/windows_runtime_packaging_
# contract.test.js. This suite is the library's direct test home (per-throw-
# site negative cases), run via `npm run test:windows-runtime-manifest` in
# pr-check's Windows job.
#
# The Write-Utf8Atomically assertions below (atomic write / FILE_SHARE_DELETE
# / litter cleanup) dot-source resolve-runtime.ps1 -ImportFunctionsOnly,
# which runs its top-level lock validation (read-only, and it passes against
# the checked-in x86_64.lock.json) before returning ahead of all staging
# work, so no further resolve-runtime.ps1 parity is required for them.
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

  Assert-Throws -Name "schema2 missing stagedFiles property" -MessagePattern "must contain stagedFiles" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest ([pscustomobject]@{ schemaVersion = 2 }) `
      -StageRoot $schema2Root
  }
  Assert-Throws -Name "schema2 empty stagedFiles" -MessagePattern "must contain stagedFiles" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest ([pscustomobject]@{ schemaVersion = 2; stagedFiles = @() }) `
      -StageRoot $schema2Root
  }
  Assert-Throws -Name "schema2 missing lifecycle root" -MessagePattern "lifecycle root is missing" -Action {
    Assert-WindowsRuntimeStagedFilesExact `
      -Manifest $schema2Manifest `
      -StageRoot (Join-Path $fixtureRoot "missing-stage-root")
  }

  Write-FixtureFile -Root $schema2Root -RelativePath "payload/archive.zip" -Content "archive-tampered" | Out-Null
  Assert-Throws -Name "schema2 byte mismatch" -MessagePattern "failed verification" -Action {
    Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root
  }
  Write-FixtureFile -Root $schema2Root -RelativePath "payload/archive.zip" -Content "Archive" | Out-Null
  Assert-Throws -Name "schema2 sha256 mismatch" -MessagePattern "failed verification" -Action {
    Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root
  }
  Write-FixtureFile -Root $schema2Root -RelativePath "payload/archive.zip" -Content "archive" | Out-Null
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

  # Pattern is per-throw-site (not the shared word "canonical") so a regression
  # in one guard cannot be masked by the other one throwing instead. The
  # whole-path guard's whitespace-only case and the segment guard's
  # trailing-dot / colon (ADS) / trailing-whitespace cases previously shipped
  # without direct assertions -- a guard regression there passed the suite
  # while staging validation weakened.
  foreach ($wholePathViolation in @(
    "/absolute.txt",
    "C:/absolute.txt",
    "payload\archive.zip",
    "   "
  )) {
    Assert-Throws -Name "schema2 non-canonical path $wholePathViolation" -MessagePattern "must be a canonical relative path" -Action {
      Assert-WindowsRuntimeStagedFilesExact `
        -Manifest ([pscustomobject]@{
          schemaVersion = 2
          stagedFiles = @([pscustomobject]@{ path = $wholePathViolation; bytes = 1; sha256 = ("0" * 64) })
        }) `
        -StageRoot $schema2Root
    }
  }

  foreach ($segmentViolation in @(
    "payload//archive.zip",
    "payload/./archive.zip",
    "payload/../archive.zip",
    "payload/archive.zip.",
    "payload /archive.zip",
    "payload/arch:ive.zip"
  )) {
    Assert-Throws -Name "schema2 non-canonical segment $segmentViolation" -MessagePattern "non-canonical segment" -Action {
      Assert-WindowsRuntimeStagedFilesExact `
        -Manifest ([pscustomobject]@{
          schemaVersion = 2
          stagedFiles = @([pscustomobject]@{ path = $segmentViolation; bytes = 1; sha256 = ("0" * 64) })
        }) `
        -StageRoot $schema2Root
    }
  }

  [System.IO.Directory]::Delete((Join-Path $schema2Root "payload"), $true)
  Assert-Throws -Name "schema2 after payload cleanup" -MessagePattern "staged file is missing" -Action {
    Assert-WindowsRuntimeStagedFilesExact -Manifest $schema2Manifest -StageRoot $schema2Root
  }

  # Write-Utf8Atomically keeps shared-visibility descriptors continuously
  # present for the unlocked readers in scripts/tauri/windows-runtime.js:
  # replacement must swap the file in atomically, a failed replacement must
  # preserve the previous generation and leave no temp litter behind.
  . (Join-Path $PSScriptRoot "resolve-runtime.ps1") -ImportFunctionsOnly
  $atomicRoot = Join-Path $fixtureRoot "atomic"
  New-Item -ItemType Directory -Path $atomicRoot | Out-Null
  $atomicPath = Join-Path $atomicRoot "descriptor.json"
  Write-Utf8Atomically -Path $atomicPath -Content "first"
  if ([System.IO.File]::ReadAllText($atomicPath) -ne "first") {
    throw "atomic write must create the destination with the requested content"
  }
  foreach ($generation in @("second", "third")) {
    Write-Utf8Atomically -Path $atomicPath -Content $generation
    if ([System.IO.File]::ReadAllText($atomicPath) -ne $generation) {
      throw "atomic replacement must swap in the new content in one operation"
    }
  }
  # A plain .NET reader opens the descriptor without FILE_SHARE_DELETE, which
  # blocks ReplaceFile: the writer must fail loudly instead of deleting the
  # destination first (Move-Item -Force semantics).
  $sharingReader = [System.IO.File]::Open(
    $atomicPath,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::Read
  )
  try {
    Assert-Throws -Name "atomic replace under a sharing reader" -Action {
      Write-Utf8Atomically -Path $atomicPath -Content "interrupted"
    }
  } finally {
    $sharingReader.Dispose()
  }
  if ([System.IO.File]::ReadAllText($atomicPath) -ne "third") {
    throw "a failed atomic replacement must preserve the previous descriptor"
  }
  $tempLitter = @(Get-ChildItem -LiteralPath $atomicRoot -Filter ".descriptor.json.tmp-*" -Force)
  if ($tempLitter.Count -ne 0) {
    throw "atomic replacement left temp files behind: $($tempLitter.Count)"
  }
  Write-Utf8Atomically -Path $atomicPath -Content "fourth"
  if ([System.IO.File]::ReadAllText($atomicPath) -ne "fourth") {
    throw "atomic replacement must succeed again once the reader is gone"
  }
} finally {
  Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Windows runtime manifest lifecycle fixture: ok"
