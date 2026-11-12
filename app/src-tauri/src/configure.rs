//! Build a candidate from disposable client snapshots, validate it, then commit
//! only after the user confirms. No client configuration is written by staging.
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, env, fs, io::Write, path::{Component, Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};

const STATIC_GROUP: &str = "静态ISP";
const SYNC_SCRIPT: &str = include_str!("../../../toolkit/sync-proxy.ps1");
const STAGE_SCRIPT: &str = include_str!("../../../toolkit/hopway-stage.ps1");
const WRAPPER_BEGIN: &str = "// BEGIN HOPWAY MANAGED CHAIN";
const WRAPPER_END: &str = "// END HOPWAY MANAGED CHAIN";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub target: String,
    pub first_relay: String,
    #[serde(default)]
    pub extra_relays: Vec<String>,
    pub exit: ExitDraft,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ExitDraft {
    Existing { name: String },
    New { name: String, server: String, port: u16, username: String, password: String },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub region: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorOptions {
    pub target: String,
    pub relays: Vec<Choice>,
    pub nodes: Vec<Choice>,
    pub exits: Vec<Choice>,
    pub default_relay: Option<String>,
    pub default_extras: Vec<String>,
    pub default_exit: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub id: String,
    pub target: String,
    pub route: Vec<String>,
    pub exit_node: String,
    pub script_preview: String,
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

struct Guard { path: PathBuf, hash: Option<[u8; 32]> }
struct Change { path: PathBuf, bytes: Vec<u8>, existed: bool }
pub struct Prepared {
    id: String,
    target: String,
    exit_node: String,
    guards: Vec<Guard>,
    changes: Vec<Change>,
    probe: Option<ProbePlan>,
}
struct ProbePlan { nodes: Vec<Value>, expected_ip: String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult { pub state: String, pub ip: Option<String>, pub expected_ip: String, pub message: String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub target: String,
    pub exit_node: String,
    pub backups: Vec<String>,
    pub next_step: String,
    pub probe: Option<ProbeResult>,
}

fn key(k: &str) -> Value { Value::String(k.to_owned()) }
fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> { v.as_mapping()?.get(key(k)) }
fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> { get(v, k)?.as_str() }
fn list<'a>(v: Option<&'a Value>) -> &'a [Value] { v.and_then(Value::as_sequence).map(Vec::as_slice).unwrap_or(&[]) }
fn mapping(v: &mut Value) -> Result<&mut Mapping, String> { v.as_mapping_mut().ok_or_else(|| "配置格式无效".into()) }
fn ensure_list<'a>(v: &'a mut Value, k: &str) -> Result<&'a mut Vec<Value>, String> {
    let map = mapping(v)?;
    let item = map.entry(key(k)).or_insert_with(|| Value::Sequence(Vec::new()));
    if item.is_null() { *item = Value::Sequence(Vec::new()); }
    item.as_sequence_mut().ok_or_else(|| format!("配置中的 {k} 不是列表"))
}
fn parse_yaml(raw: &[u8]) -> Result<Value, String> {
    serde_yaml::from_slice(raw).map_err(|_| "配置文件无法解析，请先在原客户端修复".into())
}
fn yaml_bytes(value: &Value) -> Result<Vec<u8>, String> {
    serde_yaml::to_string(value).map(String::into_bytes).map_err(|_| "无法生成配置".into())
}
fn read(path: &Path) -> Result<Vec<u8>, String> { fs::read(path).map_err(|_| "无法读取客户端配置".into()) }
fn hash(bytes: &[u8]) -> [u8; 32] { Sha256::digest(bytes).into() }
fn bytes_at(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) { Ok(v) => Ok(Some(v)), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None), Err(_) => Err("无法读取客户端配置".into()) }
}
fn guard(guards: &mut Vec<Guard>, path: &Path, bytes: Option<&[u8]>) {
    if !guards.iter().any(|g| g.path == path) {
        guards.push(Guard { path: path.to_path_buf(), hash: bytes.map(hash) });
    }
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 180 && !name.chars().any(char::is_control)
}
fn label(name: &str) -> String {
    if name.starts_with("__hopway_") { name.split_once("::").map(|(_, s)| s).unwrap_or(name).to_owned() } else { name.to_owned() }
}
fn managed(name: &str) -> bool { name.starts_with("__hopway_") && name.contains("::") }
fn runtime_path(root: &Path) -> PathBuf { root.join("clash-verge.yaml") }
fn client_root(target: &str) -> Result<PathBuf, String> {
    if target != "verge" { return Err("仅支持 Clash Verge".into()); }
    let base = PathBuf::from(env::var_os("APPDATA").ok_or("找不到客户端数据目录")?);
    Ok(base.join("io.github.clash-verge-rev.clash-verge-rev"))
}
fn named<'a>(doc: &'a Value, section: &str, name: &str) -> Option<&'a Value> {
    list(get(doc, section)).iter().find(|item| str_field(item, "name") == Some(name))
}
fn names_unique(doc: &Value) -> Result<(), String> {
    let mut names = HashSet::new();
    for item in list(get(doc, "proxies")).iter().chain(list(get(doc, "proxy-groups"))) {
        let name = str_field(item, "name").ok_or("节点或策略组缺少名称")?;
        if !valid_name(name) || !names.insert(name) { return Err("客户端配置中存在无效或重复的节点名称".into()); }
    }
    Ok(())
}

pub fn editor_options(target: &str) -> Result<EditorOptions, String> {
    let root = client_root(target)?;
    editor_options_from(target, &root)
}
fn editor_options_from(target: &str, root: &Path) -> Result<EditorOptions, String> {
    let doc = parse_yaml(&read(&runtime_path(root))?)?;
    names_unique(&doc)?;
    let mut relays = Vec::new();
    let mut nodes = Vec::new();
    let mut exits = Vec::new();
    for group in list(get(&doc, "proxy-groups")) {
        let Some(name) = str_field(group, "name") else { continue };
        if name == STATIC_GROUP { continue; }
        let kind = str_field(group, "type").unwrap_or("");
        if !["select", "url-test", "fallback", "load-balance"].contains(&kind) { continue; }
        relays.push(Choice { name: name.into(), label: name.into(), kind: "策略组".into(), region: None });
    }
    for proxy in list(get(&doc, "proxies")) {
        let Some(name) = str_field(proxy, "name") else { continue };
        let kind = str_field(proxy, "type").unwrap_or("");
        let item = Choice { name: name.into(), label: label(name), kind: kind.into(), region: super::region_from_node_name(name) };
        if kind == "socks5" { exits.push(item.clone()); }
        if !managed(name) {
            nodes.push(item.clone());
            relays.push(item);
        }
    }
    let target_name = list(get(&doc, "rules")).iter().filter_map(Value::as_str).find_map(|line| {
        let mut p = line.split(',').map(str::trim);
        match p.next() { Some("MATCH") | Some("FINAL") => p.next(), _ => None }
    });
    let default_exit = target_name.and_then(|group_name| named(&doc, "proxy-groups", group_name))
        .and_then(|group| list(get(group, "proxies")).iter().filter_map(Value::as_str)
            .find(|name| exits.iter().any(|item| item.name == *name)))
        .map(str::to_owned)
        .or_else(|| exits.iter().find(|n| n.name == "LS-ISP-01").map(|n| n.name.clone()));
    let mut first = default_exit.as_deref().and_then(|name| named(&doc, "proxies", name))
        .and_then(|n| str_field(n, "dialer-proxy"));
    let mut default_extras = Vec::new();
    for _ in 0..8 {
        let Some(name) = first else { break };
        if !managed(name) { break; }
        let original = label(name);
        default_extras.push(original);
        first = named(&doc, "proxies", name).and_then(|n| str_field(n, "dialer-proxy"));
    }
    default_extras.reverse();
    let default_relay = first.filter(|name| relays.iter().any(|n| n.name == *name)).map(str::to_owned);
    Ok(EditorOptions { target: target.into(), relays, nodes, exits, default_relay, default_extras, default_exit })
}

