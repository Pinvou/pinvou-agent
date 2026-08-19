param(
  [string]$FixturePath = "tests/fixtures/voice-normalize-labeled-samples.json",
  [string]$OutputDir = "tests/fixtures/voice-audio-samples",
  [string]$ManifestPath = "tests/fixtures/voice-audio-samples.json",
  [string]$PreferredVoice = "Microsoft Huihui Desktop"
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Speech

$root = (Get-Location).Path
$fixtureFullPath = Join-Path $root $FixturePath
$outputFullDir = Join-Path $root $OutputDir
$manifestFullPath = Join-Path $root $ManifestPath

if (-not (Test-Path -LiteralPath $fixtureFullPath)) {
  throw "Fixture file not found: $fixtureFullPath"
}

New-Item -ItemType Directory -Force -Path $outputFullDir | Out-Null

$samples = Get-Content -LiteralPath $fixtureFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$voice = $synth.GetInstalledVoices() |
  Where-Object { $_.Enabled -and $_.VoiceInfo.Name -eq $PreferredVoice } |
  Select-Object -First 1

if (-not $voice) {
  $voice = $synth.GetInstalledVoices() |
    Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -eq "zh-CN" } |
    Select-Object -First 1
}

if (-not $voice) {
  throw "No zh-CN TTS voice is installed."
}

$synth.SelectVoice($voice.VoiceInfo.Name)
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(
  16000,
  [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
  [System.Speech.AudioFormat.AudioChannel]::Mono
)

$ratePattern = @{
  "short_clear" = 1
  "filler_only" = -2
  "multi_constraint" = -1
  "mixed_language" = 0
}

$manifest = @()

foreach ($sample in $samples) {
  $spokenText = $sample.gold_text
  if ([string]::IsNullOrWhiteSpace($spokenText)) {
    $spokenText = $sample.asr_text
  }

  $fileName = "$($sample.id).wav"
  $relativeAudioPath = ($OutputDir.TrimEnd("/\") + "/" + $fileName).Replace("\", "/")
  $fullAudioPath = Join-Path $outputFullDir $fileName

  $synth.Rate = 0
  if ($ratePattern.ContainsKey([string]$sample.type)) {
    $synth.Rate = [int]$ratePattern[[string]$sample.type]
  }

  $synth.SetOutputToWaveFile($fullAudioPath, $format)
  $synth.Speak($spokenText)
  $synth.SetOutputToNull()

  $audioFile = Get-Item -LiteralPath $fullAudioPath
  $manifest += [PSCustomObject]@{
    id = $sample.id
    audio = $relativeAudioPath
    spoken_text = $spokenText
    gold_text = $sample.gold_text
    mode = $sample.mode
    type = $sample.type
    voice = $voice.VoiceInfo.Name
    sample_rate_hz = 16000
    channels = 1
    bytes = $audioFile.Length
    synthetic = $true
  }
}

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText(
  $manifestFullPath,
  (($manifest | ConvertTo-Json -Depth 20) + [Environment]::NewLine),
  $utf8NoBom
)

Write-Host "Generated $($manifest.Count) wav files in $OutputDir using $($voice.VoiceInfo.Name)."
