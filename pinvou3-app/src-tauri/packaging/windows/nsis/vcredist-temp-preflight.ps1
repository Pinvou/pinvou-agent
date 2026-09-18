[CmdletBinding()]
param(
  [string]$WindowsRoot = $env:SystemRoot,
  [switch]$SkipMachineEnvironment
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

$systemSid = New-Object System.Security.Principal.SecurityIdentifier("S-1-5-18")
$administratorsSid = New-Object System.Security.Principal.SecurityIdentifier("S-1-5-32-544")
$requiredSids = @($systemSid, $administratorsSid)
$requiredRights = [System.Security.AccessControl.FileSystemRights]::FullControl
$inheritanceFlags = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
  [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
$propagationFlags = [System.Security.AccessControl.PropagationFlags]::None
$allow = [System.Security.AccessControl.AccessControlType]::Allow

function Test-Administrator {
  $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-RequiredAccess {
  param(
    [System.Security.AccessControl.DirectorySecurity]$Acl,
    [System.Security.Principal.SecurityIdentifier]$Sid
  )

  $rules = $Acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])
  foreach ($rule in $rules) {
    if ($rule.IdentityReference.Value -eq $Sid.Value -and
        $rule.AccessControlType -eq $allow -and
        ($rule.FileSystemRights -band $requiredRights) -eq $requiredRights) {
      return $true
    }
  }
  return $false
}

function Add-RequiredAccess {
  param([string]$Path)

  $acl = Get-Acl -LiteralPath $Path
  $changed = $false
  foreach ($sid in $requiredSids) {
    if (Test-RequiredAccess -Acl $acl -Sid $sid) {
      continue
    }
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
      $sid,
      $requiredRights,
      $inheritanceFlags,
      $propagationFlags,
      $allow
    )
    [void]$acl.AddAccessRule($rule)
    $changed = $true
  }
  if ($changed) {
    Set-Acl -LiteralPath $Path -AclObject $acl
    Write-Host "Repaired required ACL entries: $Path"
  } else {
    Write-Host "Required ACL entries are ready: $Path"
  }
}

function Ensure-SystemDirectory {
  param(
    [string]$Path,
    [switch]$HiddenSystem
  )

  $created = -not (Test-Path -LiteralPath $Path -PathType Container)
  if ($created) {
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    Write-Host "Created required directory: $Path"
  }
  if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
    throw "Required Windows Installer directory is unavailable: $Path"
  }
  if ($created -and $HiddenSystem) {
    $directory = Get-Item -LiteralPath $Path -Force
    $directory.Attributes = $directory.Attributes -bor
      [System.IO.FileAttributes]::Hidden -bor [System.IO.FileAttributes]::System
  }
  Add-RequiredAccess -Path $Path
}

function Repair-MachineTempVariable {
  param(
    [string]$Name,
    [string]$FallbackPath,
    [ref]$ResolvedPath
  )

  $current = [Environment]::GetEnvironmentVariable($Name, "Machine")
  $expanded = if ([string]::IsNullOrWhiteSpace($current)) {
    ""
  } else {
    [Environment]::ExpandEnvironmentVariables($current)
  }
  $valid = -not [string]::IsNullOrWhiteSpace($expanded) -and
    [System.IO.Path]::IsPathRooted($expanded) -and
    -not $expanded.StartsWith("\\") -and
    (Test-Path -LiteralPath $expanded -PathType Container)

  if (-not $valid) {
    [Environment]::SetEnvironmentVariable($Name, $FallbackPath, "Machine")
    Write-Host "Repaired machine $Name to $FallbackPath"
    $ResolvedPath.Value = $FallbackPath
    return
  }
  Write-Host "Machine $Name is ready: $expanded"
  $ResolvedPath.Value = $expanded
}

if ([string]::IsNullOrWhiteSpace($WindowsRoot) -or
    -not [System.IO.Path]::IsPathRooted($WindowsRoot)) {
  throw "WindowsRoot must be an absolute path."
}
if (-not $SkipMachineEnvironment -and -not (Test-Administrator)) {
  throw "Administrator privileges are required to repair Windows Installer directories."
}

$windowsRootPath = [System.IO.Path]::GetFullPath($WindowsRoot).TrimEnd('\')
$windowsTempPath = Join-Path $windowsRootPath "Temp"
$windowsInstallerPath = Join-Path $windowsRootPath "Installer"

Ensure-SystemDirectory -Path $windowsTempPath
Ensure-SystemDirectory -Path $windowsInstallerPath -HiddenSystem

if (-not $SkipMachineEnvironment) {
  [string]$machineTemp = ""
  [string]$machineTmp = ""
  Repair-MachineTempVariable `
    -Name "TEMP" `
    -FallbackPath $windowsTempPath `
    -ResolvedPath ([ref]$machineTemp)
  Repair-MachineTempVariable `
    -Name "TMP" `
    -FallbackPath $windowsTempPath `
    -ResolvedPath ([ref]$machineTmp)
  Add-RequiredAccess -Path $machineTemp
  if (-not [string]::Equals($machineTemp, $machineTmp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Add-RequiredAccess -Path $machineTmp
  }
}

Write-Host "VC++ prerequisite temp preflight completed."
