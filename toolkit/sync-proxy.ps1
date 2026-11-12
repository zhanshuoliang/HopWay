<#
.SYNOPSIS
    把「静态 ISP 出口 + 本机规则」同步到 Clash Verge 与 FlClash。
    只写两个客户端各自的配置文件，不写 Windows 注册表代理、不改环境变量。
    不预置任何机场或服务器：中继策略组、节点来源、静态 ISP 地址全部由参数指定。

.DESCRIPTION
    Verge 侧：只补/更新当前订阅已绑定的 规则层 / 代理层 / 策略组层，不动 verge.yaml、不动系统代理。
    FlClash 侧：三种模式
      1) -ProviderUrl <订阅链接>  生成独立配置，节点走 proxy-provider，机场更新不覆盖本文件（推荐）
      2) -BaseConfig <yaml>       用你自己的配置文件作底，注入静态出口与规则
      3) 都不给                     仅当当前 profile 已含静态节点时原地更新（不会碰机场订阅）

.EXAMPLE
    .\sync-proxy.ps1 -CheckOnly
    # 用自己的机场订阅作为节点来源，生成独立 FlClash 配置
    .\sync-proxy.ps1 -Target FlClash -ProviderUrl 'https://example.com/sub?token=xxx' -RelayGroup '我的机场' `
        -StaticServer 你的静态IP服务器 -StaticPort 443 -StaticUser xxx -StaticPass yyy
    # 用自己已有的 yaml 作底
    .\sync-proxy.ps1 -Target FlClash -BaseConfig D:\my-config.yaml -RelayGroup '我的机场'
    # 首次写 Verge 代理层需要服务器、凭证与中继组
    .\sync-proxy.ps1 -Target Verge -RelayGroup '我的机场' -StaticServer 你的静态IP服务器 -StaticUser xxx -StaticPass yyy
#>
[CmdletBinding()]
param(
    [ValidateSet('Both', 'Verge', 'FlClash')]
    [string]$Target = 'Both',

    [string]$VergeRoot = (Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev'),
    [string]$FlClashRoot = (Join-Path $env:APPDATA 'com.follow\clash'),
    [string]$CoreExe,

    # 静态 ISP 出口（服务器与凭证由用户提供，脚本不预置任何地址）
    [string]$StaticServer,
    [int]$StaticPort = 443,
    [string]$StaticUser,
    [string]$StaticPass,
    [string]$StaticGroup = '静态ISP',
    [string]$ProxyName = 'LS-ISP-01',

    # 中继：你自己的机场/代理组名。留空则从现有配置中读取，读不到就报错。
    [string]$RelayGroup,

    # FlClash 节点来源（二选一）
    [string]$ProviderUrl,
    [string]$ProviderName = 'airport',
    [Alias('SourceConfig')]
    [string]$BaseConfig,

    # 是否把最后的 MATCH 指向静态出口（默认是：保证出口 IP 固定）
    [bool]$MatchToStatic = $true,

    [string]$FlClashPort = '7890',

    # 旧机场专用规则（域名 + 3 个服务器 IP），默认不写
    [switch]$LegacyRelayRules,

    [switch]$DryRun,
    [switch]$ListGroups,
    [switch]$CheckOnly,
    [switch]$LibraryOnly
)

$ErrorActionPreference = 'Stop'

function Say-Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Say-Ok($m) { Write-Host "    $m" -ForegroundColor Green }
function Say-Warn($m) { Write-Host "    [!] $m" -ForegroundColor Yellow }

function Get-BomBytes($path) {
    $b = [IO.File]::ReadAllBytes($path)
    if ($b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) { return $true }
    return $false
}

function Save-Text($path, $text, [bool]$bom) {
    if ($DryRun) { Say-Ok "dry-run: 不写入 $path"; return }
    $enc = New-Object System.Text.UTF8Encoding($bom)
    [IO.File]::WriteAllText($path, $text, $enc)
}

function Read-Lines($path) {
    $list = New-Object System.Collections.Generic.List[string]
    foreach ($l in ([IO.File]::ReadAllText($path) -split "`r?`n")) { $list.Add($l) }
    return ,$list
}

# 写之前留一份原件，出问题能立刻回滚
function Backup-File($path) {
    if ($DryRun -or -not (Test-Path $path)) { return $null }
    $bak = $path + '.bak-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
    Copy-Item $path $bak -Force
    return $bak
}

# 收集"可以作为中继的组名"：订阅本体 + 策略组层
# 返回字符串数组（逐条输出，方便调用侧用管道过滤）
function Get-RelayCandidates($Layers) {
    $names = New-Object System.Collections.Generic.List[string]
    foreach ($k in 'self', 'groups') {
        $p = $Layers[$k]
        if ($p -and (Test-Path $p)) {
            foreach ($n in (Get-GroupNames (Read-Lines $p))) { if (-not $names.Contains($n)) { $names.Add($n) } }
        }
    }
    return $names.ToArray()
}

function Get-CanonicalRules {
    $r = New-Object System.Collections.Generic.List[string]
    $r.Add('DOMAIN-SUFFIX,example.com,DIRECT')
    $r.Add('PROCESS-NAME,example-app.exe,DIRECT')
    $r.Add('PROCESS-NAME,example-tool.exe,DIRECT')
    $r.Add('IP-CIDR,172.16.0.0/12,DIRECT,no-resolve')
    if ($LegacyRelayRules) {
        $r.Add('DOMAIN-SUFFIX,sub-a.example.com,DIRECT')
        $r.Add('IP-CIDR,203.0.113.31/32,DIRECT,no-resolve')
        $r.Add('IP-CIDR,203.0.113.32/32,DIRECT,no-resolve')
        $r.Add('IP-CIDR,203.0.113.33/32,DIRECT,no-resolve')
    }
    $r.Add("DOMAIN-SUFFIX,ipify.org,$StaticGroup")
    $r.Add("DOMAIN-SUFFIX,speed.cloudflare.com,$StaticGroup")
    $r.Add("DOMAIN-SUFFIX,ping0.cc,$StaticGroup")
    $r.Add("DOMAIN-SUFFIX,todetect.cn,$StaticGroup")
    $r.Add("DOMAIN-SUFFIX,ip.cn,$StaticGroup")
    $r.Add("DOMAIN-SUFFIX,ip138.com,$StaticGroup")
    return $r
}

# Verge「脚本」增强层：把订阅里的分流目标统一改到静态出口，并补 MATCH
# 少了这一层，静态 ISP 只接管 ipify 之类的探测域名，ChatGPT 等仍从机场 IP 出去
function Get-ScriptLayerText {
    $js = @'
// managed-by: sync-proxy
// 作用：订阅里的分流目标统一改到 __STATIC__，末尾补 MATCH，__STATIC__ 才是真正的默认出口
function main(config, profileName) {
  if (!Array.isArray(config.rules)) return config;
  var keep = ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "__STATIC__"];

  function rewrite(rule) {
    var t = rule.trim();
    if (!t) return null;
    if (/^MATCH,/i.test(t)) return null;
    if (/^PROCESS-/i.test(t)) return t;
    var parts = t.split(",");
    var i = parts.length - 1;
    if ((parts[i] || "").trim().toLowerCase() === "no-resolve") i -= 1;
    var target = (parts[i] || "").trim();
    if (!target || keep.indexOf(target) >= 0) return t;
    parts[i] = "__STATIC__";
    return parts.join(",");
  }

  config.rules = config.rules
    .filter(function (r) { return typeof r === "string"; })
    .map(rewrite)
    .filter(function (r) { return r !== null; });
  config.rules.push("MATCH,__STATIC__");

  // 静态节点只能待在 __STATIC__ 组里；出现在别的组（尤其 url-test）会和中继组成自环
  (config["proxy-groups"] || []).forEach(function (g) {
    if (!g || g.name === "__STATIC__") return;
    if (Array.isArray(g.proxies)) {
      g.proxies = g.proxies.filter(function (n) { return n !== "__NODE__" && n !== "__STATIC__"; });
    }
    if (g["include-all"] || g["include-all-proxies"]) {
      g["exclude-filter"] = "(?i)(__NODE__|__STATIC__)";
    }
  });

  return config;
}
'@
    return $js.Replace('__STATIC__', $StaticGroup).Replace('__NODE__', $ProxyName)
}

# 整块插入：只看块首行（- name: "X"）是否已存在。
# 不能用 Insert-AfterAnchor 逐行去重——文件里别的节点已有 "    type: socks5"，
# 会把新块里的 type 当成重复行丢掉，写出来的代理缺 type，mihomo 直接判配置非法。
function Insert-BlockAfterAnchor {
    param([System.Collections.Generic.List[string]]$Lines, [string]$AnchorPattern, [string[]]$Block)
    if (-not $Block -or $Block.Count -eq 0) { return $false }
    $head = $Block[0].Trim()
    foreach ($l in $Lines) { if ($l.Trim() -eq $head) { return $true } }
    $idx = -1
    for ($i = 0; $i -lt $Lines.Count; $i++) { if ($Lines[$i] -match $AnchorPattern) { $idx = $i; break } }
    if ($idx -lt 0) { return $false }
    $Lines.InsertRange($idx + 1, [string[]]$Block)
    return $true
}

function Insert-AfterAnchor {
    param([System.Collections.Generic.List[string]]$Lines, [string]$AnchorPattern, [string[]]$Items, [string]$Prefix)
    $idx = -1
    for ($i = 0; $i -lt $Lines.Count; $i++) { if ($Lines[$i] -match $AnchorPattern) { $idx = $i; break } }
    if ($idx -lt 0) { return $false }
    $toAdd = New-Object System.Collections.Generic.List[string]
    foreach ($it in $Items) {
        $needle = $it
        $found = $false
        foreach ($l in $Lines) { if ($l.Trim() -eq ($Prefix + $needle).Trim() -or $l.Trim() -eq $needle) { $found = $true; break } }
        if (-not $found) { $toAdd.Add($Prefix + $needle) }
    }
    if ($toAdd.Count -eq 0) { return $true }
    $Lines.InsertRange($idx + 1, $toAdd)
    return $true
}

# 解析组名。订阅配置里组在 proxy-groups: 下；Verge 增强层里组在 prepend:/append: 下。
function Get-GroupNames([System.Collections.Generic.List[string]]$Lines) {
    $names = New-Object System.Collections.Generic.List[string]
    $section = ''
    foreach ($l in $Lines) {
        if ($l -match '^([A-Za-z0-9_.-]+):\s*$') {
            $k = $Matches[1]
            if ($k -eq 'proxy-groups' -or $k -eq 'prepend' -or $k -eq 'append') { $section = $k } else { $section = '' }
            continue
        }
        if ($l -match '^[A-Za-z0-9_.-]+\s*:') { $section = ''; continue }
        if (-not $section) { continue }
        $n = $null
        if ($l -match "^\s*-\s+name:\s*['`"]?([^'`"]+?)['`"]?\s*$") { $n = $Matches[1] }
        elseif ($l -match "^\s*-\s*\{\s*name:\s*['`"]?([^,'`"]+)") { $n = $Matches[1].Trim() }
        if ($n -and -not $names.Contains($n)) { $names.Add($n) }
    }
    return ,$names
}

# 同名顶层键出现多次时，YAML 只保留最后一个（正是它把刚写进去的组洗掉了）。
# 这里只保留内容最多的那块，其余整块删掉。
function Repair-DuplicatePrepend([System.Collections.Generic.List[string]]$Lines) {
    $starts = New-Object System.Collections.Generic.List[int]
    for ($i = 0; $i -lt $Lines.Count; $i++) { if ($Lines[$i] -match '^prepend:') { $starts.Add($i) } }
    if ($starts.Count -le 1) { return ,$Lines }

    $endAt = @{}
    foreach ($s in $starts) {
        $j = $s + 1
        while ($j -lt $Lines.Count -and $Lines[$j] -notmatch '^\S') { $j++ }
        $endAt[$s] = $j
    }
    $best = $starts[0]; $bestScore = -1
    foreach ($s in $starts) {
        $score = 0
        for ($k = $s + 1; $k -lt $endAt[$s]; $k++) { if ($Lines[$k].Trim()) { $score++ } }
        if ($score -gt $bestScore) { $bestScore = $score; $best = $s }
    }

    $out = New-Object System.Collections.Generic.List[string]
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        $skip = $false
        foreach ($s in $starts) {
            if ($s -ne $best -and $i -ge $s -and $i -lt $endAt[$s]) { $skip = $true; break }
        }
        if (-not $skip) { $out.Add($Lines[$i]) }
    }
    return ,$out
}

