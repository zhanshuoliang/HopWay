#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod configure;
mod relay_status;
mod workspace;

use serde::Serialize;
use serde_yaml::Value;
use std::{collections::HashSet, env, fs, net::IpAddr, path::PathBuf, process::Command, sync::Mutex, time::{SystemTime, UNIX_EPOCH}};
use tauri::{Emitter, Manager, State, menu::{Menu, MenuItem}, tray::TrayIconBuilder};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    choice: String,
    client: String,
    config_found: bool,
    proxy_port: Option<u16>,
    system_proxy_enabled: bool,
    system_proxy_matches: bool,
    route_target: Option<String>,
    static_node: Option<String>,
    chain_attached: bool,
    relay: Option<String>,
    relay_kind: Option<String>,
    relay_region: Option<String>,
    relay_region_source: Option<String>,
    relays: Vec<RelayInfo>,
    ip: Option<String>,
    probe_state: String,
    egress_match: Option<bool>,
    // Kept in Rust only; the UI receives the comparison, not the server address.
    #[serde(skip_serializing)]
    expected_ip: Option<IpAddr>,
    updated_at: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayInfo {
    name: String,
    kind: String,
    region: Option<String>,
    region_source: Option<String>,
    protocol: Option<String>,
    #[serde(skip_serializing)]
    raw_name: String,
}

struct Inner { snapshot: Snapshot, generation: u64 }
struct AppState(Mutex<Inner>);
struct TrayItems(Mutex<Option<(MenuItem<tauri::Wry>, MenuItem<tauri::Wry>)>>);
struct PlanSlot { generation: u64, plan: Option<configure::Prepared> }
struct PlanState(Mutex<PlanSlot>);
struct WorkspaceState(Mutex<()>);

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_mapping()?.get(Value::String(key.to_owned()))
}
fn text(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned).filter(|s| !s.trim().is_empty())
}
fn seq(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_sequence).map(Vec::as_slice).unwrap_or(&[])
}

// Only inspect the user's existing Internet Settings. HopWay never modifies them.
fn system_proxy() -> (bool, Option<u16>) {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = root.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings") else { return (false, None) };
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    let server: String = key.get_value("ProxyServer").unwrap_or_default();
    let mut port = None;
    for part in server.split(';') {
        let endpoint = part.rsplit('=').next().unwrap_or("").trim().trim_start_matches("http://").trim_start_matches("https://");
        if let Some((host, number)) = endpoint.rsplit_once(':') {
            if matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "[::1]") {
                port = number.parse::<u16>().ok();
                if port.is_some() { break; }
            }
        }
    }
    (enabled != 0, port)
}

fn config_path() -> PathBuf {
    let mut p = PathBuf::from(env::var_os("APPDATA").unwrap_or_default());
    p.push("io.github.clash-verge-rev.clash-verge-rev");
    p.push("clash-verge.yaml");
    p
}

// A flag denotes the node's own label, not a geolocation of the final egress IP.
fn region_from_node_name(name: &str) -> Option<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut flags = HashSet::new();
    for pair in chars.windows(2) {
        let a = pair[0] as u32;
        let b = pair[1] as u32;
        if (0x1f1e6..=0x1f1ff).contains(&a) && (0x1f1e6..=0x1f1ff).contains(&b) {
            flags.insert(format!("{}{}", (b'a' + (a - 0x1f1e6) as u8) as char, (b'a' + (b - 0x1f1e6) as u8) as char));
        }
    }
    if flags.len() == 1 { return flags.into_iter().next(); }
    if !flags.is_empty() { return None; }
    let lower = name.to_lowercase();
    let tokens: HashSet<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();
    let hints: &[(&str, &[&str], &[&str])] = &[
        ("hk", &["香港", "hong kong"], &["hk", "hkg"]),
        ("jp", &["日本", "东京", "東京", "osaka", "tokyo"], &["jp", "jpn"]),
        ("sg", &["新加坡", "singapore"], &["sg", "sgp"]),
        ("tw", &["台湾", "台灣", "taiwan"], &["tw", "twn"]),
        ("us", &["美国", "美國", "united states"], &["us", "usa"]),
        ("gb", &["英国", "英國", "united kingdom"], &["uk", "gb"]),
        ("kr", &["韩国", "韓國", "south korea"], &["kr", "kor"]),
        ("de", &["德国", "德國", "germany"], &["de", "deu"]),
        ("fr", &["法国", "法國", "france"], &["fr", "fra"]),
        ("ca", &["加拿大", "canada"], &["ca", "can"]),
        ("au", &["澳大利亚", "澳洲", "australia"], &["au", "aus"]),
        ("nl", &["荷兰", "荷蘭", "netherlands"], &["nl", "nld"]),
    ];
    let matches: Vec<&str> = hints.iter().filter_map(|(code, words, codes)| {
        (words.iter().any(|word| lower.contains(word)) || codes.iter().any(|token| tokens.contains(token))).then_some(*code)
    }).collect();
    if matches.len() == 1 { Some(matches[0].to_owned()) } else { None }
}

