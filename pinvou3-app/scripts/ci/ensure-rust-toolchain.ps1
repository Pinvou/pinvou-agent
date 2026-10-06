param(
  [switch]$CheckOnly,
  [ValidateRange(1, 3600)]
  [int]$InstallAttemptTimeoutSeconds = 600,
  [ValidateRange(1, 5)]
  [int]$RepairAttemptsPerSource = 2,
  [ValidateRange(1, 7200)]
  [int]$RepairTimeoutSeconds = 1500,
  # A waiter must out-wait the holder's legitimate worst case: the overall
  # repair budget, plus the final budget-cleared attempt's install AND its
  # uninstall reset (each bounded by InstallAttemptTimeoutSeconds and
  # carrying its own kill/drain slack), plus probe overhead. The lock is
  # handle-based and released on crash, so a long wait can only mean a live,
  # still-useful repair on the other side; a fixed 600s deadline made a
  # second build on the same checkout fail spuriously while the holder was
  # legitimately mid-repair.
  [ValidateRange(1, 14700)]
  [int]$LockTimeoutSeconds = (
    $RepairTimeoutSeconds + 2 * $InstallAttemptTimeoutSeconds + 300
  )
)

$ErrorActionPreference = "Stop"

# Version probes use --version, the long form all six probe commands accept:
# rustfmt 1.10.0 (stable 1.99.0) rewrote cargo-fmt on clap and rejects the
# short -V, so a -V probe classified every healthy current stable as
# incomplete and sent the repair into a non-convergent reinstall loop.
# Probes never touch the standard library, so a toolchain whose rust-std was
# wiped or half-extracted still passes every binary probe; the target libdir
# reported by rustc must exist and be populated for real compiles.
function Test-RustStdTargetLib {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RustupPath,
    [Parameter(Mandatory = $true)]
    [string]$Toolchain
  )

  $previousErrorActionPreference = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $targetLibDir = & $RustupPath run $Toolchain rustc --print target-libdir 2>$null |
      Select-Object -Last 1
    if ($LASTEXITCODE -ne 0) {
      return $false
    }
    $targetLibDir = "$targetLibDir".Trim()
    if ([string]::IsNullOrWhiteSpace($targetLibDir)) {
      return $false
    }
    if (-not (Test-Path -LiteralPath $targetLibDir -PathType Container)) {
      return $false
    }
    return @(
      Get-ChildItem -LiteralPath $targetLibDir -Force -ErrorAction SilentlyContinue
    ).Count -gt 0
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }
}

if ($CheckOnly) {
  $accountToolchainFile = Join-Path $PSScriptRoot "..\..\src-tauri\rust-toolchain.toml"
  if (-not (Test-Path -LiteralPath $accountToolchainFile)) {
    throw "[rustup] Toolchain file not found: $accountToolchainFile"
  }
  $accountToolchainConfig = Get-Content -LiteralPath $accountToolchainFile -Raw
  $accountChannelMatch = [regex]::Match(
    $accountToolchainConfig,
    '(?m)^\s*channel\s*=\s*"([^"]+)"'
  )
  if (-not $accountChannelMatch.Success) {
    throw "[rustup] Unable to read the Rust channel from: $accountToolchainFile"
  }
  $accountToolchain = $accountChannelMatch.Groups[1].Value
  $accountRustupCommand = Get-Command rustup -ErrorAction SilentlyContinue
  if (-not $accountRustupCommand) {
    throw "[rustup] rustup is not available for the current build account."
  }
  $accountRustupPath = $accountRustupCommand.Source

  $invalidAccountCommands = @()
  $previousErrorActionPreference = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    foreach ($command in @(
      "cargo", "rustc", "clippy-driver", "rustfmt", "cargo-clippy", "cargo-fmt"
    )) {
      & $accountRustupPath run $accountToolchain $command --version 2>&1 |
        ForEach-Object { Write-Host "[rustup] $_" }
      if ($LASTEXITCODE -ne 0) {
        $invalidAccountCommands += $command
      }
    }
    if (-not (Test-RustStdTargetLib -RustupPath $accountRustupPath -Toolchain $accountToolchain)) {
      $invalidAccountCommands += "rust-std"
    }
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }

  if ($invalidAccountCommands.Count -gt 0) {
    Write-Warning (
      "[rustup] Account Rust $accountToolchain is unavailable ({0}); isolated recovery is required." -f
      ($invalidAccountCommands -join ", ")
    )
    exit 2
  }
  Write-Host "[rustup] Account Rust toolchain is ready: $accountToolchain"
  exit 0
}

