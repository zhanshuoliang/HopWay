# 交接文档：HopWay（跳径）—— 静态 ISP 多跳代理客户端

> 当前产品范围更新（2026-11-03）：用户决定桌面端只支持 Clash Verge，并要求暂停测试以保护现有配置。以下内容为历史交接，涉及 FlClash 的说明不代表当前桌面端仍支持它；未经用户再次授权，不要运行测试或操作其真实客户端配置。

> 项目代号 **HopWay**（中文名「跳径」），项目根目录 `D:\HopWay`。
> 给接手的新 session 看。这里只写**已经验证过的事实**和**明确的边界**；推测会单独标注。
> 最后一段是可直接粘贴的启动指令。

---

## 1. 一句话背景

用户买的是一条**静态 ISP 出口**（socks5，固定 IP），要求所有流量都经由它出去；它自己连不上中国网络，所以前面要挂一个机场节点做中继（`dialer-proxy` 两跳）。目前靠一套 PowerShell 脚本把配置写进 Clash Verge / FlClash，已经能跑通，但配置和排障都太原始。现在要把它升级成一个桌面客户端。

---

## 2. 当前情况（两台机器，同一套订阅）

### 2.1 网络结构

```
本机程序 → 127.0.0.1:7897(Verge) / 7890(FlClash)
        → 中继：机场节点（如 机场A 香港高速03）        ← AWS ap-east-1（香港），机房 IP，ChatGPT 会拦
        → 静态出口：LS-ISP-01 (socks5)                  ← 203.0.113.10，NTT/M-SQUARE 东京，非机房
        → 目标网站
```

- 静态出口固定 IP：`203.0.113.10`（实测 3/3 一致）。
- 机场节点实测归属：`203.0.113.21`、`203.0.113.22` 均为 AWS EC2 `ap-east-1`（香港），`hosting=true`。**Cloudflare 会把这类 IP 判成 VPN**，ChatGPT 返回 "Unable to load site / try turning it off"。
- 因此：**ChatGPT 能不能开，取决于流量有没有真的进静态出口，而不是链路通不通。**

### 2.2 配置落点（四层）

Verge 一个订阅可以绑多层增强文件，缺一层症状完全不同：

| 层 | 作用 | 缺了会怎样 |
|---|---|---|
| rules | 指名域名走哪（指定域名直连、探测域名走静态组） | 探测口径不对 |
| proxies | 写 `LS-ISP-01` 节点本身（server/port/账号/`dialer-proxy`） | 没有静态节点 |
| groups | 建 `静态ISP` 这个 select 组（成员 `LS-ISP-01` + `DIRECT`） | 选不到出口 |
| **script** | 把每条规则目标改写成 `静态ISP`，末尾补 `MATCH,静态ISP`，并把静态节点从其它组剔除 | **只有探测域名走静态出口，普通网站（含 ChatGPT）全走机场** |

**关键认知：默认出口由 script 层的 `MATCH` 决定，不是 rules 层。** 这是最后才被发现、也是最容易漏的一层。

### 2.3 两台机器的实际状态

| | 本机（开发机） | 另一台（用户测试机） |
|---|---|---|
| 客户端 | Verge(7897) + FlClash(7890) 都在 | 只装 Verge(7897) |
| 系统代理 | 指向 7890（FlClash） | 指向 7897（Verge） |
| 当前订阅 | 机场A（profile `P-new02`，2026-11-06 换的号，替换了旧的 `P-old01`） | 机场A（profile `P-b03`） |
| 增强层 uid | rules `r-A` / proxies `p-A` / groups `g-A` / script `s-A` / merge `m-A` | rules `r-B` / proxies `p-B` / groups `g-B` / script `s-B` |
| 状态 | 五层齐全（含 merge），`MATCH,静态ISP`，ChatGPT 正常 | 四层齐全（脚本层已手工粘贴），ChatGPT 正常 |

> 换订阅时用 `toolkit\copy-verge-layers.ps1 -FromUid <旧> -ToUid <新>`：新订阅会自动生成五层**空模板**，直接切过去等于丢了静态出口；这个工具把旧层整份复制过去（含 merge 的 `dns.ipv6:false`，漏了会放开 v6 泄漏），写前备份、写后校验哈希。
> 新旧两个订阅（sub-a.example.com / sub-b.example.com）归一化流量数字后 diff 为 0，节点名与组名完全一致，所以 `dialer-proxy: 机场A` 不用改。

