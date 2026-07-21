param(
  [Parameter(Mandatory = $true)]
  [string]$FilePath,

  [string]$TimestampUrl = $env:PINVOU3_WOSIGN_TIMESTAMP_URL,
  [string]$ToolPath = $env:PINVOU3_WOSIGN_TOOL_PATH,
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

if ([string]::IsNullOrWhiteSpace($ToolPath)) {
  $ToolPath = Join-Path $PSScriptRoot "wosigncodecmd.exe"
}

$resolvedToolPath = [System.IO.Path]::GetFullPath($ToolPath)
if (-not (Test-Path -LiteralPath $resolvedToolPath -PathType Leaf)) {
  throw "WoSign command-line tool was not found: $resolvedToolPath"
}

$resolvedFilePath = [System.IO.Path]::GetFullPath($FilePath)
if (-not (Test-Path -LiteralPath $resolvedFilePath -PathType Leaf)) {
  throw "File to sign was not found: $resolvedFilePath"
}

if ($ValidateOnly) {
  Write-Host "WoSign parameters validated for: $resolvedFilePath"
  exit 0
}

$signArguments = @(
  "sign",
  "/tp", $normalizedThumbprint,
  "/p", $Password,
  "/hide",
  "/c",
  "/dig", "sha256",
  "/tr", $TimestampUrl,
  "/file", $resolvedFilePath
)

& $resolvedToolPath @signArguments
$signExitCode = $LASTEXITCODE
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
  throw "The file signature has no timestamp certificate: $resolvedFilePath"
}

Write-Host "WoSign signed and verified: $resolvedFilePath"
