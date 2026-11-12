//! Local visual-chain workspace. Subscription URLs, downloaded node details and
//! static credentials are protected together with the current Windows user.
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::{collections::{HashMap, HashSet}, fs, io::{Read, Write}, net::IpAddr, path::Path, process::{Command, Stdio}};

const MAX_DOWNLOAD: u64 = 8 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Point { pub x: f64, pub y: f64 }
#[derive(Clone, Serialize, Deserialize)]
pub struct Link { pub source: String, pub target: String }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String, pub name: String, pub url: String, pub proxies: Vec<Value>,
    pub selected: Option<String>, pub position: Point,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticNode {
    pub id: String, pub name: String, pub server: String, pub port: u16,
    pub username: String, pub password: String, pub position: Point,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub version: u32, pub subscriptions: Vec<Subscription>, pub exits: Vec<StaticNode>, pub links: Vec<Link>,
}
impl Default for Workspace {
    fn default() -> Self { Self { version: 1, subscriptions: vec![], exits: vec![], links: vec![] } }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeChoice { pub name: String, pub region: Option<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView { pub id: String, pub name: String, pub choices: Vec<NodeChoice>, pub selected: Option<String>, pub position: Point }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticView { pub id: String, pub name: String, pub position: Point }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView { pub subscriptions: Vec<SubscriptionView>, pub exits: Vec<StaticView>, pub links: Vec<Link> }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionInput { pub id: String, pub position: Point }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionInput { pub id: String, pub selected: Option<String> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutInput { pub positions: Vec<PositionInput>, pub links: Vec<Link>, pub selections: Vec<SelectionInput> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticInput { pub id: Option<String>, pub name: String, pub server: String, pub port: Option<u16>, pub username: String, pub password: String }

pub struct VisualChain { pub relays: Vec<(String, Value)>, pub exit: StaticNode, pub unused_count: usize }

fn valid_name(name: &str) -> bool { !name.trim().is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control) }
fn node_name(node: &Value) -> Option<&str> { node.get("name")?.as_str() }
fn new_id(prefix: &str) -> String { format!("{prefix}-{}", uuid::Uuid::new_v4().simple()) }

fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN};
    let size = u32::try_from(data.len()).map_err(|_| "保存内容过大")?;
    let input = CRYPT_INTEGER_BLOB { cbData: size, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        }
    };
    if ok == 0 || output.pbData.is_null() { return Err("无法使用当前 Windows 账户保护或读取配置".into()); }
    let result = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut _); }
    Ok(result)
}

pub fn load(path: &Path) -> Result<Workspace, String> {
    let bytes = match fs::read(path) { Ok(bytes) => bytes, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Workspace::default()), Err(_) => return Err("无法读取已保存的配置".into()) };
    let plain = protect(&bytes, false)?;
    let workspace: Workspace = serde_json::from_slice(&plain).map_err(|_| "已保存的配置无法解析")?;
    if workspace.version != 1 { return Err("此版本无法读取该配置".into()); }
    Ok(workspace)
}
pub fn save(path: &Path, workspace: &Workspace) -> Result<(), String> {
    let plain = serde_json::to_vec(workspace).map_err(|_| "无法保存配置")?;
    let encrypted = protect(&plain, true)?;
    let parent = path.parent().ok_or("无法定位保存目录")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建保存目录")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法保存配置")?;
    temp.write_all(&encrypted).and_then(|_| temp.as_file().sync_all()).map_err(|_| "无法保存配置")?;
    temp.persist(path).map_err(|_| "无法保存配置，请稍后重试")?;
    Ok(())
}
pub fn view(workspace: &Workspace) -> WorkspaceView {
    WorkspaceView {
        subscriptions: workspace.subscriptions.iter().map(|s| SubscriptionView {
            id: s.id.clone(), name: s.name.clone(), selected: s.selected.clone(), position: s.position.clone(),
            choices: s.proxies.iter().filter_map(node_name).map(|name| NodeChoice { name: name.into(), region: super::region_from_node_name(name) }).collect(),
        }).collect(),
        exits: workspace.exits.iter().map(|n| StaticView { id: n.id.clone(), name: n.name.clone(), position: n.position.clone() }).collect(),
        links: workspace.links.clone(),
    }
}

