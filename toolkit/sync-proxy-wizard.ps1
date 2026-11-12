#Requires -Version 5.1
<#
.SYNOPSIS
    sync-proxy.ps1 的交互式向导。
    依次询问：同步到哪个客户端 / FlClash 的节点来源 / 是否现在填写静态 ISP 账号。
    最后调用 sync-proxy.ps1 执行；执行前会打印摘要并要求确认。
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$sync = Join-Path $here 'sync-proxy.ps1'

$flRoot = Join-Path $env:APPDATA 'com.follow\clash'
$vrRoot = Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'

function Line($c = '-') { Write-Host ($c * 64) -ForegroundColor DarkGray }
function Note($t) { Write-Host "    $t" -ForegroundColor DarkGray }

# 按实际提问顺序动态编号：跳过的问题不占号
$script:qn = 0
function NextQ([string]$t) { $script:qn++; return ("{0}. {1}" -f $script:qn, $t) }

function Ask-Choice {
    param([string]$Title, [string[]]$Options, [int]$Default = 1, [string]$Hint)
    while ($true) {
        Write-Host ''
        Write-Host $Title -ForegroundColor White
        for ($i = 0; $i -lt $Options.Count; $i++) {
            Write-Host ("  {0}) {1}" -f ($i + 1), $Options[$i])
        }
        if ($Hint) { Note $Hint }
        Write-Host ("  选择 [{0}]: " -f $Default) -NoNewline
        $raw = Read-Host
        if ([string]::IsNullOrWhiteSpace($raw)) { return $Default }
        [int]$n = 0
        if ([int]::TryParse($raw.Trim(), [ref]$n) -and $n -ge 1 -and $n -le $Options.Count) { return $n }
        Write-Host '  请输入列表里的编号' -ForegroundColor Yellow
    }
}

function Ask-Text {
    param([string]$Prompt, [string]$Default = '')
    if ($Default) { Write-Host ("  {0} [{1}]: " -f $Prompt, $Default) -NoNewline }
    else { Write-Host ("  {0}: " -f $Prompt) -NoNewline }
    $raw = Read-Host
    if ([string]::IsNullOrWhiteSpace($raw)) { return $Default }
    return $raw.Trim()
}

function Ask-Secret {
    param([string]$Prompt)
    if ([Console]::IsInputRedirected) {
        Write-Host ''
        Write-Host '  密码输入需要真实控制台。请双击 sync-proxy.cmd 运行本向导。' -ForegroundColor Red
        exit 1
    }
    Write-Host ("  {0}: " -f $Prompt) -NoNewline
    $sec = Read-Host -AsSecureString
    $b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec)
    try { return [Runtime.InteropServices.Marshal]::PtrToStringAuto($b) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }
}

function Abort-Input {
    Write-Host ''
    Write-Host '  连续多次没有有效输入，已停止（没有改动任何文件）。' -ForegroundColor Red
    Write-Host '  如果这是在计划任务/重定向环境里跑的，请双击 sync-proxy.cmd。' -ForegroundColor DarkGray
    exit 1
}

if (-not (Test-Path $sync)) {
    Write-Host "找不到 sync-proxy.ps1，请把它和本向导放在同一个目录：$here" -ForegroundColor Red
    exit 1
}

Clear-Host
Line '='
Write-Host ' 代理规则同步向导' -ForegroundColor Cyan
Write-Host ' 把「静态 ISP 出口 + 本机规则」写进 FlClash / Clash Verge' -ForegroundColor DarkGray
Write-Host ' 只改客户端自己的配置文件：不写注册表代理，不改环境变量，不动 TUN' -ForegroundColor DarkGray
Line '='

# ---------- 1. 客户端 ----------
$c = Ask-Choice (NextQ '要同步到哪个客户端？') @(
    'FlClash',
    'Clash Verge',
    '两个都同步',
    '只看现状（不改任何文件）'
) 1

