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
#
# Transport contract (lossless by construction): the path is printed as
# LOWERCASE HEX of its UTF-16LE code units - four hex digits per unit - so
# the stdout payload is pure ASCII. ASCII bytes are a fixed point of every
# OEM code page (including 65001) and of the no-conversion path, so the
# nsExec /OEM consumer decodes the payload identically regardless of the
# system code page: no text encoding, no console output encoding, and no
# consumer-side decoder behavior is trusted anywhere on the wire. The
# caller decodes positionally and treats ANY malformed payload (odd hex
# digit count, non-lowercase-hex characters, empty output) as a resolver
# failure under the same keep-data contract as a non-zero exit.
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

# Encode for the lossless transport: each UTF-16 code unit as exactly four
# lowercase hex digits, most significant digit first, contiguous. Emitting
# per code unit (rather than per byte) keeps the digit order identical on
# both sides - there is no byte-order interpretation anywhere on the wire.
# Non-BMP characters such as emoji are stored as surrogate pairs, i.e. two
# ordinary code units, and ride through as eight ordinary digits.
$pathHex = $profileMatches[0].ToCharArray() | ForEach-Object { '{0:x4}' -f [int]$_ }
[Console]::Out.Write(($pathHex -join ''))