# The build wrapper assigns a workspace-scoped RUSTUP_HOME and marks it as a
# Pinvou-managed directory. Destructive recovery is allowed only inside that
# isolated directory, never in the build account's shared rustup installation.
if ($env:PINVOU3_MANAGED_RUSTUP -ne "1") {
  throw "[rustup] Refusing automatic repair without PINVOU3_MANAGED_RUSTUP=1."
}
if ([string]::IsNullOrWhiteSpace($env:RUSTUP_HOME)) {
  throw "[rustup] Refusing automatic repair without an isolated RUSTUP_HOME."
}

$managedRustupHome = [IO.Path]::GetFullPath($env:RUSTUP_HOME)
$pathRoot = [IO.Path]::GetPathRoot($managedRustupHome)
if ($managedRustupHome.TrimEnd("\", "/") -eq $pathRoot.TrimEnd("\", "/")) {
  throw "[rustup] Refusing to use a filesystem root as RUSTUP_HOME: $managedRustupHome"
}
if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
  $sharedRustupHome = [IO.Path]::GetFullPath(
    (Join-Path $env:USERPROFILE ".rustup")
  )
  if ($managedRustupHome.TrimEnd("\", "/") -ieq $sharedRustupHome.TrimEnd("\", "/")) {
    throw "[rustup] Refusing to modify the build account's shared RUSTUP_HOME."
  }
}

New-Item -ItemType Directory -Path $managedRustupHome -Force | Out-Null

$lockPath = Join-Path $managedRustupHome ".pinvou3-toolchain.lock"
$lockDeadline = [DateTime]::UtcNow.AddSeconds($LockTimeoutSeconds)
$lockStream = $null
while ($null -eq $lockStream) {
  try {
    $lockStream = [IO.File]::Open(
      $lockPath,
      [IO.FileMode]::OpenOrCreate,
      [IO.FileAccess]::ReadWrite,
      [IO.FileShare]::None
    )
  } catch [IO.IOException] {
    if ([DateTime]::UtcNow -ge $lockDeadline) {
      throw "[rustup] Timed out waiting for the managed toolchain lock: $lockPath"
    }
    Start-Sleep -Seconds 1
  }
}

try {
  # The adopt-or-refuse decision runs under the lock so two concurrent
  # first-time builds cannot both adopt the same home. The lock file is the
  # only entry a fresh home may already carry.
  $managedMarker = Join-Path $managedRustupHome ".pinvou3-managed-rustup"
  if (-not (Test-Path -LiteralPath $managedMarker)) {
    $existingEntries = @(Get-ChildItem -LiteralPath $managedRustupHome -Force | Where-Object {
      $_.Name -ne ".pinvou3-toolchain.lock"
    })
    if ($existingEntries.Count -gt 0) {
      throw (
        "[rustup] Refusing to adopt a non-empty unmarked RUSTUP_HOME: $managedRustupHome. " +
        "It is safe to delete the isolated RUSTUP_HOME at $managedRustupHome and re-run the build."
      )
    }
    Set-Content -LiteralPath $managedMarker `
      -Value "pinvou3-managed-rustup-v1" -Encoding Ascii
  }
  $markerValue = (Get-Content -LiteralPath $managedMarker -Raw).Trim()
  if ($markerValue -ne "pinvou3-managed-rustup-v1") {
    throw "[rustup] Invalid managed RUSTUP_HOME marker: $managedMarker"
  }
  $env:RUSTUP_HOME = $managedRustupHome

  Write-Host "[rustup] Checking the isolated Rust toolchain: $managedRustupHome"

  $toolchainFile = Join-Path $PSScriptRoot "..\..\src-tauri\rust-toolchain.toml"
  if (-not (Test-Path -LiteralPath $toolchainFile)) {
    throw "[rustup] Toolchain file not found: $toolchainFile"
  }

  $toolchainConfig = Get-Content -LiteralPath $toolchainFile -Raw
  $channelMatch = [regex]::Match(
    $toolchainConfig,
    '(?m)^\s*channel\s*=\s*"([^"]+)"'
  )
  if (-not $channelMatch.Success) {
    throw "[rustup] Unable to read the Rust channel from: $toolchainFile"
  }
  $toolchain = $channelMatch.Groups[1].Value

  $rustupCommand = Get-Command rustup -ErrorAction SilentlyContinue
  if (-not $rustupCommand) {
    throw "[rustup] rustup is not available for the current build account."
  }
  $rustupPath = $rustupCommand.Source

  if ([string]::IsNullOrWhiteSpace($env:RUSTUP_DOWNLOAD_TIMEOUT)) {
    $env:RUSTUP_DOWNLOAD_TIMEOUT = "600"
  }

  $configuredDistServer = $env:RUSTUP_DIST_SERVER
  $configuredUpdateRoot = $env:RUSTUP_UPDATE_ROOT
  $repairSources = @()

  if (-not [string]::IsNullOrWhiteSpace($configuredDistServer)) {
    $repairSources += [pscustomobject]@{
      Name = "configured source"
      DistServer = $configuredDistServer
      UpdateRoot = $configuredUpdateRoot
    }
  }

  # Mirror-first, the ordering #619 uses for its runtime downloads: this
  # engine exists for machines where the official source stalls, so the CN
  # mirrors come first and the official source is the last resort. Note the
  # integrity model differs from #619's SHA-256-pinned downloads: rustup
  # verifies components only against the channel manifest served by the same
  # mirror, so mirror trust rests on HTTPS; RUSTUP_DIST_SERVER restores
  # official-first with mirror fallback. rsproxy and USTC
  # each serve both the dist manifests and the rustup update root for rolling
  # and version-pinned channels alike (verified live). TUNA is not in the
  # chain: it 404s version-pinned manifests, so it only works while the
  # configured channel happens to be rolling, and rsproxy + USTC already keep
  # the chain channel-agnostic.
  $fallbackSources = @(
    [pscustomobject]@{
      Name = "rsproxy mirror"
      DistServer = "https://rsproxy.cn"
      UpdateRoot = "https://rsproxy.cn/rustup"
    },
    [pscustomobject]@{
      Name = "USTC mirror"
      DistServer = "https://mirrors.ustc.edu.cn/rust-static"
      UpdateRoot = "https://mirrors.ustc.edu.cn/rust-static/rustup"
    },
    [pscustomobject]@{
      Name = "Rust official source"
      DistServer = "https://static.rust-lang.org"
      UpdateRoot = "https://static.rust-lang.org/rustup"
    }
  )

  foreach ($source in $fallbackSources) {
    $duplicate = $repairSources | Where-Object {
      $_.DistServer.TrimEnd("/") -ieq $source.DistServer.TrimEnd("/")
    }
    if (-not $duplicate) {
      $repairSources += $source
    }
  }

  function Set-RustupSource {
    param(
      [Parameter(Mandatory = $true)]
      [psobject]$Source
    )

    $env:RUSTUP_DIST_SERVER = $Source.DistServer
    if ([string]::IsNullOrWhiteSpace($Source.UpdateRoot)) {
      Remove-Item Env:RUSTUP_UPDATE_ROOT -ErrorAction SilentlyContinue
    } else {
      $env:RUSTUP_UPDATE_ROOT = $Source.UpdateRoot
    }
    Write-Host "[rustup] Using $($Source.Name): $($Source.DistServer)"
  }

  Set-RustupSource -Source $repairSources[0]

  function Invoke-Rustup {
    param(
      [Parameter(Mandatory = $true)]
      [string[]]$Arguments,
      [int]$TimeoutSeconds = 0
    )

    if ($TimeoutSeconds -gt 0) {
      foreach ($argument in $Arguments) {
        if ($argument -match '[\s"]') {
          throw "[rustup] Unsupported whitespace or quote in native argument: $argument"
        }
      }

      $startInfo = New-Object System.Diagnostics.ProcessStartInfo
      $startInfo.FileName = $rustupPath
      $startInfo.Arguments = $Arguments -join " "
      $startInfo.UseShellExecute = $false
      $startInfo.CreateNoWindow = $true
      $startInfo.RedirectStandardOutput = $true
      $startInfo.RedirectStandardError = $true

      $process = New-Object System.Diagnostics.Process
      $process.StartInfo = $startInfo
      try {
        if (-not $process.Start()) {
          throw "[rustup] Failed to start rustup."
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
          # rustup can exit inside the window between the timed-out wait and
          # the kill; .NET Kill() then throws InvalidOperationException, and
          # a Win32Exception (already gone, access denied) is equally
          # possible. Either race must count as a failed attempt, not abort
          # the whole repair.
          try {
            $process.Kill()
          } catch [System.InvalidOperationException], [System.ComponentModel.Win32Exception] {
          }
          # A kill that could not complete must not hang the repair on the
          # unbounded WaitForExit either; the attempt is failed regardless.
          $null = $process.WaitForExit(5000)

          # Bounded drain: a surviving handle holder could otherwise stall
          # the stream reads past the per-attempt timeout. A faulted read
          # task throws AggregateException out of Wait(); the attempt is
          # already failed here, so only its output is lost.
          try { $null = $stdoutTask.Wait(10000) } catch [System.AggregateException] { }
          try { $null = $stderrTask.Wait(10000) } catch [System.AggregateException] { }
          $stdoutText = ""
          if ($stdoutTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion) { $stdoutText = $stdoutTask.Result }
          $stderrText = ""
          if ($stderrTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion) { $stderrText = $stderrTask.Result }
          $nativeOutput = @($stdoutText, $stderrText) -join "`n"
          $nativeOutput -split '[\r\n]+' | ForEach-Object {
            if (-not [string]::IsNullOrWhiteSpace($_)) {
              Write-Host "[rustup] $_"
            }
          }
          Write-Warning (
            "[rustup] Rustup attempt timed out after $TimeoutSeconds seconds."
          )
          return 124
        }

        # Bounded drain on the success path too: a faulted read task is
        # completed but not RanToCompletion, so an IsCompleted guard would
        # still let .Result rethrow past the exit-code classification; only
        # a RanToCompletion read may contribute output, and an inherited
        # pipe handle must not hang a completed process's output read.
        try { $null = $stdoutTask.Wait(10000) } catch [System.AggregateException] { }
        try { $null = $stderrTask.Wait(10000) } catch [System.AggregateException] { }
        $stdoutText = ""
        if ($stdoutTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion) { $stdoutText = $stdoutTask.Result }
        $stderrText = ""
        if ($stderrTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion) { $stderrText = $stderrTask.Result }
        $nativeOutput = @($stdoutText, $stderrText) -join "`n"
        $nativeOutput -split '[\r\n]+' | ForEach-Object {
          if (-not [string]::IsNullOrWhiteSpace($_)) {
            Write-Host "[rustup] $_"
          }
        }

        return $process.ExitCode
      } finally {
        $process.Dispose()
      }
    }

    # Windows PowerShell 5.1 converts native stderr redirected through 2>&1
    # into NativeCommandError records. Preserve the native exit code so an
    # expected failed probe can enter the recovery path.
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
      # A native command that cannot start (AV lock, blocked executable)
      # never updates $LASTEXITCODE, so a stale 0 from an earlier call would
      # false-pass the probe classification below; seed a failing code.
      $LASTEXITCODE = 1
      & $rustupPath @Arguments 2>&1 | ForEach-Object {
        $message = if ($_ -is [System.Management.Automation.ErrorRecord]) {
          $_.Exception.Message
        } else {
          [string]$_
        }
        if (-not [string]::IsNullOrWhiteSpace($message)) {
          Write-Host "[rustup] $message"
        }
      }
      $exitCode = $LASTEXITCODE
    } finally {
      $ErrorActionPreference = $previousErrorActionPreference
    }
    return $exitCode
  }

  function Test-RustComponent {
    param(
      [Parameter(Mandatory = $true)]
      [string]$Command
    )

    $exitCode = Invoke-Rustup -Arguments @("run", $toolchain, $Command, "--version")
    return $exitCode -eq 0
  }

  function Test-ManagedToolchainPresent {
    $toolchainsRoot = Join-Path $managedRustupHome "toolchains"
    if (-not (Test-Path -LiteralPath $toolchainsRoot)) {
      return $false
    }
    # $matches is a PowerShell automatic variable; use a distinct name.
    $managedToolchainDirs = @(Get-ChildItem -LiteralPath $toolchainsRoot -Directory | Where-Object {
      $_.Name -eq $toolchain -or $_.Name.StartsWith(
        "$toolchain-",
        [StringComparison]::OrdinalIgnoreCase
      )
    })
    return $managedToolchainDirs.Count -gt 0
  }

  # cargo clippy / cargo fmt shell out to the cargo-clippy/cargo-fmt shims, so
  # a toolchain missing them passes the driver probes and still breaks the
  # lint gates later.
  $requiredCommands = @(
    "cargo", "rustc", "clippy-driver", "rustfmt", "cargo-clippy", "cargo-fmt"
  )
  $invalidEntries = @($requiredCommands | Where-Object {
    -not (Test-RustComponent -Command $_)
  })
  if (-not (Test-RustStdTargetLib -RustupPath $rustupPath -Toolchain $toolchain)) {
    $invalidEntries += "rust-std"
  }

  if ($invalidEntries.Count -gt 0) {
    Write-Warning (
      "[rustup] Rust $toolchain is incomplete ({0}); repairing the isolated toolchain." -f
      ($invalidEntries -join ", ")
    )

    $repairSucceeded = $false
    $repairFailures = @()
    # Checked between attempts, so the worst case is this budget plus one
    # bounded install attempt and its bounded uninstall reset; without it
    # four sources could wander for over an hour before anything gives up.
    $repairDeadline = [DateTime]::UtcNow.AddSeconds($RepairTimeoutSeconds)
    foreach ($source in $repairSources) {
      Set-RustupSource -Source $source
      foreach ($attempt in 1..$RepairAttemptsPerSource) {
        if ([DateTime]::UtcNow -ge $repairDeadline) {
          $repairFailures += "$($source.Name) (repair budget of $RepairTimeoutSeconds s exhausted before attempt $attempt)"
          throw (
            (
              "[rustup] Rust toolchain repair exceeded its {0}s budget; attempts: {1}. " +
              "It is safe to delete the isolated RUSTUP_HOME at {2} and re-run the build."
            ) -f
            $RepairTimeoutSeconds,
            ($repairFailures -join "; "),
            $managedRustupHome
          )
        }
        Write-Host (
          "[rustup] Repair attempt $attempt/$RepairAttemptsPerSource using $($source.Name)."
        )
        # --force only overrides rustup's component-completeness check. Over a
        # partially-installed directory it can exit 0 as "unchanged" without
        # restoring missing binaries, so the post-install probe below is the
        # real success gate and the uninstall reset after a failed attempt is
        # what actually reinstalls a broken directory.
        $repairExitCode = Invoke-Rustup -Arguments @(
          "toolchain", "install", $toolchain,
          "--profile", "minimal",
          "--component", "clippy",
          "--component", "rustfmt",
          "--no-self-update",
          "--force"
        ) -TimeoutSeconds $InstallAttemptTimeoutSeconds
        $postInstallInvalid = @()
        if ($repairExitCode -eq 0) {
          $postInstallInvalid = @($requiredCommands | Where-Object {
            -not (Test-RustComponent -Command $_)
          })
          if (-not (Test-RustStdTargetLib -RustupPath $rustupPath -Toolchain $toolchain)) {
            $postInstallInvalid += "rust-std"
          }
          if ($postInstallInvalid.Count -eq 0) {
            $repairSucceeded = $true
            Write-Host "[rustup] Toolchain repair succeeded using $($source.Name)."
            break
          }
        }

        $failureDetail = if ($postInstallInvalid.Count -gt 0) {
          "missing: $($postInstallInvalid -join ', ')"
        } else {
          "exit $repairExitCode"
        }
        $repairFailures += "$($source.Name) attempt $attempt ($failureDetail)"
        Write-Warning (
          "[rustup] Repair attempt $attempt/$RepairAttemptsPerSource failed using $($source.Name): $failureDetail."
        )

        # Only the marked, isolated RUSTUP_HOME can reach this destructive
        # reset. The build account's shared toolchain is never selected.
        if (Test-ManagedToolchainPresent) {
          Write-Host "[rustup] Resetting the incomplete isolated toolchain before retry."
          # Bounded like the install: a reset stalled by AV scanning or a
          # transiently locked binary must abort the repair instead of
          # outlasting the budget and the waiters' derived lock deadline. A
          # directory that cannot be reset cannot be repaired by a later
          # attempt over the same directory, so aborting is the honest
          # outcome; the error names the way out.
          $resetExitCode = Invoke-Rustup -Arguments @(
            "toolchain", "uninstall", $toolchain
          ) -TimeoutSeconds $InstallAttemptTimeoutSeconds
          if ($resetExitCode -ne 0) {
            throw (
              "[rustup] Failed to reset the isolated Rust toolchain: $toolchain (exit $resetExitCode). " +
              "It is safe to delete the isolated RUSTUP_HOME at $managedRustupHome and re-run the build."
            )
          }
        }
        if ($attempt -lt $RepairAttemptsPerSource) {
          Start-Sleep -Seconds 2
        }
      }
      if ($repairSucceeded) {
        break
      }
    }

    if (-not $repairSucceeded) {
      throw (
        (
          "[rustup] Failed to repair Rust toolchain {0}; attempts: {1}. " +
          "It is safe to delete the isolated RUSTUP_HOME at {2} and re-run the build."
        ) -f
        $toolchain,
        ($repairFailures -join "; "),
        $managedRustupHome
      )
    }

    $failedEntries = @($requiredCommands | Where-Object {
      -not (Test-RustComponent -Command $_)
    })
    if (-not (Test-RustStdTargetLib -RustupPath $rustupPath -Toolchain $toolchain)) {
      $failedEntries += "rust-std"
    }
    if ($failedEntries.Count -gt 0) {
      throw (
        "[rustup] Rust toolchain verification failed after repair ({0}): {1}" -f
        $toolchain,
        ($failedEntries -join ", ")
      )
    }
  }

  Write-Host "[rustup] Rust toolchain is ready: $toolchain"
} finally {
  if ($null -ne $lockStream) {
    $lockStream.Dispose()
  }
}
