#Requires -Version 5.1
<#
.SYNOPSIS
    出口体检：确认流量确实从静态 ISP 出去、没有 IPv6 / DNS 泄漏，并且规则在按预期分流。
    只读，不修改任何配置。

.EXAMPLE
    .\check-egress.ps1
    .\check-egress.ps1 -ExpectIp 203.0.113.10
#>
[CmdletBinding()]
param(
    [string]$ProxyHost = '127.0.0.1',
    [int]$ProxyPort = 7890,
    [string]$ExpectIp = '',
    [int]$DownloadBytes = 5000000
)

$ErrorActionPreference = 'Continue'
$px = "http://${ProxyHost}:${ProxyPort}"
$script:pass = 0
$script:fail = 0
$script:warn = 0

function Sec($t) {
    $pad = [Math]::Max(0, 58 - $t.Length)
    Write-Host ''
    Write-Host ("── $t " + ('─' * $pad)) -ForegroundColor Cyan
}
function Kv($k, $v, $color = 'Gray') { Write-Host ("   {0,-18}{1}" -f $k, $v) -ForegroundColor $color }
function Ok($m) { Write-Host "   [PASS] $m" -ForegroundColor Green; $script:pass++ }
function Bad($m) { Write-Host "   [FAIL] $m" -ForegroundColor Red; $script:fail++ }
function Warn($m) { Write-Host "   [WARN] $m" -ForegroundColor Yellow; $script:warn++ }
function Info($m) { Write-Host "   [ .. ] $m" -ForegroundColor DarkGray }

function FetchText {
    param([string]$Url, [switch]$NoProxy, [int]$Timeout = 12, [string[]]$Extra)
    $a = @('-s', '-L', '-m', $Timeout)
    if (-not $NoProxy) { $a += @('-x', $px) }
    if ($Extra) { $a += $Extra }
    $a += $Url
    $out = & curl.exe @a 2>$null
    return @{ Out = (($out | Out-String).Trim()); Code = $LASTEXITCODE }
}

function TimeUrl {
    param([string]$Url, [switch]$NoProxy, [int]$Timeout = 12)
    $a = @('-s', '-L', '-o', 'NUL', '-w', '%{http_code} %{time_total}', '-m', $Timeout)
    if (-not $NoProxy) { $a += @('-x', $px) }
    $a += $Url
    $r = & curl.exe @a 2>$null
    return (($r | Out-String).Trim())
}

$sw = [Diagnostics.Stopwatch]::StartNew()
Clear-Host
Write-Host ''
Write-Host ' 出口体检' -ForegroundColor Cyan
Write-Host (" 代理入口 $px    " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) -ForegroundColor DarkGray

# ---------------- 1 ----------------
Sec '1. 代理入口'
$reg = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction SilentlyContinue
Kv '系统代理' ("Enable={0}  {1}" -f $reg.ProxyEnable, $reg.ProxyServer)
if ($reg.ProxyEnable -eq 1 -and ("$($reg.ProxyServer)" -match (':' + $ProxyPort + '$'))) { Ok "系统代理指向 $ProxyPort" }
else { Bad "系统代理没指向 $ProxyPort（浏览器/其他程序可能绕开）" }

foreach ($p in $ProxyPort, 7897) {
    $c = Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($c) {
        $pr = Get-Process -Id $c.OwningProcess -ErrorAction SilentlyContinue
        Kv "端口 $p" ("LISTENING  PID={0} {1}" -f $c.OwningProcess, $pr.ProcessName) 'Green'
    }
    else { Kv "端口 $p" '无监听' 'Yellow' }
}

$envHit = @()
foreach ($n in 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy') {
    if ([Environment]::GetEnvironmentVariable($n, 'User')) { $envHit += $n }
}
if ($envHit.Count -eq 0) { Ok '无用户级代理环境变量残留' }
else { Bad ("仍有代理环境变量: " + ($envHit -join ', ')) }

