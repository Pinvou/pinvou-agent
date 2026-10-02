param(
  [string]$Toolchain
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Toolchain)) {
  $toolchainFile = Join-Path $PSScriptRoot "..\src-tauri\rust-toolchain.toml"
  $toolchainConfig = Get-Content -LiteralPath $toolchainFile -Raw
  $channelMatch = [regex]::Match(
    $toolchainConfig,
    '(?m)^\s*channel\s*=\s*"([^"]+)"'
  )
  if (-not $channelMatch.Success) {
    throw "Unable to read the Rust channel from: $toolchainFile"
  }
  $Toolchain = $channelMatch.Groups[1].Value
}

$rustupHome = (& rustup show home).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($rustupHome)) {
  throw "Unable to resolve the current rustup home."
}

$installedToolchain = (& rustup toolchain list) |
  Where-Object { $_ -match "^$([regex]::Escape($Toolchain))-" } |
  Select-Object -First 1
if (-not $installedToolchain) {
  throw "Source toolchain is not installed locally: $Toolchain"
}
$installedToolchain = ($installedToolchain -split "\s+")[0]
$sourceToolchain = Join-Path $rustupHome "toolchains\$installedToolchain"

$temporaryRoot = Join-Path (
  [IO.Path]::GetTempPath()
) ("pinvou-rustup-repair-" + [guid]::NewGuid().ToString("N"))
$temporaryRustup = Join-Path $temporaryRoot "rustup"
$temporaryCargo = Join-Path $temporaryRoot "cargo"
$temporaryToolchains = Join-Path $temporaryRustup "toolchains"
$temporaryToolchain = Join-Path $temporaryToolchains $installedToolchain
$repairScript = Join-Path $PSScriptRoot "..\scripts\ci\ensure-rust-toolchain.ps1"

& $repairScript -CheckOnly
if ($LASTEXITCODE -ne 0) {
  throw "Account toolchain check failed before the isolated repair smoke test."
}

