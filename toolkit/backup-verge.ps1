#Requires -Version 5.1
<#
.SYNOPSIS
    给 Clash Verge 做一份可回滚快照：profiles.yaml、verge.yaml、运行配置、以及全部订阅/增强层文件。

.DESCRIPTION
    只读 Verge 目录，写到自己的 backups\ 下（带时间戳），并生成 manifest.txt 与可执行的回滚脚本。
    不碰注册表、不改系统代理、不重启客户端。

.EXAMPLE
    .\backup-verge.ps1
    .\backup-verge.ps1 -Tag before-hopway-apply
#>
[CmdletBinding()]
param(
    [string]$VergeRoot = (Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'),
    [string]$OutRoot = 'D:\HopWay\backups',
    [string]$Tag = 'snapshot',
    [int]$ProxyPort = 7897
)

$ErrorActionPreference = 'Stop'
function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "    $m" -ForegroundColor Green }
function Warn($m) { Write-Host "    [!] $m" -ForegroundColor Yellow }

if (-not (Test-Path $VergeRoot)) { throw "找不到 Verge 目录: $VergeRoot" }
if (-not (Test-Path $OutRoot)) { New-Item -ItemType Directory -Path $OutRoot -Force | Out-Null }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$snap = Join-Path $OutRoot ("verge-{0}-{1}" -f $stamp, $Tag)
$dest = Join-Path $snap 'verge'
New-Item -ItemType Directory -Path (Join-Path $dest 'profiles') -Force | Out-Null

Step "快照目录 $snap"

# 1) 顶层关键文件
$top = @('profiles.yaml', 'verge.yaml', 'clash-verge.yaml')
foreach ($f in $top) {
    $src = Join-Path $VergeRoot $f
    if (Test-Path $src) { Copy-Item $src (Join-Path $dest $f) -Force; Ok "复制 $f" }
    else { Warn "缺少 $f（跳过）" }
}

# 2) 订阅本体 + 增强层（不复制 .bak-*，不复制 mmdb/日志）
$layerFiles = Get-ChildItem (Join-Path $VergeRoot 'profiles') -File |
    Where-Object { $_.Name -notmatch '\.bak-' }
foreach ($f in $layerFiles) {
    Copy-Item $f.FullName (Join-Path (Join-Path $dest 'profiles') $f.Name) -Force
}
Ok ("复制 profiles\ 下 {0} 个文件" -f $layerFiles.Count)

# 3) 清单
$manifest = Join-Path $snap 'manifest.txt'
$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("HopWay snapshot  $stamp")
$lines.Add("source: $VergeRoot")
$lines.Add("")
foreach ($f in (Get-ChildItem $dest -Recurse -File | Sort-Object FullName)) {
    $rel = $f.FullName.Substring($dest.Length + 1)
    $h = (Get-FileHash $f.FullName -Algorithm SHA256).Hash
    $lines.Add(("{0}`t{1}`t{2}" -f $h, $f.Length, $rel))
}
[IO.File]::WriteAllLines($manifest, $lines, (New-Object System.Text.UTF8Encoding($false)))
Ok ("manifest: {0} 个文件" -f ($lines.Count - 3))

# 4) 当时的口径（只记结论，不记凭证）
$facts = New-Object System.Collections.Generic.List[string]
$facts.Add("# 快照时的状态（$stamp）")
try {
    $p = [IO.File]::ReadAllText((Join-Path $VergeRoot 'profiles.yaml'))
    $cur = [regex]::Match($p, '(?m)^current:\s*(\S+)\s*$')
    if ($cur.Success) { $facts.Add("current: " + $cur.Groups[1].Value) }
} catch {}
try {
    $rt = [IO.File]::ReadAllText((Join-Path $VergeRoot 'clash-verge.yaml'))
    foreach ($k in 'ipv6', 'mixed-port', 'allow-lan') {
        $m = [regex]::Match($rt, "(?m)^${k}:\s*(\S+)")
        if ($m.Success) { $facts.Add(("{0}: {1}" -f $k, $m.Groups[1].Value)) }
    }
    $m = [regex]::Match($rt, '(?m)^- MATCH,(\S+)\s*$')
    if ($m.Success) { $facts.Add("MATCH: " + $m.Groups[1].Value) }
    foreach ($n in 'LS-ISP-01', '静态ISP') {
        if ($rt -match ('name:\s*"?' + [regex]::Escape($n) + '"?\s*$')) { $facts.Add("存在: $n") }
    }
} catch {}
try {
    $ip = (& curl.exe -s -m 12 -x ("http://127.0.0.1:" + $ProxyPort) 'https://api.ipify.org' 2>$null | Out-String).Trim()
    if ($ip -match '^\d+\.\d+\.\d+\.\d+$') { $facts.Add("出口 IP($ProxyPort): $ip") }
} catch { $facts.Add("出口 IP: 探测失败") }
try {
    $reg = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction SilentlyContinue
    $facts.Add(("系统代理: Enable={0} {1}" -f $reg.ProxyEnable, $reg.ProxyServer))
} catch {}
[IO.File]::WriteAllLines((Join-Path $snap 'STATE.md'), $facts, (New-Object System.Text.UTF8Encoding($false)))
Ok '记录当时状态（STATE.md）'

# 5) 压缩一份，便于整体搬走
$zip = Join-Path $OutRoot ("verge-{0}-{1}.zip" -f $stamp, $Tag)
Compress-Archive -Path (Join-Path $snap '*') -DestinationPath $zip -Force
Ok ("打包 " + (Split-Path $zip -Leaf))

Write-Host ''
Write-Host ("快照: " + $snap) -ForegroundColor White
Write-Host ("压缩: " + $zip) -ForegroundColor White
Write-Host ''
Write-Host '回滚（出问题再用）:' -ForegroundColor White
Write-Host ("  .\restore-verge-snapshot.ps1 -Snapshot '{0}'" -f $snap)