fn set_field(value: &mut Value, name: &str, content: Value) -> Result<(), String> {
    mapping(value)?.insert(key(name), content);
    Ok(())
}
fn validate_draft<'a>(draft: &'a Draft, runtime: &Value) -> Result<(Vec<Value>, Vec<String>, String), String> {
    if draft.target != "verge" { return Err("仅支持 Clash Verge".into()); }
    if !valid_name(&draft.first_relay) || draft.first_relay == STATIC_GROUP { return Err("请选择有效的首跳中继".into()); }
    if draft.extra_relays.len() > 3 { return Err("最多支持四个中继节点".into()); }
    if named(runtime, "proxies", &draft.first_relay).is_none() && named(runtime, "proxy-groups", &draft.first_relay).is_none() {
        return Err("首跳中继已不存在，请刷新节点列表".into());
    }
    let mut seen = HashSet::new();
    seen.insert(draft.first_relay.as_str());
    let mut route = vec![label(&draft.first_relay)];
    let mut chain = Vec::new();
    let mut previous = draft.first_relay.clone();
    let nonce = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    for (index, name) in draft.extra_relays.iter().enumerate() {
        if !valid_name(name) || managed(name) || !seen.insert(name) { return Err("中继节点重复或无效".into()); }
        let mut node = named(runtime, "proxies", name).cloned().ok_or("后续中继必须选择现有具体节点")?;
        let copy_name = format!("__hopway_{nonce}_r{}::{name}", index + 1);
        set_field(&mut node, "name", Value::String(copy_name.clone()))?;
        set_field(&mut node, "dialer-proxy", Value::String(previous))?;
        previous = copy_name;
        chain.push(node);
        route.push(label(name));
    }
    let (mut exit, source_name, exit_label) = match &draft.exit {
        ExitDraft::Existing { name } => {
            if !valid_name(name) || seen.contains(name.as_str()) { return Err("静态出口不能与中继重复".into()); }
            let node = named(runtime, "proxies", name).cloned().ok_or("所选出口已不存在，请刷新节点列表")?;
            if str_field(&node, "type") != Some("socks5") { return Err("请选择 SOCKS5 静态出口".into()); }
            (node, Some(name.clone()), label(name))
        }
        ExitDraft::New { name, server, port, username, password } => {
            let address = server.trim();
            if !valid_name(name) || address.is_empty() || address.len() > 253 || address.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '@')) || *port == 0 || username.is_empty() || password.is_empty() {
                return Err("请完整填写出口名称、服务器、端口和凭证".into());
            }
            if named(runtime, "proxies", name).is_some() || named(runtime, "proxy-groups", name).is_some() {
                return Err("出口名称已存在，请换一个名称".into());
            }
            let mut m = Mapping::new();
            m.insert(key("name"), Value::String(name.clone()));
            m.insert(key("type"), Value::String("socks5".into()));
            m.insert(key("server"), Value::String(address.into()));
            m.insert(key("port"), Value::Number((*port).into()));
            m.insert(key("username"), Value::String(username.clone()));
            m.insert(key("password"), Value::String(password.clone()));
            m.insert(key("udp"), Value::Bool(false));
            (Value::Mapping(m), None, name.clone())
        }
    };
    if let Some(source) = source_name.as_ref() {
        if first_reaches(runtime, &draft.first_relay, source)? { return Err("所选中继可能再次经过出口，形成循环；请选择其他中继".into()); }
    }
    let exit_name = format!("__hopway_{nonce}_exit::{exit_label}");
    set_field(&mut exit, "name", Value::String(exit_name.clone()))?;
    set_field(&mut exit, "dialer-proxy", Value::String(previous))?;
    chain.push(exit);
    route.push(exit_label);
    Ok((chain, route, exit_name))
}

fn first_reaches(doc: &Value, start: &str, target: &str) -> Result<bool, String> {
    fn visit(doc: &Value, current: &str, target: &str, stack: &mut HashSet<String>, done: &mut HashSet<String>) -> Result<bool, String> {
        if current == target { return Ok(true); }
        if ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE"].contains(&current) { return Ok(false); }
        if stack.contains(current) { return Err("中继依赖存在循环，请在原客户端检查策略组".into()); }
        if done.contains(current) { return Ok(false); }
        stack.insert(current.to_owned());
        if let Some(node) = named(doc, "proxies", current) {
            if let Some(next) = str_field(node, "dialer-proxy") {
                if visit(doc, next, target, stack, done)? { return Ok(true); }
            }
        } else if let Some(group) = named(doc, "proxy-groups", current) {
            let members = list(get(group, "proxies"));
            if get(group, "use").is_some() || ["include-all", "include-all-proxies", "include-all-providers"].iter().any(|k| get(group, k).and_then(Value::as_bool) == Some(true)) {
                return Err("无法确认该策略组的全部成员，请先在客户端加载订阅或选择具体节点".into());
            }
            for member in members.iter().filter_map(Value::as_str) {
                if visit(doc, member, target, stack, done)? { return Ok(true); }
            }
        } else { return Err("中继引用了不存在的节点或策略组".into()); }
        stack.remove(current);
        done.insert(current.to_owned());
        Ok(false)
    }
    visit(doc, start, target, &mut HashSet::new(), &mut HashSet::new())
}

fn add_nodes(doc: &mut Value, nodes: &[Value]) -> Result<(), String> {
    let names: HashSet<String> = list(get(doc, "proxies")).iter().filter_map(|v| str_field(v, "name").map(str::to_owned)).collect();
    let list = ensure_list(doc, "proxies")?;
    for node in nodes {
        let name = str_field(node, "name").ok_or("节点缺少名称")?;
        if names.contains(name) { return Err("生成的节点名称冲突，请重试".into()); }
        list.push(node.clone());
    }
    Ok(())
}

fn set_static_group(doc: &mut Value, exit_name: &str) -> Result<(), String> {
    let groups = ensure_list(doc, "proxy-groups")?;
    let pos = groups.iter().position(|g| str_field(g, "name") == Some(STATIC_GROUP));
    let group = if let Some(pos) = pos { &mut groups[pos] } else {
        let mut m = Mapping::new();
        m.insert(key("name"), Value::String(STATIC_GROUP.into()));
        groups.push(Value::Mapping(m));
        groups.last_mut().unwrap()
    };
    let m = mapping(group)?;
    m.insert(key("type"), Value::String("select".into()));
    m.insert(key("proxies"), Value::Sequence(vec![Value::String(exit_name.into()), Value::String("DIRECT".into())]));
    for field in ["use", "include-all", "include-all-proxies", "include-all-providers", "filter", "exclude-filter", "exclude-type"] { m.remove(key(field)); }
    Ok(())
}

fn escaped_regex(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if ".+*?()|[]{}^$\\".contains(c) { out.push('\\'); }
        out.push(c);
    }
    out
}
fn filter_managed_from_groups(doc: &mut Value, protected: &[String]) -> Result<(), String> {
    for group in ensure_list(doc, "proxy-groups")? {
        if str_field(group, "name") == Some(STATIC_GROUP) { continue; }
        let map = mapping(group)?;
        if let Some(Value::Sequence(members)) = map.get_mut(key("proxies")) {
            members.retain(|v| v.as_str().map(|s| !protected.iter().any(|n| n == s)).unwrap_or(true));
        }
        let dynamic = ["include-all", "include-all-proxies", "include-all-providers"].iter().any(|k| map.get(key(k)).and_then(Value::as_bool) == Some(true))
            || map.get(key("use")).is_some();
        if dynamic && !protected.is_empty() {
            let exclusion = protected.iter().map(|s| format!("^{}$", escaped_regex(s))).collect::<Vec<_>>().join("|");
            let merged = map.get(key("exclude-filter")).and_then(Value::as_str).filter(|s| !s.is_empty())
                .map(|old| format!("({old})|({exclusion})")).unwrap_or(exclusion);
            map.insert(key("exclude-filter"), Value::String(merged));
        }
    }
    Ok(())
}