if ($c -eq 4) {
    & $sync -CheckOnly
    exit 0
}

$target = @('FlClash', 'Verge', 'Both')[$c - 1]
$targetText = @('FlClash', 'Clash Verge', 'FlClash + Clash Verge')[$c - 1]

$missing = @()
if (($target -eq 'FlClash' -or $target -eq 'Both') -and -not (Test-Path $flRoot)) { $missing += 'FlClash' }
if (($target -eq 'Verge' -or $target -eq 'Both') -and -not (Test-Path $vrRoot)) { $missing += 'Clash Verge' }
if ($missing.Count -gt 0) {
    Write-Host ''
    Write-Host ("  没找到 {0} 的数据目录，可能没装或从没启动过。继续会失败。" -f ($missing -join ' / ')) -ForegroundColor Yellow
    if ((Ask-Choice '仍然继续？' @('继续', '退出') 2) -eq 2) { exit 0 }
}

# ---------- 2. FlClash 节点来源 ----------
$providerUrl = $null
$baseConfig = $null
$srcText = '不涉及'

if ($target -ne 'Verge') {
    $s = Ask-Choice (NextQ 'FlClash 的节点（VPN）从哪来？') @(
        '添加 VPN 订阅链接（推荐：机场更新不会覆盖本文件）',
        '不用订阅，拿我已有的 yaml 文件作底',
        '都不加，只更新当前已激活的配置（前提：里面已经有静态节点）'
    ) 1

    if ($s -eq 1) {
        Note '订阅链接会明文写进本地配置文件，这是 Clash 的常规做法'
        $tries = 0
        while ($true) {
            $providerUrl = Ask-Text '粘贴订阅链接'
            if ($providerUrl -match '^https?://') { break }
            $tries++
            if ($tries -ge 5) { Abort-Input }
            Write-Host '  链接要以 http:// 或 https:// 开头' -ForegroundColor Yellow
        }
        $srcText = '订阅链接（proxy-provider）'
    }
    elseif ($s -eq 2) {
        $tries = 0
        while ($true) {
            $baseConfig = Ask-Text '配置文件完整路径（yml/yaml）'
            if ($baseConfig -and (Test-Path $baseConfig)) { break }
            $tries++
            if ($tries -ge 5) { Abort-Input }
            Write-Host '  路径不存在，请重新输入' -ForegroundColor Yellow
        }
        $srcText = "沿用 " + (Split-Path $baseConfig -Leaf)
    }
    else {
        $srcText = '更新当前已激活的配置'
    }
}

# ---------- 3. 静态 ISP 账号 ----------
$fill = Ask-Choice (NextQ '现在填写静态 ISP 出口的地址和账号吗？') @(
    '填写（首次使用，或要换账号时必须）',
    '不填，沿用已有配置里的凭证'
) 1

$server = $null
$port = 0
$sUser = $null
$sPass = $null

if ($fill -eq 1) {
    $tries = 0
    while ($true) {
        $server = Ask-Text '静态 ISP 服务器地址（IP 或域名，必填）'
        if ($server -and $server -match '^[A-Za-z0-9][A-Za-z0-9._-]*$') { break }
        $tries++
        if ($tries -ge 5) { Abort-Input }
        Write-Host '  请填写你自己的服务器地址，脚本不预置' -ForegroundColor Yellow
    }
    $tries = 0
    while ($true) {
        $p = Ask-Text '端口'
        [int]$portNum = 0
        if ([int]::TryParse($p, [ref]$portNum) -and $portNum -gt 0 -and $portNum -lt 65536) { $port = $portNum; break }
        $tries++
        if ($tries -ge 5) { Abort-Input }
        Write-Host '  端口要是 1-65535 的数字' -ForegroundColor Yellow
    }
    $tries = 0
    while (-not $sUser) {
        $sUser = Ask-Text '账号'
        if (-not $sUser) { $tries++; if ($tries -ge 3) { Abort-Input } }
    }
    $tries = 0
    while (-not $sPass) {
        $sPass = Ask-Secret '密码（输入不回显）'
        if (-not $sPass) { $tries++; if ($tries -ge 3) { Abort-Input } }
    }
}

