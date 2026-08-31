$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$resolverPath = Join-Path $appRoot "src-tauri\packaging\windows\runtime\scripts\resolve-runtime.ps1"
. $resolverPath -ImportFunctionsOnly

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) {
    throw $Message
  }
}

function New-ValidManifestFixture {
  return (@'
{
  "id": "fixture",
  "command": "python",
  "args": ["server.py"],
  "pip_dependencies": ["demo"],
  "python_dependencies": {
    "schema_version": 1,
    "targets": [{
      "platform": "windows-x64",
      "python": "3.13",
      "imports": ["demo"],
      "wheels": [{
        "name": "demo",
        "version": "1.0.0",
        "filename": "demo-1.0.0-py3-none-any.whl",
        "url": "https://files.pythonhosted.org/packages/demo-1.0.0-py3-none-any.whl",
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }]
    }]
  }
}
'@ | ConvertFrom-Json)
}

function Copy-ManifestFixture {
  param($Manifest)
  return ($Manifest | ConvertTo-Json -Depth 20 | ConvertFrom-Json)
}

function Assert-ManifestFixtureRejected {
  param([string]$Name, $Manifest, [string]$FixtureRoot, [string]$PythonRoot)
  $manifestDirectory = Join-Path $FixtureRoot $Name
  New-Item -ItemType Directory -Path $manifestDirectory -Force | Out-Null
  $manifestPath = Join-Path $manifestDirectory "manifest.json"
  [System.IO.File]::WriteAllText(
    $manifestPath,
    (($Manifest | ConvertTo-Json -Depth 20) + "`n"),
    [System.Text.UTF8Encoding]::new($false)
  )
  Assert-True -Condition (-not (Test-PythonDependencyTargets -PythonRoot $PythonRoot -ManifestRoot $manifestDirectory -AbiProbe { param($PythonExe, $Target) $true })) -Message "invalid manifest fixture was accepted: $Name"
}

$compatibleWheels = @(
  "demo-1-cp313-cp313-win_amd64.whl",
  "demo-1-cp313-abi3-win_amd64.whl",
  "demo-1-cp312-abi3-win_amd64.whl",
  "demo-1-py313-none-any.whl",
  "demo-1-py3-none-any.whl"
)
foreach ($filename in $compatibleWheels) {
  Assert-True -Condition (Test-PythonWheelTarget -Filename $filename -Target "cp313-win_amd64") -Message "compatible wheel target was rejected: $filename"
}
foreach ($filename in @(
  "demo-1-cp314-cp314-win_amd64.whl",
  "demo-1-cp313-cp313-win_arm64.whl",
  "demo-1-cp313-abi3-any.whl",
  "demo-1-CP313-cp313-win_amd64.whl"
)) {
  Assert-True -Condition (-not (Test-PythonWheelTarget -Filename $filename -Target "cp313-win_amd64")) -Message "incompatible wheel target was accepted: $filename"
}

$manifestPaths = @(Get-ChildItem -LiteralPath (Join-Path $appRoot "resources\mcp-servers") -Filter manifest.json -File -Recurse)
$lockedManifestCount = 0
foreach ($manifestPath in $manifestPaths) {
  $manifest = Get-Content -LiteralPath $manifestPath.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($null -eq $manifest.python_dependencies) {
    continue
  }
  $lockedManifestCount += 1
  Assert-True -Condition ([int]$manifest.python_dependencies.schema_version -eq 1) -Message "unexpected dependency schema: $($manifestPath.FullName)"
  foreach ($target in @($manifest.python_dependencies.targets)) {
    Assert-True -Condition (Test-PythonDependencyTarget -Target $target) -Message "invalid reviewed dependency target: $($manifestPath.FullName)"
  }
}
Assert-True -Condition ($lockedManifestCount -eq 2) -Message "expected exactly the reviewed gongwen and pptx dependency locks"

$probeCount = 0
$matchingProbe = {
  param($PythonExe, $Target)
  $script:probeCount += 1
  return [string]$Target.platform -ceq "windows-x64" -and [string]$Target.python -ceq "3.13"
}
$fakePythonRoot = Join-Path ([System.IO.Path]::GetTempPath()) "pinvou-python-contract-no-runtime"
Assert-True -Condition (Test-PythonDependencyTargets -PythonRoot $fakePythonRoot -AbiProbe $matchingProbe) -Message "reviewed manifests must pass a matching bundled-Python ABI probe"
Assert-True -Condition ($probeCount -eq 2) -Message "every locked built-in MCP must be ABI-probed"
Assert-True -Condition (-not (Test-PythonDependencyTargets -PythonRoot $fakePythonRoot -AbiProbe { param($PythonExe, $Target) $false })) -Message "an ABI mismatch must fail closed"

$fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou-python-manifest-contract-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null

# Behavioral coverage for the real ABI probe branch (no -AbiProbe injection). Windows
# cannot fake an executable named python.exe without a real PE image, so copy the PATH
# python and stub its platform module: the probe then prints to stdout with a
# controllable machine() result, which is exactly the stray-output case the
# "$null = & $pythonExe" fix guards. Skipped when no runnable PATH python exists.
$pathPython = (Get-Command python.exe -ErrorAction SilentlyContinue).Source
$realProbeReady = $false
$realProbeRoot = $null
$probeVersion = ""
if ($pathPython) {
  try {
    $null = & $pathPython -c "print('sanity')"
    if ($LASTEXITCODE -eq 0) {
      $realProbeRoot = Join-Path $fixtureRoot "real-probe-python"
      $pathPythonDir = Split-Path -Parent $pathPython
      New-Item -ItemType Directory -Path $realProbeRoot -Force | Out-Null
      Copy-Item -LiteralPath $pathPython -Destination (Join-Path $realProbeRoot "python.exe")
      Copy-Item -Path (Join-Path $pathPythonDir "python3*.dll") -Destination $realProbeRoot -ErrorAction SilentlyContinue
      Copy-Item -Path (Join-Path $pathPythonDir "vcruntime140*.dll") -Destination $realProbeRoot -ErrorAction SilentlyContinue
      Copy-Item -Path (Join-Path $pathPythonDir "Lib") -Destination (Join-Path $realProbeRoot "Lib") -Recurse
      $realProbePython = Join-Path $realProbeRoot "python.exe"
      $probeVersion = ([string](& $realProbePython -c "import sys; print('%d.%d' % sys.version_info[:2])")).Trim()
      $realProbeReady = $probeVersion -cmatch '^\d+\.\d+$'
    }
  } catch {
    $realProbeReady = $false
  }
}
if ($realProbeReady) {
  function Write-StubPlatform {
    param([string]$PythonRoot, [string]$Machine)
    $stub = "import sys`nprint(`"stray probe output`")`ndef machine():`n    return `"$Machine`"`n"
    [System.IO.File]::WriteAllText(
      (Join-Path $PythonRoot "Lib\platform.py"),
      $stub,
      [System.Text.UTF8Encoding]::new($false)
    )
  }
  function New-RealProbeManifestRoot {
    param([string]$Name, [string]$PythonVersion)
    $directory = Join-Path $fixtureRoot $Name
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $manifest = New-ValidManifestFixture
    $manifest.python_dependencies.targets[0].python = $PythonVersion
    [System.IO.File]::WriteAllText(
      (Join-Path $directory "manifest.json"),
      (($manifest | ConvertTo-Json -Depth 20) + "`n"),
      [System.Text.UTF8Encoding]::new($false)
    )
    return $directory
  }

  $probePassRoot = New-RealProbeManifestRoot -Name "real-probe-pass" -PythonVersion $probeVersion
  Write-StubPlatform -PythonRoot $realProbeRoot -Machine "AMD64"
  Assert-True -Condition (Test-PythonDependencyTargets -PythonRoot $realProbeRoot -ManifestRoot $probePassRoot) -Message "a matching real ABI probe must stay accepted when it prints to stdout"

  $probeFailRoot = New-RealProbeManifestRoot -Name "real-probe-fail" -PythonVersion $probeVersion
  Write-StubPlatform -PythonRoot $realProbeRoot -Machine "i386"
  Assert-True -Condition (-not (Test-PythonDependencyTargets -PythonRoot $realProbeRoot -ManifestRoot $probeFailRoot)) -Message "a real ABI probe printing to stdout with a non-zero exit must be rejected"
}

