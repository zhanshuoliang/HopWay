#Requires -Version 5.1
<#
.SYNOPSIS
    只读查询 Clash Verge 内核：通过 Verge 的命名管道调 mihomo 控制器，列出各策略组当前选中的节点。
    不打印 secret、不打印节点凭证。

.EXAMPLE
    .\query-verge.ps1
    .\query-verge.ps1 -Json
#>
[CmdletBinding()]
param(
    [string]$VergeRoot = (Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'),
    [switch]$Json
)

$ErrorActionPreference = 'Stop'
$rt = Join-Path $VergeRoot 'clash-verge.yaml'
if (-not (Test-Path $rt)) { throw "找不到 $rt（Verge 没在运行过？）" }
$text = [IO.File]::ReadAllText($rt)

$pipe = [regex]::Match($text, '(?m)^external-controller-pipe:\s*\\\\.\\pipe\\(.+?)\s*$').Groups[1].Value
$secret = [regex]::Match($text, '(?m)^secret:\s*(\S*)\s*$').Groups[1].Value
# Verge 2.5.x 实际创建的是 verge-mihomo-production-<hash>；运行配置里的 sidecar-release 名可能已过期
$live = Get-ChildItem -Path '\\.\pipe\' | Where-Object { $_.Name -like 'verge-mihomo-*' } | Select-Object -First 1
if ($live) { $pipe = $live.Name }
if (-not $pipe) { throw '运行配置里没有 external-controller-pipe，也没找到 verge-mihomo 管道' }

$client = New-Object IO.Pipes.NamedPipeClientStream('.', $pipe, [IO.Pipes.PipeDirection]::InOut)
try { $client.Connect(3000) } catch { throw "连不上 Verge 控制器管道（Verge 没在运行？）" }

try {
    $req = "GET /proxies HTTP/1.1`r`nHost: localhost`r`nConnection: close`r`n"
    if ($secret) { $req += "Authorization: Bearer $secret`r`n" }
    $req += "`r`n"
    $bytes = [Text.Encoding]::UTF8.GetBytes($req)
    $client.Write($bytes, 0, $bytes.Length)
    $client.Flush()

    $sr = New-Object IO.StreamReader($client, [Text.Encoding]::UTF8)
    $raw = $sr.ReadToEnd()
}
finally { $client.Dispose() }

$split = $raw.IndexOf("`r`n`r`n")
if ($split -lt 0) { throw '控制器响应不完整' }
$headers = $raw.Substring(0, $split)
$body = $raw.Substring($split + 4)
if ($headers -match '401') { throw '控制器拒绝访问（secret 不匹配）' }

# mihomo 用分块传输；这里只有一个大块，直接取首尾大括号之间的 JSON 最稳
$first = $body.IndexOf('{')
$last = $body.LastIndexOf('}')
if ($first -lt 0 -or $last -le $first) { throw '控制器响应里没有 JSON' }
$body = $body.Substring($first, $last - $first + 1)

$data = $body | ConvertFrom-Json
if ($Json) { $data | ConvertTo-Json -Depth 6; exit 0 }

$groups = $data.proxies.PSObject.Properties.Value |
    Where-Object { $_.type -in @('selector', 'select', 'url-test', 'fallback', 'load-balance', 'relay') } |
    Sort-Object name

Write-Host ''
Write-Host '策略组当前选择' -ForegroundColor Cyan
foreach ($g in $groups) {
    $now = $g.now
    $line = ("  {0,-28} {1,-12} -> {2}" -f $g.name, $g.type, $now)
    if ($g.name -match '静态|机场|自动选择|故障转移') { Write-Host $line -ForegroundColor White }
    else { Write-Host $line -ForegroundColor DarkGray }
}

Write-Host ''
Write-Host '全部节点（名称 / 类型 / 延迟）' -ForegroundColor Cyan
$nodes = $data.proxies.PSObject.Properties.Value |
    Where-Object { $_.type -notin @('selector', 'select', 'url-test', 'fallback', 'load-balance', 'relay', 'direct', 'reject', 'compatible') } |
    Sort-Object name
foreach ($n in $nodes) {
    $hist = $n.history | Select-Object -Last 1
    $delay = if ($hist) { "$($hist.delay)ms" } else { '-' }
    Write-Host ("  {0,-42} {1,-14} {2}" -f $n.name, $n.type, $delay)
}

Write-Host ''
Write-Host ("共 {0} 个策略组 / {1} 个节点" -f $groups.Count, $nodes.Count) -ForegroundColor DarkGray