机器 2 的脚本层是**手动**贴进去的（订阅页右键 → 扩展脚本），不是脚本自动绑定的。见第 6 节。

### 2.4 已验证的出口行为

- `check-egress.ps1 -ProxyPort 7897`（机器 2）：PASS / WARN / FAIL 全绿，出口 203.0.113.10，ChatGPT `cdn-cgi/trace` = 203.0.113.10，首页 200。
- 本机（`-ProxyPort 7890`，FlClash）：PASS 10 / WARN 0 / FAIL 0，同上。
- TUN 关闭，系统代理只由客户端开关控制，脚本不写注册表、不改环境变量。

---

## 3. 安全与行为约束（硬性，不可协商）

1. **绝不预置静态 ISP 服务器地址、账号、密码**。用户明确说过那是他私有的，不是公共值。首次必须手填。
2. **不写 Windows 注册表代理，不设用户级代理环境变量**。代理开关只由客户端自己的界面控制。
3. **TUN 始终交客户端开关控制**，脚本/客户端不得擅自开启。
4. **凭证不能出现在源码、日志、UI 明文里**。界面显示掩码（`***`）；配置文件只允许打印键名，不打印值。
5. **写配置文件前先备份**（`.bak-yyyyMMdd-HHmmss`），写前用 `verge-mihomo -t` 校验，不通过不落盘。
6. **代理审批逐条确认**，不自动批准。

---

## 4. 目标产品

一个常驻 Windows 的桌面程序，三块能力：

### 4.1 托盘 + 桌面小组件（看得见）

- 托盘图标 + 菜单：当前链路、出口 IP、一键开关代理、打开主窗。
- 桌面小组件：无边框、透明、可拖动的卡片，显示
  `本机 → 中继节点 → 静态出口 → 203.0.113.10`，
  每跳一个状态点（正常 / 超时 / 未知），出口 IP 可见时每隔 ~60s 刷新，卡片隐藏时不探测。
- **不接 Win11 官方小组件板**（要 MSIX + Widget Provider，成本高、曝光低）。用独立无边框窗口实现。

### 4.2 多跳链路的可视化编辑（配得明白）

- 节点卡片：机场组 / 机场节点 / socks5 静态出口。
- 连线表示"谁经谁出去"，支持拖动、增删、后接第三跳。
- 导出映射：
  - proxies 层：`A(dialer-proxy=B)` → `B(dialer-proxy=C)` → … 最后一跳挂中继组
  - groups 层：`静态ISP` select = 链头节点 + DIRECT
  - rules / script 层：沿用现有实现
- 必须拦截：成环、名字不存在、凭证缺失、节点重名。
- 写前 `mihomo -t`；写后自动跑体检并把结果回填卡片。

### 4.3 一键下发（有妥协）

- Verge：写三层 + 脚本层 → **必须重启客户端或点"重启内核"才生效**。
- FlClash：写入当前 profile → **必须点一下配置卡片或重启 App**。
- 不做"直接给运行中的内核推配置"：会和客户端自身状态脱钩，下次操作被覆盖，属于埋雷。
- 推荐按钮语义：`应用并重启客户端` + 自动体检回归。

---

## 5. 实测约束（决定架构，别绕）

| 约束 | 证据 | 影响 |
|---|---|---|
| Verge 没有 TCP 控制器 | `verge.yaml` 里 `enable_external_controller: null`；`127.0.0.1:9097` 连接被拒 | 想读实时连接/每跳延迟，需让用户先在 Verge 设置里打开外部控制器 |
| FlClash 没有本地 API | `%APPDATA%\com.follow\clash\config.yaml` 里 `external-controller: ""` | 只能读配置文件 + 主动探测出口 |
| 客户端不支持外部触发重载 | 两端都无 CLI/热重载 | "一键"只能做到"应用 + 重启进程" |
| 桌面卡片层级 | — | 用置顶无边框窗；不要碰 WorkerW 挂桌面（Win11 升级易碎） |

**推论：应用自己持有"链路模型"作为唯一真相源**。控制器只用于补延迟、流量等运行时数据，不作为链路的来源。

---

## 6. 扩展到其他机器（必须写进产品设计）

当前流程是手工的，机器 2 就是踩坑踩出来的。产品化时要按这个清单做：

