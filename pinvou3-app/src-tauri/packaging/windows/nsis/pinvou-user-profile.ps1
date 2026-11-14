# Prints the interactive session user's profile directory (no trailing
# newline) so the NSIS uninstaller can target the prompting user's
# ~/.pinvou3 under over-the-shoulder elevation, where the elevated process
# token belongs to the admin account rather than the uninstalling user
# ($PROFILE and SetShellVarContext current both follow the elevation token).
#
# Resolution: the owner of the first explorer.exe process (the shell of the
# interactive session) mapped to a Win32_UserProfile LocalPath whose leaf
# equals the owner's account name. Exits non-zero WITHOUT printing a path
# when the owner cannot be determined or the mapping is ambiguous; the
# caller must treat any non-zero exit as "keep user data" (fail safe —
# never guess a profile to wipe).
$ErrorActionPreference = "Stop"

$owner = $null
foreach ($process in Get-CimInstance -ClassName Win32_Process -Filter "Name = 'explorer.exe'") {
    try {
        $candidate = Invoke-CimMethod -InputObject $process -MethodName GetOwner
    } catch {
        continue
    }
    if ($null -ne $candidate -and -not [string]::IsNullOrWhiteSpace($candidate.User)) {
        $owner = $candidate
        break
    }
}
if ($null -eq $owner) {
    exit 2
}

$profileMatches = @()
foreach ($userProfile in Get-CimInstance -ClassName Win32_UserProfile) {
    if ([string]::IsNullOrWhiteSpace($userProfile.LocalPath)) {
        continue
    }
    if ((Split-Path -Path $userProfile.LocalPath -Leaf) -ieq $owner.User) {
        $profileMatches += $userProfile.LocalPath
    }
}
if ($profileMatches.Count -ne 1) {
    exit 2
}
[Console]::Out.Write($profileMatches[0])