# ---------------- 2 ----------------
Sec '2. 出口 IP（经代理，连续 3 次）'
$samples = @()
for ($i = 1; $i -le 3; $i++) {
    $r = FetchText 'https://api.ipify.org'
    if ($r.Code -eq 0 -and $r.Out -match '^\d+\.\d+\.\d+\.\d+$') { $samples += $r.Out; Info ("采样 $i : " + $r.Out) }
    else { Warn ("采样 $i 失败 (curl=$($r.Code))") }
}
$distinct = @($samples | Select-Object -Unique)
$egress = if ($distinct.Count -gt 0) { $distinct[0] } else { $null }

if ($distinct.Count -eq 1) { Ok ("出口固定: " + $egress) }
elseif ($distinct.Count -gt 1) { Bad ("出口 IP 在变: " + ($distinct -join ', ') + "（说明没走静态出口）") }
else { Bad '拿不到出口 IP，代理链路不通' }

if ($ExpectIp) {
    if ($egress -eq $ExpectIp) { Ok "与期望的静态 IP 一致 ($ExpectIp)" }
    else { Bad "期望 $ExpectIp，实际 $egress" }
}

$j = FetchText 'https://ipinfo.io/json'
if ($j.Code -eq 0 -and $j.Out -match '^\s*\{') {
    try {
        $o = $j.Out | ConvertFrom-Json
        Kv '归属地' ("{0}, {1}, {2}" -f $o.city, $o.region, $o.country)
        Kv 'ASN/组织' $o.org
        Kv '时区' $o.timezone
    } catch { Warn 'ipinfo 返回无法解析' }
}
else { Warn 'ipinfo 查询失败（不影响结论）' }

# ---------------- 3 ----------------
Sec '3. 直连对照（证明流量确实被代理改写）'
$directIp = $null
foreach ($u in @('https://api.ip.sb/ip', 'https://api.ipify.org')) {
    $d = FetchText $u -NoProxy -Timeout 8
    if ($d.Code -eq 0 -and $d.Out -match '^([0-9a-fA-F:]{6,}|\d+\.\d+\.\d+\.\d+)$') { $directIp = $Matches[1]; break }
}
if ($directIp) {
    Kv '直连出口' $directIp
    if ($directIp -like '*:*') {
        Info '直连走的是 IPv6；代理侧已关 v6，两条路不同族'
    }
    elseif ($egress -and $directIp -ne $egress) { Ok '代理出口与直连出口不同，代理链路生效' }
    elseif ($egress) { Bad '代理出口与直连相同，代理可能未生效' }
}
else { Warn '直连查询失败（本机直连外网受限，属正常）' }

# ---------------- 4 ----------------
Sec '4. DNS 出口'
$dnsP = FetchText 'https://edns.ip-api.com/json'
$dnsD = FetchText 'https://edns.ip-api.com/json' -NoProxy
$dnsPip = $null; $dnsPgeo = $null; $dnsDip = $null
if ($dnsP.Code -eq 0 -and $dnsP.Out -match '\{') {
    try { $x = $dnsP.Out | ConvertFrom-Json; $dnsPip = $x.dns.ip; $dnsPgeo = $x.dns.geo } catch {}
}
if ($dnsD.Code -eq 0 -and $dnsD.Out -match '\{') {
    try { $y = $dnsD.Out | ConvertFrom-Json; $dnsDip = $y.dns.ip; if ($dnsDip) { Kv '直连时的 DNS 出口' ("$dnsDip  $($y.dns.geo)") } } catch {}
}
if ($dnsPip) {
    Kv '经代理时的 DNS 出口' ("$dnsPip  $dnsPgeo")
    if ($dnsPip -eq $egress) { Ok 'DNS 也经静态出口，无泄漏' }
    elseif ($dnsDip -and $dnsPip -ne $dnsDip) { Info ("解析器出口是 {0}，与直连时（{1}）不同" -f $dnsPip, $dnsDip) }
    else { Warn '解析器出口与直连时相同，建议核对 DNS 配置' }
}
else { Warn '拿不到 DNS 出口信息' }

