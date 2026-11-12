#Requires -Version 5.1
<#
.SYNOPSIS
    把 A 订阅绑定的 Verge 增强层，整份复制到 B 订阅自己的层文件上。
    用于换订阅 / 换账号：出口链路（静态节点 + 组 + 规则 + 脚本 + merge）跟着走，出口 IP 不变。

.DESCRIPTION
    只做文件级复制，不改 profiles.yaml，不切 current，不重启客户端。
    目标层必须是空模板（Verge 新建订阅时生成的那种），否则需要 -Force。
    复制前给目标层留 .bak-yyyyMMdd-HHmmss 备份。
    复制后需要你在 Verge 里点一下新的配置卡片，再重启/重载内核才生效。

.EXAMPLE
    .\copy-verge-layers.ps1 -FromUid P-old01 -ToUid P-new02
    .\copy-verge-layers.ps1 -FromUid P-old01 -ToUid P-new02 -DryRun
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$FromUid,
    [Parameter(Mandatory = $true)][string]$ToUid,
    [string]$VergeRoot = (Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'),
    [ValidateSet('rules', 'proxies', 'groups', 'script', 'merge')]
    [string[]]$Layers = @('rules', 'proxies', 'groups', 'script', 'merge'),
    [switch]$Force,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Say($m, $c = 'Gray') { Write-Host "    $m" -ForegroundColor $c }
function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Die($m) { Write-Host "    [x] $m" -ForegroundColor Red; exit 1 }

$profilesYaml = Join-Path $VergeRoot 'profiles.yaml'
$profilesDir = Join-Path $VergeRoot 'profiles'
if (-not (Test-Path $profilesYaml)) { Die "找不到 $profilesYaml" }

# 从 profiles.yaml 取某个 uid 条目的 file: 与它绑定的各层 uid
function Get-ProfileEntry {
    param([string]$Uid)
    $lines = [IO.File]::ReadAllLines($profilesYaml)
    $start = -1; $end = $lines.Count
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match ('^-\s+uid:\s*' + [regex]::Escape($Uid) + '\s*$')) { $start = $i; break }
    }
    if ($start -lt 0) { return $null }
    for ($i = $start + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^-\s+uid:') { $end = $i; break }
    }
    $block = $lines[$start..($end - 1)]
    $entry = @{ uid = $Uid; file = $null; layers = @{}; current = $false }
    foreach ($l in $block) {
        if ($l -match '^\s{2}file:\s*(\S+)\s*$') { $entry.file = $Matches[1] }
        elseif ($l -match '^\s{4}(rules|proxies|groups|script|merge):\s*(\S+)\s*$') {
            $entry.layers[$Matches[1]] = $Matches[2]
        }
    }
    return $entry
}

$from = Get-ProfileEntry $FromUid
$to = Get-ProfileEntry $ToUid
if (-not $from) { Die "profiles.yaml 里没有 $FromUid" }
if (-not $to) { Die "profiles.yaml 里没有 $ToUid" }
Say ("源  : {0} ({1})" -f $FromUid, $from.file)
Say ("目标: {0} ({1})" -f $ToUid, $to.file)

$currentUid = $null
foreach ($l in [IO.File]::ReadAllLines($profilesYaml)) { if ($l -match '^current:\s*(\S+)\s*$') { $currentUid = $Matches[1]; break } }
if ($currentUid -eq $FromUid) { Say '注意: 当前激活的还是源订阅，切换要在 Verge 界面里点卡片' 'Yellow' }

# 目标层是否为 Verge 空模板（只有注释 / prepend: [] / append: [] / delete: [] / 空 main）
function Test-Template {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $false }
    $t = [IO.File]::ReadAllText($Path)
    $body = ($t -split "`r?`n" | Where-Object { $_ -notmatch '^\s*#' -and $_.Trim() -ne '' }) -join "`n"
    if ($body.Trim() -eq '') { return $true }
    if ($Path -like '*.js') { return ($body -match 'function main\(config,\s*profileName\)\s*\{\s*return config;\s*\}') }
    foreach ($l in ($body -split "`n")) {
        if ($l -notmatch '^\s*(prepend|append|delete):\s*(\[\s*\]|)\s*$') { return $false }
    }
    return $true
}