# 增强层文件若没有可用的 prepend 块（没有、或只有 prepend: []），补成块形式。
function Ensure-PrependAnchor([System.Collections.Generic.List[string]]$Lines) {
    $Lines = Repair-DuplicatePrepend $Lines
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match '^prepend:') {
            if ($Lines[$i] -match '^prepend:\s*\[\s*\]\s*$') { $Lines[$i] = 'prepend:' }
            return ,$Lines
        }
    }
    $head = New-Object System.Collections.Generic.List[string]
    $head.Add('# managed-by: sync-proxy')
    $head.Add('prepend:')
    foreach ($l in $Lines) { $head.Add($l) }
    if (-not ($head -match '^append:')) { $head.Add('append: []') }
    if (-not ($head -match '^delete:')) { $head.Add('delete: []') }
    return ,$head
}

function Get-DialerProxy([System.Collections.Generic.List[string]]$Lines, [string]$Name) {
    $inBlock = $false
    $pat = '^\s*-\s+name:\s*["'']?' + [regex]::Escape($Name) + '["'']?\s*$'
    foreach ($l in $Lines) {
        if ($l -match $pat) { $inBlock = $true; continue }
        if ($inBlock -and $l -match '^\s*-\s+name:') { break }
        if ($inBlock -and $l -match '^\s+dialer-proxy:\s*["'']?([^"''\s]+)') { return $Matches[1] }
    }
    return $null
}

