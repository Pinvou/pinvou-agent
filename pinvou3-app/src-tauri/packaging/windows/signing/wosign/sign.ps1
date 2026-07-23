param(
  [Parameter(Mandatory = $true)]
  [string]$FilePath,

  [string]$TimestampUrl = $env:PINVOU3_WOSIGN_TIMESTAMP_URL,
  [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"

function Read-DotEnvValue {
  param(
    [string]$Path,
    [string]$Name
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "The private secrets file was not found: $Path"
  }

  foreach ($sourceLine in Get-Content -Encoding UTF8 -LiteralPath $Path) {
    $line = ([string]$sourceLine).TrimStart([char]0xFEFF).Trim()
    if ([string]::IsNullOrWhiteSpace($line) -or $line.StartsWith("#")) {
      continue
    }
    if ($line.StartsWith("export ")) {
      $line = $line.Substring("export ".Length).TrimStart()
    }

    $separator = $line.IndexOf("=")
    if ($separator -lt 1 -or $line.Substring(0, $separator).Trim() -ne $Name) {
      continue
    }

    $rawValue = $line.Substring($separator + 1).Trim()
    if ($rawValue.StartsWith("'") -or $rawValue.StartsWith('"')) {
      $quote = $rawValue[0]
      $closing = $rawValue.LastIndexOf($quote)
      if ($closing -le 0 -or $rawValue.Substring($closing + 1) -notmatch '^\s*(?:#.*)?$') {
        throw "The value format for $Name in the private secrets file is invalid."
      }
      return $rawValue.Substring(1, $closing - 1)
    }

    return ([regex]::Replace($rawValue, '\s+#.*$', '')).Trim()
  }

  throw "The private secrets file does not define $Name."
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../../../../../.."))
$SecretsPath = Join-Path $repoRoot "scripts/.builtin-secrets.env"
$Thumbprint = Read-DotEnvValue -Path $SecretsPath -Name "PINVOU3_WOSIGN_THUMBPRINT"
$Password = Read-DotEnvValue -Path $SecretsPath -Name "PINVOU3_WOSIGN_PASSWORD"

if ([string]::IsNullOrWhiteSpace($Password)) {
  throw "The configured WoSign password must not be empty."
}

function Normalize-Thumbprint {
  param([string]$Value)

  return ([regex]::Replace([string]$Value, "[^0-9A-Fa-f]", "")).ToUpperInvariant()
}

$normalizedThumbprint = Normalize-Thumbprint -Value $Thumbprint
if ($normalizedThumbprint -notmatch "^[0-9A-F]{40}$") {
  throw "The configured WoSign certificate thumbprint must contain 40 hexadecimal characters."
}

if ([string]::IsNullOrWhiteSpace($TimestampUrl)) {
  $TimestampUrl = "http://timestamp.digicert.com"
}

$resolvedToolPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "wosigncodecmd.exe"))
if (-not (Test-Path -LiteralPath $resolvedToolPath -PathType Leaf)) {
  throw "WoSign command-line tool was not found: $resolvedToolPath"
}

$companionToolPath = Join-Path ([System.IO.Path]::GetDirectoryName($resolvedToolPath)) "wosigncode.exe"
if (-not (Test-Path -LiteralPath $companionToolPath -PathType Leaf)) {
  throw "WoSign companion tool was not found: $companionToolPath"
}
$toolDirectory = [System.IO.Path]::GetDirectoryName($resolvedToolPath)

$resolvedFilePath = [System.IO.Path]::GetFullPath($FilePath)
if (-not (Test-Path -LiteralPath $resolvedFilePath -PathType Leaf)) {
  throw "File to sign was not found: $resolvedFilePath"
}

if ($ValidateOnly) {
  Push-Location -LiteralPath $toolDirectory
  try {
    & $resolvedToolPath "help" 2>&1 | Out-Null
    $validationExitCode = $LASTEXITCODE
  } finally {
    Pop-Location
  }
  if ($validationExitCode -ne 0) {
    throw "WoSign command-line tool validation failed with exit code $validationExitCode."
  }

  Write-Host "WoSign configuration validated for: $resolvedFilePath"
  exit 0
}

$signArguments = @(
  "sign",
  "/tp", $normalizedThumbprint,
  "/p", $Password,
  "/hide",
  "/isf",
  "/c",
  "/dig", "sha256",
  "/tr", $TimestampUrl,
  "/file", $resolvedFilePath
)

Write-Host "WoSign signing started: $resolvedFilePath"
Push-Location -LiteralPath $toolDirectory
try {
  & $resolvedToolPath @signArguments
  $signExitCode = $LASTEXITCODE
} finally {
  Pop-Location
}
Write-Host "WoSign command exited with code $signExitCode for: $resolvedFilePath"
if ($signExitCode -ne 0) {
  throw "WoSign failed with exit code $signExitCode while signing: $resolvedFilePath"
}

Write-Host "WoSign signing completed: $resolvedFilePath"