# 只报名字，不碰凭证
function Get-LayerSummary {
    param([string]$Path, [string]$Kind)
    $t = [IO.File]::ReadAllText($Path)
    if ($Kind -eq 'proxies') {
        $names = [regex]::Matches($t, '(?m)^\s*-\s+name:\s*"?([^"\r\n]+)"?') | ForEach-Object { $_.Groups[1].Value }
        return ('节点 ' + ($names -join ', '))
    }
    if ($Kind -eq 'groups') {
        $names = [regex]::Matches($t, '(?m)^\s*-\s+name:\s*"?([^"\r\n]+)"?') | ForEach-Object { $_.Groups[1].Value }
        return ('组 ' + ($names -join ', '))
    }
    if ($Kind -eq 'rules') {
        $n = ([regex]::Matches($t, '(?m)^\s*-\s+(DOMAIN|IP-CIDR|PROCESS|GEOIP|MATCH)')).Count
        return ("规则 $n 条")
    }
    if ($Kind -eq 'script') {
        return $(if ($t -match 'MATCH,') { '脚本层含 MATCH' } else { '脚本层无 MATCH' })
    }
    return 'merge'
}

$missing = @()
foreach ($k in $Layers) {
    if (-not $from.layers.ContainsKey($k)) { $missing += "$k(源未绑定)" }
    elseif (-not $to.layers.ContainsKey($k)) { $missing += "$k(目标未绑定)" }
}
if ($missing.Count -gt 0) { Die ("这些层无法复制: " + ($missing -join ', ')) }

$plan = @()
foreach ($k in $Layers) {
    $src = Join-Path $profilesDir ($from.layers[$k] + $(if ($k -eq 'script') { '.js' } else { '.yaml' }))
    $dst = Join-Path $profilesDir ($to.layers[$k] + $(if ($k -eq 'script') { '.js' } else { '.yaml' }))
    if (-not (Test-Path $src)) { Die "源层文件不存在: $src" }
    if (-not (Test-Path $dst)) { Die "目标层文件不存在: $dst" }
    if (-not (Test-Template $dst) -and -not $Force) { Die "目标层已有自定义内容，拒绝覆盖: $dst（确认要覆盖请加 -Force）" }
    $plan += [pscustomobject]@{ Kind = $k; Src = $src; Dst = $dst }
}

Step '复制计划'
foreach ($p in $plan) {
    $size = (Get-Item $p.Src).Length
    Say ("{0,-8} {1,6} B  {2} -> {3}" -f $p.Kind, $size, (Split-Path $p.Src -Leaf), (Split-Path $p.Dst -Leaf))
    Say ("         " + (Get-LayerSummary $p.Src $p.Kind)) 'DarkGray'
}

if ($DryRun) { Write-Host ''; Say 'dry-run：没有写任何文件' 'Yellow'; exit 0 }

Step '写入'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
foreach ($p in $plan) {
    Copy-Item $p.Dst ($p.Dst + '.bak-' + $stamp) -Force
    [IO.File]::WriteAllBytes($p.Dst, [IO.File]::ReadAllBytes($p.Src))
    Say ("{0,-8} 已写入（原文件备份为 {1}.bak-{2}）" -f $p.Kind, (Split-Path $p.Dst -Leaf), $stamp) 'Green'
}

Step '核对'
$bad = 0
foreach ($p in $plan) {
    $a = (Get-FileHash $p.Src -Algorithm SHA256).Hash
    $b = (Get-FileHash $p.Dst -Algorithm SHA256).Hash
    if ($a -eq $b) { Say ("{0,-8} 哈希一致" -f $p.Kind) 'Green' } else { Say ("{0,-8} 哈希不一致!" -f $p.Kind) 'Red'; $bad++ }
}
if ($bad -gt 0) { Die '有文件没写对，先别切订阅' }

Write-Host ''
Say '下一步：' 'White'
Say "1) 回到 Verge → 订阅页，点一下新配置卡片（${ToUid}）让它成为当前配置"
Say '2) 右键任意配置 → 重启内核，或退出并重开 Verge'
Say '3) 跑体检：.\check-egress.ps1 -ProxyPort 7897'
Say '4) 体检全绿、ChatGPT 首页 200 之后，再删旧订阅'