# ---------------- 5 ----------------
Sec '5. 配置口径（默认出口 / TUN）'
$cfgText = $null; $cfgName = $null
$vergeCfg = Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev\clash-verge.yaml'
$flclashCfg = Join-Path $env:APPDATA 'com.follow\clash\config.yaml'
if (Test-Path $vergeCfg) { $cfgText = [IO.File]::ReadAllText($vergeCfg); $cfgName = 'Clash Verge' }
elseif (Test-Path $flclashCfg) { $cfgText = [IO.File]::ReadAllText($flclashCfg); $cfgName = 'FlClash' }
else { Info '本机没有 Verge / FlClash 的运行配置，跳过（只装了客户端但没启动过才会这样）' }

if ($cfgText) {
    Kv '运行配置' $cfgName
    foreach ($k in 'ipv6', 'mixed-port', 'allow-lan') {
        $m = [regex]::Match($cfgText, "(?m)^${k}:\s*(\S+)")
        if ($m.Success) { Kv $k $m.Groups[1].Value }
    }
    $tun = [regex]::Match($cfgText, '(?ms)^tun:\s*\r?\n(?<body>(?:[ \t]+.*\r?\n)+)')
    if ($tun.Success) {
        $en = [regex]::Match($tun.Groups['body'].Value, '(?m)^\s+enable:\s*(\S+)')
        if ($en.Success) {
            Kv 'TUN' $en.Groups[1].Value
            if ($en.Groups[1].Value -eq 'false') { Ok 'TUN 已关闭' } else { Bad 'TUN 处于开启状态' }
        }
    }
    if ([regex]::Match($cfgText, '(?m)^ipv6:\s*true').Success) { Warn 'ipv6 已开启，可能有 IPv6 泄漏' }

    $entry = [regex]::Match($cfgText, '(?m)^\s*-\s*DOMAIN-SUFFIX,example\.com,DIRECT\s*$')
    if ($entry.Success) { Ok 'example.com 直连规则在位' } else { Warn '运行配置里没找到 example.com 直连规则' }

    # 默认出口：MATCH 指向谁，普通网站（含 ChatGPT）就从谁出去
    $mt = [regex]::Match($cfgText, '(?m)^\s*-\s*MATCH,\s*([^\s,#]+)\s*$')
    $pr = [regex]::Match($cfgText, '(?m)^\s*-\s*DOMAIN-SUFFIX,ipify\.org,\s*([^\s,#]+)\s*$')
    if ($mt.Success) {
        Kv '默认路由 MATCH' $mt.Groups[1].Value
        if ($pr.Success -and $pr.Groups[1].Value -ne $mt.Groups[1].Value) {
            Bad ("探测域名走 {0}，普通网站却走 {1}：ChatGPT 这类站点实际从机场 IP 出去（脚本层未生效）" -f $pr.Groups[1].Value, $mt.Groups[1].Value)
        }
        elseif ($pr.Success) { Ok ("默认出口与探测口径一致（都是 {0}）" -f $mt.Groups[1].Value) }
    }
    else { Warn '运行配置里没有 MATCH 规则，无法判断默认出口' }
}

$v6 = FetchText 'https://api6.ipify.org' -Timeout 8
if ($v6.Code -eq 0 -and $v6.Out -match ':') { Bad ("经代理仍拿到 IPv6: " + $v6.Out) }
else { Ok '经代理取不到 IPv6 出口（无 v6 泄漏）' }

