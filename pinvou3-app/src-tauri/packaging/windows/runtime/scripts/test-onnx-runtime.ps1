$ErrorActionPreference = "Stop"

$resolver = Join-Path $PSScriptRoot "resolve-runtime.ps1"
& $resolver -Mode StageOnnx
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$tauriRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")).Path
$descriptorPath = Join-Path $tauriRoot "target\windows-runtime\onnx-dev-descriptor.json"
$descriptor = Get-Content -LiteralPath $descriptorPath -Raw -Encoding UTF8 | ConvertFrom-Json
$dylib = [System.IO.Path]::GetFullPath((Join-Path $tauriRoot ([string]$descriptor.onnxRuntimeDylib).Replace('/', '\')))

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class PinvouOnnxRuntimeSmoke {
  [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);
  [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Ansi)]
  public static extern IntPtr GetProcAddress(IntPtr module, string name);
  [DllImport("kernel32", SetLastError = true)]
  public static extern bool FreeLibrary(IntPtr module);
}
"@

$loadLibrarySearchDllLoadDir = 0x00000100
$loadLibrarySearchDefaultDirs = 0x00001000
$module = [PinvouOnnxRuntimeSmoke]::LoadLibraryEx(
  $dylib,
  [IntPtr]::Zero,
  $loadLibrarySearchDllLoadDir -bor $loadLibrarySearchDefaultDirs
)
if ($module -eq [IntPtr]::Zero) {
  throw "Failed to load pinned ONNX Runtime: $dylib (Win32 $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
}
try {
  if ([PinvouOnnxRuntimeSmoke]::GetProcAddress($module, "OrtGetApiBase") -eq [IntPtr]::Zero) {
    throw "Pinned ONNX Runtime does not export OrtGetApiBase: $dylib"
  }
} finally {
  [void][PinvouOnnxRuntimeSmoke]::FreeLibrary($module)
}

Write-Host "Loaded pinned ONNX Runtime and resolved OrtGetApiBase: $dylib"