try {
  # The repair engine's refusal guards are the core safety contract: they must
  # reject the shared account rustup, a filesystem root, a non-empty unmarked
  # directory, a missing RUSTUP_HOME and a run without the managed flag before
  # touching anything. Exercise them for real here; every case below throws
  # before rustup runs, so at most an empty lock file is created inside the
  # throwaway test home.
  $previousRustupHome = $env:RUSTUP_HOME
  $previousCargoHome = $env:CARGO_HOME
  $previousManagedFlag = $env:PINVOU3_MANAGED_RUSTUP
  $previousDistServer = $env:RUSTUP_DIST_SERVER
  $previousUpdateRoot = $env:RUSTUP_UPDATE_ROOT
  $previousDownloadTimeout = $env:RUSTUP_DOWNLOAD_TIMEOUT
  $env:PINVOU3_MANAGED_RUSTUP = "1"
  $unmarkedHome = Join-Path $temporaryRoot "unmarked"
  New-Item -ItemType Directory -Path $unmarkedHome -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $unmarkedHome "placeholder") `
    -Value "not a rustup home" -Encoding Ascii
  $guardRejections = @(
    [pscustomobject]@{
      Name = "the build account's shared rustup home"
      Home = Join-Path $env:USERPROFILE ".rustup"
      Managed = "1"
      Message = "Refusing to modify the build account's shared RUSTUP_HOME"
    },
    [pscustomobject]@{
      Name = "a filesystem root"
      Home = [IO.Path]::GetPathRoot(([IO.Path]::GetFullPath($temporaryRoot)))
      Managed = "1"
      Message = "Refusing to use a filesystem root as RUSTUP_HOME"
    },
    [pscustomobject]@{
      Name = "a non-empty unmarked directory"
      Home = $unmarkedHome
      Managed = "1"
      Message = "Refusing to adopt a non-empty unmarked RUSTUP_HOME"
    },
    [pscustomobject]@{
      Name = "a missing RUSTUP_HOME"
      Home = $null
      Managed = "1"
      Message = "Refusing automatic repair without an isolated RUSTUP_HOME"
    },
    [pscustomobject]@{
      Name = "a run without the managed flag"
      Home = $temporaryRustup
      Managed = $null
      Message = "Refusing automatic repair without PINVOU3_MANAGED_RUSTUP=1"
    }
  )
  foreach ($guard in $guardRejections) {
    if ($null -eq $guard.Home) {
      Remove-Item Env:RUSTUP_HOME -ErrorAction SilentlyContinue
    } else {
      $env:RUSTUP_HOME = $guard.Home
    }
    if ($null -eq $guard.Managed) {
      Remove-Item Env:PINVOU3_MANAGED_RUSTUP -ErrorAction SilentlyContinue
    } else {
      $env:PINVOU3_MANAGED_RUSTUP = $guard.Managed
    }
    $rejectionMessage = $null
    try {
      & $repairScript
    } catch {
      $rejectionMessage = "$($_.Exception.Message)"
    }
    if ($null -eq $rejectionMessage -or -not $rejectionMessage.Contains($guard.Message)) {
      throw (
        "The repair guard did not reject {0} (got: {1}); RUSTUP_HOME={2}" -f
        $guard.Name, $rejectionMessage, $guard.Home
      )
    }
    Write-Host "[test] Guard rejected $($guard.Name): OK"
  }
  $env:PINVOU3_MANAGED_RUSTUP = "1"

  New-Item -ItemType Directory -Path $temporaryToolchains, $temporaryCargo -Force |
    Out-Null
  Set-Content -LiteralPath (Join-Path $temporaryRustup ".pinvou3-managed-rustup") `
    -Value "pinvou3-managed-rustup-v1" -Encoding Ascii
  Write-Host "[test] Copying the source toolchain into the isolated test root."
  & robocopy $sourceToolchain $temporaryToolchain /E /COPY:DAT /DCOPY:DAT `
    /R:1 /W:1 /MT:8 /NFL /NDL /NJH /NJS /NP
  $robocopyExitCode = $LASTEXITCODE
  if ($robocopyExitCode -ge 8) {
    throw "Failed to copy the source toolchain with robocopy: $robocopyExitCode"
  }

  $sourceSettings = Join-Path $rustupHome "settings.toml"
  if (Test-Path -LiteralPath $sourceSettings) {
    Copy-Item -LiteralPath $sourceSettings `
      -Destination (Join-Path $temporaryRustup "settings.toml") -Force
  }

  $resolvedTemporaryRoot = (Resolve-Path -LiteralPath $temporaryRoot).Path
  $missingExecutables = @(
    (Join-Path $temporaryToolchain "bin\cargo.exe"),
    (Join-Path $temporaryToolchain "bin\rustc.exe")
  )
  foreach ($target in $missingExecutables) {
    $targetParent = (Resolve-Path -LiteralPath (Split-Path -Parent $target)).Path
    if (-not $targetParent.StartsWith(
      $resolvedTemporaryRoot,
      [StringComparison]::OrdinalIgnoreCase
    )) {
      throw "Refusing to corrupt a path outside the isolated test root: $target"
    }
    Remove-Item -LiteralPath $target -Force
  }

  Write-Host "[test] Prepared isolated inconsistent toolchain: $temporaryRoot"
  $env:RUSTUP_HOME = $temporaryRustup
  $env:CARGO_HOME = $temporaryCargo
  $env:PINVOU3_MANAGED_RUSTUP = "1"
  $env:RUSTUP_DIST_SERVER = "http://127.0.0.1:1"
  $env:RUSTUP_UPDATE_ROOT = "http://127.0.0.1:1/rustup"
  # Keep the production timeout unchanged; this test must reach the next
  # fallback source promptly when a distribution endpoint stalls.
  $env:RUSTUP_DOWNLOAD_TIMEOUT = "120"

  # With RUSTUP_HOME pointed at the corrupted copy, the real -CheckOnly probe
  # must classify it as incomplete (exit 2) before any repair runs; this is
  # the only place the incomplete branch executes against real rustup state.
  & $repairScript -CheckOnly
  if ($LASTEXITCODE -ne 2) {
    throw (
      "Inconsistent toolchain was not detected by -CheckOnly (exit $LASTEXITCODE, expected 2)."
    )
  }
  Write-Host "[test] -CheckOnly detected the corrupted toolchain: OK"

  $repairOutput = @(
    & $repairScript `
      -InstallAttemptTimeoutSeconds 300 `
      -RepairAttemptsPerSource 1 *>&1
  )
  $repairOutput | ForEach-Object { Write-Host $_ }
  $repairLog = $repairOutput | Out-String

  $expectedRepairEvidence = @(
    "Using configured source: http://127.0.0.1:1",
    "Repair attempt 1/1 failed using configured source",
    "Resetting the incomplete isolated toolchain before retry",
    "Using rsproxy mirror: https://rsproxy.cn",
    "Toolchain repair succeeded using"
  )
  foreach ($evidence in $expectedRepairEvidence) {
    if (-not $repairLog.Contains($evidence)) {
      throw "Repair output did not contain expected evidence: $evidence"
    }
  }

  foreach ($command in @("cargo", "rustc", "clippy-driver", "rustfmt")) {
    & rustup run $Toolchain $command -V
    if ($LASTEXITCODE -ne 0) {
      throw "Post-repair verification failed: $command"
    }
  }

  # rust-std corruption with intact binaries must be detected too: the four
  # binary probes never link against std, so the target libdir is probed
  # directly. Remove it and require -CheckOnly to classify the toolchain as
  # incomplete through the real probe path.
  $stdTargetLibDir = (& rustup run $Toolchain rustc --print target-libdir |
    Select-Object -Last 1)
  $stdTargetLibDir = "$stdTargetLibDir".Trim()
  if ([string]::IsNullOrWhiteSpace($stdTargetLibDir) -or -not $stdTargetLibDir.StartsWith(
    $resolvedTemporaryRoot,
    [StringComparison]::OrdinalIgnoreCase
  )) {
    throw "Unexpected std target libdir outside the isolated test root: $stdTargetLibDir"
  }
  Remove-Item -LiteralPath $stdTargetLibDir -Recurse -Force
  & $repairScript -CheckOnly
  if ($LASTEXITCODE -ne 2) {
    throw (
      "A missing rust-std was not detected by -CheckOnly (exit $LASTEXITCODE, expected 2)."
    )
  }
  Write-Host "[test] -CheckOnly detected the missing rust-std: OK"

  Write-Host "[test] Isolated inconsistent-toolchain repair: PASS"
} finally {
  foreach ($entry in @(
    @("RUSTUP_HOME", $previousRustupHome),
    @("CARGO_HOME", $previousCargoHome),
    @("PINVOU3_MANAGED_RUSTUP", $previousManagedFlag),
    @("RUSTUP_DIST_SERVER", $previousDistServer),
    @("RUSTUP_UPDATE_ROOT", $previousUpdateRoot),
    @("RUSTUP_DOWNLOAD_TIMEOUT", $previousDownloadTimeout)
  )) {
    if ($null -eq $entry[1]) {
      Remove-Item -LiteralPath "Env:\$($entry[0])" -ErrorAction SilentlyContinue
    } else {
      Set-Item -LiteralPath "Env:\$($entry[0])" -Value $entry[1]
    }
  }
  if (Test-Path -LiteralPath $temporaryRoot) {
    $resolved = (Resolve-Path -LiteralPath $temporaryRoot).Path
    $temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $safeName = (Split-Path -Leaf $resolved).StartsWith(
      "pinvou-rustup-repair-",
      [StringComparison]::OrdinalIgnoreCase
    )
    if (-not $resolved.StartsWith(
      $temporaryBase,
      [StringComparison]::OrdinalIgnoreCase
    ) -or -not $safeName) {
      throw "Refusing to remove an unexpected test path: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
  }
}
