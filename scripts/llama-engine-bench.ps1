# llama-engine-bench.ps1 — 本地识图引擎参数组基准（PR3 默认值决策验证）。
#
# 用法（真机，如 i5-11500）：
#   powershell -ExecutionPolicy Bypass -File scripts/llama-engine-bench.ps1 `
#     -ServerBin "$env:USERPROFILE\.pinvou3\llama-engine\bin\llama-server.exe" `
#     -Model    "$env:USERPROFILE\.pinvou3\llama-engine\models\Qwen3VL-2B-Instruct-Q4_K_M.gguf" `
#     -Mmproj   "$env:USERPROFILE\.pinvou3\llama-engine\models\mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf" `
#     -Image    .\test-image.png
#
# 对 CPU(-ngl 0)/GPU(-ngl 99) × 默认参数/PR3 调优参数 四组分别启动 llama-server，
# 对同一张测试图发请求，打印 加载 / 首 token / 总耗时 三段，组间杀干净再起。
# 每组结束打印一行结果，最后给汇总表。

param(
  [Parameter(Mandatory = $true)][string]$ServerBin,
  [Parameter(Mandatory = $true)][string]$Model,
  [Parameter(Mandatory = $true)][string]$Mmproj,
  [Parameter(Mandatory = $true)][string]$Image,
  [int]$BasePort = 18231,
  [int]$MaxTokens = 64,
  [int]$HealthTimeoutSec = 300
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $ServerBin)) { throw "llama-server 不存在: $ServerBin" }
if (-not (Test-Path $Model))     { throw "模型不存在: $Model" }
if (-not (Test-Path $Mmproj))    { throw "mmproj 不存在: $Mmproj" }
if (-not (Test-Path $Image))     { throw "测试图不存在: $Image" }

$imageB64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes((Resolve-Path $Image)))
$ext = [IO.Path]::GetExtension($Image).TrimStart('.').ToLower()
if ($ext -eq 'jpg') { $ext = 'jpeg' }
$imageUrl = "data:image/$ext;base64,$imageB64"

function Stop-Server($proc) {
  if ($proc -and -not $proc.HasExited) {
    # llama-server 无子进程树,直接 kill 即可
    try { $proc.Kill() } catch {}
    try { $proc.WaitForExit(5000) | Out-Null } catch {}
  }
}

function Wait-Healthy([int]$port, $proc, [int]$timeoutSec) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.Elapsed.TotalSeconds -lt $timeoutSec) {
    if ($proc.HasExited) { throw "llama-server 启动即退出（code $($proc.ExitCode)）" }
    try {
      $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2
      if ($resp.status -eq 'ok') { return $sw.Elapsed }
    } catch {}
    Start-Sleep -Milliseconds 500
  }
  throw "等待 /health 超时（${timeoutSec}s）"
}

function Measure-VisionRequest([int]$port) {
  # 流式请求:记录首 token 与整段完成时间。
  $body = @{
    model = 'bench'
    messages = @(@{
      role = 'user'
      content = @(
        @{ type = 'text'; text = 'Describe this image briefly.' },
        @{ type = 'image_url'; image_url = @{ url = $imageUrl } }
      )
    })
    max_tokens = $MaxTokens
    stream = $true
  } | ConvertTo-Json -Depth 8 -Compress

  $sw = [Diagnostics.Stopwatch]::StartNew()
  $req = [Net.HttpWebRequest]::Create("http://127.0.0.1:$port/v1/chat/completions")
  $req.Method = 'POST'
  $req.ContentType = 'application/json'
  $req.ReadWriteTimeout = 600000
  $req.Timeout = 600000
  $bytes = [Text.Encoding]::UTF8.GetBytes($body)
  $req.GetRequestStream().Write($bytes, 0, $bytes.Length)
  $resp = $req.GetResponse()
  $reader = New-Object IO.StreamReader($resp.GetResponseStream())
  $firstTokenAt = $null
  while (-not $reader.EndOfStream) {
    $line = $reader.ReadLine()
    if ($null -eq $firstTokenAt -and $line -match '^\s*data:\s*\{.*"content"') {
      $firstTokenAt = $sw.Elapsed
    }
    if ($line -match 'data:\s*\[DONE\]') { break }
  }
  $reader.Close(); $resp.Close()
  if ($null -eq $firstTokenAt) { $firstTokenAt = $sw.Elapsed }
  return @{ FirstToken = $firstTokenAt; Total = $sw.Elapsed }
}