# 只改写指定节点的 dialer-proxy 行
function Set-DialerProxy([System.Collections.Generic.List[string]]$Lines, [string]$Name, [string]$New) {
    $inBlock = $false
    $pat = '^\s*-\s+name:\s*["'']?' + [regex]::Escape($Name) + '["'']?\s*$'
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        $l = $Lines[$i]
        if ($l -match $pat) { $inBlock = $true; continue }
        if ($inBlock -and $l -match '^\s*-\s+name:') { break }
        if ($inBlock -and $l -match '^(\s*dialer-proxy:\s*)') {
            $Lines[$i] = $Matches[1] + '"' + $New + '"'
            return $true
        }
    }
    return $false
}

# ---------------- 现状检查（只读） ----------------

function Show-State {
    Say-Step '只读检查'
    $reg = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
    $s = Get-ItemProperty $reg
    $listening = @{}
    foreach ($line in (netstat -ano | Select-String 'LISTENING')) {
        if ($line -match ':(\d+)\s') { $listening[[int]$Matches[1]] = $true }
    }
    $server = [string]$s.ProxyServer
    $port = 0
    if ($server -match ':(\d+)\s*$') { $port = [int]$Matches[1] }
    Say-Ok ("系统代理: Enable={0} Server={1}" -f $s.ProxyEnable, $server)
    if ($s.ProxyEnable -eq 1) {
        if ($port -gt 0 -and $listening.ContainsKey($port)) { Say-Ok "  指向的 $port 有监听，当前有效" }
        else { Say-Warn "  指向的端口没有监听 —— 这就是'整个系统连不上'的典型状态；脚本不会自动改注册表" }
    }
    foreach ($n in 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy') {
        $v = [Environment]::GetEnvironmentVariable($n, 'User')
        if ($v) { Say-Warn "用户环境变量 $n 长期生效，会绕过客户端开关（建议删除）" }
    }
    foreach ($p in 7890, 7897) {
        if ($listening.ContainsKey($p)) { Say-Ok "端口 $p 正在监听" } else { Say-Warn "端口 $p 无监听" }
    }
}

