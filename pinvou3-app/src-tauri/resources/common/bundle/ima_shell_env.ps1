$toolArgs = [Environment]::GetEnvironmentVariable("DEEPSEEK_TOOL_ARGS", "Process")
if ([string]::IsNullOrWhiteSpace($toolArgs)) {
    exit 0
}

$keys = @(
    "IMA_CLIENT_ID",
    "IMA_API_KEY",
    "IMA_OPENAPI_CLIENTID",
    "IMA_OPENAPI_APIKEY"
)

$usesHelper = $toolArgs -match "ima_api\.cjs"
$probesImaEnv = $toolArgs -match "IMA_(CLIENT_ID|API_KEY|OPENAPI_CLIENTID|OPENAPI_APIKEY)"

if (-not $usesHelper -and -not $probesImaEnv) {
    exit 0
}

$values = @{}
foreach ($key in $keys) {
    $value = [Environment]::GetEnvironmentVariable($key, "Process")
    if ([string]::IsNullOrWhiteSpace($value)) {
        continue
    }
    if ($value -match "[`r`n]") {
        continue
    }
    $values[$key] = $value
}

if (-not $values.ContainsKey("IMA_CLIENT_ID") -or -not $values.ContainsKey("IMA_API_KEY")) {
    exit 0
}

foreach ($key in $keys) {
    if (-not $values.ContainsKey($key)) {
        continue
    }
    if ($usesHelper) {
        [Console]::Out.WriteLine($key + "=" + $values[$key])
    } else {
        [Console]::Out.WriteLine($key + "=__PINVOU3_IMA_CONNECTED__")
    }
}
