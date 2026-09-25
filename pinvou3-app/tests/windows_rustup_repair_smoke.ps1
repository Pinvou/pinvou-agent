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
    throw "Unable to read the pinned Rust channel from: $toolchainFile"
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

  $repairOutput = @(
    & $repairScript `
      -InstallAttemptTimeoutSeconds 120 `
      -RepairAttemptsPerSource 1 *>&1
  )
  $repairOutput | ForEach-Object { Write-Host $_ }
  $repairLog = $repairOutput | Out-String

  $expectedRepairEvidence = @(
    "Using configured source: http://127.0.0.1:1",
    "Repair attempt 1/1 failed using configured source",
    "Resetting the incomplete isolated toolchain before retry",
    "Using Rust official source: https://static.rust-lang.org",
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

  Write-Host "[test] Isolated inconsistent-toolchain repair: PASS"
} finally {
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