fn rewrite_rules(doc: &mut Value) -> Result<(), String> {
    let rules = ensure_list(doc, "rules")?;
    let mut output = Vec::new();
    for rule in rules.iter() {
        let Some(line) = rule.as_str() else { return Err("规则格式不受支持".into()) };
        let mut parts: Vec<String> = line.split(',').map(|s| s.trim().to_owned()).collect();
        if parts.first().map(|s| s.eq_ignore_ascii_case("MATCH") || s.eq_ignore_ascii_case("FINAL")).unwrap_or(false) { continue; }
        if parts.len() < 2 { return Err("规则格式不受支持".into()); }
        let last = parts.len() - 1;
        let idx = if parts[last].eq_ignore_ascii_case("no-resolve") && last > 0 { last - 1 } else { last };
        if !["DIRECT", "REJECT", "REJECT-DROP", "PASS", STATIC_GROUP].contains(&parts[idx].as_str()) { parts[idx] = STATIC_GROUP.into(); }
        output.push(Value::String(parts.join(",")));
    }
    output.push(Value::String(format!("MATCH,{STATIC_GROUP}")));
    *rules = output;
    Ok(())
}

fn write_stage(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|_| "无法创建暂存目录")?; }
    fs::write(path, bytes).map_err(|_| "无法写入暂存配置".into())
}
fn path_leaf(value: &str) -> Result<&str, String> {
    let path = Path::new(value);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !value.contains(':') && !value.contains('/') && !value.contains('\\') => Ok(value),
        _ => Err("客户端配置包含不安全的文件名".into()),
    }
}
fn run_stage(root: &Path, action: &str, proxy_name: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let windows = PathBuf::from(env::var_os("SystemRoot").ok_or("无法启动配置组件")?);
    let powershell = windows.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let request = serde_json::json!({
        "root": root.to_string_lossy(), "action": action, "proxyName": proxy_name,
        "staticGroup": STATIC_GROUP
    });
    let mut child = Command::new(powershell)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root.join("hopway-stage.ps1"))
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(0x08000000).spawn().map_err(|_| "无法启动配置组件")?;
    let input = serde_json::to_vec(&request).map_err(|_| "无法准备配置")?;
    if child.stdin.take().ok_or("无法启动配置组件")?.write_all(&input).is_err() {
        let _ = child.kill(); let _ = child.wait();
        return Err("暂存配置失败".into());
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err("暂存配置失败，请检查客户端当前配置和增强层".into()),
            Ok(None) if start.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => { let _ = child.kill(); let _ = child.wait(); return Err("暂存配置超时".into()); }
        }
    }
}

struct Layer { kind: &'static str, path: PathBuf, stage: PathBuf, original: Option<Vec<u8>> }
fn verge_layers(root: &Path, stage: &Path, guards: &mut Vec<Guard>) -> Result<(Vec<Layer>, Option<Change>), String> {
    let index_path = root.join("profiles.yaml");
    let index_bytes = read(&index_path)?;
    guard(guards, &index_path, Some(&index_bytes));
    let mut index = parse_yaml(&index_bytes)?;
    let current = str_field(&index, "current").ok_or("请先在 Clash Verge 选择配置")?.to_owned();
    let items = get(&index, "items").and_then(Value::as_sequence).ok_or("Clash Verge 订阅列表格式无效")?;
    let current_idx = items.iter().position(|i| str_field(i, "uid") == Some(&current)).ok_or("找不到当前 Clash Verge 配置")?;
    let mut options = get(&items[current_idx], "option").and_then(Value::as_mapping).cloned().unwrap_or_default();
    let base_file = str_field(&items[current_idx], "file").map(str::to_owned);
    let mut new_entries = Vec::new();
    let mut descriptors = Vec::new();
    let mut changed_index = false;
    for kind in ["rules", "groups", "proxies", "script"] {
        let uid = options.get(key(kind)).and_then(Value::as_str).map(str::to_owned);
        let existing = uid.as_ref().and_then(|u| items.iter().find(|i| str_field(i, "uid") == Some(u)));
        let file = if let Some(entry) = existing {
            if str_field(entry, "type") != Some(kind) { return Err("增强层类型不匹配，请先在 Clash Verge 检查配置".into()); }
            path_leaf(str_field(entry, "file").ok_or("增强层缺少文件名")?)?.to_owned()
        } else if uid.is_some() {
            return Err("当前配置引用了不存在的增强层，请先在 Clash Verge 修复".into());
        } else {
            let prefix = match kind { "rules" => "r", "groups" => "g", "proxies" => "p", _ => "s" };
            let random = uuid::Uuid::new_v4().simple().to_string();
            let new_uid = format!("{prefix}{}", &random[..12]);
            let filename = format!("{new_uid}.{}", if kind == "script" { "js" } else { "yaml" });
            options.insert(key(kind), Value::String(new_uid.clone()));
            let mut entry = Mapping::new();
            entry.insert(key("uid"), Value::String(new_uid));
            entry.insert(key("type"), Value::String(kind.into()));
            entry.insert(key("name"), Value::String(format!("HopWay {kind}")));
            entry.insert(key("file"), Value::String(filename.clone()));
            entry.insert(key("updated"), Value::Number((chrono::Utc::now().timestamp()).into()));
            new_entries.push(Value::Mapping(entry));
            changed_index = true;
            filename
        };
        let path = root.join("profiles").join(&file);
        let stage_path = stage.join("verge").join("profiles").join(&file);
        let original = bytes_at(&path)?;
        guard(guards, &path, original.as_deref());
        let fallback = if kind == "script" { b"function main(config) { return config; }\n".as_slice() } else { b"prepend: []\nappend: []\ndelete: []\n".as_slice() };
        write_stage(&stage_path, original.as_deref().unwrap_or(fallback))?;
        descriptors.push(Layer { kind, path, stage: stage_path, original });
    }
    if let Some(file) = base_file {
        let file = path_leaf(&file)?;
        let path = root.join("profiles").join(file);
        if let Some(bytes) = bytes_at(&path)? {
            guard(guards, &path, Some(&bytes));
            write_stage(&stage.join("verge").join("profiles").join(file), &bytes)?;
        }
    }
    if changed_index {
        let items = mapping(&mut index)?.get_mut(key("items")).and_then(Value::as_sequence_mut).ok_or("订阅列表格式无效")?;
        mapping(&mut items[current_idx])?.insert(key("option"), Value::Mapping(options));
        items.extend(new_entries);
    }
    let staged_index = if changed_index { yaml_bytes(&index)? } else { index_bytes.clone() };
    write_stage(&stage.join("verge").join("profiles.yaml"), &staged_index)?;
    let index_change = changed_index.then(|| Change { path: index_path, bytes: staged_index, existed: true });
    Ok((descriptors, index_change))
}

fn layer_doc(layer: &Layer) -> Result<Value, String> { parse_yaml(&read(&layer.stage)?) }
fn stage_layer_nodes(layer: &Layer, chain: &[Value]) -> Result<(), String> {
    let mut doc = layer_doc(layer)?;
    let existing: HashSet<String> = ["prepend", "append"].iter().flat_map(|k| list(get(&doc, k)).iter())
        .filter_map(|v| str_field(v, "name").map(str::to_owned)).collect();
    let prepend = ensure_list(&mut doc, "prepend")?;
    for node in chain.iter().rev() {
        if existing.contains(str_field(node, "name").ok_or("节点缺少名称")?) { return Err("节点名称冲突，请重试".into()); }
        prepend.insert(0, node.clone());
    }
    write_stage(&layer.stage, &yaml_bytes(&doc)?)
}
fn set_layer_group(layer: &Layer, exit_name: &str) -> Result<(), String> {
    let mut doc = layer_doc(layer)?;
    let count = ["prepend", "append"].iter().flat_map(|k| list(get(&doc, k)).iter())
        .filter(|g| str_field(g, "name") == Some(STATIC_GROUP)).count();
    if count > 1 { return Err("静态出口策略组重复，请先在 Clash Verge 修复".into()); }
    let section = if list(get(&doc, "prepend")).iter().any(|g| str_field(g, "name") == Some(STATIC_GROUP)) { "prepend" }
        else if list(get(&doc, "append")).iter().any(|g| str_field(g, "name") == Some(STATIC_GROUP)) { "append" } else { "prepend" };
    let groups = ensure_list(&mut doc, section)?;
    let idx = if let Some(idx) = groups.iter().position(|g| str_field(g, "name") == Some(STATIC_GROUP)) { idx } else {
        let mut item = Mapping::new(); item.insert(key("name"), Value::String(STATIC_GROUP.into()));
        groups.insert(0, Value::Mapping(item)); 0
    };
    let group = mapping(&mut groups[idx])?;
    group.insert(key("type"), Value::String("select".into()));
    group.insert(key("proxies"), Value::Sequence(vec![Value::String(exit_name.into()), Value::String("DIRECT".into())]));
    for field in ["use", "include-all", "include-all-proxies", "include-all-providers", "filter", "exclude-filter", "exclude-type"] { group.remove(key(field)); }
    write_stage(&layer.stage, &yaml_bytes(&doc)?)
}

fn wrap_verge_script(source: &[u8], protected: &[String], exit_name: &str) -> Result<Vec<u8>, String> {
    let mut original = String::from_utf8(source.to_vec()).map_err(|_| "扩展脚本不是 UTF-8")?;
    match (original.find(WRAPPER_BEGIN), original.find(WRAPPER_END)) {
        (Some(start), Some(end)) if end >= start => { original.replace_range(start..end + WRAPPER_END.len(), ""); }
        (None, None) => (),
        _ => return Err("HopWay 扩展脚本标记不完整，请先在 Clash Verge 检查".into()),
    }
    if !original.contains("function main") || original.contains("__hopway_previous_main") {
        return Err("当前扩展脚本的入口无法安全保留，请先在 Clash Verge 检查".into());
    }
    let names = serde_json::to_string(protected).map_err(|_| "无法生成脚本")?;
    let exit = serde_json::to_string(exit_name).map_err(|_| "无法生成脚本")?;
    let pattern = protected.iter().map(|s| format!("^{}$", escaped_regex(s))).collect::<Vec<_>>().join("|");
    let pattern = serde_json::to_string(&pattern).map_err(|_| "无法生成脚本")?;
    let template = r#"
var __hopway_previous_main = main;
main = function(config, profileName) {
  var c = __hopway_previous_main(config, profileName) || config;
  var blocked = __NAMES__;
  var excluded = __PATTERN__;
  var exit = __EXIT__;
  var keep = ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "静态ISP"];
  var rules = Array.isArray(c.rules) ? c.rules : [];
  c.rules = rules.filter(function(r) {
    return typeof r === "string" && !/^(MATCH|FINAL),/i.test(r.trim());
  }).map(function(r) {
    var p = r.split(",");
    var i = p.length - 1;
    if ((p[i] || "").trim().toLowerCase() === "no-resolve") i--;
    var target = (p[i] || "").trim();
    if (i >= 0 && target && keep.indexOf(target) < 0) p[i] = "静态ISP";
    return p.join(",");
  });
  c.rules.push("MATCH,静态ISP");
  if (!Array.isArray(c["proxy-groups"])) c["proxy-groups"] = [];
  var staticGroup = c["proxy-groups"].find(function(g) { return g && g.name === "静态ISP"; });
  if (!staticGroup) { staticGroup = { name: "静态ISP" }; c["proxy-groups"].push(staticGroup); }
  staticGroup.type = "select";
  staticGroup.proxies = [exit, "DIRECT"];
  ["use", "include-all", "include-all-proxies", "include-all-providers", "filter", "exclude-filter", "exclude-type"].forEach(function(k) { delete staticGroup[k]; });
  (c["proxy-groups"] || []).forEach(function(g) {
    if (!g || g.name === "静态ISP") return;
    if (Array.isArray(g.proxies)) g.proxies = g.proxies.filter(function(n) { return blocked.indexOf(n) < 0; });
    if (g["include-all"] || g["include-all-proxies"] || g["include-all-providers"] || (Array.isArray(g.use) && g.use.length)) {
      var old = g["exclude-filter"];
      g["exclude-filter"] = old ? "(" + old + ")|(" + excluded + ")" : excluded;
    }
  });
  return c;
};
"#;
    let (before_names, rest) = template.split_once("__NAMES__").unwrap();
    let (before_pattern, rest) = rest.split_once("__PATTERN__").unwrap();
    let (before_exit, after_exit) = rest.split_once("__EXIT__").unwrap();
    let wrapper = format!("{before_names}{names}{before_pattern}{pattern}{before_exit}{exit}{after_exit}");
    Ok(format!("{}\n{}\n{}\n{}\n", original.trim_end(), WRAPPER_BEGIN, wrapper.trim(), WRAPPER_END).into_bytes())
}