fn parse_config(choice: &str, client: &str, raw: &str, sys: (bool, Option<u16>)) -> Snapshot {
    let mut result = Snapshot { choice: choice.into(), client: client.into(), system_proxy_enabled: sys.0, probe_state: "idle".into(), ..Default::default() };
    let Ok(doc) = serde_yaml::from_str::<Value>(raw) else { return result };
    result.config_found = true;
    result.proxy_port = field(&doc, "mixed-port").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0);
    result.system_proxy_matches = sys.0 && sys.1.is_some() && sys.1 == result.proxy_port;
    result.route_target = seq(field(&doc, "rules")).iter().filter_map(Value::as_str).find_map(|rule| {
        let mut parts = rule.split(',').map(str::trim);
        match parts.next() { Some("MATCH") | Some("FINAL") => parts.next().filter(|v| !v.is_empty()).map(str::to_owned), _ => None }
    });

    let groups = seq(field(&doc, "proxy-groups"));
    let proxies = seq(field(&doc, "proxies"));
    let group = groups.iter().find(|g| text(field(g, "name")) == result.route_target);
    let is_chained_socks = |p: &&Value| field(p, "dialer-proxy").is_some() && field(p, "type").and_then(Value::as_str).map(|kind| kind.eq_ignore_ascii_case("socks5")).unwrap_or(false);
    let selected_candidate = group.and_then(|g| seq(field(g, "proxies")).iter().filter_map(Value::as_str).find_map(|name| {
        proxies.iter().filter(is_chained_socks).find(|p| field(p, "name").and_then(Value::as_str) == Some(name))
    })).or_else(|| result.route_target.as_deref().and_then(|target| proxies.iter().filter(is_chained_socks).find(|p| field(p, "name").and_then(Value::as_str) == Some(target))));
    // The known setup uses LS-ISP-01. This is a node label only, never an ISP address or credential.
    let static_proxy = selected_candidate.or_else(|| proxies.iter().find(|p| field(p, "name").and_then(Value::as_str) == Some("LS-ISP-01")));
    if let Some(proxy) = static_proxy {
        let in_static_group = group.map(|g| seq(field(g, "proxies")).iter().any(|item|
            item.as_str() == field(proxy, "name").and_then(Value::as_str))).unwrap_or(false);
        let is_static_socks = field(proxy, "type").and_then(Value::as_str).map(|kind| kind.eq_ignore_ascii_case("socks5")).unwrap_or(false);
        result.chain_attached = is_static_socks && (in_static_group || result.route_target.as_deref() == field(proxy, "name").and_then(Value::as_str));
        if result.route_target.as_deref() == Some("静态ISP") && in_static_group && is_static_socks {
            result.expected_ip = field(proxy, "server").and_then(Value::as_str).and_then(|server| server.parse().ok());
        }
        let display_name = |name: &str| {
            if name.starts_with("__hopway_") { name.split_once("::").map(|(_, label)| label).unwrap_or(name).to_owned() }
            else { name.to_owned() }
        };
        result.static_node = text(field(proxy, "name")).map(|name| display_name(&name));
        let mut current = text(field(proxy, "dialer-proxy"));
        let mut seen = HashSet::new();
        let mut relays = Vec::new();
        while let Some(name) = current {
            if !seen.insert(name.clone()) || seen.len() > 8 { relays.clear(); break; }
            let display = display_name(&name);
            let proxy = proxies.iter().find(|p| field(p, "name").and_then(Value::as_str) == Some(name.as_str()));
            let is_group = groups.iter().any(|g| field(g, "name").and_then(Value::as_str) == Some(name.as_str()));
            let kind = if is_group { "策略组" } else if proxy.is_some() { "节点" } else { "未知" }.to_owned();
            let region = if proxy.is_some() { region_from_node_name(&display) } else { None };
            let region_source = region.as_ref().map(|_| "configured".to_owned());
            let protocol = proxy.and_then(|p| text(field(p, "type"))).filter(|kind| kind.chars().count() <= 32 && !kind.chars().any(char::is_control));
            relays.push(RelayInfo { name: display, kind, region, region_source, protocol, raw_name: name.clone() });
            current = proxy.and_then(|p| text(field(p, "dialer-proxy")));
        }
        relays.reverse();
        result.relay = relays.first().map(|relay| relay.name.clone());
        result.relay_kind = relays.first().map(|relay| relay.kind.clone());
        result.relay_region = relays.first().and_then(|r| r.region.clone());
        result.relay_region_source = relays.first().and_then(|r| r.region_source.clone());
        result.relays = relays;
    }
    // If the default rule points to a group without a recognized static
    // candidate, the group itself can still be resolved by the local API.
    if !result.chain_attached {
        if let Some(group_name) = group.and_then(|g| text(field(g, "name"))) {
            result.relay = Some(group_name.clone());
            result.relay_kind = Some("策略组".into());
            result.relay_region = None;
            result.relay_region_source = None;
            result.relays = vec![RelayInfo { name: group_name.clone(), kind: "策略组".into(), region: None, region_source: None, protocol: None, raw_name: group_name }];
        }
    }
    result
}

