#Requires -Version 5.1
<#
.SYNOPSIS
    用 backup-verge.ps1 生成的快照回滚 Clash Verge 的配置。

.DESCRIPTION
    回滚前要求 Verge 已退出（否则它退出时会把内存里的 profiles.yaml 再写回去）。
    回滚时先把"当前"文件另存为 .pre-restore-时间戳，再覆盖。
    不碰注册表、不改环境变量；回滚完成后需要你手动启动 Verge 并跑 check-egress.ps1。

.EXAMPLE
    .\restore-verge-snapshot.ps1 -Snapshot 'D:\HopWay\backups\verge-20261106-193000-before'
    .\restore-verge-snapshot.ps1 -Snapshot '...' -DryRun
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Snapshot,
    [string]$VergeRoot = (Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'),
    [switch]$KillVerge,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "    $m" -ForegroundColor Green }
function Warn($m) { Write-Host "    [!] $m" -ForegroundColor Yellow }
function Die($m) { Write-Host "    [x] $m" -ForegroundColor Red; exit 1 }

if ($Snapshot -like '*.zip') { Die '请解压后传目录，不要直接传 zip' }
$src = Join-Path $Snapshot 'verge'
if (-not (Test-Path $src)) { Die "快照里没有 verge\ 目录: $src" }
if (-not (Test-Path $VergeRoot)) { Die "找不到 Verge 目录: $VergeRoot" }

# Verge 必须在退出状态（dry-run 只读，不检查）
if (-not $DryRun) {
    $running = Get-Process -Name 'clash-verge', 'verge-mihomo' -ErrorAction SilentlyContinue
    if ($running) {
        if ($KillVerge) {
            Warn ('结束进程: ' + (($running | Select-Object -ExpandProperty ProcessName -Unique) -join ', '))
            $running | Stop-Process -Force
            Start-Sleep -Seconds 2
        }
        else {
            Die 'Verge 还在运行。先退出 Verge，或加 -KillVerge（会强杀 clash-verge / verge-mihomo）'
        }
    }
}

$files = Get-ChildItem $src -Recurse -File
Step ("准备回滚 {0} 个文件 -> {1}" -f $files.Count, $VergeRoot)

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
foreach ($f in $files) {
    $rel = $f.FullName.Substring($src.Length + 1)
    $dst = Join-Path $VergeRoot $rel
    $dstDir = Split-Path $dst -Parent
    if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir -Force | Out-Null }
    if ($DryRun) { Ok ("dry-run: {0}" -f $rel); continue }
    if (Test-Path $dst) { Copy-Item $dst ($dst + '.pre-restore-' + $stamp) -Force }
    Copy-Item $f.FullName $dst -Force
    Ok $rel
}

if ($DryRun) { Write-Host ''; Warn 'dry-run：没有写任何文件'; exit 0 }

Write-Host ''
Ok '回滚写入完成'
Write-Host ''
Write-Host '下一步：' -ForegroundColor White
Write-Host '  1) 启动 Clash Verge，在订阅页点一下当前配置卡片，再右键 → 重启内核'
Write-Host '  2) 核对代理页：静态ISP 组成员是 LS-ISP-01 + DIRECT，且选中的是 LS-ISP-01'
Write-Host '  3) 跑体检: .\check-egress.ps1 -ProxyPort 7897'