fn plan_verge(root: &Path, stage: &Path, runtime: &Value, chain: &[Value], exit_name: &str, guards: &mut Vec<Guard>) -> Result<(Value, Vec<Change>), String> {
    let (layers, index_change) = verge_layers(root, stage, guards)?;
    run_stage(stage, "repair-verge", exit_name)?;
    let proxies = layers.iter().find(|l| l.kind == "proxies").unwrap();
    stage_layer_nodes(proxies, chain)?;
    run_stage(stage, "sync-verge", exit_name)?;
    set_layer_group(layers.iter().find(|l| l.kind == "groups").unwrap(), exit_name)?;
    let mut candidate = runtime.clone();
    add_nodes(&mut candidate, chain)?;
    set_static_group(&mut candidate, exit_name)?;
    let protected: Vec<String> = list(get(&candidate, "proxies")).iter().filter_map(|p| str_field(p, "name")).filter(|n| managed(n)).map(str::to_owned).collect();
    filter_managed_from_groups(&mut candidate, &protected)?;
    let rules_layer = layer_doc(layers.iter().find(|l| l.kind == "rules").unwrap())?;
    let additional = list(get(&rules_layer, "prepend")).to_vec();
    let existing = ensure_list(&mut candidate, "rules")?;
    for rule in additional.into_iter().rev() { if !existing.contains(&rule) { existing.insert(0, rule); } }
    rewrite_rules(&mut candidate)?;
    names_unique(&candidate)?;
    let script = layers.iter().find(|l| l.kind == "script").unwrap();
    let wrapped = wrap_verge_script(&read(&script.stage)?, &protected, exit_name)?;
    write_stage(&script.stage, &wrapped)?;
    let mut changes = Vec::new();
    for layer in layers {
        let bytes = read(&layer.stage)?;
        if layer.original.as_deref() != Some(bytes.as_slice()) {
            changes.push(Change { path: layer.path, bytes, existed: layer.original.is_some() });
        }
    }
    // The profile index is committed last so newly created layers exist first.
    if let Some(index) = index_change { changes.push(index); }
    Ok((candidate, changes))
}

fn safe_relative(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err("提供者文件路径不受支持，无法安全离线校验".into());
    }
    Ok(path.to_path_buf())
}
fn prepare_offline_validation(doc: &mut Value, root: &Path, stage: &Path) -> Result<(), String> {
    let root_canonical = fs::canonicalize(root).map_err(|_| "无法读取客户端数据目录")?;
    for section in ["proxy-providers", "rule-providers"] {
        let Some(providers) = mapping(doc)?.get_mut(key(section)).and_then(Value::as_mapping_mut) else { continue };
        for provider in providers.values_mut() {
            let m = mapping(provider)?;
            let kind = m.get(key("type")).and_then(Value::as_str).unwrap_or("");
            if kind == "inline" { continue; }
            if !["http", "file"].contains(&kind) { return Err("存在无法安全离线校验的提供者".into()); }
            let relative = safe_relative(m.get(key("path")).and_then(Value::as_str).ok_or("提供者没有本地缓存路径")?)?;
            let source = root.join(&relative);
            let canonical = fs::canonicalize(&source).map_err(|_| "提供者缓存不存在，请先在客户端更新订阅")?;
            if !canonical.starts_with(&root_canonical) { return Err("提供者缓存路径不在客户端目录内".into()); }
            let bytes = read(&canonical)?;
            write_stage(&stage.join(&relative), &bytes)?;
            m.insert(key("type"), Value::String("file".into()));
            m.remove(key("url"));
            m.remove(key("interval"));
            if let Some(Value::Mapping(hc)) = m.get_mut(key("health-check")) { hc.insert(key("enable"), Value::Bool(false)); }
        }
    }
    for file in ["GEOIP.metadb", "Country.mmdb", "geoip.dat", "GEOIP.dat", "geosite.dat", "GEOSITE.dat", "GeoSite.dat", "ASN.mmdb"] {
        let source = root.join(file);
        if source.is_file() { fs::copy(source, stage.join(file)).map_err(|_| "无法准备离线校验数据")?; }
    }
    if !stage.join("Country.mmdb").exists() && stage.join("GEOIP.metadb").exists() {
        fs::copy(stage.join("GEOIP.metadb"), stage.join("Country.mmdb")).map_err(|_| "无法准备离线校验数据")?;
    }
    let rules = list(get(doc, "rules"));
    let needs_geoip = rules.iter().filter_map(Value::as_str).any(|r| r.to_ascii_uppercase().starts_with("GEOIP,"));
    let needs_geosite = rules.iter().filter_map(Value::as_str).any(|r| r.to_ascii_uppercase().starts_with("GEOSITE,"));
    if needs_geoip && !["Country.mmdb", "geoip.dat", "GEOIP.dat"].iter().any(|f| stage.join(f).exists()) { return Err("缺少本地 GEOIP 数据，无法离线校验".into()); }
    if needs_geosite && !["geosite.dat", "GEOSITE.dat", "GeoSite.dat"].iter().any(|f| stage.join(f).exists()) { return Err("缺少本地 GEOSITE 数据，无法离线校验".into()); }
    let map = mapping(doc)?;
    map.insert(key("geo-auto-update"), Value::Bool(false));
    for name in ["external-controller", "external-controller-pipe", "external-controller-unix", "external-ui-url"] { map.remove(key(name)); }
    Ok(())
}