# ---------- 4. 中继组 ----------
if ($providerUrl) {
    Note '这个组会成为新建的策略组，节点由上面的订阅提供'
    $relay = Ask-Text (NextQ '中继组名（静态出口经它出去）') '机场出口'
}
else {
    if (-not $script:relayNames) {
        $script:relayNames = @(& $sync -ListGroups 2>$null | ForEach-Object {
                if ($_ -match '可选的中继组:\s*(.+)$') { $Matches[1].Split('|') | ForEach-Object { $_.Trim() } }
            })
    }
    if ($script:relayNames.Count -gt 0) {
        Note ('订阅里现有的组: ' + ($script:relayNames -join ' | '))
        Note '中继组必须从上面这些名字里挑一个（写错名字 Verge 会加载失败）'
    }
    $relay = Ask-Text (NextQ '中继组名（静态出口经它出去）')
    if (-not $relay) { Note '留空只在配置里已经有静态节点时可用；首次写入必须填' }
    elseif ($script:relayNames.Count -gt 0 -and $script:relayNames -notcontains $relay) {
        Write-Host '  [!] 这个名字不在订阅的组名里，继续执行会失败；请对照上面的列表重填' -ForegroundColor Yellow
    }
}

# ---------- 摘要 ----------
Write-Host ''
Line '='
Write-Host ' 即将执行' -ForegroundColor Cyan
Write-Host ("   客户端    : {0}" -f $targetText)
Write-Host ("   节点来源  : {0}" -f $srcText)
if ($relay) { Write-Host ("   中继组    : {0}" -f $relay) }
else { Write-Host '   中继组    : 未填写（仅在已有静态节点可沿用其原中继）' }
if ($sUser) { Write-Host ("   静态 ISP  : {0}:{1}（账号已填写）" -f $server, $port) }
else { Write-Host '   静态 ISP  : 沿用现有配置里的凭证' }
Write-Host '   影响范围  : 只写 FlClash / Clash Verge 的配置文件'
Line '='

$mode = Ask-Choice '开始执行？' @(
    '直接执行（写之前会自动备份原配置）',
    '只试运行，不写任何文件',
    '取消'
) 1
if ($mode -eq 3) { Write-Host ''; Write-Host ' 已取消，什么都没改' -ForegroundColor DarkGray; exit 0 }

$params = @{ Target = $target }
if ($providerUrl) { $params.ProviderUrl = $providerUrl }
if ($baseConfig) { $params.BaseConfig = $baseConfig }
if ($relay) { $params.RelayGroup = $relay }
if ($sUser) {
    $params.StaticServer = $server
    $params.StaticPort = $port
    $params.StaticUser = $sUser
    $params.StaticPass = $sPass
}
if ($mode -eq 2) { $params.DryRun = $true }

Write-Host ''
$global:LASTEXITCODE = 0
& $sync @params

Write-Host ''
if ($LASTEXITCODE -and $LASTEXITCODE -ne 0) {
    Write-Host ' 没有成功，按上面的提示处理后重跑本向导即可（原配置已备份，未改动）' -ForegroundColor Red
}
elseif ($mode -eq 2) {
    Write-Host ' 试运行结束，没有写任何文件。确认无误后重跑并选择「直接执行」。' -ForegroundColor Yellow
}
else {
    Write-Host ' 完成。FlClash 点一下当前配置卡片（或重启）才会生效；Verge 需重启后生效。' -ForegroundColor Green
    if ($target -ne 'FlClash') {
        Note '若上面提示"没有绑定脚本增强层"，按提示把生成的文件内容贴进 订阅页右键 → 扩展脚本（不是"全局扩展脚本"）'
    }
}
