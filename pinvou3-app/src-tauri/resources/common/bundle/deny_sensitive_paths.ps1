$ErrorActionPreference = "Stop"

$toolName = if ($env:DEEPSEEK_TOOL_NAME) { $env:DEEPSEEK_TOOL_NAME } else { "unknown" }
$argsText = if ($env:DEEPSEEK_TOOL_ARGS) { $env:DEEPSEEK_TOOL_ARGS } else { "" }
$haystack = $argsText.ToLowerInvariant()

function Deny-SensitivePath {
    param([string]$Reason)
    [Console]::Error.WriteLine("pinvou3-deny: tool '$toolName' attempted to touch sensitive path ($Reason) - blocked")
    exit 2
}

$sensitiveDirs = @(
    "/.ssh/",
    "\.ssh\",
    "\\.ssh\\",
    "/.ssh\",
    "\.ssh/",
    "%userprofile%\.ssh",
    "%userprofile%\\.ssh",
    "$home\.ssh",
    "$home\\.ssh",
    '$home\.ssh',
    '$home\\.ssh',
    "/.gnupg/",
    "\.gnupg\",
    "\\.gnupg\\",
    "/.aws/",
    "\.aws\",
    "\\.aws\\",
    "/.docker/",
    "\.docker\",
    "\\.docker\\",
    "/.kube/",
    "\.kube\",
    "\\.kube\\",
    "/.config/google-chrome/",
    "\.config\google-chrome\",
    "\\.config\\google-chrome\\",
    "/.mozilla/firefox/",
    "\.mozilla\firefox\",
    "\\.mozilla\\firefox\\",
    "/.password-store/",
    "\.password-store\",
    "\\.password-store\\",
    "/.tmeet/",
    "\.tmeet\",
    "\\.tmeet\\",
    "/.weibo-cli/",
    "\.weibo-cli\",
    "\\.weibo-cli\\",
    "%appdata%\microsoft\credentials",
    "%appdata%\\microsoft\\credentials",
    "%localappdata%\microsoft\credentials",
    "%localappdata%\\microsoft\\credentials",
    "%appdata%\microsoft\protect",
    "%appdata%\\microsoft\\protect",
    "%localappdata%\microsoft\protect",
    "%localappdata%\\microsoft\\protect",
    "\microsoft\credentials\",
    "\\microsoft\\credentials\\",
    "\microsoft\protect\",
    "\\microsoft\\protect\\"
)

foreach ($pattern in $sensitiveDirs) {
    if ($haystack.Contains($pattern.ToLowerInvariant())) {
        Deny-SensitivePath $pattern
    }
}

$sensitiveNames = @(
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "authorized_keys",
    ".pgp",
    ".gpg",
    "credentials",
    "credential-ref",
    "secrets",
    "/.netrc",
    "\.netrc",
    "\\.netrc",
    "/.git-credentials",
    "\.git-credentials",
    "\\.git-credentials"
)

foreach ($pattern in $sensitiveNames) {
    if ($haystack.Contains($pattern.ToLowerInvariant())) {
        Deny-SensitivePath $pattern
    }
}

if ($toolName -like "exec_shell*" -or $toolName -eq "code_execution") {
    $dangerousCommands = @(
        "cat ~/.ssh",
        "cat /etc/shadow",
        "cat /etc/sudoers",
        "ssh-keygen",
        "gpg --export-secret",
        "cat ~/.aws/credentials",
        "type %userprofile%\.ssh",
        "type %userprofile%\\.ssh",
        "get-content $home\.ssh",
        "get-content $home\\.ssh",
        'get-content $home\.ssh',
        'get-content $home\\.ssh',
        "cmdkey",
        "vaultcmd",
        "get-storedcredential",
        "get-credential",
        "keymgr.dll",
        "krshowkeymgr",
        "control /name microsoft.credentialmanager",
        "control.exe /name microsoft.credentialmanager"
    )

    foreach ($pattern in $dangerousCommands) {
        if ($haystack.Contains($pattern.ToLowerInvariant())) {
            Deny-SensitivePath $pattern
        }
    }
}

exit 0