# ---------------- Clash Verge ----------------

function Get-VergeLayers {
    $profilesYaml = Join-Path $VergeRoot 'profiles.yaml'
    if (-not (Test-Path $profilesYaml)) { throw "找不到 $profilesYaml，请用 -VergeRoot 指定 Verge 数据目录" }
    $lines = [IO.File]::ReadAllLines($profilesYaml)

    $current = $null
    foreach ($l in $lines) { if ($l -match '^current:\s*(\S+)\s*$') { $current = $Matches[1]; break } }
    if (-not $current) { throw 'profiles.yaml 里读不到 current:' }

    # 只认"当前配置"那一块的 option: 块，避免抓到别的 profile 的增强层
    $startIdx = -1
    $endIdx = $lines.Count
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^-\s+uid:\s*(\S+)\s*$') {
            if ($startIdx -ge 0) { $endIdx = $i; break }
            if ($Matches[1] -eq $current) { $startIdx = $i }
        }
    }
    if ($startIdx -lt 0) { throw "profiles.yaml 里找不到当前配置 $current 的条目" }

    $uids = @{}
    $selfFile = $null
    for ($i = $startIdx; $i -lt $endIdx; $i++) {
        if ($lines[$i] -match '^\s{4}(merge|script|rules|proxies|groups):\s*(\S+)\s*$') {
            $uids[$Matches[1]] = $Matches[2]
        }
        if (-not $selfFile -and $lines[$i] -match '^\s{2}file:\s*(\S+)\s*$') { $selfFile = $Matches[1] }
    }
    if ($uids.Count -eq 0) { throw "当前配置 $current 没有绑定任何增强层（option: 段为空）" }

    $files = @{}
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^-\s+uid:\s*(\S+)\s*$') {
            $uid = $Matches[1]
            for ($j = $i + 1; $j -lt [Math]::Min($i + 8, $lines.Count); $j++) {
                if ($lines[$j] -match '^\s+file:\s*(\S+)\s*$') { $files[$uid] = $Matches[1]; break }
                if ($lines[$j] -match '^-\s+uid:') { break }
            }
        }
    }

    $result = @{}
    foreach ($k in $uids.Keys) {
        $uid = $uids[$k]
        if ($files.ContainsKey($uid)) { $result[$k] = Join-Path (Join-Path $VergeRoot 'profiles') $files[$uid] }
        else { $result[$k] = $null }
    }
    if ($selfFile) { $result['self'] = Join-Path (Join-Path $VergeRoot 'profiles') $selfFile }
    else { $result['self'] = $null }
    Say-Ok ("当前配置 $current 绑定: " + (($uids.Keys | Sort-Object | ForEach-Object { "$_=$($uids[$_])" }) -join ', '))
    return $result
}

