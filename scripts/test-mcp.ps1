<#
.SYNOPSIS
    cs-mcp 自测脚本：直接以 stdio 启动 MCP Server，跑握手 / tools/list / tools/call。

.DESCRIPTION
    不依赖任何外部工具（不需要 MCP Inspector / node），只用 JSON-RPC over stdio。
    默认使用 release 二进制；可指定 -Exe 指向 debug 版本。

.EXAMPLE
    .\scripts\test-mcp.ps1 -List
        列出所有 tool 及其描述（贴进模型系统提示用）

.EXAMPLE
    .\scripts\test-mcp.ps1 -Tool cs_ping
        调用单个 tool

.EXAMPLE
    .\scripts\test-mcp.ps1 -Tool cs_upcoming_matches -Args '{"hours":72}'
        带参数调用

.EXAMPLE
    .\scripts\test-mcp.ps1 -All
        跑一遍全部 tool（含 cs_recent_results → cs_match_detail 的链式验证）
#>
[CmdletBinding()]
param(
    [string]$Tool,
    [string]$Args = '{}',
    [switch]$All,
    [switch]$List,
    [string]$Exe,
    [int]$TimeoutSec = 60
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) {
    $release = Join-Path $root 'target\release\cs-mcp.exe'
    $debug = Join-Path $root 'target\debug\cs-mcp.exe'
    $Exe = if (Test-Path $release) { $release } elseif (Test-Path $debug) { $debug } else {
        throw "找不到 cs-mcp.exe，请先 cargo build --release"
    }
}
if (-not (Test-Path $Exe)) { throw "可执行文件不存在: $Exe" }

Write-Host "== cs-mcp 自测 ==" -ForegroundColor Cyan
Write-Host "exe: $Exe"

# ---------------------------------------------------------------- 进程与 RPC

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = (Resolve-Path $Exe)
$psi.WorkingDirectory = $root            # 必须在此目录，dotenvy 才会读到 .env
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false

$proc = [System.Diagnostics.Process]::Start($psi)
$stderrTask = $proc.StandardError.ReadToEndAsync()

$script:nextId = 1

function Send-Rpc([string]$json) {
    $proc.StandardInput.WriteLine($json)
    $proc.StandardInput.Flush()
}

function Read-Until([scriptblock]$pred, [int]$timeout) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $timeout) {
        $t = $proc.StandardOutput.ReadLineAsync()
        while (-not $t.IsCompleted -and $sw.Elapsed.TotalSeconds -lt $timeout) {
            Start-Sleep -Milliseconds 60
        }
        if (-not $t.IsCompleted) { return $null }
        $line = $t.Result
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        try { $j = $line | ConvertFrom-Json } catch { continue }
        if (& $pred $j) { return $j }
    }
    return $null
}

function Call-Rpc([string]$method, [string]$params, [int]$timeout) {
    $id = $script:nextId
    $script:nextId = $id + 1
    Send-Rpc ('{"jsonrpc":"2.0","id":' + $id + ',"method":"' + $method + '","params":' + $params + '}')
    return Read-Until { param($j) $j.id -eq $id } $timeout
}