1. 解压 `proxy-sync-toolkit.zip` 到任意目录（机器 2 用的是 `Desktop\proxy-sync-toolkit`，是副本，改脚本要重新覆盖）。
2. 双击 `sync-proxy.cmd` → 选客户端 → 填服务器/账号（密码不回显）→ 中继组从脚本列出的真实组名里选。
3. 脚本会：补齐缺失的 `prepend:`、合并重复的 `prepend:`、校验中继组是否存在、写 `LS-ISP-01`、建 `静态ISP` 组。
4. **如果提示"没有绑定脚本层"**：脚本会在 `profiles\sync-script.js` 生成一份待粘贴内容，需要用户在 订阅页右键该配置 → **扩展脚本**（不是"全局扩展脚本"）→ 整段粘贴 → 保存。
5. 重启 Verge（或点配置卡片让 FlClash 重新应用）。
6. 验证：
   ```
   .\check-egress.ps1 -ProxyPort 7897     # Verge；FlClash 用 7890
   ```
   验收标准：出口 3 次一致；`chatgpt.com/cdn-cgi/trace` 的 `ip=` 与出口一致；ChatGPT 首页 200。
7. 新机器如果又是"体检全绿但 ChatGPT 打不开"，先查 script 层，不要怀疑链路。

**产品化要求**：新机器的接入不能依赖用户记住这些步骤。客户端要能自动检测四层是否齐全、脚本层是否生效，并直接给出"点哪里、贴什么"的指引；最好能生成一键修复包。

---

## 7. 现有资产清单

项目根目录：`D:\HopWay`

```
HopWay\
├─ HANDOFF-静态ISP客户端.md   本文件
├─ README.md                  项目说明与快速开始
├─ toolkit\                   可直接运行的脚本（工作副本）
├─ dist\                      对外分发的 zip（解压即用的扁平包）
└─ archive\                   历史版本 / 旧方案，仅留档，不要再用
```

| 文件 | 说明 |
|---|---|
| `toolkit\sync-proxy.ps1` | 主引擎：写 Verge 四层 / 生成 FlClash 配置；含重复键修复、中继校验、整块插入、脚本层写入 |
| `toolkit\sync-proxy-wizard.ps1` | 交互向导（客户端选择 / FlClash 节点来源 / 静态 ISP 账号 / 中继组） |
| `toolkit\sync-proxy.cmd` | 双击入口（ASCII-only，避免中文乱码） |
| `toolkit\check-egress.ps1` | 出口体检：代理入口、出口 IP×3、直连对照、DNS、TUN、默认路由 MATCH、ChatGPT 首页、吞吐 |
| `toolkit\verge-script.js` | 可直接粘贴的脚本层内容（组名/节点名为默认值） |
| `toolkit\copy-verge-layers.ps1` | 换订阅/换账号：把一个 profile 的五层增强整份复制到另一个 profile（空模板才允许覆盖，写前备份、写后校验哈希） |
| `dist\proxy-sync-toolkit.zip` | 上面 5 个文件的发布包（**扁平结构**：解压后 5 个文件在同一层，双击 `sync-proxy.cmd` 即可） |
| `archive\old-setup.ps1` | 旧方案（旧脚本方案），已被 `sync-proxy.ps1` 取代 |
| `archive\old-flclash.yaml` | 旧方案的本机快照，脚本不再引用 |
| `archive\sync-proxy.ps1.bak-20261030-102918` | 2026-10-30 重构前的备份，仅供对比 |

