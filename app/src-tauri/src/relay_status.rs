//! Read the selected relay from an existing local Mihomo controller.
//! Only GET /proxies is sent; configuration and proxy selection are never changed.
use std::{collections::HashSet, fs, net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr}, path::Path, time::{Duration, SystemTime, UNIX_EPOCH}};
use http_body_util::{BodyExt, Empty};
use hyper::{body::Bytes, header::{AUTHORIZATION, CONNECTION, HOST}, Request};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::Value as Json;
use serde_yaml::Value as Yaml;
use sha2::{Digest, Sha256};
use tokio::{io::{AsyncRead, AsyncWrite}, net::{windows::named_pipe::ClientOptions, TcpStream}, task::JoinHandle, time::timeout};

const MAX_CONFIG: usize = 8 * 1024 * 1024;
const MAX_REPLY: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_millis(1800);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelaySelection {
    name: String,
    kind: String,
    selected_name: Option<String>,
    region: Option<String>,
    protocol: Option<String>,
    source: Option<&'static str>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayView {
    checked_at: u64,
    proxy_port: Option<u16>,
    route_target: Option<String>,
    static_node: Option<String>,
    chain_attached: bool,
    relays: Vec<RelaySelection>,
}

fn read_yaml(path: &Path) -> Option<(Vec<u8>, Yaml)> {
    if fs::metadata(path).ok()?.len() > MAX_CONFIG as u64 { return None; }
    let raw = fs::read(path).ok()?;
    if raw.len() > MAX_CONFIG { return None; }
    let doc = serde_yaml::from_slice::<Yaml>(&raw).ok()?;
    Some((raw, doc))
}
fn checksum(raw: &[u8]) -> [u8; 32] { Sha256::digest(raw).into() }
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 180 && !name.chars().any(char::is_control)
}
fn protocol_name(value: &str) -> Option<String> {
    (!value.is_empty() && value.chars().count() <= 32 && !value.chars().any(char::is_control)).then(|| value.to_owned())
}
fn built_in(name: &str) -> bool {
    ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE", "GLOBAL"].iter().any(|v| name.eq_ignore_ascii_case(v))
}
fn group_type(kind: &str) -> bool {
    ["Selector", "URLTest", "Fallback", "LoadBalance", "Relay"].iter().any(|v| kind.eq_ignore_ascii_case(v))
}

// A live result must resolve to an actual proxy through only members listed by
// each group. Load balancing has no single selected member and stays unknown.
fn resolve_live(proxies: &serde_json::Map<String, Json>, group: &str) -> Option<(String, Option<String>)> {
    let mut current = group;
    let mut seen = HashSet::new();
    for _ in 0..16 {
        if !valid_name(current) || built_in(current) || !seen.insert(current.to_owned()) { return None; }
        let item = proxies.get(current)?.as_object()?;
        let kind = item.get("type")?.as_str()?;
        if group_type(kind) {
            if kind.eq_ignore_ascii_case("LoadBalance") || kind.eq_ignore_ascii_case("Relay") { return None; }
            let next = item.get("now")?.as_str()?;
            if !item.get("all")?.as_array()?.iter().any(|v| v.as_str() == Some(next)) { return None; }
            current = next;
        } else {
            return (current != group).then(|| (current.to_owned(), protocol_name(kind)));
        }
    }
    None
}

// The profile contains last saved manual selections, not a live selection.
// Follow only explicit select-group members that still exist in the runtime YAML.
fn resolve_saved(runtime: &Yaml, profile: &Yaml, group: &str) -> Option<(String, Option<String>)> {
    let current_id = super::field(profile, "current")?.as_str()?;
    let item = super::seq(super::field(profile, "items")).iter().find(|v| super::field(v, "uid").and_then(Yaml::as_str) == Some(current_id))?;
    let saved = super::seq(super::field(item, "selected"));
    let groups = super::seq(super::field(runtime, "proxy-groups"));
    let proxies = super::seq(super::field(runtime, "proxies"));
    let mut current = group;
    let mut seen = HashSet::new();
    for _ in 0..16 {
        if !valid_name(current) || built_in(current) || !seen.insert(current.to_owned()) { return None; }
        let policy = groups.iter().find(|v| super::field(v, "name").and_then(Yaml::as_str) == Some(current))?;
        if !super::field(policy, "type").and_then(Yaml::as_str)?.eq_ignore_ascii_case("select") { return None; }
        let mut matches = saved.iter().filter(|entry| super::field(entry, "name").and_then(Yaml::as_str) == Some(current));
        let next = super::field(matches.next()?, "now")?.as_str()?;
        if matches.next().is_some() || !valid_name(next) || built_in(next) { return None; }
        if !super::seq(super::field(policy, "proxies")).iter().any(|v| v.as_str() == Some(next)) { return None; }
        if let Some(proxy) = proxies.iter().find(|p| super::field(p, "name").and_then(Yaml::as_str) == Some(next)) {
            let protocol = super::field(proxy, "type").and_then(Yaml::as_str).and_then(protocol_name);
            return Some((next.to_owned(), protocol));
        }
        current = next;
    }
    None
}