fn find_core() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    for var in ["ProgramFiles", "LOCALAPPDATA"] {
        if let Some(root) = env::var_os(var) {
            let root = PathBuf::from(root);
            if var == "ProgramFiles" { candidates.push(root.join("Clash Verge").join("verge-mihomo.exe")); }
            else { candidates.push(root.join("Programs").join("Clash Verge").join("verge-mihomo.exe")); }
        }
    }
    candidates.push(PathBuf::from(r"D:\Program Files\Clash Verge\verge-mihomo.exe"));
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| "没有找到 verge-mihomo 内核，无法安全校验配置".into())
}
fn validate_with_core(candidate: &Value, root: &Path, stage: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let core = find_core()?;
    let validation = stage.join("validation");
    fs::create_dir_all(&validation).map_err(|_| "无法创建校验目录")?;
    let mut doc = candidate.clone();
    prepare_offline_validation(&mut doc, root, &validation)?;
    let path = validation.join("config.yaml");
    write_stage(&path, &yaml_bytes(&doc)?)?;
    let mut child = Command::new(core).arg("-t").arg("-d").arg(&validation).arg("-f").arg(&path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(0x08000000).spawn().map_err(|_| "无法启动 mihomo 校验")?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err("mihomo 校验未通过，现有配置未修改".into()),
            Ok(None) if start.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => { let _ = child.kill(); let _ = child.wait(); return Err("mihomo 校验超时，现有配置未修改".into()); }
        }
    }
}

pub fn prepare(draft: Draft) -> Result<(Prepared, PlanPreview), String> {
    let root = client_root(&draft.target)?;
    prepare_for_root(draft, &root)
}
fn prepare_for_root(draft: Draft, root: &Path) -> Result<(Prepared, PlanPreview), String> {
    let runtime_path = runtime_path(root);
    let runtime_raw = read(&runtime_path)?;
    let runtime = parse_yaml(&runtime_raw)?;
    names_unique(&runtime)?;
    let (chain, route, exit_name) = validate_draft(&draft, &runtime)?;
    // Check the existing first hop even when the final exit is newly entered.
    let _ = first_reaches(&runtime, &draft.first_relay, &exit_name)?;
    prepare_built(&draft.target, root, &runtime_raw, &runtime, chain, route, exit_name, None, draft.extra_relays.len(), 0)
}

pub fn prepare_visual(target: &str, visual: super::workspace::VisualChain) -> Result<(Prepared, PlanPreview), String> {
    let root = client_root(target)?;
    prepare_visual_for_root(target, visual, &root)
}
pub fn guard_workspace(prepared: &mut Prepared, path: &Path, bytes: &[u8]) {
    guard(&mut prepared.guards, path, Some(bytes));
}
fn prepare_visual_for_root(target: &str, visual: super::workspace::VisualChain, root: &Path) -> Result<(Prepared, PlanPreview), String> {
    if target != "verge" { return Err("仅支持 Clash Verge".into()); }
    let runtime_raw = read(&runtime_path(root))?;
    let runtime = parse_yaml(&runtime_raw)?;
    names_unique(&runtime)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let mut nodes = Vec::new();
    let mut previous: Option<String> = None;
    let mut route = Vec::new();
    for (index, (display, mut node)) in visual.relays.into_iter().enumerate() {
        let display: String = display.chars().take(130).collect();
        let node_label: String = str_field(&node, "name").ok_or("订阅节点缺少名称")?.chars().take(130).collect();
        let name = format!("__hopway_{nonce}_r{index}::{node_label}");
        set_field(&mut node, "name", Value::String(name.clone()))?;
        mapping(&mut node)?.remove(key("dialer-proxy"));
        if let Some(parent) = previous { set_field(&mut node, "dialer-proxy", Value::String(parent))?; }
        previous = Some(name);
        nodes.push(node);
        route.push(display);
    }
    if nodes.is_empty() { return Err("请先连接 VPN 订阅".into()); }
    let exit_label: String = visual.exit.name.chars().take(130).collect();
    let exit_name = format!("__hopway_{nonce}_exit::{exit_label}");
    let mut fields = Mapping::new();
    fields.insert(key("name"), Value::String(exit_name.clone()));
    fields.insert(key("type"), Value::String("socks5".into()));
    fields.insert(key("server"), Value::String(visual.exit.server.clone()));
    fields.insert(key("port"), Value::Number(visual.exit.port.into()));
    fields.insert(key("username"), Value::String(visual.exit.username));
    fields.insert(key("password"), Value::String(visual.exit.password));
    fields.insert(key("udp"), Value::Bool(false));
    fields.insert(key("dialer-proxy"), Value::String(previous.ok_or("请先连接 VPN 订阅")?));
    nodes.push(Value::Mapping(fields));
    route.push(exit_label);
    let count = nodes.len() - 1;
    let probe = Some(ProbePlan { nodes: nodes.clone(), expected_ip: visual.exit.server });
    prepare_built(target, root, &runtime_raw, &runtime, nodes, route, exit_name, probe, count, visual.unused_count)
}

fn redacted_script(nodes: &[Value], exit_name: &str) -> Result<String, String> {
    let mut doc = Mapping::new();
    let mut proxies = Vec::new();
    for node in nodes {
        let mut item = Mapping::new();
        for field in ["name", "type", "dialer-proxy"] { if let Some(value) = get(node, field) { item.insert(key(field), value.clone()); } }
        proxies.push(Value::Mapping(item));
    }
    doc.insert(key("proxies"), Value::Sequence(proxies));
    let mut group = Mapping::new();
    group.insert(key("name"), Value::String(STATIC_GROUP.into()));
    group.insert(key("type"), Value::String("select".into()));
    group.insert(key("proxies"), Value::Sequence(vec![Value::String(exit_name.into()), Value::String("DIRECT".into())]));
    doc.insert(key("proxy-groups"), Value::Sequence(vec![Value::Mapping(group)]));
    doc.insert(key("rules"), Value::Sequence(vec![Value::String(format!("MATCH,{STATIC_GROUP}"))]));
    serde_yaml::to_string(&Value::Mapping(doc)).map_err(|_| "无法生成配置预览".into())
}