fn load_config() -> Snapshot {
    let sys = system_proxy();
    match fs::read_to_string(config_path()) {
        Ok(raw) => parse_config("verge", "verge", &raw, sys),
        Err(_) => Snapshot { choice: "verge".into(), client: "verge".into(), system_proxy_enabled: sys.0, probe_state: "idle".into(), ..Default::default() },
    }
}

fn update_tray(app: &tauri::AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.try_state::<TrayItems>() else { return };
    if let Ok(guard) = tray.0.lock() {
        if let Some((route, ip)) = guard.as_ref() {
            let _ = route.set_text(format!("链路：{} → {}", snapshot.relay.as_deref().unwrap_or("未知中继"), snapshot.static_node.as_deref().unwrap_or("未知出口")));
            let _ = ip.set_text(format!("出口 IP：{}", snapshot.ip.as_deref().unwrap_or("尚未探测")));
        }
    };
}

fn broadcast(app: &tauri::AppHandle, snapshot: &Snapshot) {
    update_tray(app, snapshot);
    let _ = app.emit("hopway:snapshot", snapshot);
}

#[tauri::command]
fn get_snapshot(app: tauri::AppHandle, state: State<'_, AppState>) -> Snapshot {
    let mut inner = state.0.lock().unwrap();
    let mut fresh = load_config();
    if fresh.client == inner.snapshot.client && fresh.proxy_port == inner.snapshot.proxy_port
        && fresh.expected_ip == inner.snapshot.expected_ip && fresh.relay == inner.snapshot.relay
        && fresh.chain_attached == inner.snapshot.chain_attached
        && fresh.relays.iter().map(|r| (&r.name, &r.kind)).eq(inner.snapshot.relays.iter().map(|r| (&r.name, &r.kind)))
        && fresh.static_node == inner.snapshot.static_node && fresh.route_target == inner.snapshot.route_target {
        fresh.ip = inner.snapshot.ip.clone();
        fresh.probe_state = inner.snapshot.probe_state.clone();
        fresh.egress_match = inner.snapshot.egress_match;
        fresh.updated_at = inner.snapshot.updated_at;
    } else { inner.generation += 1; }
    inner.snapshot = fresh.clone();
    drop(inner);
    update_tray(&app, &fresh);
    fresh
}

#[tauri::command]
async fn get_relay_selections(app: tauri::AppHandle) -> Option<relay_status::RelayView> {
    let result = relay_status::read_current().await;
    let _ = app.emit("hopway:relay-selection", result.clone());
    result
}

fn get_ip(port: u16) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let curl = PathBuf::from(env::var_os("SystemRoot").unwrap_or_default()).join("System32").join("curl.exe");
    let out = Command::new(curl)
        .args(["--silent", "--fail", "--noproxy", "", "--connect-timeout", "4", "--max-time", "12", "--proxy", &format!("http://127.0.0.1:{port}"), "https://api.ipify.org"])
        .creation_flags(0x08000000)
        .output().ok()?;
    if !out.status.success() { return None; }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    value.parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

