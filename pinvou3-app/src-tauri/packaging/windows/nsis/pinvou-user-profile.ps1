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
# "keep user data" (fail safe - never guess a profile to wipe). The same
# contract covers the stdout encoding: the uninstaller consumes this output
# through nsExec /OEM, which decodes the byte stream with the system OEM
# code page, so a profile path that is not representable in that code page
# would be silently rewritten with replacement characters (the printed path
# would stop matching the real profile) - the resolver detects that case
# and exits non-zero before printing anything, keeping the data instead.
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

# Loss check for the nsExec /OEM consumption (the uninstaller decodes this
# stream with the system OEM code page): verify the resolved path survives a
# strict round-trip through that exact encoding. With an exception-throwing
# encoder fallback, the first unrepresentable character aborts before any
# output, so the caller keeps the data and shows the manual-removal guidance
# instead of deleting against a replacement-mangled prefix that would
# silently miss every real target.
# OEMCP is a REG_SZ numeric string ("936", "437", ...). The three-argument
# GetEncoding overload interprets its argument as an encoding NAME, and
# Windows PowerShell 5.1 throws on ordinary numeric strings such as "936" -
# construction therefore happens inside the same fail-safe boundary as the
# representability check, after the value is parsed as a number. An
# unparseable or non-positive code page is handled like any other
# ambiguity: exit non-zero without printing, keep the data.
$oemCpRaw = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Nls\CodePage' -Name OEMCP).OEMCP
$oemCp = 0
if (-not [int]::TryParse($oemCpRaw, [ref]$oemCp) -or $oemCp -le 0) {
    exit 2
}
try {
    $oemEncoding = [System.Text.Encoding]::GetEncoding(
        $oemCp,
        [System.Text.EncoderExceptionFallback]::new(),
        [System.Text.DecoderExceptionFallback]::new()
    )
    # Bind the write channel to the validated encoder BEFORE emitting:
    # Console.Out would otherwise use its own output encoding, so a path
    # that passed the strict round-trip could still be mangled on the wire
    # (the reviewer's transport finding). With the write channel pinned to
    # the OEM code page, the bytes nsExec /OEM reads are exactly the bytes
    # the strict round-trip just proved lossless; anything unrepresentable
    # already exited above.
    [Console]::OutputEncoding = $oemEncoding
    [void]$oemEncoding.GetBytes($profileMatches[0])
} catch {
    exit 2
}
[Console]::Out.Write($profileMatches[0])