fn prepare_built(target: &str, root: &Path, runtime_raw: &[u8], runtime: &Value, chain: Vec<Value>, route: Vec<String>, exit_name: String, probe: Option<ProbePlan>, relay_count: usize, unused_count: usize) -> Result<(Prepared, PlanPreview), String> {
    if target != "verge" { return Err("仅支持 Clash Verge".into()); }
    let temp = tempfile::tempdir().map_err(|_| "无法创建暂存目录")?;
    write_stage(&temp.path().join("sync-proxy.ps1"), SYNC_SCRIPT.as_bytes())?;
    write_stage(&temp.path().join("hopway-stage.ps1"), STAGE_SCRIPT.as_bytes())?;
    let mut guards = Vec::new();
    guard(&mut guards, &runtime_path(root), Some(runtime_raw));
    let (candidate, changes) = plan_verge(root, temp.path(), runtime, &chain, &exit_name, &mut guards)?;
    if changes.is_empty() { return Err("没有需要写入的配置".into()); }
    validate_with_core(&candidate, root, temp.path())?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut warnings = vec!["静态出口仅作为策略组候选，需在客户端确认该组已选中它".into()];
    warnings.push("候选基于当前运行配置校验；写入后需在 Clash Verge 重启内核并复核出口".into());
    if relay_count > 0 { warnings.push("中继使用当前节点的配置副本；订阅更新后需重新下发".into()); }
    if unused_count > 0 { warnings.push(format!("有 {unused_count} 个未连接模块不会写入此次链路")); }
    let files = vec!["规则增强".into(), "节点增强".into(), "策略组增强".into(), "扩展脚本".into(), "订阅绑定（如需）".into()];
    let script_preview = redacted_script(&chain, &exit_name)?;
    let preview = PlanPreview { id: id.clone(), target: target.into(), route, exit_node: exit_name.clone(), script_preview, files, warnings };
    Ok((Prepared { id, target: target.into(), exit_node: exit_name, guards, changes, probe }, preview))
}

fn check_guards(guards: &[Guard]) -> Result<(), String> {
    for guard in guards {
        if bytes_at(&guard.path)?.as_deref().map(hash) != guard.hash {
            return Err("客户端配置在预览后发生变化，请重新校验".into());
        }
    }
    Ok(())
}
fn fresh_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|_| "无法创建备份或暂存文件")?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("无法保存备份或暂存文件".into());
    }
    Ok(())
}
fn backup_path(path: &Path, stamp: &str, suffix: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".bak-{stamp}"));
    if suffix > 0 { name.push(format!("-{suffix:02}")); }
    PathBuf::from(name)
}
fn create_backup(path: &Path, stamp: &str) -> Result<PathBuf, String> {
    let bytes = read(path)?;
    for suffix in 0..100 {
        let backup = backup_path(path, stamp, suffix);
        match fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
            Ok(mut f) => {
                if f.write_all(&bytes).and_then(|_| f.sync_all()).is_err() { let _ = fs::remove_file(&backup); return Err("备份失败，现有配置未修改".into()); }
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("备份失败，现有配置未修改".into()),
        }
    }
    Err("无法创建不重名的备份".into())
}
fn adjacent_temp(path: &Path, tag: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".hopway-{tag}-{}", uuid::Uuid::new_v4().simple()));
    PathBuf::from(name)
}
fn replace_file(source: &Path, target: &Path, existed: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH};
    let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        if existed { ReplaceFileW(to.as_ptr(), from.as_ptr(), std::ptr::null(), 0, std::ptr::null(), std::ptr::null()) }
        else { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) }
    };
    if ok == 0 {
        #[cfg(test)]
        eprintln!("replace_file failed: {}", std::io::Error::last_os_error());
        Err("写入失败，请确认客户端文件没有被占用".into())
    } else { Ok(()) }
}

