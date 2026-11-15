# Prints the interactive session user's profile directory (no trailing
# newline) so the NSIS uninstaller can target the prompting user's data
# under over-the-shoulder elevation, where the elevated process token
# belongs to the admin account rather than the uninstalling user.
#
# Resolution: the session owner is taken from the shell processes of the
# uninstaller's OWN session - UAC elevation swaps the token but keeps the
# requesting user's session, so this process's session id identifies the
# prompting user regardless of the elevation - and is matched against
# Win32_UserProfile by SID, never by name (account renames and cross-domain
# name collisions would make a name-based map pick the wrong profile or
# none). Exits non-zero WITHOUT printing a path when the owner cannot be
# determined unambiguously; the caller must treat any non-zero exit as
# "keep user data" (fail safe - never guess a profile to wipe).
$ErrorActionPreference = "Stop"

$sessionId = (Get-Process -Id $PID).SessionId

$ownerSids = @()
foreach ($process in Get-CimInstance -ClassName Win32_Process -Filter "Name = 'explorer.exe' AND SessionId = $sessionId") {
    try {
        $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwnerSid
    } catch {
        continue
    }
    if ($null -ne $owner -and $owner.ReturnValue -eq 0 -and -not [string]::IsNullOrWhiteSpace($owner.Sid)) {
        $ownerSids += $owner.Sid
    }
}
$ownerSids = @($ownerSids | Select-Object -Unique)
if ($ownerSids.Count -ne 1) {
    exit 2
}

$profileMatches = @()
foreach ($userProfile in Get-CimInstance -ClassName Win32_UserProfile) {
    if ([string]::IsNullOrWhiteSpace($userProfile.LocalPath) -or [string]::IsNullOrWhiteSpace($userProfile.SID)) {
        continue
    }
    if ($userProfile.SID -ieq $ownerSids[0]) {
        $profileMatches += $userProfile.LocalPath
    }
}
if ($profileMatches.Count -ne 1) {
    exit 2
}
[Console]::Out.Write($profileMatches[0])