function Sync-Verge {
    Say-Step 'Clash Verge: 同步增强层'
    $layers = Get-VergeLayers
    $canonical = Get-CanonicalRules

    # 0) 先验中继组名：写错一个名字，mihomo 会拒绝整份配置（代理页全空）
    # 静态ISP / LS-ISP-01 不能当自己的中继（会自环），从候选里去掉
    $relayNames = @(Get-RelayCandidates $layers | Where-Object { $_ -ne $StaticGroup -and $_ -ne $ProxyName })
    if ($RelayGroup -and $relayNames.Count -gt 0 -and -not $relayNames.Contains($RelayGroup)) {
        throw ("中继组 '$RelayGroup' 不在订阅里，写进去 Verge 会加载失败（代理页全空）。`n可用的组: " + (($relayNames | Select-Object -First 30) -join ' / '))
    }
    if ($RelayGroup -and $relayNames.Count -eq 0) {
        Say-Warn "订阅里读不到组名，无法校验中继组 '$RelayGroup'；如果 Verge 加载失败，先怀疑这个名字"
    }

    # 1) 规则层
    $rulesPath = $layers['rules']
    if (-not $rulesPath -or -not (Test-Path $rulesPath)) {
        Say-Warn '当前订阅没有绑定"规则"增强层，跳过；请在 Verge 订阅页右键 → 编辑规则'
    } else {
        $bom = Get-BomBytes $rulesPath
        $list = Read-Lines $rulesPath
        $list = Ensure-PrependAnchor $list
        $ok = Insert-AfterAnchor -Lines $list -AnchorPattern '^prepend:\s*$' -Items $canonical -Prefix '  - '
        if (-not $ok) {
            $fallback = New-Object System.Collections.Generic.List[string]
            foreach ($r in $canonical) { $fallback.Add('  - ' + $r) }
            $list.InsertRange(0, $fallback)
        }
        if ($DryRun) { Say-Ok "dry-run: 规则层将包含 $($canonical.Count) 条 canonical 规则 -> $rulesPath" }
        else { $null = Backup-File $rulesPath; Save-Text $rulesPath ($list -join "`n") $bom; Say-Ok "规则层已更新: $rulesPath" }
    }

    # 2) 策略组层
    $groupsPath = $layers['groups']
    if (-not $groupsPath -or -not (Test-Path $groupsPath)) {
        Say-Warn '当前订阅没有绑定"策略组"增强层，跳过'
    } else {
        $bom = Get-BomBytes $groupsPath
        $list = Read-Lines $groupsPath
        $orig = $list -join "`n"
        $dup = @($list -match '^prepend:').Count
        $list = Ensure-PrependAnchor $list
        if ($dup -gt 1) { Say-Warn "组层里有 $dup 个 prepend: 键，YAML 只认最后一个；已合并成一块" }

        $hasGroup = @($list -match ('^\s*-\s+name:\s*"?' + [regex]::Escape($StaticGroup) + '"?\s*$')).Count -gt 0
        if ($hasGroup) {
            Say-Ok "策略组 $StaticGroup 已存在"
        } else {
            # 注意：元素里带 + 拼接时必须加括号。逗号优先级高于 +，否则整个数组会被拼成一行
            $block = @(
                ('  - name: "' + $StaticGroup + '"'),
                '    type: select',
                '    proxies:',
                ('      - "' + $ProxyName + '"'),
                '      - DIRECT'
            )
            if (Insert-BlockAfterAnchor -Lines $list -AnchorPattern '^prepend:\s*$' -Block $block) {
                Say-Ok "策略组 $StaticGroup 已插入"
            } else { Say-Warn '策略组层缺少 prepend: 段，未改动' }
        }

        if (($list -join "`n") -ne $orig) {
            $null = Backup-File $groupsPath
            Save-Text $groupsPath ($list -join "`n") $bom
        }
    }

    # 3) 代理层
    $proxiesPath = $layers['proxies']
    if (-not $proxiesPath -or -not (Test-Path $proxiesPath)) {
        Say-Warn '当前订阅没有绑定"代理"增强层，跳过；静态 ISP 节点无法写入'
        return
    }

    $bom = Get-BomBytes $proxiesPath
    $list = Read-Lines $proxiesPath
    $orig = $list -join "`n"
    $dup = @($list -match '^prepend:').Count
    $list = Ensure-PrependAnchor $list
    if ($dup -gt 1) { Say-Warn "代理层里有 $dup 个 prepend: 键，已合并成一块" }
    $existing = Get-DialerProxy $list $ProxyName

    if ($existing) {
        Say-Ok "代理 $ProxyName 已存在，中继组: $existing"
        if ($RelayGroup -and $RelayGroup -ne $existing) {
            if (Set-DialerProxy $list $ProxyName $RelayGroup) {
                $null = Backup-File $proxiesPath
                Save-Text $proxiesPath ($list -join "`n") $bom
                Say-Ok "中继组已改写: $existing -> $RelayGroup"
            } else { Say-Warn "找不到 $ProxyName 的 dialer-proxy 行，未改写" }
        } elseif (($list -join "`n") -ne $orig) {
            $null = Backup-File $proxiesPath
            Save-Text $proxiesPath ($list -join "`n") $bom
            Say-Ok '代理层结构已修复（重复 prepend 已合并）'
        }
    } else {
        if (-not $StaticUser -or -not $StaticPass) {
            throw "代理层里没有 $ProxyName，必须提供静态 ISP 的账号和密码（向导里第 3 问选'填写'）"
        }
        if (-not $StaticServer) {
            throw "没有 $ProxyName，需要知道它连哪台服务器。请用 -StaticServer 指定（脚本不预置地址）"
        }
        if (-not $RelayGroup) {
            $hint = '当前订阅与策略组层里都没有可用的组名'
            if ($relayNames.Count -gt 0) { $hint = '可选的组: ' + (($relayNames | Select-Object -First 20) -join ' / ') }
            throw "没有 $ProxyName，需要知道它走哪个中继组。请在向导里填写中继组名（或用 -RelayGroup 指定）。$hint"
        }

        $block = @(
            ('  - name: "' + $ProxyName + '"'),
            '    type: socks5',
            "    server: $StaticServer",
            "    port: $StaticPort",
            ('    username: "' + $StaticUser + '"'),
            ('    password: "' + $StaticPass + '"'),
            ('    dialer-proxy: "' + $RelayGroup + '"'),
            '    udp: false'
        )
        # $list 已经过 Ensure-PrependAnchor（含重复键修复），这里直接插入
        if (Insert-BlockAfterAnchor -Lines $list -AnchorPattern '^prepend:\s*$' -Block $block) {
            $null = Backup-File $proxiesPath
            Save-Text $proxiesPath ($list -join "`n") $bom
            Say-Ok "已写入 $ProxyName（中继组 $RelayGroup）"
        } else { Say-Warn '代理层缺少 prepend: 段，未改动' }
    }

    # 4) 脚本层：决定"默认出口"。只有规则层时，静态出口只接管探测域名，普通网站仍从机场出去
    $scriptPath = $layers['script']
    if (-not $scriptPath) {
        Say-Warn '当前订阅没有绑定"脚本"增强层：静态出口只接管探测域名，ChatGPT 等网站仍从机场 IP 出去'
        $hintPath = Join-Path (Join-Path $VergeRoot 'profiles') 'sync-script.js'
        try {
            if (-not $DryRun) { [IO.File]::WriteAllText($hintPath, (Get-ScriptLayerText), (New-Object System.Text.UTF8Encoding($false))) }
            Say-Warn "已生成待粘贴的脚本: $hintPath"
        } catch { Say-Warn '生成待粘贴脚本失败，可直接用包里自带的 verge-script.js' }
        Say-Warn '请在 Verge 订阅页右键该配置 → 扩展脚本，把上面文件的内容整段贴进去（不要用"全局扩展脚本"），保存后重启 Verge'
    } elseif (-not (Test-Path $scriptPath)) {
        Say-Warn "脚本层文件不存在: $scriptPath"
    } else {
        $cur = [IO.File]::ReadAllText($scriptPath)
        # 只覆盖三种情况：本脚本写过的、空文件、Verge 的空白模板。用户自己的脚本一律不碰
        $isTemplate = ($cur -match 'managed-by: sync-proxy') -or ($cur.Trim().Length -eq 0) -or ($cur -match '//\s*Define main function')
        if ($cur -match ('MATCH,' + [regex]::Escape($StaticGroup))) {
            Say-Ok "脚本层已是全量直出（MATCH,$StaticGroup）"
        } elseif ($isTemplate) {
            $want = Get-ScriptLayerText
            if ($DryRun) { Say-Ok "dry-run: 将写入脚本层 -> $scriptPath" }
            else {
                $null = Backup-File $scriptPath
                [IO.File]::WriteAllText($scriptPath, $want, (New-Object System.Text.UTF8Encoding($false)))
                Say-Ok "脚本层已写入：默认出口 = $StaticGroup（普通网站不再走机场）"
            }
        } else {
            Say-Warn "脚本层里有自定义内容，未覆盖；需要全量直出请手动补 MATCH,$StaticGroup"
        }
    }
}