# ---------------- 6 ----------------
Sec '6. 连通性与延迟'
$checks = @(
    @('example.com', 'https://example.com/', 5.0),
    @('gstatic 204', 'https://www.gstatic.com/generate_204', 3.0),
    @('cloudflare', 'https://www.cloudflare.com/cdn-cgi/trace', 3.0)
)
foreach ($c in $checks) {
    $r = TimeUrl $c[1]
    $parts = $r -split '\s+'
    if ($parts.Count -ge 2 -and $parts[1]) {
        $sec = [double]$parts[1]
        $line = "HTTP {0}   {1:N2}s" -f $parts[0], $sec
        if ($sec -le $c[2]) { Kv $c[0] $line 'Green' } else { Kv $c[0] $line 'Yellow' }
    }
    else { Kv $c[0] '失败' 'Red' }
}
$q = TimeUrl 'https://example.com/'
$qp = $q -split '\s+'
if ($qp.Count -ge 2 -and [double]$qp[1] -le 0.5) { Ok ("example.com 走直连（{0}s）" -f $qp[1]) }
elseif ($qp.Count -ge 2) { Warn ("example.com 偏慢（{0}s），可能没走直连" -f $qp[1]) }

$b = TimeUrl 'https://www.baidu.com/' -NoProxy
$bp = $b -split '\s+'
if ($bp.Count -ge 2 -and $bp[1]) { Kv '百度(直连)' ("HTTP {0}  {1:N2}s" -f $bp[0], [double]$bp[1]) }

$trace = FetchText 'https://chatgpt.com/cdn-cgi/trace'
if ($trace.Code -eq 0 -and $trace.Out -match 'ip=([0-9a-fA-F:.]+)') {
    $cgIp = $Matches[1]
    Kv 'ChatGPT 看到' $cgIp
    if ($egress -and $cgIp -eq $egress) { Ok 'ChatGPT 侧看到的也是静态出口' }
    else { Warn ("ChatGPT 侧是 $cgIp，不是静态出口：这个域名没被分流到静态 ISP") }
}
else { Warn 'ChatGPT trace 取不到' }

# 用浏览器 UA 取首页：curl 默认 UA 会被 Cloudflare 判成机器人（403），不能当成"被封"
$ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'
$pg = FetchText 'https://chatgpt.com/' -Extra @('-A', $ua) -Timeout 25
if ($pg.Out -match 'Unable to load site|try turning it off') { Bad 'ChatGPT 首页返回 "Unable to load site"：Cloudflare 拦的是出口 IP，换出口才有效' }
elseif ($pg.Code -eq 0) { Ok 'ChatGPT 首页 200（浏览器可正常打开）' }
else { Warn "ChatGPT 首页 HTTP $($pg.Code)" }

# ---------------- 7 ----------------
Sec '7. 吞吐（下载 5MB）'
$dl = & curl.exe -s -o NUL -w '%{speed_download} %{time_total}' -m 45 -x $px "https://speed.cloudflare.com/__down?bytes=$DownloadBytes" 2>$null
$dps = (($dl | Out-String).Trim() -split '\s+')
if ($dps.Count -ge 2 -and [double]$dps[0] -gt 0) {
    $mbps = [math]::Round(([double]$dps[0] * 8) / 1MB, 2)
    Kv '速率' ("{0} Mbps  用时 {1:N1}s" -f $mbps, [double]$dps[1])
}
else { Warn '下载测速失败' }

# ---------------- 汇总 ----------------
$sw.Stop()
Write-Host ''
Write-Host ('=' * 60) -ForegroundColor DarkGray
Write-Host (" 结果: PASS {0}   WARN {1}   FAIL {2}   用时 {3:N1}s" -f $script:pass, $script:warn, $script:fail, $sw.Elapsed.TotalSeconds) -ForegroundColor $(if ($script:fail -gt 0) { 'Red' } elseif ($script:warn -gt 0) { 'Yellow' } else { 'Green' })
if ($egress) { Write-Host (" 出口: $egress") -ForegroundColor Gray }
Write-Host ('=' * 60) -ForegroundColor DarkGray

if ($script:fail -gt 0) { exit 1 }