pub fn parse_subscription(data: &[u8]) -> Result<Vec<Value>, String> {
    if data.len() > MAX_DOWNLOAD as usize { return Err("订阅内容过大".into()) }
    let doc: Value = serde_yaml::from_slice(data).map_err(|_| "订阅不是有效的 Clash YAML 配置")?;
    let nodes = doc.get("proxies").and_then(Value::as_sequence).ok_or("订阅中没有可直接使用的 proxies 节点列表")?;
    if nodes.is_empty() || nodes.len() > 2000 { return Err("订阅节点为空或数量过多".into()) }
    let mut seen = HashSet::new();
    for node in nodes {
        let name = node_name(node).ok_or("订阅中有节点缺少名称")?;
        let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
        if !valid_name(name) || kind.is_empty() || !seen.insert(name) { return Err("订阅中存在无效或重复的节点名称".into()) }
    }
    Ok(nodes.clone())
}

// The URL (which may contain a token) is sent on stdin, never in process args.
pub fn fetch_subscription(raw_url: &str) -> Result<Vec<Value>, String> {
    use std::os::windows::process::CommandExt;
    let parsed = url::Url::parse(raw_url.trim()).map_err(|_| "订阅地址无效")?;
    if !["http", "https"].contains(&parsed.scheme()) || parsed.host_str().is_none() { return Err("订阅地址必须以 http:// 或 https:// 开头".into()); }
    let encoded = parsed.as_str().replace('\\', "\\\\").replace('"', "\\\"");
    let system = std::env::var_os("SystemRoot").ok_or("无法启动订阅下载")?;
    let curl = Path::new(&system).join("System32").join("curl.exe");
    let mut child = Command::new(curl).args(["--silent", "--fail", "--location", "--proto", "=http,https", "--proto-redir", "=http,https", "--max-time", "30", "--max-filesize", "8388608", "--config", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(0x08000000).spawn().map_err(|_| "无法启动订阅下载")?;
    let input = format!("url = \"{encoded}\"\nuser-agent = \"clash.meta\"\n");
    if child.stdin.take().ok_or("无法启动订阅下载")?.write_all(input.as_bytes()).is_err() { let _ = child.kill(); let _ = child.wait(); return Err("订阅下载失败".into()); }
    let mut body = Vec::new();
    let result = child.stdout.take().ok_or("无法读取订阅")?.take(MAX_DOWNLOAD + 1).read_to_end(&mut body);
    if result.is_err() || body.len() as u64 > MAX_DOWNLOAD { let _ = child.kill(); let _ = child.wait(); return Err("订阅下载失败或内容过大".into()); }
    let status = child.wait().map_err(|_| "订阅下载失败")?;
    if !status.success() { return Err("订阅下载失败，请检查地址和网络".into()) }
    parse_subscription(&body)
}

pub fn put_subscription(workspace: &mut Workspace, name: String, url: String, proxies: Vec<Value>) -> Result<(), String> {
    if !valid_name(&name) { return Err("请填写订阅名称".into()); }
    if workspace.subscriptions.iter().any(|s| s.name == name) { return Err("订阅名称已存在".into()); }
    if workspace.subscriptions.len() >= 30 { return Err("最多添加 30 个订阅".into()); }
    let selected = proxies.first().and_then(node_name).map(str::to_owned);
    let index = workspace.subscriptions.len();
    workspace.subscriptions.push(Subscription { id: new_id("vpn"), name, url, proxies, selected, position: Point { x: 70.0 + (index % 3) as f64 * 270.0, y: 80.0 + (index / 3) as f64 * 170.0 } });
    Ok(())
}
pub fn replace_subscription(workspace: &mut Workspace, id: &str, proxies: Vec<Value>) -> Result<(), String> {
    let sub = workspace.subscriptions.iter_mut().find(|s| s.id == id).ok_or("订阅已不存在")?;
    if !sub.selected.as_ref().map(|selected| proxies.iter().any(|p| node_name(p) == Some(selected))).unwrap_or(false) {
        sub.selected = None;
    }
    sub.proxies = proxies;
    Ok(())
}
pub fn edit_subscription(workspace: &mut Workspace, id: &str, name: String, url: Option<String>, proxies: Option<Vec<Value>>) -> Result<(), String> {
    let name = name.trim();
    if !valid_name(name) { return Err("请填写订阅名称".into()); }
    if workspace.subscriptions.iter().any(|s| s.name == name && s.id != id) { return Err("订阅名称已存在".into()); }
    let sub = workspace.subscriptions.iter_mut().find(|s| s.id == id).ok_or("订阅已不存在")?;
    sub.name = name.into();
    if let Some(url) = url { sub.url = url; }
    if let Some(proxies) = proxies { replace_subscription(workspace, id, proxies)?; }
    Ok(())
}
pub fn put_static(workspace: &mut Workspace, input: StaticInput) -> Result<(), String> {
    let name = input.name.trim();
    if !valid_name(name) { return Err("请填写节点名称".into()); }
    if workspace.exits.iter().any(|x| x.name == name && Some(x.id.as_str()) != input.id.as_deref()) { return Err("静态节点名称已存在".into()); }
    if let Some(id) = input.id.as_deref() {
        if input.port == Some(0) { return Err("端口必须在 1 至 65535 之间".into()); }
        let node = workspace.exits.iter_mut().find(|n| n.id == id).ok_or("静态节点已不存在")?;
        node.name = name.into();
        if !input.server.trim().is_empty() { node.server = input.server.trim().into(); }
        if let Some(port) = input.port.filter(|p| *p > 0) { node.port = port; }
        if !input.username.is_empty() { node.username = input.username; }
        if !input.password.is_empty() { node.password = input.password; }
        if node.server.parse::<IpAddr>().is_err() { return Err("静态出口地址必须是有效的 IP".into()) }
    } else {
        if workspace.exits.len() >= 30 { return Err("最多添加 30 个静态出口".into()); }
        if input.server.trim().parse::<IpAddr>().is_err() || input.port.filter(|p| *p > 0).is_none() || input.username.is_empty() || input.password.is_empty() { return Err("请填写有效的 IP、端口、账号和密码".into()); }
        let index = workspace.exits.len();
        workspace.exits.push(StaticNode { id: new_id("exit"), name: name.into(), server: input.server.trim().into(), port: input.port.unwrap(), username: input.username, password: input.password, position: Point { x: 70.0 + workspace.subscriptions.len().clamp(1, 3) as f64 * 270.0, y: 95.0 + index as f64 * 150.0 } });
    }
    Ok(())
}
pub fn remove(workspace: &mut Workspace, id: &str) -> Result<(), String> {
    let before = workspace.subscriptions.len() + workspace.exits.len();
    workspace.subscriptions.retain(|s| s.id != id);
    workspace.exits.retain(|s| s.id != id);
    if before == workspace.subscriptions.len() + workspace.exits.len() { return Err("节点已不存在".into()); }
    workspace.links.retain(|link| link.source != id && link.target != id);
    Ok(())
}

pub fn save_layout(workspace: &mut Workspace, input: LayoutInput) -> Result<(), String> {
    let vpn: HashSet<&str> = workspace.subscriptions.iter().map(|s| s.id.as_str()).collect();
    let exits: HashSet<&str> = workspace.exits.iter().map(|s| s.id.as_str()).collect();
    let all: HashSet<&str> = vpn.union(&exits).copied().collect();
    let mut sources = HashSet::new(); let mut targets = HashSet::new();
    for link in &input.links {
        if !vpn.contains(link.source.as_str()) || !all.contains(link.target.as_str()) || link.source == link.target || !sources.insert(link.source.as_str()) || !targets.insert(link.target.as_str()) {
            return Err("每个节点只能有一条上游和下游，静态出口只能作为终点".into());
        }
    }
    let outgoing: HashMap<&str, &str> = input.links.iter().map(|e| (e.source.as_str(), e.target.as_str())).collect();
    for &start in &vpn {
        let mut seen = HashSet::new(); let mut current = start;
        while let Some(next) = outgoing.get(current) {
            if !seen.insert(current) { return Err("链路不能形成循环".into()); }
            current = next;
        }
    }
    if input.positions.len() != all.len() || input.selections.len() != vpn.len() { return Err("画布节点已变化，请重新加载".into()) }
    let mut positioned = HashSet::new();
    for item in &input.positions {
        if !all.contains(item.id.as_str()) || !positioned.insert(item.id.as_str()) || !item.position.x.is_finite() || !item.position.y.is_finite() || item.position.x.abs() > 1_000_000.0 || item.position.y.abs() > 1_000_000.0 { return Err("画布位置无效".into()) }
    }
    let mut selected = HashSet::new();
    for item in &input.selections {
        let sub = workspace.subscriptions.iter().find(|s| s.id == item.id).ok_or("订阅已变化，请重新加载")?;
        if !selected.insert(item.id.as_str()) || item.selected.as_ref().map(|name| !sub.proxies.iter().any(|p| node_name(p) == Some(name))).unwrap_or(false) { return Err("所选订阅节点已不存在".into()) }
    }
    for item in input.positions {
        if let Some(sub) = workspace.subscriptions.iter_mut().find(|s| s.id == item.id) { sub.position = item.position; }
        else if let Some(node) = workspace.exits.iter_mut().find(|n| n.id == item.id) { node.position = item.position; }
    }
    for item in input.selections { if let Some(sub) = workspace.subscriptions.iter_mut().find(|s| s.id == item.id) { sub.selected = item.selected; } }
    workspace.links = input.links;
    Ok(())
}

pub fn chain(workspace: &Workspace) -> Result<VisualChain, String> {
    let connected_exits: Vec<&StaticNode> = workspace.exits.iter().filter(|exit| workspace.links.iter().any(|e| e.target == exit.id)).collect();
    if connected_exits.len() != 1 { return Err("请连接一条以静态 IP 为终点的链路".into()) }
    let exit = connected_exits[0].clone();
    let incoming: HashMap<&str, &str> = workspace.links.iter().map(|e| (e.target.as_str(), e.source.as_str())).collect();
    let mut next = exit.id.as_str(); let mut seen = HashSet::new(); let mut reverse = Vec::new();
    while let Some(source) = incoming.get(next) {
        if !seen.insert(*source) { return Err("链路不能形成循环".into()) }
        let sub = workspace.subscriptions.iter().find(|s| s.id == *source).ok_or("链路包含不存在的订阅")?;
        let selected = sub.selected.as_deref().ok_or("请选择每个订阅的节点")?;
        let proxy = sub.proxies.iter().find(|p| node_name(p) == Some(selected)).cloned().ok_or("订阅节点已不存在，请重新选择")?;
        reverse.push((format!("{} · {}", sub.name, selected), proxy));
        next = source;
    }
    if reverse.is_empty() || reverse.len() > 4 { return Err("链路需要 1 至 4 个 VPN 中继".into()) }
    reverse.reverse();
    let unused_count = workspace.subscriptions.len() + workspace.exits.len() - reverse.len() - 1;
    Ok(VisualChain { relays: reverse, exit, unused_count })
}

#[cfg(test)]
mod tests {
    use super::*;
    const YAML: &str = "proxies:\n - name: '🇭🇰 HK'\n   type: socks5\n   server: relay.invalid\n   port: 1080\n   password: private-sub-secret\n";
    fn sample() -> Workspace {
        let mut w = Workspace::default();
        put_subscription(&mut w, "机场".into(), "https://example.com/sub?token=private-url".into(), parse_subscription(YAML.as_bytes()).unwrap()).unwrap();
        put_static(&mut w, StaticInput { id: None, name: "出口".into(), server: "192.0.2.8".into(), port: Some(1080), username: "private-user".into(), password: "private-pass".into() }).unwrap();
        w.links.push(Link { source: w.subscriptions[0].id.clone(), target: w.exits[0].id.clone() });
        w
    }
    #[test]
    fn workspace_is_encrypted_and_view_never_contains_private_fields() {
        let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let path = temp.path().join("workspace.dat");
        let w = sample(); save(&path, &w).unwrap(); save(&path, &w).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private-"));
        let loaded = load(&path).unwrap();
        let view = serde_json::to_string(&view(&loaded)).unwrap();
        assert!(!view.contains("private-"));
        assert_eq!(chain(&loaded).unwrap().relays.len(), 1);
    }
    #[test]
    fn rejects_invalid_and_duplicate_subscriptions() {
        assert!(parse_subscription(b"not: valid").is_err());
        assert!(parse_subscription(b"proxies:\n- {name: A, type: socks5}\n- {name: A, type: socks5}\n").is_err());
    }
    #[test]
    fn detects_cycles_and_preserves_disconnected_modules() {
        let mut w = sample();
        put_subscription(&mut w, "另一机场".into(), "https://example.com".into(), parse_subscription(YAML.as_bytes()).unwrap()).unwrap();
        assert_eq!(chain(&w).unwrap().unused_count, 1);
        let a = w.subscriptions[0].id.clone(); let b = w.subscriptions[1].id.clone();
        let positions = w.subscriptions.iter().map(|n| PositionInput { id: n.id.clone(), position: n.position.clone() }).chain(w.exits.iter().map(|n| PositionInput { id: n.id.clone(), position: n.position.clone() })).collect();
        let selections = w.subscriptions.iter().map(|n| SelectionInput { id: n.id.clone(), selected: n.selected.clone() }).collect();
        assert!(save_layout(&mut w, LayoutInput { positions, selections, links: vec![Link { source: a.clone(), target: b.clone() }, Link { source: b, target: a }] }).is_err());
    }
    #[test]
    fn downloads_a_clash_subscription_without_leaking_its_url() {
        use std::{net::TcpListener, time::{Duration, Instant}};
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let address = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            loop {
                match server.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0u8; 2048]; let _ = stream.read(&mut request);
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", YAML.len(), YAML);
                        stream.write_all(response.as_bytes()).unwrap(); break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
        });
        let nodes = fetch_subscription(&format!("http://{address}/sub?token=private-url")).unwrap();
        handle.join().unwrap();
        assert_eq!(node_name(&nodes[0]), Some("🇭🇰 HK"));
    }
}
