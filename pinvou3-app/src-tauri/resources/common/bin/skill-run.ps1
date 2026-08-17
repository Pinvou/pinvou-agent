# skill-run.ps1 <skill-name> <tool-name> '<json-args>'
#
# Super-skill 协议 wrapper（Windows PowerShell 版本）。语义与 bash 版本一致：
# 读 SKILL.md frontmatter 的 `runtime` + `tools` 段、定位 entry、<tool> 调用、回
# 收 stdout JSON。
#
# PowerShell 5+ 兼容。错误返 exit code（与 bash 版一致）+ stderr 写人类可读信息。

[CmdletBinding()]
param(
    [Parameter(Mandatory=$true, Position=0)] [string]$SkillName,
    [Parameter(Mandatory=$true, Position=1)] [string]$ToolName,
    [Parameter(Mandatory=$false, Position=2)] [string]$JsonArgs = '{}'
)

$ErrorActionPreference = 'Stop'

# 1) 定位 skill 目录
$Root = Join-Path $env:USERPROFILE ".pinvou3\bundles\$SkillName\skills\$SkillName"
if (-not (Test-Path $Root)) {
    $Root = Join-Path $env:USERPROFILE ".pinvou3\bundle\skills\$SkillName"
}
if (-not (Test-Path $Root)) {
    $Root = Join-Path $env:USERPROFILE ".pinvou3\bundles\$SkillName"
}
if (-not (Test-Path $Root)) {
    Write-Error "找不到 skill 目录：$SkillName"
    exit 65
}

$MdPath = Join-Path $Root "SKILL.md"
if (-not (Test-Path $MdPath)) {
    Write-Error "SKILL.md 缺失：$MdPath"
    exit 66
}

# 2) 解析 frontmatter——PowerShell 不能像 bash 那样边读边 state machine，
#    走“先抓 frontmatter 段再正则”的两段式。
$content = Get-Content -Raw $MdPath
$lines = $content -split "`n"

if ($lines.Count -lt 2 -or $lines[0].Trim() -ne '---') {
    # 无 frontmatter，直接是 content-only skill
    Write-Error "skill '$SkillName' frontmatter 缺失"
    exit 67
}

$idx = 1
$yamlStart = -1
$yamlEnd = -1
for ($i = 1; $i -lt $lines.Count; $i++) {
    if ($lines[$i].Trim() -eq '---') {
        if ($yamlStart -eq -1) { $yamlStart = $i + 1 }
        else {
            $yamlEnd = $i
            break
        }
    }
}
if ($yamlStart -eq -1 -or $yamlEnd -eq -1) {
    Write-Error "frontmatter 未闭合"
    exit 67
}

$runtimeKind = $null
$runtimeDir = $null
$entry = $null
$toolNameHere = $null
$toolTimeout = $null

for ($j = $yamlStart; $j -lt $yamlEnd; $j++) {
    $line = $lines[$j]
    $trimmed = $line.TrimEnd()
    if ($trimmed -match '^(?<indent>\s*)runtime:\s*$') {
        $inRuntime = $true
        $inTools = $false
        continue
    }
    if ($trimmed -match '^(?<indent>\s*)tools:\s*$') {
        $inRuntime = $false
        $inTools = $true
        continue
    }
    if ($inRuntime) {
        if ($trimmed -match '^\s*kind:\s*(.+)$') { $runtimeKind = $Matches[1].Trim() }
        if ($trimmed -match '^\s*dir:\s*(.+)$') { $runtimeDir = $Matches[1].Trim() }
    }
    if ($inTools) {
        if ($trimmed -match '^\s*-\s*name:\s*(.+)$') {
            $toolNameHere = $Matches[1].Trim()
            continue
        }
        if ($toolNameHere -eq $ToolName) {
            if ($trimmed -match '^\s*entry:\s*(.+)$') { $entry = $Matches[1].Trim() }
            if ($trimmed -match '^\s*timeout_secs:\s*(\d+)') { $toolTimeout = [int]$Matches[1] }
        }
    }
}

if (-not $runtimeKind) {
    Write-Error "未声明 runtime.kind"
    exit 67
}
if (-not $entry) {
    Write-Error "未声明 tool '$ToolName' 的 entry"
    exit 68
}

# 3) 选 runtime
$runtimeBin = $null
switch ($runtimeKind) {
    { $_ -in @('python', 'python3') } {
        if ($runtimeDir) {
            $candidates = @(
                (Join-Path $Root "$runtimeDir\bin\python.exe"),
                (Join-Path $Root "$runtimeDir\bin\python3.exe"),
                (Join-Path $Root "$runtimeDir\python.exe"),
                (Join-Path $Root "$runtimeDir\python3.exe")
            )
            foreach ($c in $candidates) {
                if (Test-Path $c) { $runtimeBin = $c; break }
            }
        }
        if (-not $runtimeBin) {
            $runtimeBin = (Get-Command python -ErrorAction SilentlyContinue).Source
            if (-not $runtimeBin) { $runtimeBin = (Get-Command python3 -ErrorAction SilentlyContinue).Source }
        }
    }
    { $_ -in @('node', 'nodejs') } {
        if ($runtimeDir) {
            $candidates = @(
                (Join-Path $Root "$runtimeDir\bin\node.exe"),
                (Join-Path $Root "$runtimeDir\node.exe")
            )
            foreach ($c in $candidates) {
                if (Test-Path $c) { $runtimeBin = $c; break }
            }
        }
        if (-not $runtimeBin) { $runtimeBin = (Get-Command node -ErrorAction SilentlyContinue).Source }
    }
    'deno' {
        $runtimeBin = (Get-Command deno -ErrorAction SilentlyContinue).Source
    }
    default {
        Write-Error "未支持的 runtime kind: $runtimeKind"
        exit 69
    }
}

if (-not $runtimeBin -or -not (Test-Path $runtimeBin)) {
    Write-Error "runtime '$runtimeKind' 不可执行"
    exit 70
}

# 4) entry 路径
if ($entry.StartsWith('/') -or $entry.StartsWith('\')) {
    $entryPath = $entry
} else {
    $entryPath = Join-Path $Root $entry
}
if (-not (Test-Path $entryPath)) {
    Write-Error "entry 不存在：$entryPath"
    exit 71
}

# 5) 喂 stdin 调 entry
$proc = Start-Process -FilePath $runtimeBin -ArgumentList "`"$entryPath`"" `
    -NoNewWindow -PassThru -RedirectStandardOutput 'stdout.tmp' -RedirectStandardInput 'stdin.tmp' -RedirectStandardError 'stderr.tmp' -WorkingDirectory $Root
$stdinPath = Join-Path $env:TEMP 'skill_run_stdin.tmp'
$JsonArgs | Out-File -FilePath $stdinPath -Encoding ASCII -NoNewline
Copy-Item $stdinPath 'stdin.tmp' -Force

if ($toolTimeout -and $toolTimeout -gt 0) {
    if (-not $proc.WaitForExit($toolTimeout * 1000)) {
        try { $proc.Kill() } catch {}
        Write-Error "skill-run 超时（${toolTimeout}s）"
        exit 72
    }
} else {
    $proc.WaitForExit()
}

if (Test-Path 'stdout.tmp') {
    Get-Content 'stdout.tmp' -Raw
    Remove-Item 'stdout.tmp' -ErrorAction SilentlyContinue
}
$procExit = $proc.ExitCode
if ($procExit -ne 0) {
    Write-Error "{\"error\":\"skill-run exit $procExit\"}"
    exit $procExit
}
