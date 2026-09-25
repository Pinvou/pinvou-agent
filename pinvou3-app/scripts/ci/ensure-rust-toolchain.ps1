param(
  [switch]$CheckOnly,
  [int]$LockTimeoutSeconds = 600,
  [ValidateRange(1, 3600)]
  [int]$InstallAttemptTimeoutSeconds = 600,
  [ValidateRange(1, 5)]
  [int]$RepairAttemptsPerSource = 2
)

$ErrorActionPreference = "Stop"

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
    throw "[rustup] Unable to read the pinned Rust channel from: $accountToolchainFile"
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
    foreach ($command in @("cargo", "rustc", "clippy-driver", "rustfmt")) {
      & $accountRustupPath run $accountToolchain $command -V 2>&1 |
        ForEach-Object { Write-Host "[rustup] $_" }
      if ($LASTEXITCODE -ne 0) {
        $invalidAccountCommands += $command
      }
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
$managedMarker = Join-Path $managedRustupHome ".pinvou3-managed-rustup"
if (-not (Test-Path -LiteralPath $managedMarker)) {
  $existingEntries = @(Get-ChildItem -LiteralPath $managedRustupHome -Force)
  if ($existingEntries.Count -gt 0) {
    throw "[rustup] Refusing to adopt a non-empty unmarked RUSTUP_HOME: $managedRustupHome"
  }
  Set-Content -LiteralPath $managedMarker `
    -Value "pinvou3-managed-rustup-v1" -Encoding Ascii
}
$markerValue = (Get-Content -LiteralPath $managedMarker -Raw).Trim()
if ($markerValue -ne "pinvou3-managed-rustup-v1") {
  throw "[rustup] Invalid managed RUSTUP_HOME marker: $managedMarker"
}
$env:RUSTUP_HOME = $managedRustupHome

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
    throw "[rustup] Unable to read the pinned Rust channel from: $toolchainFile"
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

  $fallbackSources = @(
    [pscustomobject]@{
      Name = "Rust official source"
      DistServer = "https://static.rust-lang.org"
      UpdateRoot = "https://static.rust-lang.org/rustup"
    },
    [pscustomobject]@{
      Name = "rsproxy mirror"
      DistServer = "https://rsproxy.cn"
      UpdateRoot = "https://rsproxy.cn/rustup"
    },
    [pscustomobject]@{
      Name = "TUNA mirror"
      DistServer = "https://mirrors.tuna.tsinghua.edu.cn/rustup"
      UpdateRoot = "https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup"
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
          $process.Kill()
        }
        $process.WaitForExit()

        $nativeOutput = @(
          $stdoutTask.GetAwaiter().GetResult(),
          $stderrTask.GetAwaiter().GetResult()
        ) -join "`n"
        $nativeOutput -split '[\r\n]+' | ForEach-Object {
          if (-not [string]::IsNullOrWhiteSpace($_)) {
            Write-Host "[rustup] $_"
          }
        }

        if ($timedOut) {
          Write-Warning (
            "[rustup] Rustup attempt timed out after $TimeoutSeconds seconds."
          )
          return 124
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

    $exitCode = Invoke-Rustup -Arguments @("run", $toolchain, $Command, "-V")
    return $exitCode -eq 0
  }

  function Test-ManagedToolchainPresent {
    $toolchainsRoot = Join-Path $managedRustupHome "toolchains"
    if (-not (Test-Path -LiteralPath $toolchainsRoot)) {
      return $false
    }
    $matches = @(Get-ChildItem -LiteralPath $toolchainsRoot -Directory | Where-Object {
      $_.Name -eq $toolchain -or $_.Name.StartsWith(
        "$toolchain-",
        [StringComparison]::OrdinalIgnoreCase
      )
    })
    return $matches.Count -gt 0
  }

  $requiredCommands = @("cargo", "rustc", "clippy-driver", "rustfmt")
  $invalidCommands = @($requiredCommands | Where-Object {
    -not (Test-RustComponent -Command $_)
  })

  if ($invalidCommands.Count -gt 0) {
    Write-Warning (
      "[rustup] Rust $toolchain is incomplete ({0}); repairing the isolated toolchain." -f
      ($invalidCommands -join ", ")
    )

    $repairSucceeded = $false
    $repairFailures = @()
    foreach ($source in $repairSources) {
      Set-RustupSource -Source $source
      foreach ($attempt in 1..$RepairAttemptsPerSource) {
        Write-Host (
          "[rustup] Repair attempt $attempt/$RepairAttemptsPerSource using $($source.Name)."
        )
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
          $resetExitCode = Invoke-Rustup -Arguments @(
            "toolchain", "uninstall", $toolchain
          )
          if ($resetExitCode -ne 0) {
            throw "[rustup] Failed to reset the isolated Rust toolchain: $toolchain"
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
        "[rustup] Failed to repair Rust toolchain {0}; attempts: {1}" -f
        $toolchain,
        ($repairFailures -join "; ")
      )
    }

    $failedCommands = @($requiredCommands | Where-Object {
      -not (Test-RustComponent -Command $_)
    })
    if ($failedCommands.Count -gt 0) {
      throw (
        "[rustup] Rust toolchain verification failed after repair ({0}): {1}" -f
        $toolchain,
        ($failedCommands -join ", ")
      )
    }
  }

  Write-Host "[rustup] Rust toolchain is ready: $toolchain"
} finally {
  if ($null -ne $lockStream) {
    $lockStream.Dispose()
  }
}
