#Requires -Version 5.1
# 通过 Verge 控制器切换某个策略组的选中节点（只改运行态，不改文件）。
param(
    [Parameter(Mandatory = $true)][string]$Group,
    [Parameter(Mandatory = $true)][string]$Node
)
$ErrorActionPreference = 'Stop'
$rt = Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev\clash-verge.yaml'
$text = [IO.File]::ReadAllText($rt)
$secret = [regex]::Match($text, '(?m)^secret:\s*(\S*)\s*$').Groups[1].Value
$pipe = (Get-ChildItem -Path '\\.\pipe\' | Where-Object { $_.Name -like 'verge-mihomo-*' } | Select-Object -First 1).Name
if (-not $pipe) { throw '没找到 verge-mihomo 管道' }

$json = (@{ name = $Node } | ConvertTo-Json -Compress)
$bodyBytes = [Text.Encoding]::UTF8.GetBytes($json)
$path = '/proxies/' + [uri]::EscapeDataString($Group)
$head = "PUT $path HTTP/1.1`r`nHost: localhost`r`nContent-Type: application/json`r`nContent-Length: $($bodyBytes.Length)`r`nConnection: close`r`n"
if ($secret) { $head += "Authorization: Bearer $secret`r`n" }
$head += "`r`n"
$headBytes = [Text.Encoding]::UTF8.GetBytes($head)

$c = New-Object IO.Pipes.NamedPipeClientStream('.', $pipe, [IO.Pipes.PipeDirection]::InOut)
$c.Connect(3000)
try {
    $c.Write($headBytes, 0, $headBytes.Length)
    $c.Write($bodyBytes, 0, $bodyBytes.Length)
    $c.Flush()
    $ms = New-Object IO.MemoryStream
    $buf = New-Object byte[] 4096
    while ($true) { $n = $c.Read($buf, 0, $buf.Length); if ($n -le 0) { break }; $ms.Write($buf, 0, $n) }
    $status = [Text.Encoding]::UTF8.GetString($ms.ToArray()).Split("`r")[0]
    Write-Host ("{0} -> {1} : {2}" -f $Group, $Node, $status)
}
finally { $c.Dispose() }