try {
    # ------------------------------------------------------------ 握手
    $init = Call-Rpc 'initialize' '{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cs-mcp-selftest","version":"0"}}' 20
    if (-not $init) { throw "握手失败（initialize 无响应）" }
    Send-Rpc '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    Write-Host ("握手 OK: {0} {1}  protocol={2}" -f `
        $init.result.serverInfo.name, $init.result.serverInfo.version, $init.result.protocolVersion) -ForegroundColor Green

    # ------------------------------------------------------------ tools/list
    # 注意：变量名不能叫 $list —— PowerShell 变量名大小写不敏感，
    # 会覆盖上面 param 里的 [switch]$List，触发 SwitchParameter 转换错误。
    $listResp = Call-Rpc 'tools/list' '{}' 20
    if (-not $listResp) { throw "tools/list 无响应" }
    $tools = @($listResp.result.tools)
    Write-Host ("tools: {0} 个" -f $tools.Count) -ForegroundColor Green

    if ($List) {
        foreach ($t in ($tools | Sort-Object name)) {
            $desc = $t.description
            if ($desc.Length -gt 90) { $desc = $desc.Substring(0, 90) + '…' }
            Write-Host ""
            Write-Host ("  " + $t.name) -ForegroundColor Yellow
            Write-Host ("    " + $desc)
        }
        return
    }

    # ------------------------------------------------------------ 选定要跑的用例
    $cases = @()
    if ($All) {
        $cases = @(
            @{ name = 'cs_ping';             args = '{}' },
            @{ name = 'cs_live_matches';     args = '{}' },
            @{ name = 'cs_upcoming_matches'; args = '{"hours":72}' },
            @{ name = 'cs_recent_results';   args = '{"hours":48}' },
            @{ name = 'cs_today_schedule';   args = '{}' },
            @{ name = 'cs_team_matches';     args = '{"team":"MOUZ","limit":3}' },
            @{ name = 'cs_world_ranking';    args = '{"top":10}' }
        )
    }
    elseif ($Tool) {
        $cases = @(@{ name = $Tool; args = $Args })
    }
    else {
        Write-Host ""
        Write-Host "用法: -List | -All | -Tool <name> [-Args '<json>']" -ForegroundColor Yellow
        return
    }

    # ------------------------------------------------------------ 逐个调用
    $pass = 0; $fail = 0
    $firstMatchId = $null

    foreach ($c in $cases) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $resp = Call-Rpc 'tools/call' ('{"name":"' + $c.name + '","arguments":' + $c.args + '}') $TimeoutSec
        $sw.Stop()

        if (-not $resp) {
            Write-Host ("  {0,-22} 超时({1}s)" -f $c.name, $TimeoutSec) -ForegroundColor Red
            $fail++; continue
        }

        $payload = $resp.result.content[0].text | ConvertFrom-Json
        if ($payload.ok) {
            $meta = $payload.meta
            $src = if ($meta.sources_used) { ($meta.sources_used -join ',') } else { '-' }
            Write-Host ("  {0,-22} OK   {1,6} ms  count={2,-3} src=[{3}]{4}" -f `
                    $c.name, [int]$sw.ElapsedMilliseconds, $meta.count, $src,
                $(if ($meta.cached) { ' (cached)' } else { '' })) -ForegroundColor Green
            $pass++

            if ($c.name -eq 'cs_recent_results' -and -not $firstMatchId -and $payload.data.Count -gt 0) {
                # 只挑「已结束」的场次：单场详情端点对进行中/未开赛的 id 可能返回 403。
                $finished = $payload.data | Where-Object { $_.status -eq 'finished' } | Select-Object -First 1
                if ($finished) { $firstMatchId = $finished.id } else { $firstMatchId = $payload.data[0].id }
            }
        }
        else {
            Write-Host ("  {0,-22} 错误 {1}: {2}" -f $c.name, $payload.error.code, $payload.error.message) -ForegroundColor Red
            $fail++
        }
    }

    # ------------------------------------------------------------ 链式验证 match_detail
    if ($All -and $firstMatchId) {
        $a = '{"match_id":"' + $firstMatchId + '"}'
        $resp = Call-Rpc 'tools/call' ('{"name":"cs_match_detail","arguments":' + $a + '}') $TimeoutSec
        if ($resp) {
            $payload = $resp.result.content[0].text | ConvertFrom-Json
            if ($payload.ok) {
                Write-Host ("  cs_match_detail        OK   id={0}" -f $firstMatchId) -ForegroundColor Green
                $pass++
            }
            else {
                # 403/查不到并不代表程序错，如实报告即可
                Write-Host ("  cs_match_detail        {0}: {1}" -f $payload.error.code, $payload.error.message) -ForegroundColor Yellow
            }
        }
    }

    Write-Host ""
    Write-Host ("结果: 通过 {0} / 失败 {1}" -f $pass, $fail) -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Red' })
    if ($fail -gt 0) { exit 1 }
}
finally {
    try { $proc.StandardInput.Close() } catch {}
    $proc.WaitForExit(5000) | Out-Null
    if (-not $proc.HasExited) { $proc.Kill() }

    $err = $stderrTask.Result
    if ($err -and $err -match 'ERROR|panick') {
        Write-Host ""
        Write-Host "-- stderr 中的错误行 --" -ForegroundColor DarkYellow
        ($err -split "`n" | Where-Object { $_ -match 'ERROR|panick' }) | Select-Object -First 5 | ForEach-Object { Write-Host "  $_" }
    }
}