> 改了 `toolkit\` 里的脚本后，记得重新打 `dist\proxy-sync-toolkit.zip`，否则发给别的机器还是旧版（机器 2 就因此报过"找不到参数 ListGroups"）。

配置目录：

- Verge：`%APPDATA%\io.github.clash-verge-rev.clash-verge-rev\`
  - `profiles.yaml`：订阅列表 + 每订阅 `option:` 绑定哪几层
  - `profiles\<uid>.yaml|.js`：各层文件
  - `clash-verge.yaml`：Verge 生成的**运行配置**（含 MATCH、组、节点，可用于只读核对）
- FlClash：`%APPDATA%\com.follow\clash\`
  - `profiles\<id>.yaml`、`shared_preferences.json`（`currentProfileId`）

**建议**：客户端不要重写写文件的逻辑，把 `sync-proxy.ps1` 当 sidecar 调用（输入 JSON、输出状态）；UI 只做模型、校验、下发、展示。

---

## 8. 已经踩过的坑（新 session 不要在同一个地方再摔）

| 坑 | 症状 | 现有处理 |
|---|---|---|
| 层文件没有 `prepend:` | 静默跳过，什么也没写 | `Ensure-PrependAnchor` 自动补骨架 |
| 两个 `prepend:` 键 | YAML 只认最后一个，刚写的组被 `[]` 覆盖；Verge 报 `proxy [静态ISP] not found`、代理页全空 | `Repair-DuplicatePrepend` 合并，`prepend: []` 就地展开 |
| PowerShell 数组里 `+` 与逗号优先级 | `@('a'+$x, 'b')` 拼成一行字符串 | 元素拼接必须加括号 |
| `dialer-proxy` 指向不存在的组 | 整份配置被拒，Verge 回落默认配置，所有组消失 | 写前用真实组名校验，不符拒写；`Set-DialerProxy` 原地改写 |
| 逐行去重插入节点块 | 别的节点已有 `type: socks5`，新块的 `type` 被当重复行丢掉 → `proxy 0: missing type` → 整份配置非法 | `Insert-BlockAfterAnchor`：只看块首行 `- name: "X"`，整块插或整块不插 |
| 只做 rules 层就以为"全量走静态" | 体检全绿，但 ChatGPT 走机场被 Cloudflare 拦 | 写 script 层；`check-egress` 对比 MATCH 目标与探测域名目标 |

---

## 9. 分期与验收

**P1（1–2 天）只读面板**
托盘 + 主窗 + 桌面卡片；显示当前链路（读配置模型）与出口 IP（经代理探测）。
验收：卡片上 IP 与 `check-egress` 一致；不改任何配置文件。

**P2（约 1 周）两跳编辑 + 下发**
卡片式两跳编辑；校验（环/名字/凭证）；写 Verge 四层；写 FlClash；写前 `mihomo -t`；应用后自动体检。
验收：从零配置一台机器，全程不打开文本编辑器；改完重启客户端后 ChatGPT 首页 200。

**P3（约 1 周）多跳 + 桌面卡片 + 重启联动**
第三跳接入；每跳延迟（依赖 Verge 外部控制器，未开则显示"未知"）；`应用并重启客户端`。
验收：三跳链路能出网，出口 IP 仍固定在最后一跳；环检测能挡住 A→B→A。

**P4（可选）** 深接 Verge 控制器：实时连接列表、按域名看走哪条链。

---

## 10. 待验证 / 未知

- mihomo 三跳 `dialer-proxy` 在本机与机器 2 上尚未实跑（单跳已验证；机制上支持）。P3 必须先做一次 `mihomo -t` + 实际出网验证。
- Verge 打开外部控制器后的接口细节（`/connections` 的 `chains` 字段能否稳定给出完整链）未验证。
- 桌面卡片在 Win11 多显示器 / 缩放 125% 下的表现未验证。
- 客户端重启是否会影响正在进行的下载/会话，需要实测后决定是否给"只重启内核"选项。

---

## 11. 给新 session 的启动指令（可直接粘贴）

```
接手 HopWay 项目：一个 Windows 桌面代理客户端（多跳链路可视化 + 一键下发到 FlClash / Clash Verge）。
项目根目录 D:\HopWay。先读 HANDOFF-静态ISP客户端.md，再看 README.md，
脚本在 toolkit\，发布包在 dist\，历史文件在 archive\（不要用）。

约束（必须遵守）：
- 不预置静态 ISP 的服务器地址/账号，不写注册表代理，不改环境变量，TUN 只由客户端控制。
- 凭证不进源码/日志/UI 明文；写配置前备份并用 verge-mihomo -t 校验。
- 系统里已经有两台机器的真实配置，改动前先只读核对，别直接覆盖。

第一步只做 P1：Tauri 2 + React + React Flow，托盘 + 主窗 + 桌面卡片，显示链路与出口 IP，只读不改配置。
技术选型固定为 Tauri（Rust 1.96 / Node 24 / WebView2 均已就绪）；写文件复用 toolkit\sync-proxy.ps1 作为 sidecar，不要重写。
先给我跑通的可执行程序与验证步骤，再讨论 P2。
```

---

## 12. 交付物建议

- 代码仓库：`D:\HopWay`（目前还不是 git 仓库，P1 开始前 `git init`，把 `toolkit\` 与客户端代码一起纳入；`archive\`、`dist\*.zip` 可加 `.gitignore`）。
- 每个阶段一个可运行的构建产物 + 一段验收记录（出口 IP、trace 结果、截图），沿用现有"用证据收尾"的习惯。