// Run a short-lived, isolated core with only the generated chain. This checks
// the new chain without pushing configuration into the user's running client.
fn probe_chain(plan: ProbePlan) -> ProbeResult {
    let expected = plan.expected_ip.clone();
    match run_probe_chain(&plan) {
        Ok(values) => {
            let first = values.first().cloned();
            let normalized = expected.parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string()).unwrap_or_else(|| expected.clone());
            let passed = !values.is_empty() && values.iter().all(|ip| ip == &normalized);
            ProbeResult {
                state: if passed { "ok" } else { "mismatch" }.into(), ip: first, expected_ip: expected,
                message: if passed { "临时内核出口与静态 IP 一致" } else { "临时内核出口与静态 IP 不一致" }.into(),
            }
        }
        Err(message) => ProbeResult { state: "failed".into(), ip: None, expected_ip: expected, message: format!("临时内核：{message}") },
    }
}
fn run_probe_chain(plan: &ProbePlan) -> Result<Vec<String>, String> {
    run_probe_chain_at(plan, "https://api.ipify.org")
}
fn run_probe_chain_at(plan: &ProbePlan, probe_url: &str) -> Result<Vec<String>, String> {
    use std::{net::{IpAddr, TcpListener, TcpStream}, os::windows::process::CommandExt};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let expected: IpAddr = plan.expected_ip.parse().map_err(|_| "静态出口 IP 无效")?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "无法为出口校验分配端口")?;
    let port = listener.local_addr().map_err(|_| "无法为出口校验分配端口")?.port();
    drop(listener);
    let temp = tempfile::tempdir().map_err(|_| "无法创建出口校验目录")?;
    let mut doc = Value::Mapping(Mapping::new());
    set_field(&mut doc, "mixed-port", Value::Number(port.into()))?;
    set_field(&mut doc, "allow-lan", Value::Bool(false))?;
    set_field(&mut doc, "mode", Value::String("rule".into()))?;
    set_field(&mut doc, "log-level", Value::String("silent".into()))?;
    set_field(&mut doc, "proxies", Value::Sequence(plan.nodes.clone()))?;
    let exit = str_field(plan.nodes.last().ok_or("出口节点不存在")?, "name").ok_or("出口节点不存在")?;
    set_static_group(&mut doc, exit)?;
    set_field(&mut doc, "rules", Value::Sequence(vec![Value::String(format!("MATCH,{STATIC_GROUP}"))]))?;
    let path = temp.path().join("probe.yaml");
    write_stage(&path, &yaml_bytes(&doc)?)?;
    let child = Command::new(find_core()?).arg("-d").arg(temp.path()).arg("-f").arg(&path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x08000000)
        .spawn().map_err(|_| "无法启动临时内核校验出口")?;
    let mut child = ChildGuard(child);
    let address = format!("127.0.0.1:{port}");
    let start = Instant::now();
    loop {
        if child.0.try_wait().map_err(|_| "临时内核启动失败")?.is_some() { return Err("临时内核启动失败".into()) }
        if TcpStream::connect_timeout(&address.parse().map_err(|_| "出口校验地址无效")?, Duration::from_millis(200)).is_ok() { break; }
        if start.elapsed() > Duration::from_secs(8) { return Err("临时内核启动超时".into()) }
        std::thread::sleep(Duration::from_millis(100));
    }
    let curl = PathBuf::from(env::var_os("SystemRoot").ok_or("无法校验出口")?).join("System32").join("curl.exe");
    let mut values = Vec::new();
    for _ in 0..3 {
        let out = Command::new(&curl).args(["--silent", "--fail", "--noproxy", "", "--connect-timeout", "5", "--max-time", "12", "--proxy", &format!("http://{address}"), probe_url])
            .stdin(Stdio::null()).stderr(Stdio::null()).creation_flags(0x08000000).output().map_err(|_| "出口 IP 探测失败")?;
        if !out.status.success() {
            return Err("出口 IP 探测失败，请检查中继和静态节点".into());
        }
        let parsed = String::from_utf8(out.stdout).ok().and_then(|v| v.trim().parse::<IpAddr>().ok()).ok_or("出口 IP 响应无效")?;
        values.push(parsed.to_string());
    }
    let _ = expected;
    Ok(values)
}
pub fn apply(prepared: Prepared, id: &str) -> Result<ApplyResult, String> {
    if prepared.id != id { return Err("预览已失效，请重新校验".into()); }
    check_guards(&prepared.guards)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut backups: Vec<Option<PathBuf>> = Vec::new();
    for change in &prepared.changes {
        if change.existed { backups.push(Some(create_backup(&change.path, &stamp)?)); }
        else { backups.push(None); }
    }
    // Detect edits that occurred while backups were being made.
    check_guards(&prepared.guards)?;
    let mut temps = Vec::new();
    for change in &prepared.changes {
        if let Some(parent) = change.path.parent() {
            if !parent.is_dir() {
                for p in &temps { let _ = fs::remove_file(p); }
                return Err("客户端配置目录不存在，现有配置未修改".into());
            }
        }
        let temp = adjacent_temp(&change.path, "new");
        if let Err(e) = fresh_file(&temp, &change.bytes) {
            for p in &temps { let _ = fs::remove_file(p); }
            return Err(e);
        }
        temps.push(temp);
    }
    if let Err(e) = check_guards(&prepared.guards) {
        for p in &temps { let _ = fs::remove_file(p); }
        return Err(e);
    }
    let mut committed = 0usize;
    for (change, temp) in prepared.changes.iter().zip(&temps) {
        if let Err(error) = replace_file(temp, &change.path, change.existed) {
            let mut rollback_failed = false;
            for idx in (0..committed).rev() {
                let item = &prepared.changes[idx];
                if let Some(backup) = &backups[idx] {
                    let restore = adjacent_temp(&item.path, "restore");
                    let restored = read(backup).and_then(|bytes| fresh_file(&restore, &bytes)).and_then(|_| replace_file(&restore, &item.path, true));
                    if restored.is_err() { rollback_failed = true; let _ = fs::remove_file(&restore); }
                } else if fs::remove_file(&item.path).is_err() { rollback_failed = true; }
            }
            for p in &temps { let _ = fs::remove_file(p); }
            return Err(if rollback_failed { "写入中断，部分文件未能自动回滚；请用同目录的 .bak 文件恢复".into() } else { error });
        }
        committed += 1;
    }
    let names = backups.into_iter().flatten().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let next_step = "在 Clash Verge 重启内核，再检查出口。";
    let probe = prepared.probe.map(probe_chain);
    Ok(ApplyResult { target: prepared.target, exit_node: prepared.exit_node, backups: names, next_step: next_step.into(), probe })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"
mode: rule
mixed-port: 7890
allow-lan: true
tun:
  enable: true
proxies:
  - name: Relay A
    type: socks5
    server: 192.0.2.1
    port: 1080
  - name: Relay B
    type: socks5
    server: 192.0.2.2
    port: 1080
  - name: Existing Exit
    type: socks5
    server: 192.0.2.3
    port: 1080
    username: test-user
    password: test-secret
    dialer-proxy: RelayGroup
proxy-groups:
  - name: RelayGroup
    type: select
    proxies: [Relay A, Relay B]
  - name: 静态ISP
    type: select
    proxies: [Existing Exit, DIRECT]
rules:
  - DOMAIN-SUFFIX,example.com,RelayGroup
  - DOMAIN-SUFFIX,example.com,DIRECT
  - PROCESS-NAME,app.exe,RelayGroup
  - PROCESS-NAME,editor.exe,DIRECT
  - MATCH,静态ISP
"#;
    fn draft(target: &str) -> Draft {
        Draft { target: target.into(), first_relay: "RelayGroup".into(), extra_relays: vec!["Relay B".into()], exit: ExitDraft::Existing { name: "Existing Exit".into() } }
    }
    fn verge_fixture(root: &Path, with_layers: bool) -> PathBuf {
        fs::create_dir_all(root.join("profiles")).unwrap();
        fs::write(root.join("clash-verge.yaml"), SAMPLE).unwrap();
        fs::write(root.join("profiles").join("base.yaml"), SAMPLE).unwrap();
        let index = if with_layers {
            "current: P1\nitems:\n- uid: P1\n  type: local\n  file: base.yaml\n  option:\n    rules: r1\n    groups: g1\n    proxies: p1\n    script: s1\n- uid: r1\n  type: rules\n  file: r1.yaml\n- uid: g1\n  type: groups\n  file: g1.yaml\n- uid: p1\n  type: proxies\n  file: p1.yaml\n- uid: s1\n  type: script\n  file: s1.js\n"
        } else {
            "current: P1\nitems:\n- uid: P1\n  type: local\n  file: base.yaml\n  option: {}\n"
        };
        let path = root.join("profiles.yaml");
        fs::write(&path, index).unwrap();
        if with_layers {
            for file in ["r1.yaml", "g1.yaml", "p1.yaml"] { fs::write(root.join("profiles").join(file), "prepend: []\nappend: []\ndelete: []\n").unwrap(); }
            fs::write(root.join("profiles").join("s1.js"), "function main(config) { return config; }\n").unwrap();
        }
        path
    }
    #[test]
    fn builds_three_hops_without_exposing_credentials() {
        let doc = parse_yaml(SAMPLE.as_bytes()).unwrap();
        let (nodes, route, exit) = validate_draft(&draft("verge"), &doc).unwrap();
        assert_eq!(route, ["RelayGroup", "Relay B", "Existing Exit"]);
        assert_eq!(str_field(&nodes[0], "dialer-proxy"), Some("RelayGroup"));
        assert_eq!(str_field(&nodes[1], "dialer-proxy"), str_field(&nodes[0], "name"));
        assert_eq!(str_field(&nodes[1], "name"), Some(exit.as_str()));
        assert!(!serde_json::to_string(&route).unwrap().contains("test-secret"));
        let duplicate = Draft { target: "verge".into(), first_relay: "Relay B".into(), extra_relays: vec!["Relay B".into()], exit: ExitDraft::Existing { name: "Existing Exit".into() } };
        assert!(validate_draft(&duplicate, &doc).is_err());
    }
    #[test]
    fn rewrites_proxy_rules_but_preserves_direct_and_tun() {
        let mut doc = parse_yaml(SAMPLE.as_bytes()).unwrap();
        let before = get(&doc, "tun").cloned();
        rewrite_rules(&mut doc).unwrap();
        let rules: Vec<&str> = list(get(&doc, "rules")).iter().filter_map(Value::as_str).collect();
        assert!(rules.contains(&"DOMAIN-SUFFIX,example.com,静态ISP"));
        assert!(rules.contains(&"DOMAIN-SUFFIX,example.com,DIRECT"));
        assert!(rules.contains(&"PROCESS-NAME,app.exe,静态ISP"));
        assert!(rules.contains(&"PROCESS-NAME,editor.exe,DIRECT"));
        assert_eq!(rules.last(), Some(&"MATCH,静态ISP"));
        assert_eq!(get(&doc, "tun"), before.as_ref());
    }
    #[test]
    fn preserves_user_script_and_replaces_only_own_wrapper() {
        let original = b"function main(config) { return config; }\n";
        let first = wrap_verge_script(original, &["__hopway_12345678_exit::test".into()], "__hopway_12345678_exit::test").unwrap();
        let next = wrap_verge_script(&first, &["__hopway_87654321_exit::other".into()], "__hopway_87654321_exit::other").unwrap();
        let output = String::from_utf8(next).unwrap();
        assert!(output.starts_with("function main(config) { return config; }"));
        assert_eq!(output.matches(WRAPPER_BEGIN).count(), 1);
        assert!(!output.contains("__hopway_12345678_exit::test"));
    }
    #[test]
    fn verge_wrapper_sets_final_group_and_rules() {
        let exit = "__hopway_test_exit::__PATTERN__";
        let script = String::from_utf8(wrap_verge_script(b"function main(c) { return c; }", &[exit.into()], exit).unwrap()).unwrap();
        let harness = format!(r#"{script}
const c = main({{ rules: ["PROCESS-NAME,app.exe,Airport", "DOMAIN,example.com,DIRECT", "MATCH,Airport"], "proxy-groups": [{{name:"Airport", type:"select", proxies:[{exit_json}, "DIRECT"]}}, {{name:"静态ISP", type:"select", proxies:["OLD"]}}] }});
if (c.rules.join("|") !== "PROCESS-NAME,app.exe,静态ISP|DOMAIN,example.com,DIRECT|MATCH,静态ISP") process.exit(1);
if (c["proxy-groups"][1].proxies[0] !== {exit_json}) process.exit(2);
if (c["proxy-groups"][0].proxies.length !== 1) process.exit(3);
"#, exit_json = serde_json::to_string(exit).unwrap());
        let status = Command::new("node").args(["-e", &harness]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if let Ok(status) = status { assert!(status.success()); }
    }
    #[test]
    fn rejects_stale_preview_before_any_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profile.yaml");
        fs::write(&path, b"original").unwrap();
        let prepared = Prepared { id: "id".into(), target: "verge".into(), exit_node: "exit".into(), guards: vec![Guard { path: path.clone(), hash: Some(hash(b"original")) }], changes: vec![Change { path: path.clone(), bytes: b"new".to_vec(), existed: true }], probe: None };
        fs::write(&path, b"changed externally").unwrap();
        assert!(apply(prepared, "id").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"changed externally");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
    #[test]
    fn verge_creates_missing_layers_only_after_validation() {
        if find_core().is_err() { return; }
        let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let index = verge_fixture(temp.path(), false);
        let before = fs::read(&index).unwrap();
        let (prepared, preview) = prepare_for_root(draft("verge"), temp.path()).unwrap();
        assert_eq!(fs::read(&index).unwrap(), before);
        assert_eq!(fs::read_dir(temp.path().join("profiles")).unwrap().count(), 1);
        let result = apply(prepared, &preview.id).unwrap();
        assert!(!result.backups.is_empty());
        let new_index = parse_yaml(&fs::read(index).unwrap()).unwrap();
        assert_eq!(list(get(&new_index, "items")).len(), 5);
        assert!(fs::read_dir(temp.path().join("profiles")).unwrap().count() >= 5);
    }
    #[test]
    fn verge_updates_existing_layers_and_preserves_script() {
        if find_core().is_err() { return; }
        let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let index = verge_fixture(temp.path(), true);
        let before_index = fs::read(&index).unwrap();
        let (prepared, preview) = prepare_for_root(draft("verge"), temp.path()).unwrap();
        assert_eq!(fs::read(&index).unwrap(), before_index);
        let result = apply(prepared, &preview.id).unwrap();
        assert!(result.backups.len() >= 3);
        assert_eq!(fs::read(&index).unwrap(), before_index);
        let script = fs::read_to_string(temp.path().join("profiles").join("s1.js")).unwrap();
        assert!(script.contains("function main(config) { return config; }"));
        assert!(script.contains(WRAPPER_BEGIN));
    }
    #[test]
    fn visual_subscriptions_form_a_real_validated_multihop_chain() {
        if find_core().is_err() { return; }
        let source = parse_yaml(SAMPLE.as_bytes()).unwrap();
        for target in ["verge"] {
            let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let path = verge_fixture(temp.path(), true);
            let before = fs::read(&path).unwrap();
            let chain = super::super::workspace::VisualChain {
                relays: vec![("机场一 · Relay A".into(), named(&source, "proxies", "Relay A").unwrap().clone()), ("机场二 · Relay B".into(), named(&source, "proxies", "Relay B").unwrap().clone())],
                exit: super::super::workspace::StaticNode { id: "exit-test".into(), name: "静态出口".into(), server: "192.0.2.8".into(), port: 1080, username: "private-user".into(), password: "private-password".into(), position: super::super::workspace::Point { x: 0.0, y: 0.0 } },
                unused_count: 0,
            };
            let (mut prepared, preview) = prepare_visual_for_root(target, chain, temp.path()).unwrap();
            assert_eq!(fs::read(&path).unwrap(), before);
            assert!(!preview.script_preview.contains("private-password"));
            assert!(!preview.script_preview.contains("192.0.2.8"));
            let probe = prepared.probe.as_ref().unwrap();
            assert_eq!(probe.nodes.len(), 3);
            assert!(str_field(&probe.nodes[0], "dialer-proxy").is_none());
            assert_eq!(str_field(&probe.nodes[1], "dialer-proxy"), str_field(&probe.nodes[0], "name"));
            assert_eq!(str_field(&probe.nodes[2], "dialer-proxy"), str_field(&probe.nodes[1], "name"));
            // Fixture addresses are deliberately unreachable; test the isolated
            // commit without attempting any real external egress.
            prepared.probe = None;
            assert!(!apply(prepared, &preview.id).unwrap().backups.is_empty());
        }
    }
    #[test]
    fn isolated_probe_sends_traffic_through_both_socks_hops() {
        use std::{io::{Read, Write}, net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream}, sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc}, time::Duration};
        if find_core().is_err() { return; }
        fn socks(auth: bool) -> (u16, Arc<AtomicBool>, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let stop = Arc::new(AtomicBool::new(false));
            let count = Arc::new(AtomicUsize::new(0));
            let stop_thread = stop.clone(); let count_thread = count.clone();
            let thread = std::thread::spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut client, _)) => {
                            let count = count_thread.clone();
                            std::thread::spawn(move || {
                                let result = (|| -> std::io::Result<()> {
                                    client.set_nonblocking(false)?;
                                    client.set_read_timeout(Some(Duration::from_secs(8)))?;
                                    let mut hello = [0u8; 2]; client.read_exact(&mut hello)?;
                                    let mut methods = vec![0; hello[1] as usize]; client.read_exact(&mut methods)?;
                                    let method = if auth { 2 } else { 0 };
                                    if hello[0] != 5 || !methods.contains(&method) { return Err(std::io::ErrorKind::InvalidData.into()); }
                                    client.write_all(&[5, method])?;
                                    if auth {
                                        let mut head = [0u8; 2]; client.read_exact(&mut head)?;
                                        let mut user = vec![0; head[1] as usize]; client.read_exact(&mut user)?;
                                        let mut n = [0u8; 1]; client.read_exact(&mut n)?;
                                        let mut pass = vec![0; n[0] as usize]; client.read_exact(&mut pass)?;
                                        if user != b"test-user" || pass != b"test-password" { return Err(std::io::ErrorKind::PermissionDenied.into()); }
                                        client.write_all(&[1, 0])?;
                                    }
                                    let mut head = [0u8; 4]; client.read_exact(&mut head)?;
                                    if head[0] != 5 || head[1] != 1 || head[3] != 1 { return Err(std::io::ErrorKind::InvalidData.into()); }
                                    let mut ip = [0u8; 4]; client.read_exact(&mut ip)?;
                                    let mut p = [0u8; 2]; client.read_exact(&mut p)?;
                                    let address = SocketAddrV4::new(Ipv4Addr::from(ip), u16::from_be_bytes(p));
                                    if !address.ip().is_loopback() { return Err(std::io::ErrorKind::PermissionDenied.into()); }
                                    let mut upstream = TcpStream::connect(address)?;
                                    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])?;
                                    count.fetch_add(1, Ordering::Relaxed);
                                    let mut reader = client.try_clone()?; let mut writer = upstream.try_clone()?;
                                    let forward = std::thread::spawn(move || { let _ = std::io::copy(&mut reader, &mut writer); });
                                    let _ = std::io::copy(&mut upstream, &mut client);
                                    let _ = client.shutdown(std::net::Shutdown::Both);
                                    let _ = forward.join();
                                    Ok(())
                                })();
                                let _ = result;
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                        Err(_) => break,
                    }
                }
            });
            (port, stop, count, thread)
        }
        let (relay_port, relay_stop, relay_count, relay_thread) = socks(false);
        let (exit_port, exit_stop, exit_count, exit_thread) = socks(true);
        let http = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = http.local_addr().unwrap().port();
        http.set_nonblocking(true).unwrap();
        let http_stop = Arc::new(AtomicBool::new(false)); let stop = http_stop.clone();
        let http_thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match http.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let mut request = [0u8; 2048]; let _ = stream.read(&mut request);
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n127.0.0.1");
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        });
        let yaml = format!("- name: relay\n  type: socks5\n  server: 127.0.0.1\n  port: {relay_port}\n- name: exit\n  type: socks5\n  server: 127.0.0.1\n  port: {exit_port}\n  username: test-user\n  password: test-password\n  dialer-proxy: relay\n");
        let nodes = serde_yaml::from_str::<Vec<Value>>(&yaml).unwrap();
        let result = run_probe_chain_at(&ProbePlan { nodes, expected_ip: "127.0.0.1".into() }, &format!("http://127.0.0.1:{http_port}/"));
        relay_stop.store(true, Ordering::Relaxed); exit_stop.store(true, Ordering::Relaxed); http_stop.store(true, Ordering::Relaxed);
        relay_thread.join().unwrap(); exit_thread.join().unwrap(); http_thread.join().unwrap();
        assert_eq!(result.unwrap(), ["127.0.0.1", "127.0.0.1", "127.0.0.1"]);
        assert!(relay_count.load(Ordering::Relaxed) >= 3);
        assert!(exit_count.load(Ordering::Relaxed) >= 3);
    }
}