// Only accept a local Windows named pipe specified by the runtime configuration.
pub(crate) fn local_pipe(runtime: &Yaml) -> Option<String> {
    let value = super::field(runtime, "external-controller-pipe")?.as_str()?.trim();
    let prefix = r"\\.\pipe\";
    if value.len() <= prefix.len() || !value.get(..prefix.len())?.eq_ignore_ascii_case(prefix) { return None; }
    let name = value.get(prefix.len()..)?;
    if name.len() > 180 || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')) { return None; }
    Some(value.to_owned())
}
pub(crate) fn loopback_controller(runtime: &Yaml) -> Option<(SocketAddr, Option<String>)> {
    let value = super::field(runtime, "external-controller")?.as_str()?.trim();
    let address = if let Some(port) = value.strip_prefix("localhost:") {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port.parse::<u16>().ok()?))
    } else { value.parse::<SocketAddr>().ok()? };
    if address.port() == 0 { return None; }
    let address = match address.ip() {
        ip if ip.is_loopback() => address,
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::from((Ipv4Addr::LOCALHOST, address.port())),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::from((Ipv6Addr::LOCALHOST, address.port())),
        _ => return None,
    };
    let secret = super::field(runtime, "secret").and_then(Yaml::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    Some((address, secret))
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop { fn drop(&mut self) { self.0.abort(); } }

async fn query<I>(stream: I, secret: Option<&str>) -> Option<Json>
where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.ok()?;
    let _connection = AbortOnDrop(tokio::spawn(async move { let _ = connection.await; }));
    let mut request = Request::builder().method("GET").uri("/proxies").header(HOST, "localhost").header(CONNECTION, "close");
    if let Some(secret) = secret {
        let value = hyper::header::HeaderValue::from_str(&format!("Bearer {secret}")).ok()?;
        request = request.header(AUTHORIZATION, value);
    }
    let request = request.body(Empty::<Bytes>::new()).ok()?;
    let response = sender.send_request(request).await.ok()?;
    if !response.status().is_success() { return None; }
    if response.headers().get(hyper::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<usize>().ok()).map(|n| n > MAX_REPLY).unwrap_or(false) { return None; }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.ok()?;
        if let Ok(data) = frame.into_data() {
            if bytes.len().checked_add(data.len())? > MAX_REPLY { return None; }
            bytes.extend_from_slice(&data);
        }
    }
    let value = serde_json::from_slice::<Json>(&bytes).ok()?;
    value.get("proxies")?.as_object()?;
    Some(value)
}

async fn query_live(runtime: &Yaml) -> Option<Json> {
    if let Some(pipe) = local_pipe(runtime) {
        if let Ok(client) = ClientOptions::new().open(&pipe) {
            if let Ok(Some(data)) = timeout(TIMEOUT, query(client, None)).await { return Some(data); }
        }
    }
    if let Some((address, secret)) = loopback_controller(runtime) {
        let request = async {
            let stream = TcpStream::connect(address).await.ok()?;
            query(stream, secret.as_deref()).await
        };
        if let Ok(Some(data)) = timeout(TIMEOUT, request).await { return Some(data); }
    }
    None
}

pub async fn read_current() -> Option<RelayView> {
    let checked_at = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis().try_into().ok()?;
    let (path, hash, runtime, profile, snapshot) = tokio::task::spawn_blocking(|| {
        let path = super::config_path();
        let (raw, runtime) = read_yaml(&path)?;
        let snapshot = super::parse_config("verge", "verge", std::str::from_utf8(&raw).ok()?, (false, None));
        if !snapshot.config_found { return None; }
        let profile = if snapshot.relays.iter().any(|r| r.kind == "策略组") {
            path.parent().and_then(|root| read_yaml(&root.join("profiles.yaml"))).map(|(_, doc)| doc)
        } else { None };
        Some((path, checksum(&raw), runtime, profile, snapshot))
    }).await.ok()??;

    let has_group = snapshot.relays.iter().any(|r| r.kind == "策略组");
    let live = if has_group { query_live(&runtime).await } else { None };
    let live_proxies = live.as_ref().and_then(|value| value.get("proxies")).and_then(Json::as_object);
    let mut relays = Vec::new();
    for relay in &snapshot.relays {
        let selected = if relay.kind != "策略组" { None }
            else if let Some(proxies) = live_proxies { resolve_live(proxies, &relay.raw_name).map(|(name, protocol)| (name, protocol, "live")) }
            else { profile.as_ref().and_then(|p| resolve_saved(&runtime, p, &relay.raw_name)).map(|(name, protocol)| (name, protocol, "saved")) };
        let (selected_name, region, protocol, source) = match selected {
            Some((name, protocol, source)) => { let region = super::region_from_node_name(&name); (Some(name), region, protocol, Some(source)) },
            None => (None, None, None, None),
        };
        relays.push(RelaySelection { name: relay.name.clone(), kind: relay.kind.clone(), selected_name, region, protocol, source });
    }
    // Discard a selection if the runtime file changed while querying the core.
    let unchanged = tokio::task::spawn_blocking(move || {
        if fs::metadata(&path).map(|m| m.len() > MAX_CONFIG as u64).unwrap_or(true) { return false; }
        fs::read(path).map(|raw| raw.len() <= MAX_CONFIG && checksum(&raw) == hash).unwrap_or(false)
    }).await.unwrap_or(false);
    if !unchanged { return None; }
    Some(RelayView { checked_at, proxy_port: snapshot.proxy_port, route_target: snapshot.route_target, static_node: snapshot.static_node, chain_attached: snapshot.chain_attached, relays })
}
