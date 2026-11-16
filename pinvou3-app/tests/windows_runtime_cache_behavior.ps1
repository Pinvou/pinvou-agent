$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$resolverPath = Join-Path $appRoot "src-tauri\packaging\windows\runtime\scripts\resolve-runtime.ps1"
$runtimeInitPath = Join-Path $appRoot "src-tauri\packaging\windows\runtime\scripts\init-submodule.ps1"
. $resolverPath -ImportFunctionsOnly
. $runtimeInitPath -ImportFunctionsOnly

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) {
    throw $Message
  }
}

function Write-TestFile {
  param([string]$Path, [string]$Content)
  New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
  [System.IO.File]::WriteAllText($Path, $Content, [System.Text.Encoding]::UTF8)
}

function Write-StageCacheMetadata {
  param([string]$Root)
  $writtenManifestPath = Write-StageManifest -Root $Root
  $manifestSha256 = Get-Sha256 -Path $writtenManifestPath
  Write-Utf8WithoutBom -Path (Join-Path $Root ".verified-lock") -Content (Get-VerificationMarkerContent -StageManifestSha256 $manifestSha256)
}

function Test-FixtureStageReusable {
  param($Manifest)
  return Test-VerifiedStageReusable -Manifest $Manifest -PythonDependencyProbe { param($PythonRoot) $true }
}