$wrongSha = New-ValidManifestFixture
$wrongSha.python_dependencies.targets[0].wheels[0].sha256 = "z" * 64
Assert-ManifestFixtureRejected -Name "wrong-sha" -Manifest $wrongSha -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$untrustedHost = New-ValidManifestFixture
$untrustedHost.python_dependencies.targets[0].wheels[0].url = "https://example.com/demo-1.0.0-py3-none-any.whl"
Assert-ManifestFixtureRejected -Name "untrusted-host" -Manifest $untrustedHost -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$unsafeFilename = New-ValidManifestFixture
$unsafeFilename.python_dependencies.targets[0].wheels[0].filename = "../demo-1.0.0-py3-none-any.whl"
Assert-ManifestFixtureRejected -Name "unsafe-filename" -Manifest $unsafeFilename -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$mismatchedFilename = New-ValidManifestFixture
$mismatchedFilename.python_dependencies.targets[0].wheels[0].url = "https://files.pythonhosted.org/packages/other-1.0.0-py3-none-any.whl"
Assert-ManifestFixtureRejected -Name "url-filename-mismatch" -Manifest $mismatchedFilename -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$invalidSchema = New-ValidManifestFixture
$invalidSchema.python_dependencies.schema_version = 2
Assert-ManifestFixtureRejected -Name "invalid-schema" -Manifest $invalidSchema -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$duplicateWindows = New-ValidManifestFixture
$duplicateWindows.python_dependencies.targets = [object[]]@(
  $duplicateWindows.python_dependencies.targets[0],
  (Copy-ManifestFixture -Manifest $duplicateWindows.python_dependencies.targets[0])
)
Assert-ManifestFixtureRejected -Name "duplicate-windows-target" -Manifest $duplicateWindows -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$missingWindows = New-ValidManifestFixture
$missingWindows.python_dependencies.targets[0].platform = "linux-x64"
Assert-ManifestFixtureRejected -Name "missing-windows-target" -Manifest $missingWindows -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$missingLock = New-ValidManifestFixture
$missingLock.PSObject.Properties.Remove("python_dependencies")
Assert-ManifestFixtureRejected -Name "pip-without-lock" -Manifest $missingLock -FixtureRoot $fixtureRoot -PythonRoot $fakePythonRoot

