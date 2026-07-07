param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Install", "Uninstall")]
    [string]$Mode,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"

$pythonDir = Join-Path $InstallDir "python"
$nodeDir = Join-Path $InstallDir "node"
$sevenZipDir = Join-Path $InstallDir "7zip"
$asrDir = Join-Path $InstallDir "asr"
$pandocDir = Join-Path $InstallDir "pandoc"
$popplerDir = Join-Path $InstallDir "poppler"
$tesseractDir = Join-Path $InstallDir "tesseract"
$pythonExe = Join-Path $pythonDir "pythonw.exe"
$privateRuntimeDirs = @(
    $pythonDir,
    $nodeDir,
    $sevenZipDir,
    $asrDir,
    $pandocDir,
    $popplerDir,
    $tesseractDir
)

function Normalize-PathEntry {
    param([string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ""
    }

    $expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim().Trim('"'))
    try {
        return ([IO.Path]::GetFullPath($expanded)).TrimEnd('\')
    } catch {
        return $expanded.TrimEnd('\')
    }
}

function Remove-MachinePathEntries {
    param([string[]]$RemovedEntries)

    $current = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if ([string]::IsNullOrEmpty($current)) {
        return
    }

    $entries = @($current -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $kept = @()
    foreach ($entry in $entries) {
        $remove = $false
        foreach ($removed in $RemovedEntries) {
            if ([string]::Equals((Normalize-PathEntry $entry), (Normalize-PathEntry $removed), [StringComparison]::OrdinalIgnoreCase)) {
                $remove = $true
                break
            }
        }
        if (-not $remove) {
            $kept += $entry
        }
    }

    if ($kept.Count -ne $entries.Count) {
        [Environment]::SetEnvironmentVariable("Path", ($kept -join ';'), "Machine")
    }
}

function Publish-EnvironmentChange {
    try {
        Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class PinvouEnvironmentBroadcast
{
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(
        IntPtr hWnd,
        uint Msg,
        UIntPtr wParam,
        string lParam,
        uint fuFlags,
        uint uTimeout,
        out UIntPtr lpdwResult);
}
"@
        $result = [UIntPtr]::Zero
        [PinvouEnvironmentBroadcast]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment", 0x0002, 5000, [ref]$result) | Out-Null
    } catch {
        Write-Warning "Failed to broadcast environment change: $($_.Exception.Message)"
    }
}

if ($Mode -eq "Install") {
    if (-not (Test-Path -LiteralPath $pythonExe -PathType Leaf)) {
        throw "Bundled pythonw.exe was not found: $pythonExe"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $nodeDir "node.exe") -PathType Leaf)) {
        throw "Bundled node.exe was not found: $(Join-Path $nodeDir "node.exe")"
    }

    [Environment]::SetEnvironmentVariable("PINVOU3_PYTHON", $pythonExe, "Machine")
    Remove-MachinePathEntries -RemovedEntries $privateRuntimeDirs
    Publish-EnvironmentChange
    Write-Output "Configured PINVOU3_PYTHON and removed private runtime entries from machine PATH."
} else {
    $currentPython = [Environment]::GetEnvironmentVariable("PINVOU3_PYTHON", "Machine")
    if ([string]::Equals((Normalize-PathEntry $currentPython), (Normalize-PathEntry $pythonExe), [StringComparison]::OrdinalIgnoreCase)) {
        [Environment]::SetEnvironmentVariable("PINVOU3_PYTHON", $null, "Machine")
    }

    Remove-MachinePathEntries -RemovedEntries $privateRuntimeDirs
    Publish-EnvironmentChange
    Write-Output "Removed Pinvou runtime entries from machine environment."
}
