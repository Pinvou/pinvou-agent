param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Install", "Uninstall")]
    [string]$Mode,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"

$runtimeDir = Join-Path $InstallDir "runtime"
$pythonDir = Join-Path $runtimeDir "python"
$nodeDir = Join-Path $runtimeDir "node"
$sevenZipDir = Join-Path $runtimeDir "7zip"
$asrDir = Join-Path $runtimeDir "asr"
$pandocDir = Join-Path $runtimeDir "pandoc"
$popplerDir = Join-Path $runtimeDir "poppler"
$tesseractDir = Join-Path $runtimeDir "tesseract"
$onnxRuntimeDir = Join-Path $runtimeDir "onnxruntime"
$legacyPythonDir = Join-Path $InstallDir "python"
$legacyNodeDir = Join-Path $InstallDir "node"
$legacySevenZipDir = Join-Path $InstallDir "7zip"
$legacyAsrDir = Join-Path $InstallDir "asr"
$legacyPandocDir = Join-Path $InstallDir "pandoc"
$legacyPopplerDir = Join-Path $InstallDir "poppler"
$legacyTesseractDir = Join-Path $InstallDir "tesseract"
$pythonExe = Join-Path $pythonDir "pythonw.exe"
$privateRuntimeDirs = @(
    $pythonDir,
    $nodeDir,
    $sevenZipDir,
    $asrDir,
    $pandocDir,
    $popplerDir,
    $tesseractDir,
    $onnxRuntimeDir,
    $legacyPythonDir,
    $legacyNodeDir,
    $legacySevenZipDir,
    $legacyAsrDir,
    $legacyPandocDir,
    $legacyPopplerDir,
    $legacyTesseractDir
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

    # [Environment]::GetEnvironmentVariable expands REG_EXPAND_SZ entries and
    # SetEnvironmentVariable writes back as REG_SZ, which would permanently
    # bake %SystemRoot%-style entries into literal paths. Go through the
    # registry API instead: keep the raw text and restore the original value
    # kind on write.
    $environmentKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Control\Session Manager\Environment", $true)
    if ($null -eq $environmentKey) {
        throw "Machine environment registry key is not available: SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
    }
    try {
        $rawPath = $environmentKey.GetValue("Path", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ([string]::IsNullOrEmpty($rawPath)) {
            return
        }
        $pathKind = $environmentKey.GetValueKind("Path")

        $entries = @($rawPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
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
            $environmentKey.SetValue("Path", ($kept -join ';'), $pathKind)
        }
    } finally {
        $environmentKey.Close()
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