$freshRejected = $false
try {
  Assert-FreshStagePythonDependencies -PythonRoot $fakePythonRoot -PythonDependencyProbe { param($PythonRoot) $false }
} catch {
  $freshRejected = $_.Exception.Message -match "Bundled Python ABI is incompatible"
}
Assert-True -Condition $freshRejected -Message "fresh staging must reject a failed Python dependency probe"

$savedStageId = $stageId
$savedStagingParent = $stagingParent
$savedStagingRoot = $stagingRoot
$savedGeneratedConfigPath = $generatedConfigPath
$savedRuntimeDescriptorPath = $runtimeDescriptorPath
try {
  $stageId = "python-contract-stage"
  $stagingParent = Join-Path $fixtureRoot "windows-runtime"
  $stagingRoot = Join-Path $stagingParent $stageId
  $generatedConfigPath = Join-Path $stagingParent "tauri.generated.conf.json"
  $runtimeDescriptorPath = Join-Path $stagingParent "runtime-descriptor.json"
  New-Item -ItemType Directory -Path (Join-Path $stagingRoot "expanded\python") -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $stagingRoot "expanded\vc_redist") -Force | Out-Null
  [System.IO.File]::WriteAllText((Join-Path $stagingRoot "expanded\python\python.exe"), "fixture")
  [System.IO.File]::WriteAllText((Join-Path $stagingRoot "expanded\vc_redist\VC_redist.x64.exe"), "fixture")
  Write-Utf8WithoutBom -Path (Join-Path $stagingRoot ".verified-stage.json") -Content (Get-StageInventoryContent -StageRoot $stagingRoot)
  Write-Utf8WithoutBom -Path (Join-Path $stagingRoot ".verified-lock") -Content (Get-VerificationMarkerContent)
  Write-TauriOverlay -StageId $stageId
  Write-RuntimeDescriptor -Manifest ([pscustomobject]@{}) -StageId $stageId

  Assert-True -Condition (Test-VerifiedStageReusable -Manifest ([pscustomobject]@{}) -PythonDependencyProbe { param($PythonRoot) $true }) -Message "verified-stage fixture must be reusable before dependency probe failure injection"
  Assert-True -Condition (-not (Test-VerifiedStageReusable -Manifest ([pscustomobject]@{}) -PythonDependencyProbe { param($PythonRoot) $false })) -Message "verified-stage reuse must reject a failed Python dependency probe"
} finally {
  $stageId = $savedStageId
  $stagingParent = $savedStagingParent
  $stagingRoot = $savedStagingRoot
  $generatedConfigPath = $savedGeneratedConfigPath
  $runtimeDescriptorPath = $savedRuntimeDescriptorPath
}

$resolverSource = Get-Content -LiteralPath $resolverPath -Raw -Encoding UTF8
$reuseStart = $resolverSource.IndexOf("function Test-VerifiedStageReusable")
$freshStart = $resolverSource.IndexOf("function Stage-Submodule", $reuseStart)
$onnxStart = $resolverSource.IndexOf("function Get-OnnxDevDescriptorContent", $freshStart)
Assert-True -Condition ($reuseStart -ge 0 -and $freshStart -gt $reuseStart -and $onnxStart -gt $freshStart) -Message "runtime resolver function boundaries changed unexpectedly"
$reuseBody = $resolverSource.Substring($reuseStart, $freshStart - $reuseStart)
$freshBody = $resolverSource.Substring($freshStart, $onnxStart - $freshStart)
Assert-True -Condition ($reuseBody.Contains("Test-PythonDependencyTargets")) -Message "verified runtime reuse must revalidate Python locks and ABI"
Assert-True -Condition ($freshBody.Contains("Assert-FreshStagePythonDependencies")) -Message "fresh runtime staging must validate Python locks and ABI before publication"

Write-Host "Windows Python dependency contract: ok"
Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