#[tauri::command]
async fn probe_egress(app: tauri::AppHandle) -> Result<Snapshot, String> {
    if !app.get_webview_window("widget").and_then(|w| w.is_visible().ok()).unwrap_or(false) { return Err("桌面卡片已隐藏，探测已暂停".into()); }
    let (port, generation, checking) = {
        let state = app.state::<AppState>();
        let mut inner = state.0.lock().unwrap();
        let port = inner.snapshot.proxy_port.ok_or("未找到客户端的本地代理端口")?;
        if inner.snapshot.probe_state == "checking" { return Ok(inner.snapshot.clone()); }
        inner.snapshot.probe_state = "checking".into();
        (port, inner.generation, inner.snapshot.clone())
    };
    broadcast(&app, &checking);
    let ip = tauri::async_runtime::spawn_blocking(move || get_ip(port)).await.map_err(|_| "出口探测失败")?;
    let result = {
        let state = app.state::<AppState>();
        let mut inner = state.0.lock().unwrap();
        if inner.generation != generation { return Ok(inner.snapshot.clone()); }
        inner.snapshot.ip = ip;
        inner.snapshot.probe_state = if inner.snapshot.ip.is_some() { "ok" } else { "failed" }.into();
        inner.snapshot.egress_match = inner.snapshot.expected_ip.and_then(|expected|
            inner.snapshot.ip.as_deref().and_then(|value| value.parse::<IpAddr>().ok()).map(|actual| actual == expected));
        inner.snapshot.updated_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());
        inner.snapshot.clone()
    };
    broadcast(&app, &result);
    Ok(result)
}