function New-PythonDependencyTarget {
  param([string]$Target, [string]$Filename)
  $wheelSha = "a" * 64
  if ($Target -cnotmatch '^cp(?<major>\d)(?<minor>\d+)-win_amd64$') {
    throw "invalid test target: $Target"
  }
  return [pscustomobject]@{
    platform = "windows-x64"
    python = "$($Matches['major']).$($Matches['minor'])"
    imports = [object[]]@("demo")
    wheels = [object[]]@(
      [pscustomobject]@{
        name = "demo"
        version = "1"
        filename = $Filename
        url = "https://files.pythonhosted.org/packages/$Filename"
        sha256 = $wheelSha
      }
    )
  }
}

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou3-runtime-cache-test-" + [System.Guid]::NewGuid().ToString("N"))
try {
  New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
  foreach ($filename in @(
    "demo-1-cp313-cp313-win_amd64.whl",
    "demo-1-cp313-abi3-win_amd64.whl",
    "demo-1-cp312-abi3-win_amd64.whl",
    "demo-1-cp32-abi3-win_amd64.whl",
    "demo-1-cp313-none-any.whl",
    "demo-1-py313-none-any.whl",
    "demo-1-py312-none-any.whl",
    "demo-1-py313-none-win_amd64.whl",
    "demo-1-py3-none-any.whl"
  )) {
    Assert-True -Condition (Test-PythonWheelTarget -Filename $filename -Target "cp313-win_amd64") -Message "compatible PEP 425 tag should pass: $filename"
    Assert-True -Condition (Test-PythonDependencyTarget -Target (New-PythonDependencyTarget -Target "cp313-win_amd64" -Filename $filename)) -Message "valid target-bound dependency lock should pass: $filename"
  }
  foreach ($filename in @(
    "demo-1-cp27-abi3-win_amd64.whl",
    "demo-1-cp31-abi3-win_amd64.whl",
    "demo-1-cp314-abi3-win_amd64.whl",
    "demo-1-py314-none-any.whl",
    "demo-1-cp313-cp313-win_arm64.whl",
    "demo-1-CP313-cp313-win_amd64.whl",
    "demo-1-cp313-cp313-any.whl",
    "demo-1-cp313-abi3-any.whl"
  )) {
    Assert-True -Condition (-not (Test-PythonWheelTarget -Filename $filename -Target "cp313-win_amd64")) -Message "incompatible PEP 425 tag should fail: $filename"
  }
  $targetDrift = New-PythonDependencyTarget -Target "cp313-win_amd64" -Filename "demo-1-cp313-cp313-win_amd64.whl"
  $targetDrift.python = "3.14"
  Assert-True -Condition (-not (Test-PythonDependencyTarget -Target $targetDrift)) -Message "dependency lock must bind target canonically"
  $uppercaseSha = New-PythonDependencyTarget -Target "cp313-win_amd64" -Filename "demo-1-py3-none-any.whl"
  $uppercaseSha.wheels[0].sha256 = "A" * 64
  Assert-True -Condition (-not (Test-PythonDependencyTarget -Target $uppercaseSha)) -Message "wheel SHA must be lowercase hexadecimal"
  $nonStringTarget = New-PythonDependencyTarget -Target "cp313-win_amd64" -Filename "demo-1-py3-none-any.whl"
  $nonStringTarget.python = 313
  Assert-True -Condition (-not (Test-PythonDependencyTarget -Target $nonStringTarget)) -Message "dependency target must remain a JSON string"
  $emptyImports = New-PythonDependencyTarget -Target "cp313-win_amd64" -Filename "demo-1-py3-none-any.whl"
  $emptyImports.imports = [object[]]@()
  Assert-True -Condition (-not (Test-PythonDependencyTarget -Target $emptyImports)) -Message "dependency imports must be a non-empty array"
  Assert-True -Condition (Test-PythonDependencyTargets -PythonRoot $testRoot -AbiProbe { param($PythonExe, $Target) $Target.python -ceq "3.13" -and $Target.platform -ceq "windows-x64" }) -Message "manifest ABI/canonical gate should accept matching injected runtime ABI"
  Assert-True -Condition (-not (Test-PythonDependencyTargets -PythonRoot $testRoot -AbiProbe { param($PythonExe, $Target) $false })) -Message "manifest ABI gate should reject a mismatched injected runtime ABI"

  $successfulGitCommand = Join-Path $testRoot "git-success.cmd"
  $failingGitCommand = Join-Path $testRoot "git-failure.cmd"
  [System.IO.File]::WriteAllText(
    $successfulGitCommand,
    "@echo off`r`necho normal git progress 1>&2`r`nexit /b 0`r`n",
    [System.Text.Encoding]::ASCII
  )
  [System.IO.File]::WriteAllText(
    $failingGitCommand,
    "@echo off`r`necho real git failure 1>&2`r`nexit /b 7`r`n",
    [System.Text.Encoding]::ASCII
  )
  $successfulGitOutput = @(Invoke-Git -WorkingDirectory $testRoot -Arguments @("status") -FailureMessage "unexpected" -GitCommand $successfulGitCommand)
  Assert-True -Condition (($successfulGitOutput -join " ") -match "normal git progress") -Message "successful Git stderr must be retained without failing Windows PowerShell"
  $failedGitWasRejected = $false
  try {
    Invoke-Git -WorkingDirectory $testRoot -Arguments @("status") -FailureMessage "expected failure:" -GitCommand $failingGitCommand | Out-Null
  } catch {
    $failedGitWasRejected = $_.Exception.Message -match "expected failure:.*real git failure"
  }
  Assert-True -Condition $failedGitWasRejected -Message "non-zero Git exit codes must fail and retain stderr diagnostics"

  $firstGitDirectory = Join-Path $testRoot "git-first"
  $secondGitDirectory = Join-Path $testRoot "git-second"
  $duplicateGitName = "git-duplicate.cmd"
  New-Item -ItemType Directory -Path $firstGitDirectory, $secondGitDirectory -Force | Out-Null
  [System.IO.File]::WriteAllText(
    (Join-Path $firstGitDirectory $duplicateGitName),
    "@echo off`r`necho first git executable`r`nexit /b 0`r`n",
    [System.Text.Encoding]::ASCII
  )
  [System.IO.File]::WriteAllText(
    (Join-Path $secondGitDirectory $duplicateGitName),
    "@echo off`r`necho second git executable`r`nexit /b 0`r`n",
    [System.Text.Encoding]::ASCII
  )
  $previousPath = $env:PATH
  try {
    $env:PATH = "$firstGitDirectory;$secondGitDirectory;$previousPath"
    $duplicateCommands = @(Get-Command -Name "git-duplicate" -CommandType Application -ErrorAction Stop)
    Assert-True -Condition ($duplicateCommands.Count -ge 2) -Message "duplicate Git command test requires at least two PATH matches"
    $duplicateGitOutput = @(Invoke-Git -WorkingDirectory $testRoot -Arguments @("status") -FailureMessage "unexpected" -GitCommand "git-duplicate")
    Assert-True -Condition (($duplicateGitOutput -join " ") -match "first git executable") -Message "Invoke-Git must select one executable instead of concatenating duplicate PATH matches"
    Assert-True -Condition (($duplicateGitOutput -join " ") -notmatch "second git executable") -Message "Invoke-Git must use only the highest-precedence executable"
  } finally {
    $env:PATH = $previousPath
  }

  # Behavioral coverage for the LFS relocation contract: the submodule update
  # must run with GIT_LFS_SKIP_SMUDGE=1 in effect (pointers only), so the
  # explicit `git lfs pull -c lfs.storage=...` stays the single
  # materialization step into the relocated cache, and the previous env value
  # must be restored afterwards.
  $script:lfsRecorderCalls = New-Object System.Collections.Generic.List[object]
  function Invoke-Git {
    param(
      [string]$WorkingDirectory,
      [string[]]$Arguments,
      [string]$FailureMessage,
      [string]$GitCommand = "git"
    )
    $script:lfsRecorderCalls.Add([pscustomobject]@{
      WorkingDirectory = $WorkingDirectory
      Arguments = [string[]]$Arguments
      SkipSmudge = $env:GIT_LFS_SKIP_SMUDGE
    })
    return @()
  }
  $previousLfsSkipSmudge = $env:GIT_LFS_SKIP_SMUDGE
  try {
    $env:GIT_LFS_SKIP_SMUDGE = $null
    Update-RuntimeSubmoduleWithoutLfsSmudge
  } finally {
    $env:GIT_LFS_SKIP_SMUDGE = $previousLfsSkipSmudge
    # Restore the production Invoke-Git for the remainder of the test run.
    . $runtimeInitPath -ImportFunctionsOnly
  }
  Assert-True -Condition ($script:lfsRecorderCalls.Count -eq 1) -Message "the submodule update must invoke Git exactly once"
  $lfsRecorderCall = $script:lfsRecorderCalls[0]
  Assert-True -Condition ($lfsRecorderCall.SkipSmudge -ceq "1") -Message "the submodule update must run under GIT_LFS_SKIP_SMUDGE=1 so pointers are materialized only by the explicit lfs pull"
  Assert-True -Condition (@($lfsRecorderCall.Arguments) -ccontains "submodule" -and @($lfsRecorderCall.Arguments) -ccontains "update") -Message "the submodule update arguments must drive the gitlink checkout"
  Assert-True -Condition ($null -eq $env:GIT_LFS_SKIP_SMUDGE) -Message "GIT_LFS_SKIP_SMUDGE must be restored after the submodule update"

  $stagingParent = Join-Path $testRoot "windows-runtime"
  $stagingRoot = Join-Path $stagingParent $stageId
  $stageManifestPath = Join-Path $stagingRoot ".stage-manifest.json"
  $generatedConfigPath = Join-Path $stagingParent "tauri.generated.conf.json"
  $runtimeDescriptorPath = Join-Path $stagingParent "runtime-descriptor.json"
  $resolverLockPath = Join-Path $stagingParent ".resolver.lock"

  $firstLock = Enter-ResolverLock -TimeoutSeconds 0
  try {
    $secondLockWasRejected = $false
    try {
      $unexpectedLock = Enter-ResolverLock -TimeoutSeconds 0
      $unexpectedLock.Dispose()
    } catch {
      $secondLockWasRejected = $true
    }
    Assert-True -Condition $secondLockWasRejected -Message "concurrent resolver access must be rejected while the cache lock is held"
  } finally {
    $firstLock.Dispose()
  }
  $releasedLock = Enter-ResolverLock -TimeoutSeconds 0
  $releasedLock.Dispose()

  $bootstrapperRelativePath = "payload/vc_redist/VC_redist.x64.exe"
  $bootstrapperSourcePath = Join-Path $stagingRoot $bootstrapperRelativePath.Replace('/', '\')
  $runtimeExecutablePath = Join-Path $stagingRoot "expanded\poppler\pdftotext.exe"
  $runtimeDependencyPath = Join-Path $stagingRoot "expanded\poppler\support.dll"
  Write-TestFile -Path $bootstrapperSourcePath -Content "bootstrapper"
  Write-TestFile -Path $runtimeExecutablePath -Content "executable"
  Write-TestFile -Path $runtimeDependencyPath -Content "abc"

  Write-TauriOverlay -StageId $stageId

  # The overlay is consumed by build.js and the NSIS staging script; pin its
  # emitted shape instead of leaving the call side-effect-free.
  Assert-True -Condition (Test-Path -LiteralPath $generatedConfigPath -PathType Leaf) -Message "Write-TauriOverlay must emit tauri.generated.conf.json"
  $generatedOverlay = ConvertFrom-Json -InputObject ([System.IO.File]::ReadAllText($generatedConfigPath, [System.Text.Encoding]::UTF8))
  $overlayProperties = @($generatedOverlay.bundle.resources.PSObject.Properties)
  Assert-True -Condition ($overlayProperties.Count -gt 0) -Message "the generated runtime overlay must declare bundle.resources mappings"
  Assert-True -Condition (@($overlayProperties | Where-Object { [string]::IsNullOrEmpty([string]$_.Value) }).Count -eq 0) -Message "runtime overlay mappings must carry non-empty destinations"

  $trustedStagedFiles = @(Get-StagedRelativeFiles -Root $stagingRoot)
  Write-StageCacheMetadata -Root $stagingRoot

  $bootstrapperItem = Get-Item -LiteralPath $bootstrapperSourcePath
  $manifest = [pscustomobject]@{
    stagedFiles = $trustedStagedFiles
    files = @(
      [pscustomobject]@{
        path = $bootstrapperRelativePath
        bytes = [long]$bootstrapperItem.Length
        sha256 = Get-Sha256 -Path $bootstrapperSourcePath
      }
    )
  }
  Write-RuntimeDescriptor -Manifest $manifest -StageId $stageId

  Assert-True -Condition (Test-FixtureStageReusable -Manifest $manifest) -Message "fresh verified staging should be reusable"

  Write-TestFile -Path $runtimeDescriptorPath -Content "tampered descriptor"
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "tampered runtime descriptor must invalidate the cache"
  Write-RuntimeDescriptor -Manifest $manifest -StageId $stageId
  Assert-True -Condition (Test-FixtureStageReusable -Manifest $manifest) -Message "restoring the runtime descriptor should restore reuse"

  Write-TestFile -Path $runtimeDependencyPath -Content "xyz"
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "same-size staged content tampering must invalidate the cache"

  Write-StageCacheMetadata -Root $stagingRoot
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "rewriting cached content, stage manifest and verified lock together must not forge trust"

  Write-TestFile -Path $runtimeDependencyPath -Content "abc"
  Write-StageCacheMetadata -Root $stagingRoot
  Assert-True -Condition (Test-FixtureStageReusable -Manifest $manifest) -Message "restoring the verified staged content should restore reuse"

  Remove-Item -LiteralPath $runtimeDependencyPath -Force
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "missing staged dependencies must invalidate the cache"

  Write-TestFile -Path $runtimeDependencyPath -Content "abc"
  Write-TestFile -Path (Join-Path $stagingRoot "expanded\poppler\unexpected.dll") -Content "unexpected"
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "unexpected staged files must invalidate the cache"
  Remove-Item -LiteralPath (Join-Path $stagingRoot "expanded\poppler\unexpected.dll") -Force

  $nestedMarkerPath = Join-Path $stagingRoot "expanded\poppler\nested\.verified-lock"
  $nestedManifestPath = Join-Path $stagingRoot "expanded\poppler\nested\.stage-manifest.json"
  Write-TestFile -Path $nestedMarkerPath -Content "nested marker payload"
  Write-TestFile -Path $nestedManifestPath -Content "nested manifest payload"
  $stagedPaths = @(Get-StagedRelativePaths -Root $stagingRoot)
  Assert-True -Condition ($stagedPaths -contains "expanded/poppler/nested/.verified-lock") -Message "nested .verified-lock files must remain in the complete staged-file set"
  Assert-True -Condition ($stagedPaths -contains "expanded/poppler/nested/.stage-manifest.json") -Message "nested .stage-manifest.json files must remain in the complete staged-file set"
  Assert-True -Condition (-not (Test-FixtureStageReusable -Manifest $manifest)) -Message "unexpected nested metadata-named files must invalidate the cache"
  Remove-Item -LiteralPath $nestedMarkerPath -Force
  Remove-Item -LiteralPath $nestedManifestPath -Force

  $duplicateTrustedEntries = @($trustedStagedFiles) + @($trustedStagedFiles[0])
  Assert-True -Condition (-not (Test-StagedFilesAgainstTrustedManifest -Root $stagingRoot -TrustedEntries $duplicateTrustedEntries)) -Message "duplicate trusted paths must be rejected"

  # Same bytes and path modulo casing: the rejection must be
  # case-insensitive (OrdinalIgnoreCase), or two trusted entries that only
  # differ by case would stage the same file twice.
  $caseVariantTrustedEntries = @($trustedStagedFiles) + @(
    [pscustomobject]@{
      path = $bootstrapperRelativePath -creplace 'VC_redist', 'vc_REDIST'
      bytes = [long]$bootstrapperItem.Length
      sha256 = (Get-Sha256 -Path $bootstrapperSourcePath)
    }
  )
  Assert-True -Condition ($caseVariantTrustedEntries[-1].path -cne $bootstrapperRelativePath) -Message "case-variant fixture must differ from the trusted path letter-for-letter"
  Assert-True -Condition (-not (Test-StagedFilesAgainstTrustedManifest -Root $stagingRoot -TrustedEntries $caseVariantTrustedEntries)) -Message "case-insensitive duplicate trusted paths must be rejected"
  $escapingEntry = [pscustomobject]@{ path = "../escape.dll"; bytes = 0; sha256 = ("0" * 64) }
  Assert-True -Condition (-not (Test-StagedFilesAgainstTrustedManifest -Root $stagingRoot -TrustedEntries @($escapingEntry))) -Message "trusted paths escaping the staging root must be rejected"

  Write-Host "Windows runtime cache behavior: ok"
} finally {
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