$configs = @(
  # 基线:PR3 之前的默认参数(仅 ctx/image-max-tokens/ngl)。
  @{ Name = 'cpu-default'; Ngl = '0';  Extra = @() },
  @{ Name = 'gpu-default'; Ngl = '99'; Extra = @() },
  # PR3 调优参数组。
  @{ Name = 'cpu-tuned';   Ngl = '0';  Extra = @('--batch-size','1024','--ubatch-size','1024','--flash-attn','--cache-type-k','q8_0','--cache-type-v','q8_0','--mlock') },
  @{ Name = 'gpu-tuned';   Ngl = '99'; Extra = @('--batch-size','1024','--ubatch-size','1024','--flash-attn','--cache-type-k','q8_0','--cache-type-v','q8_0','--mlock') }
)

$results = @()
$port = $BasePort
foreach ($cfg in $configs) {
  Write-Host "== $($cfg.Name): 启动中（-ngl $($cfg.Ngl)）..."
  $args = @(
    '--model', $Model, '--mmproj', $Mmproj,
    '--host', '127.0.0.1', '--port', "$port",
    '--ctx-size', '8192', '--image-max-tokens', '1024',
    '-ngl', $cfg.Ngl, '--no-webui'
  ) + $cfg.Extra
  $proc = Start-Process -FilePath $ServerBin -ArgumentList $args `
    -WorkingDirectory (Split-Path $ServerBin) -PassThru -WindowStyle Hidden `
    -RedirectStandardError "$env:TEMP\llama-bench-$($cfg.Name).err"
  try {
    $load = Wait-Healthy -port $port -proc $proc -timeoutSec $HealthTimeoutSec
    # 第一次请求 = 冷启动（含 warmup 场景下的真实首请求体感）；第二次 = 热路径。
    $cold = Measure-VisionRequest -port $port
    $warm = Measure-VisionRequest -port $port
    $results += [pscustomobject]@{
      Config      = $cfg.Name
      LoadSec     = [math]::Round($load.TotalSeconds, 1)
      ColdFirstTokenSec = [math]::Round($cold.FirstToken.TotalSeconds, 1)
      ColdTotalSec      = [math]::Round($cold.Total.TotalSeconds, 1)
      WarmFirstTokenSec = [math]::Round($warm.FirstToken.TotalSeconds, 1)
      WarmTotalSec      = [math]::Round($warm.Total.TotalSeconds, 1)
    }
    Write-Host ("   加载 {0}s | 冷 首token {1}s 总 {2}s | 热 首token {3}s 总 {4}s" -f `
      $results[-1].LoadSec, $results[-1].ColdFirstTokenSec, $results[-1].ColdTotalSec, `
      $results[-1].WarmFirstTokenSec, $results[-1].WarmTotalSec)
  } catch {
    Write-Host "   失败: $_"
    $results += [pscustomobject]@{
      Config = $cfg.Name; LoadSec = -1; ColdFirstTokenSec = -1; ColdTotalSec = -1
      WarmFirstTokenSec = -1; WarmTotalSec = -1
    }
  } finally {
    Stop-Server $proc
  }
  $port++
}

Write-Host ''
Write-Host '== 汇总 =='
$results | Format-Table -AutoSize
Write-Host '注：gpu-* 组在无 Vulkan/独显的机器上启动失败（LoadSec=-1）属预期，按 cpu-* 组决策。'