#[tauri::command]
fn show_main(app: tauri::AppHandle) { if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus(); } }
#[tauri::command]
fn show_widget(app: tauri::AppHandle) { if let Some(w) = app.get_webview_window("widget") { let _ = w.show(); let _ = w.set_focus(); let _ = w.emit("hopway:widget-shown", ()); } }
#[tauri::command]
fn hide_widget(app: tauri::AppHandle) { if let Some(w) = app.get_webview_window("widget") { let _ = w.hide(); } }
#[tauri::command]
fn hide_main(app: tauri::AppHandle) { if let Some(w) = app.get_webview_window("main") { let _ = w.hide(); } }
#[tauri::command]
fn minimize_main(app: tauri::AppHandle) { if let Some(w) = app.get_webview_window("main") { let _ = w.minimize(); } }
#[tauri::command]
fn set_widget_expanded(app: tauri::AppHandle, expanded: bool) -> Result<(), String> {
    let window = app.get_webview_window("widget").ok_or("未找到桌面组件")?;
    let position = window.outer_position().map_err(|_| "无法读取组件位置")?;
    let previous = window.outer_size().map_err(|_| "无法读取组件大小")?;
    let scale = window.scale_factor().map_err(|_| "无法读取屏幕缩放")?;
    let (width, height) = if expanded { (600.0, 276.0) } else { (320.0, 56.0) };
    let target_width = (width * scale).round() as i64;
    let target_height = (height * scale).round() as i64;
    let mut x = i64::from(position.x) + i64::from(previous.width) - target_width;
    let mut y = i64::from(position.y);
    if let Ok(Some(monitor)) = window.current_monitor() {
        let area = monitor.work_area();
        let left = i64::from(area.position.x);
        let top = i64::from(area.position.y);
        let right = left + i64::from(area.size.width);
        let bottom = top + i64::from(area.size.height);
        if i64::from(position.y) + i64::from(previous.height) / 2 >= top + i64::from(area.size.height) / 2 {
            y = i64::from(position.y) + i64::from(previous.height) - target_height;
        }
        x = x.clamp(left, (right - target_width).max(left));
        y = y.clamp(top, (bottom - target_height).max(top));
    }
    let pos = tauri::PhysicalPosition::new(x.clamp(i32::MIN as i64, i32::MAX as i64) as i32, y.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
    window.set_size(tauri::LogicalSize::new(width, height)).map_err(|_| "无法调整组件大小")?;
    if window.set_position(pos).is_err() {
        let _ = window.set_size(previous);
        let _ = window.set_position(position);
        return Err("无法调整组件位置".into());
    }
    Ok(())
}
#[tauri::command]
fn toggle_maximize_main(app: tauri::AppHandle) -> Result<bool, String> {
    let window = app.get_webview_window("main").ok_or("未找到主窗口")?;
    if window.is_maximized().map_err(|_| "无法读取窗口状态")? {
        window.unmaximize().map_err(|_| "无法还原窗口")?;
        Ok(false)
    } else {
        window.maximize().map_err(|_| "无法最大化窗口")?;
        Ok(true)
    }
}

#[tauri::command]
fn chain_options(target: String) -> Result<configure::EditorOptions, String> {
    configure::editor_options(&target)
}
#[tauri::command]
async fn prepare_chain(app: tauri::AppHandle, draft: configure::Draft) -> Result<configure::PlanPreview, String> {
    let generation = {
        let state = app.state::<PlanState>();
        let mut slot = state.0.lock().map_err(|_| "无法准备配置")?;
        slot.generation += 1;
        slot.plan = None;
        slot.generation
    };
    let (plan, preview) = tauri::async_runtime::spawn_blocking(move || configure::prepare(draft)).await.map_err(|_| "无法准备配置")??;
    let state = app.state::<PlanState>();
    let mut slot = state.0.lock().map_err(|_| "无法准备配置")?;
    if slot.generation != generation { return Err("预览已取消，请重新校验".into()); }
    slot.plan = Some(plan);
    Ok(preview)
}
#[tauri::command]
async fn apply_chain(app: tauri::AppHandle, id: String) -> Result<configure::ApplyResult, String> {
    let plan = {
        let state = app.state::<PlanState>();
        let mut slot = state.0.lock().map_err(|_| "无法写入配置")?;
        slot.generation += 1;
        slot.plan.take().ok_or("预览已失效，请重新校验")?
    };
    tauri::async_runtime::spawn_blocking(move || configure::apply(plan, &id)).await.map_err(|_| "无法写入配置")?
}
#[tauri::command]
fn cancel_chain(state: State<'_, PlanState>) {
    if let Ok(mut slot) = state.0.lock() { slot.generation += 1; slot.plan = None; }
}

fn workspace_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = env::var_os("HOPWAY_TEST_WORKSPACE") { return Ok(PathBuf::from(path)); }
    Ok(app.path().app_data_dir().map_err(|_| "无法定位配置保存目录")?.join("workspace-v1.dat"))
}
#[tauri::command]
fn load_workspace(app: tauri::AppHandle, state: State<'_, WorkspaceState>) -> Result<workspace::WorkspaceView, String> {
    let _lock = state.0.lock().map_err(|_| "无法读取画布")?;
    Ok(workspace::view(&workspace::load(&workspace_path(&app)?)?))
}
#[tauri::command]
async fn add_subscription(app: tauri::AppHandle, name: String, url: String) -> Result<workspace::WorkspaceView, String> {
    if name.trim().is_empty() || url.trim().is_empty() { return Err("请填写订阅名称和地址".into()); }
    let download_url = url.clone();
    let proxies = tauri::async_runtime::spawn_blocking(move || workspace::fetch_subscription(&download_url)).await.map_err(|_| "订阅下载失败")??;
    let state = app.state::<WorkspaceState>();
    let _lock = state.0.lock().map_err(|_| "无法保存订阅")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    workspace::put_subscription(&mut project, name.trim().into(), url.trim().into(), proxies)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
async fn refresh_subscription(app: tauri::AppHandle, id: String) -> Result<workspace::WorkspaceView, String> {
    let url = {
        let state = app.state::<WorkspaceState>();
        let _lock = state.0.lock().map_err(|_| "无法读取订阅")?;
        let project = workspace::load(&workspace_path(&app)?)?;
        project.subscriptions.iter().find(|s| s.id == id).map(|s| s.url.clone()).ok_or("订阅已不存在")?
    };
    let download_url = url.clone();
    let proxies = tauri::async_runtime::spawn_blocking(move || workspace::fetch_subscription(&download_url)).await.map_err(|_| "订阅下载失败")??;
    let state = app.state::<WorkspaceState>();
    let _lock = state.0.lock().map_err(|_| "无法保存订阅")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    if !project.subscriptions.iter().any(|s| s.id == id && s.url == url) { return Err("订阅已变化，请重新操作".into()); }
    workspace::replace_subscription(&mut project, &id, proxies)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
async fn edit_subscription(app: tauri::AppHandle, id: String, name: String, url: String) -> Result<workspace::WorkspaceView, String> {
    if name.trim().is_empty() { return Err("请填写订阅名称".into()); }
    let new_url = if url.trim().is_empty() { None } else { Some(url.trim().to_owned()) };
    let proxies = if let Some(download_url) = new_url.clone() {
        Some(tauri::async_runtime::spawn_blocking(move || workspace::fetch_subscription(&download_url)).await.map_err(|_| "订阅下载失败")??)
    } else { None };
    let state = app.state::<WorkspaceState>();
    let _lock = state.0.lock().map_err(|_| "无法保存订阅")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    workspace::edit_subscription(&mut project, &id, name, new_url, proxies)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
fn save_static_node(app: tauri::AppHandle, state: State<'_, WorkspaceState>, input: workspace::StaticInput) -> Result<workspace::WorkspaceView, String> {
    let _lock = state.0.lock().map_err(|_| "无法保存静态节点")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    workspace::put_static(&mut project, input)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
fn remove_workspace_node(app: tauri::AppHandle, state: State<'_, WorkspaceState>, id: String) -> Result<workspace::WorkspaceView, String> {
    let _lock = state.0.lock().map_err(|_| "无法删除节点")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    workspace::remove(&mut project, &id)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
fn save_workspace_layout(app: tauri::AppHandle, state: State<'_, WorkspaceState>, layout: workspace::LayoutInput) -> Result<workspace::WorkspaceView, String> {
    let _lock = state.0.lock().map_err(|_| "无法保存画布")?;
    let path = workspace_path(&app)?;
    let mut project = workspace::load(&path)?;
    workspace::save_layout(&mut project, layout)?;
    workspace::save(&path, &project)?;
    let _ = app.emit("hopway:workspace-changed", ());
    Ok(workspace::view(&project))
}
#[tauri::command]
async fn prepare_visual_chain(app: tauri::AppHandle, target: String) -> Result<configure::PlanPreview, String> {
    let (visual, path, bytes) = {
        let state = app.state::<WorkspaceState>();
        let _lock = state.0.lock().map_err(|_| "无法读取画布")?;
        let path = workspace_path(&app)?;
        let bytes = fs::read(&path).map_err(|_| "请先保存画布配置")?;
        let project = workspace::load(&path)?;
        if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) { return Err("画布已变化，请重新保存".into()); }
        (workspace::chain(&project)?, path, bytes)
    };
    let generation = {
        let state = app.state::<PlanState>();
        let mut slot = state.0.lock().map_err(|_| "无法准备配置")?;
        slot.generation += 1; slot.plan = None; slot.generation
    };
    let (mut plan, preview) = tauri::async_runtime::spawn_blocking(move || configure::prepare_visual(&target, visual)).await.map_err(|_| "无法准备配置")??;
    configure::guard_workspace(&mut plan, &path, &bytes);
    let state = app.state::<PlanState>();
    let mut slot = state.0.lock().map_err(|_| "无法准备配置")?;
    if slot.generation != generation { return Err("预览已取消，请重新校验".into()); }
    slot.plan = Some(plan);
    Ok(preview)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveCheck { state: String, ip: Option<String>, message: String }
fn routes_use_static(doc: &Value) -> bool {
    if field(doc, "mode").and_then(Value::as_str).map(|v| !v.eq_ignore_ascii_case("rule")).unwrap_or(false) { return false; }
    let mut found_default = false;
    for rule in seq(field(doc, "rules")).iter().filter_map(Value::as_str) {
        let parts: Vec<&str> = rule.split(',').map(str::trim).collect();
        if parts.len() < 2 { return false; }
        if parts[0].eq_ignore_ascii_case("MATCH") || parts[0].eq_ignore_ascii_case("FINAL") {
            found_default = parts[1] == "静态ISP";
            break;
        }
        let mut idx = parts.len() - 1;
        if parts[idx].eq_ignore_ascii_case("no-resolve") && idx > 0 { idx -= 1; }
        if !["DIRECT", "REJECT", "REJECT-DROP", "PASS", "静态ISP"].contains(&parts[idx]) { return false; }
    }
    found_default
}
#[tauri::command]
async fn verify_visual_live(target: String, exit_node: String, expected_ip: String) -> Result<LiveCheck, String> {
    if target != "verge" || !exit_node.starts_with("__hopway_") { return Err("校验参数无效".into()); }
    let expected = expected_ip.parse::<IpAddr>().map_err(|_| "静态出口 IP 无效")?;
    tauri::async_runtime::spawn_blocking(move || {
        let raw = fs::read_to_string(config_path()).map_err(|_| "无法读取客户端运行配置")?;
        let doc: Value = serde_yaml::from_str(&raw).map_err(|_| "客户端运行配置无效")?;
        let group = seq(field(&doc, "proxy-groups")).iter().find(|g| field(g, "name").and_then(Value::as_str) == Some("静态ISP"));
        let present = group.map(|g| seq(field(g, "proxies")).iter().any(|v| v.as_str() == Some(exit_node.as_str()))).unwrap_or(false)
            && seq(field(&doc, "proxies")).iter().any(|p| field(p, "name").and_then(Value::as_str) == Some(exit_node.as_str()));
        if !present { return Ok(LiveCheck { state: "waiting".into(), ip: None, message: "等待客户端重新应用配置".into() }); }
        let port = field(&doc, "mixed-port").and_then(Value::as_u64).and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).ok_or("未找到客户端本地代理端口")?;
        let Some(ip) = get_ip(port) else { return Ok(LiveCheck { state: "failed".into(), ip: None, message: "客户端出口探测失败".into() }) };
        let matches = ip.parse::<IpAddr>().ok() == Some(expected);
        let routes_ok = routes_use_static(&doc);
        Ok(LiveCheck {
            state: if matches && routes_ok { "ok" } else if matches { "partial" } else { "mismatch" }.into(), ip: Some(ip),
            message: if matches && routes_ok { "客户端出口与静态 IP 一致" } else if matches { "探测 IP 一致，但部分规则或模式未指向静态出口" } else { "客户端出口与静态 IP 不一致，请检查策略组选择" }.into(),
        })
    }).await.map_err(|_| "客户端出口校验失败")?
}

// Clip the native window as well as the HTML, so the transparent corners do
// not leave a rectangular Windows frame. Region sizes are physical pixels.
#[cfg(windows)]
fn update_window_region(window: &tauri::Window) {
    use std::{collections::HashMap, sync::OnceLock};
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
    static LAST_REGION: OnceLock<Mutex<HashMap<String, (u32, u32, i32, bool)>>> = OnceLock::new();
    let radius = match window.label() { "main" => 24.0, "widget" => 26.0, _ => return };
    let (Ok(hwnd), Ok(size), Ok(scale)) = (window.hwnd(), window.outer_size(), window.scale_factor()) else { return };
    let maximized = window.is_maximized().unwrap_or(false);
    let diameter = (radius * scale * 2.0).round() as i32;
    let state = (size.width, size.height, diameter, maximized);
    let Ok(mut last) = LAST_REGION.get_or_init(|| Mutex::new(HashMap::new())).lock() else { return };
    if last.get(window.label()) == Some(&state) { return; }
    last.insert(window.label().to_owned(), state);
    drop(last);
    if maximized {
        unsafe { SetWindowRgn(hwnd.0 as _, std::ptr::null_mut(), 1); }
        return;
    }
    let Ok(width) = i32::try_from(size.width) else { return };
    let Ok(height) = i32::try_from(size.height) else { return };
    if width <= 0 || height <= 0 { return; }
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width.saturating_add(1), height.saturating_add(1), diameter, diameter);
        if !region.is_null() && SetWindowRgn(hwnd.0 as _, region, 1) == 0 { DeleteObject(region); }
    }
}

fn main() {
    tauri::Builder::default()
        .manage(AppState(Mutex::new(Inner { snapshot: Snapshot::default(), generation: 0 })))
        .manage(TrayItems(Mutex::new(None)))
        .manage(PlanState(Mutex::new(PlanSlot { generation: 0, plan: None })))
        .manage(WorkspaceState(Mutex::new(())))
        .setup(|app| {
            #[cfg(windows)]
            for label in ["main", "widget"] {
                if let Some(window) = app.get_webview_window(label) {
                    let webview: &tauri::Webview<tauri::Wry> = window.as_ref();
                    update_window_region(&webview.window());
                }
            }
            if let (Some(widget), Ok(Some(monitor))) = (app.get_webview_window("widget"), app.primary_monitor()) {
                if let Ok(size) = widget.outer_size() {
                    let work = monitor.work_area();
                    let margin = (24.0 * monitor.scale_factor()).round() as i32;
                    let x = (work.position.x + work.size.width as i32 - size.width as i32 - margin).max(work.position.x);
                    let y = (work.position.y + work.size.height as i32 - size.height as i32 - margin).max(work.position.y);
                    let _ = widget.set_position(tauri::PhysicalPosition::new(x, y));
                }
            }
            let route = MenuItem::with_id(app, "route", "链路：读取中", false, None::<&str>)?;
            let ip = MenuItem::with_id(app, "ip", "出口 IP：尚未探测", false, None::<&str>)?;
            let main = MenuItem::with_id(app, "main", "打开主窗口", true, None::<&str>)?;
            let widget = MenuItem::with_id(app, "widget", "显示桌面卡片", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "隐藏桌面卡片", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 HopWay", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&route, &ip, &main, &widget, &hide, &quit])?;
            *app.state::<TrayItems>().0.lock().unwrap() = Some((route, ip));
            TrayIconBuilder::new().icon(app.default_window_icon().unwrap().clone()).tooltip("HopWay · 跳径")
                .menu(&menu).show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "main" => show_main(app.clone()), "widget" => show_widget(app.clone()), "hide" => hide_widget(app.clone()), "quit" => app.exit(0), _ => ()
                }).build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(windows)]
            if matches!(event, tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }) {
                update_window_region(window);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); let _ = window.hide(); }
        })
        .invoke_handler(tauri::generate_handler![get_snapshot, get_relay_selections, probe_egress, show_main, show_widget, hide_widget, hide_main, minimize_main, set_widget_expanded, toggle_maximize_main, chain_options, prepare_chain, apply_chain, cancel_chain, load_workspace, add_subscription, edit_subscription, refresh_subscription, save_static_node, remove_workspace_node, save_workspace_layout, prepare_visual_chain, verify_visual_live])
        .run(tauri::generate_context!())
        .expect("HopWay failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_chain_without_secrets_and_does_not_infer_selected_node() {
        let config = "mixed-port: 7890\nproxies:\n  - name: LS-ISP-01\n    type: socks5\n    server: private.example\n    password: secret-value\n    dialer-proxy: Relay\nproxy-groups:\n  - name: Relay\n    type: select\n    proxies: [A, B]\n  - name: Static\n    type: select\n    proxies: [LS-ISP-01, DIRECT]\nrules: ['MATCH,Static']\n";
        let s = parse_config("verge", "verge", config, (true, Some(7890)));
        assert_eq!(s.relay.as_deref(), Some("Relay"));
        assert_eq!(s.relay_kind.as_deref(), Some("策略组"));
        assert_eq!(s.static_node.as_deref(), Some("LS-ISP-01"));
        assert_eq!(s.route_target.as_deref(), Some("Static"));
        assert!(s.system_proxy_matches);
    }
    #[test]
    fn missing_or_invalid_configuration_never_reports_a_connection() {
        let s = parse_config("auto", "verge", "not: [valid", (false, None));
        assert!(!s.config_found);
        assert!(s.ip.is_none());
        assert_eq!(s.probe_state, "idle");
    }
    #[test]
    fn relay_flag_uses_unambiguous_node_labels() {
        assert_eq!(region_from_node_name("🇭🇰香港高速01").as_deref(), Some("hk"));
        assert_eq!(region_from_node_name("日本 东京 03").as_deref(), Some("jp"));
        assert_eq!(region_from_node_name("🇭🇰香港 → 🇯🇵日本"), None);
        assert_eq!(region_from_node_name("unknown relay"), None);
    }
    #[test]
    fn direct_relay_flag_requires_a_configured_proxy() {
        let prefix = "mixed-port: 7890\nproxies:\n - name: LS-ISP-01\n   dialer-proxy: '🇭🇰香港01'\n";
        let valid = format!("{prefix} - name: '🇭🇰香港01'\n");
        assert_eq!(parse_config("auto", "verge", &valid, (false, None)).relay_region.as_deref(), Some("hk"));
        assert_eq!(parse_config("auto", "verge", prefix, (false, None)).relay_region, None);
    }
    #[test]
    fn displays_each_managed_relay_in_order_without_private_fields() {
        let config = "proxies:\n - name: '__hopway_ab_r1::日本 JP'\n   dialer-proxy: '🇭🇰香港'\n   server: private.example\n - name: '🇭🇰香港'\n - name: '__hopway_ab_exit::出口'\n   type: socks5\n   dialer-proxy: '__hopway_ab_r1::日本 JP'\nproxy-groups:\n - name: 静态ISP\n   type: select\n   proxies: ['__hopway_ab_exit::出口']\nrules: ['MATCH,静态ISP']\n";
        let snapshot = parse_config("verge", "verge", config, (false, None));
        assert_eq!(snapshot.relays.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["🇭🇰香港", "日本 JP"]);
        assert_eq!(snapshot.relays[0].region.as_deref(), Some("hk"));
        assert_eq!(snapshot.relays[1].region.as_deref(), Some("jp"));
        assert_eq!(snapshot.static_node.as_deref(), Some("出口"));
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("private.example"));
    }
    #[test]
    fn live_check_requires_all_proxy_rules_and_default_to_use_static() {
        let good: Value = serde_yaml::from_str("mode: rule\nrules: ['DOMAIN,a,DIRECT', 'PROCESS-NAME,x,静态ISP', 'MATCH,静态ISP']\n").unwrap();
        let bad: Value = serde_yaml::from_str("mode: rule\nrules: ['DOMAIN,a,Airport', 'MATCH,静态ISP']\n").unwrap();
        let global: Value = serde_yaml::from_str("mode: global\nrules: ['MATCH,静态ISP']\n").unwrap();
        assert!(routes_use_static(&good));
        assert!(!routes_use_static(&bad));
        assert!(!routes_use_static(&global));
    }

}
