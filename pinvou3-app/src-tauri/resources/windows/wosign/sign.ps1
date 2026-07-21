param(
  [Parameter(Mandatory = $true)]
  [string]$FilePath,

  [string]$TimestampUrl = $env:PINVOU3_WOSIGN_TIMESTAMP_URL,
  [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"
$Thumbprint = "454f2009a9243f9e560237965b33382362a3dd55"
$Password = "12345678"

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

$signature = Get-AuthenticodeSignature -LiteralPath $resolvedFilePath
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
  throw "WoSign completed, but Authenticode verification failed ($($signature.Status)): $resolvedFilePath"
}

$actualThumbprint = Normalize-Thumbprint -Value $signature.SignerCertificate.Thumbprint
if ($actualThumbprint -ne $normalizedThumbprint) {
  throw "The file was signed by an unexpected certificate ($actualThumbprint): $resolvedFilePath"
}

if (-not $signature.TimeStamperCertificate) {
  $timestampUrls = @(
    $TimestampUrl,
    "http://tsa.wosign.com/timestamp",
    "http://timestamp.wosign.com/rfc3161"
  ) | Select-Object -Unique

  foreach ($candidateTimestampUrl in $timestampUrls) {
    Write-Host "WoSign timestamp retry started with $candidateTimestampUrl for: $resolvedFilePath"
    $timestampArguments = @(
      "timestamp",
      "/hide",
      "/c",
      "/tr", $candidateTimestampUrl,
      "/file", $resolvedFilePath
    )

    Push-Location -LiteralPath $toolDirectory
    try {
      & $resolvedToolPath @timestampArguments
      $timestampExitCode = $LASTEXITCODE
    } finally {
      Pop-Location
    }
    Write-Host "WoSign timestamp command exited with code $timestampExitCode for: $resolvedFilePath"

    $signature = Get-AuthenticodeSignature -LiteralPath $resolvedFilePath
    if ($signature.TimeStamperCertificate) {
      break
    }
  }

  if (-not $signature.TimeStamperCertificate) {
    throw "The file signature has no timestamp certificate after all timestamp retries: $resolvedFilePath"
  }
}

Write-Host "WoSign signed and verified: $resolvedFilePath"