# ---------------- FlClash ----------------

function Get-FlClashProfilePath {
    $prefs = Join-Path $FlClashRoot 'shared_preferences.json'
    if (-not (Test-Path $prefs)) { throw "找不到 $prefs" }
    $raw = [IO.File]::ReadAllText($prefs)
    $id = $null
    if ($raw -match 'currentProfileId\\?"\s*:\s*(\d+)') { $id = $Matches[1] }
    if (-not $id) { throw 'shared_preferences.json 里读不到 currentProfileId（先在 FlClash 里选中一个配置）' }
    $p = Join-Path (Join-Path $FlClashRoot 'profiles') ($id + '.yaml')
    return @{ Id = $id; Path = $p }
}

function Invoke-CoreTest($configPath) {
    $candidates = @()
    if ($CoreExe) { $candidates += $CoreExe }
    $candidates += @(
        'C:\Program Files\Clash Verge\verge-mihomo.exe',
        'D:\Program Files\Clash Verge\verge-mihomo.exe',
        (Join-Path $env:LOCALAPPDATA 'Programs\Clash Verge\verge-mihomo.exe')
    )
    $core = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $core) { Say-Warn '没找到 mihomo 内核，跳过语法校验'; return $true }

    $tmp = Join-Path $env:TEMP ('sync-proxy-test-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null
    try {
        Copy-Item $configPath (Join-Path $tmp 'config.yaml') -Force
        foreach ($f in 'GEOIP.metadb', 'GEOSITE.dat', 'GEOIP.dat', 'ASN.mmdb', 'Country.mmdb', 'GeoSite.dat') {
            $src = Join-Path $FlClashRoot $f
            if (Test-Path $src) { Copy-Item $src (Join-Path $tmp $f) -Force }
        }
        $out = & $core -t -d $tmp 2>&1 | Out-String
        if ($out -match 'test is successful') { Say-Ok "配置校验通过 ($([IO.Path]::GetFileName($core)))"; return $true }
        Say-Warn ("配置校验失败: " + (($out.Trim() -split "`n" | Select-Object -Last 3) -join ' | '))
        return $false
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# 用订阅链接生成独立配置：节点走 proxy-provider，机场更新不会覆盖本文件
function New-ProviderProfile {
    if (-not $StaticServer) { throw '生成新配置需要 -StaticServer（脚本不预置任何服务器地址）' }
    if (-not $StaticUser -or -not $StaticPass) { throw '生成新配置需要 -StaticUser / -StaticPass' }
    if (-not $RelayGroup) { $RelayGroup = '机场出口' }

    $text = @"
# 由 sync-proxy.ps1 生成
# 节点来源: proxy-provider "$ProviderName"（订阅更新只刷新 providers 目录，不改本文件）
mode: rule
mixed-port: $FlClashPort
port: 0
allow-lan: false
log-level: info
ipv6: false
unified-delay: true
tcp-concurrent: true
profile:
  store-selected: true
tun:
  enable: false
  stack: gvisor
  auto-route: true
  strict-route: false
  auto-detect-interface: true
  dns-hijack:
  - any:53
dns:
  enable: true
  ipv6: false
  default-nameserver:
  - 223.5.5.5
  - 119.29.29.29
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  respect-rules: true
  proxy-server-nameserver:
  - 223.5.5.5
  - 119.29.29.29
  nameserver:
  - https://dns.alidns.com/dns-query
  - https://doh.pub/dns-query
proxy-providers:
  ${ProviderName}:
    type: http
    url: "$ProviderUrl"
    interval: 86400
    path: ./providers/$ProviderName.yaml
    health-check:
      enable: true
      url: https://www.gstatic.com/generate_204
      interval: 300
proxies:
- name: "$ProxyName"
  type: socks5
  server: $StaticServer
  port: $StaticPort
  username: "$StaticUser"
  password: "$StaticPass"
  udp: false
  dialer-proxy: "$RelayGroup"
proxy-groups:
- name: "$RelayGroup"
  type: select
  use:
  - $providerName
- name: "$StaticGroup"
  type: select
  proxies:
  - "$ProxyName"
  - DIRECT
rules:
"@

    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($l in ($text -split "`r?`n")) { $lines.Add($l) }
    foreach ($r in (Get-CanonicalRules)) { $lines.Add('- ' + $r) }
    $lines.Add("- MATCH,$StaticGroup")
    Say-Ok "基准: proxy-provider $ProviderName；中继组 $RelayGroup"
    return ,$lines
}

function Sync-FlClash {
    Say-Step 'FlClash: 写入静态出口配置'
    $profile = Get-FlClashProfilePath

    if ($ProviderUrl) {
        $lines = New-ProviderProfile
    } else {
        $basePath = $BaseConfig
        if (-not $basePath) { $basePath = $profile.Path }
        if (-not (Test-Path $basePath)) { throw "找不到基础配置 $basePath（用 -BaseConfig 指定）" }

        $lines = Read-Lines $basePath
        if (-not $BaseConfig -and -not ($lines -match [regex]::Escape($ProxyName))) {
            throw @"
当前 FlClash 配置是机场/订阅配置，直接改会在订阅更新时被覆盖。
请改用其一：
  -ProviderUrl <你的订阅链接>   (生成独立配置，推荐)
  -BaseConfig  <你自己的yaml>   (以它为底注入)
"@
        }
        Say-Ok "基准: $basePath"

        # 中继组：参数优先，否则沿用文件里已有的
        if (-not $RelayGroup) { $RelayGroup = Get-DialerProxy $lines $ProxyName }
        if (-not $RelayGroup) {
            $avail = (Get-GroupNames $lines) -join ', '
            throw "文件里没有 $ProxyName 的中继组，请用 -RelayGroup 指定。可选策略组: $avail"
        }
        Say-Ok "中继组: $RelayGroup"

        # 客户端自己的端口、监听、控制器和 TUN 设置必须原样保留。
        Say-Ok '保留现有端口、监听、控制器与 TUN 设置'

        # 静态 ISP 凭证
        if ($StaticUser -and $StaticPass) {
            $inBlock = $false
            $hit = 0
            for ($i = 0; $i -lt $lines.Count; $i++) {
                if ($lines[$i] -match ('^\s*-\s+name:\s*"?' + [regex]::Escape($ProxyName) + '"?\s*$')) { $inBlock = $true; continue }
                if ($inBlock -and $lines[$i] -match '^\s*-\s+name:') { $inBlock = $false }
                if ($inBlock -and $lines[$i] -match '^\s+username:') { $lines[$i] = '  username: "' + $StaticUser + '"'; $hit++ }
                if ($inBlock -and $lines[$i] -match '^\s+password:') { $lines[$i] = '  password: "' + $StaticPass + '"'; $hit++ }
            }
            if ($hit -ge 2) { Say-Ok "$ProxyName 凭证已更新" } else { Say-Warn "没找到 $ProxyName 的凭证行" }
        }

        # 策略组：缺则补
        if (-not ($lines -match ('^\s*-\s+name:\s*"?' + [regex]::Escape($StaticGroup) + '"?\s*$'))) {
            if (Insert-AfterAnchor -Lines $lines -AnchorPattern '^proxy-groups:\s*$' -Items @(
                    ('  - name: "' + $StaticGroup + '"'),
                    '    type: select',
                    '    proxies:',
                    ('      - "' + $ProxyName + '"'),
                    '      - DIRECT'
                ) -Prefix '') {
                Say-Ok "策略组 $StaticGroup 已插入"
            } else { Say-Warn '没有 proxy-groups: 段，未插入策略组' }
        }

        # 规则
        if (-not (Insert-AfterAnchor -Lines $lines -AnchorPattern '^rules:\s*$' -Items (Get-CanonicalRules) -Prefix '- ')) {
            throw '配置里找不到 rules: 段'
        }
        Say-Ok '规则已补齐（含 DOMAIN-SUFFIX,example.com,DIRECT）'

        # 末尾 MATCH
        if ($MatchToStatic) {
            $lastMatch = -1
            for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\s*-\s*MATCH,') { $lastMatch = $i } }
            $want = "- MATCH,$StaticGroup"
            if ($lastMatch -lt 0) {
                if (Insert-AfterAnchor -Lines $lines -AnchorPattern '^rules:\s*$' -Items @("MATCH,$StaticGroup") -Prefix '- ') {
                    Say-Ok "已补 $want"
                }
            } elseif ($lines[$lastMatch].Trim() -ne $want) {
                Say-Ok ("末尾规则: " + $lines[$lastMatch].Trim() + " -> $want")
                $lines[$lastMatch] = $want
            } else { Say-Ok "末尾规则已是 $want" }
        }
    }

    $stage = Join-Path $env:TEMP ('flclash-stage-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.yaml')
    $enc = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($stage, ($lines -join "`n"), $enc)

    if (-not (Invoke-CoreTest $stage)) {
        Remove-Item $stage -Force -ErrorAction SilentlyContinue
        throw '生成的配置没通过内核校验，已中止（现有配置未改动）'
    }

    if ($DryRun) {
        Say-Ok "dry-run: 将写入 $($profile.Path)（当前 profileId=$($profile.Id)）"
        Remove-Item $stage -Force -ErrorAction SilentlyContinue
        return
    }

    if (Test-Path $profile.Path) {
        $bak = $profile.Path + '.bak-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
        Copy-Item $profile.Path $bak -Force
        Say-Ok "已备份原配置: $(Split-Path $bak -Leaf)"
    }
    Copy-Item $stage $profile.Path -Force
    Remove-Item $stage -Force -ErrorAction SilentlyContinue
    Say-Ok "已写入 $($profile.Path)"
    Say-Warn '在 FlClash 里点一下当前配置卡片（或重启 FlClash）才会生效'
}

# ---------------- 入口 ----------------

if ($LibraryOnly) { return }

try {
    if ($CheckOnly) { Show-State; return }

    if ($ListGroups) {
        $layers = Get-VergeLayers
        $names = @(Get-RelayCandidates $layers | Where-Object { $_ -ne $StaticGroup -and $_ -ne $ProxyName })
        if ($names.Count -eq 0) { Say-Warn '当前订阅里读不到任何策略组（订阅可能过期或为空）' }
        else { Say-Ok ('可选的中继组: ' + ($names -join ' | ')) }
        return
    }

    Show-State
    if ($Target -eq 'Both' -or $Target -eq 'Verge') { Sync-Verge }
    if ($Target -eq 'Both' -or $Target -eq 'FlClash') { Sync-FlClash }

    Write-Host ''
    Say-Step '完成'
    Say-Ok '本脚本从不写 Windows 注册表代理，也不改环境变量；开关只由客户端自己控制'
    Say-Ok 'Verge 侧改动在重启 Verge 后生效；FlClash 侧在点配置卡片/重启后生效'
} catch {
    Write-Host ("失败: " + $_.Exception.Message) -ForegroundColor Red
    exit 1
}
