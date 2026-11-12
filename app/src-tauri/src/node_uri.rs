//! Parse a pasted proxy definition into a mihomo outbound mapping.
//! Accepts share links (ss/ssr/vmess/trojan/vless/hysteria/hysteria2/tuic)
//! or a YAML fragment describing exactly one proxy.
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use serde_yaml::{Mapping, Value};

fn key(k: &str) -> Value { Value::String(k.to_owned()) }
fn s(v: &str) -> Value { Value::String(v.to_owned()) }
fn n(v: u16) -> Value { Value::Number(v.into()) }

fn decode64(text: &str) -> Option<Vec<u8>> {
    let clean = text.trim().replace(['\n', '\r'], "");
    URL_SAFE_NO_PAD.decode(&clean).or_else(|_| STANDARD.decode(&clean)).ok()
}
fn percent(text: &str) -> String {
    url::form_urlencoded::parse(text.as_bytes()).map(|(k, v)| if v.is_empty() { k.into_owned() } else { format!("{k}={v}") }).collect::<Vec<_>>().join("&")
}
fn decode_text(text: &str) -> Option<String> { String::from_utf8(decode64(text)?).ok() }
fn host_ok(host: &str) -> bool {
    !host.is_empty() && host.len() <= 253 && !host.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\'))
}
fn name_ok(name: &str) -> bool { !name.trim().is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control) }

struct Parsed { name: String, map: Mapping }

fn finish(parsed: Parsed) -> Result<Value, String> {
    let Parsed { name, mut map } = parsed;
    if !name_ok(&name) { return Err("节点名称无效".into()); }
    map.insert(key("name"), s(&name));
    let server = map.get(key("server")).and_then(Value::as_str).map(str::to_owned).unwrap_or_default();
    let port = map.get(key("port")).and_then(Value::as_u64).unwrap_or(0);
    if !host_ok(&server) || port == 0 || port > 65535 { return Err("节点服务器或端口无效".into()); }
    Ok(Value::Mapping(map))
}

fn split_name(raw: &str) -> (String, Option<String>) {
    let (body, frag) = match raw.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (raw, None),
    };
    (body.to_owned(), frag.map(|f| percent(f)).filter(|f| !f.is_empty()))
}
fn query_pairs(query: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn parse_ss(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let (body, plugin) = match body.split_once('?') {
        Some((b, q)) => (b.to_owned(), Some(q.to_owned())),
        None => (body, None),
    };
    // SIP002: ss://base64(method:pass)@host:port or ss://base64(method:pass@host:port)
    let (decoded, name) = if let Some((userinfo, server)) = body.rsplit_once('@') {
        let cred = decode_text(userinfo).unwrap_or_else(|| userinfo.to_owned());
        (format!("{cred}@{server}"), name)
    } else {
        let whole = decode_text(&body).ok_or("ss 链接无法解码")?;
        let (main, inner_name) = split_name(&whole);
        (main, inner_name.or(name))
    };
    let (decoded, plugin) = if plugin.is_none() {
        match decoded.split_once('?') {
            Some((b, q)) => (b.to_owned(), Some(q.to_owned())),
            None => (decoded, None),
        }
    } else { (decoded, plugin) };
    let (cred, server) = decoded.rsplit_once('@').ok_or("ss 链接缺少服务器")?;
    let (method, password) = cred.split_once(':').ok_or("ss 链接缺少加密方式")?;
    let (host, port) = server.rsplit_once(':').ok_or("ss 链接缺少端口")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let mut map = Mapping::new();
    map.insert(key("type"), s("ss"));
    map.insert(key("server"), s(host));
    map.insert(key("port"), n(port.parse().map_err(|_| "ss 端口无效")?));
    map.insert(key("cipher"), s(method));
    map.insert(key("password"), s(password));
    if let Some(query) = plugin {
        for (k, v) in query_pairs(&query) {
            if k == "plugin" {
                let mut parts = v.split(';').map(str::trim);
                match parts.next().unwrap_or("") {
                    "obfs-local" | "simple-obfs" => {
                        map.insert(key("plugin"), s("obfs"));
                        let mut opts = Mapping::new();
                        for part in parts {
                            if let Some((pk, pv)) = part.split_once('=') { opts.insert(key(pk), s(pv)); }
                        }
                        map.insert(key("plugin-opts"), Value::Mapping(opts));
                    }
                    "v2ray-plugin" => {
                        map.insert(key("plugin"), s("v2ray-plugin"));
                        let mut opts = Mapping::new();
                        opts.insert(key("mode"), s("websocket"));
                        for part in parts {
                            match part.split_once('=') {
                                Some(("tls", _)) => { opts.insert(key("tls"), Value::Bool(true)); }
                                Some(("host", pv)) => { opts.insert(key("host"), s(pv)); }
                                Some(("path", pv)) => { opts.insert(key("path"), s(pv)); }
                                Some((pk, pv)) => { opts.insert(key(pk), s(pv)); }
                                None => (),
                            }
                        }
                        map.insert(key("plugin-opts"), Value::Mapping(opts));
                    }
                    other => return Err(format!("暂不支持的 ss 插件 {other}，可改用 YAML 粘贴节点")),
                }
            }
        }
    }
    Ok(Parsed { name: name.unwrap_or_else(|| host.to_owned()), map })
}

fn parse_ssr(body: &str) -> Result<Parsed, String> {
    let decoded = decode_text(body).ok_or("ssr 链接无法解码")?;
    let (main, query) = decoded.split_once("/?").unwrap_or((&decoded, ""));
    let parts: Vec<&str> = main.split(':').collect();
    if parts.len() < 6 { return Err("ssr 链接不完整".into()); }
    let (host, port, protocol, method, obfs) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    let password = decode_text(parts[5]).ok_or("ssr 密码无法解码")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("ssr"));
    map.insert(key("server"), s(host));
    map.insert(key("port"), n(port.parse().map_err(|_| "ssr 端口无效")?));
    map.insert(key("protocol"), s(protocol));
    map.insert(key("cipher"), s(method));
    map.insert(key("obfs"), s(obfs));
    map.insert(key("password"), s(&password));
    let mut name = None;
    for (k, v) in query_pairs(query) {
        match k.as_str() {
            "remarks" => name = decode_text(&v),
            "obfsparam" => { map.insert(key("obfs-param"), s(&decode_text(&v).unwrap_or(v))); }
            "protoparam" => { map.insert(key("protocol-param"), s(&decode_text(&v).unwrap_or(v))); }
            _ => (),
        }
    }
    Ok(Parsed { name: name.unwrap_or_else(|| host.to_owned()), map })
}

fn parse_vmess(body: &str) -> Result<Parsed, String> {
    let text = decode_text(body).ok_or("vmess 链接无法解码")?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|_| "vmess 链接不是有效的 JSON")?;
    let get = |k: &str| doc.get(k).and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string()))).unwrap_or_default();
    let mut map = Mapping::new();
    map.insert(key("type"), s("vmess"));
    map.insert(key("server"), s(&get("add")));
    map.insert(key("port"), n(get("port").parse().map_err(|_| "vmess 端口无效")?));
    map.insert(key("uuid"), s(&get("id")));
    let alter: u64 = get("aid").parse().unwrap_or(0);
    map.insert(key("alterId"), Value::Number(alter.into()));
    let cipher = get("scy");
    map.insert(key("cipher"), s(if cipher.is_empty() { "auto" } else { &cipher }));
    let tls = get("tls");
    if !tls.is_empty() && tls != "none" {
        map.insert(key("tls"), Value::Bool(true));
        let sni = get("sni");
        if !sni.is_empty() { map.insert(key("servername"), s(&sni)); }
        let alpn = get("alpn");
        if !alpn.is_empty() { map.insert(key("alpn"), Value::Sequence(alpn.split(',').map(|v| s(v.trim())).collect())); }
        let fp = get("fp");
        if !fp.is_empty() { map.insert(key("client-fingerprint"), s(&fp)); }
    }
    match get("net").as_str() {
        "ws" => {
            map.insert(key("network"), s("ws"));
            let mut opts = Mapping::new();
            let path = get("path");
            opts.insert(key("path"), s(if path.is_empty() { "/" } else { &path }));
            let host = get("host");
            if !host.is_empty() {
                let mut headers = Mapping::new();
                headers.insert(key("Host"), s(&host));
                opts.insert(key("headers"), Value::Mapping(headers));
            }
            map.insert(key("ws-opts"), Value::Mapping(opts));
        }
        "grpc" => {
            map.insert(key("network"), s("grpc"));
            let mut opts = Mapping::new();
            let svc = get("path");
            if !svc.is_empty() { opts.insert(key("grpc-service-name"), s(&svc)); }
            map.insert(key("grpc-opts"), Value::Mapping(opts));
        }
        "h2" => {
            map.insert(key("network"), s("h2"));
            let host = get("host");
            if !host.is_empty() {
                let mut opts = Mapping::new();
                opts.insert(key("host"), Value::Sequence(host.split(',').map(|v| s(v.trim())).collect()));
                map.insert(key("h2-opts"), Value::Mapping(opts));
            }
        }
        _ => (),
    }
    Ok(Parsed { name: { let n = get("ps"); if n.is_empty() { get("add") } else { n } }, map })
}

fn transport_opts(map: &mut Mapping, pairs: &[(String, String)]) {
    let find = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    match find("type").as_str() {
        "ws" => {
            map.insert(key("network"), s("ws"));
            let mut opts = Mapping::new();
            let path = find("path");
            opts.insert(key("path"), s(if path.is_empty() { "/" } else { &path }));
            let host = find("host");
            if !host.is_empty() {
                let mut headers = Mapping::new();
                headers.insert(key("Host"), s(&host));
                opts.insert(key("headers"), Value::Mapping(headers));
            }
            map.insert(key("ws-opts"), Value::Mapping(opts));
        }
        "grpc" => {
            map.insert(key("network"), s("grpc"));
            let mut opts = Mapping::new();
            let svc = find("serviceName");
            if !svc.is_empty() { opts.insert(key("grpc-service-name"), s(&svc)); }
            map.insert(key("grpc-opts"), Value::Mapping(opts));
        }
        _ => (),
    }
}

fn parse_trojan(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let url = url::Url::parse(&format!("trojan://{body}")).map_err(|_| "trojan 链接无效")?;
    let host = url.host_str().ok_or("trojan 缺少服务器")?.to_owned();
    let port = url.port().ok_or("trojan 缺少端口")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("trojan"));
    map.insert(key("server"), s(&host));
    map.insert(key("port"), n(port));
    map.insert(key("password"), s(url.username()));
    let pairs = query_pairs(url.query().unwrap_or(""));
    let find = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let sni = find("sni");
    if !sni.is_empty() { map.insert(key("sni"), s(&sni)); }
    if ["1", "true"].contains(&find("allowInsecure").as_str()) { map.insert(key("skip-cert-verify"), Value::Bool(true)); }
    let alpn = find("alpn");
    if !alpn.is_empty() { map.insert(key("alpn"), Value::Sequence(alpn.split(',').map(|v| s(v.trim())).collect())); }
    let flow = find("flow");
    if !flow.is_empty() { map.insert(key("flow"), s(&flow)); }
    transport_opts(&mut map, &pairs);
    Ok(Parsed { name: name.unwrap_or(host), map })
}

fn parse_vless(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let url = url::Url::parse(&format!("vless://{body}")).map_err(|_| "vless 链接无效")?;
    let host = url.host_str().ok_or("vless 缺少服务器")?.to_owned();
    let port = url.port().ok_or("vless 缺少端口")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("vless"));
    map.insert(key("server"), s(&host));
    map.insert(key("port"), n(port));
    map.insert(key("uuid"), s(url.username()));
    let pairs = query_pairs(url.query().unwrap_or(""));
    let find = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    match find("security").as_str() {
        "tls" | "reality" => {
            map.insert(key("tls"), Value::Bool(true));
            let sni = find("sni");
            if !sni.is_empty() { map.insert(key("servername"), s(&sni)); }
            let fp = find("fp");
            if !fp.is_empty() { map.insert(key("client-fingerprint"), s(&fp)); }
            if find("security") == "reality" {
                let mut reality = Mapping::new();
                let pbk = find("pbk");
                if pbk.is_empty() { return Err("vless reality 链接缺少 pbk".into()); }
                reality.insert(key("public-key"), s(&pbk));
                let sid = find("sid");
                if !sid.is_empty() { reality.insert(key("short-id"), s(&sid)); }
                map.insert(key("reality-opts"), Value::Mapping(reality));
            }
        }
        _ => (),
    }
    let flow = find("flow");
    if !flow.is_empty() { map.insert(key("flow"), s(&flow)); }
    transport_opts(&mut map, &pairs);
    Ok(Parsed { name: name.unwrap_or(host), map })
}

fn parse_hysteria2(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let url = url::Url::parse(&format!("hy2://{body}")).map_err(|_| "hysteria2 链接无效")?;
    let host = url.host_str().ok_or("hysteria2 缺少服务器")?.to_owned();
    let port = url.port().ok_or("hysteria2 缺少端口")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("hysteria2"));
    map.insert(key("server"), s(&host));
    map.insert(key("port"), n(port));
    map.insert(key("password"), s(url.username()));
    let pairs = query_pairs(url.query().unwrap_or(""));
    let find = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let sni = find("sni");
    if !sni.is_empty() { map.insert(key("sni"), s(&sni)); }
    if ["1", "true"].contains(&find("insecure").as_str()) { map.insert(key("skip-cert-verify"), Value::Bool(true)); }
    let obfs = find("obfs");
    if !obfs.is_empty() {
        map.insert(key("obfs"), s(&obfs));
        let pass = find("obfs-password");
        if !pass.is_empty() { map.insert(key("obfs-password"), s(&pass)); }
    }
    let mport = find("mport");
    if !mport.is_empty() { map.insert(key("ports"), s(&mport)); }
    Ok(Parsed { name: name.unwrap_or(host), map })
}

fn parse_hysteria(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let url = url::Url::parse(&format!("hysteria://{body}")).map_err(|_| "hysteria 链接无效")?;
    let host = url.host_str().ok_or("hysteria 缺少服务器")?.to_owned();
    let port = url.port().ok_or("hysteria 缺少端口")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("hysteria"));
    map.insert(key("server"), s(&host));
    map.insert(key("port"), n(port));
    let pairs = query_pairs(url.query().unwrap_or(""));
    let find = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    for (param, field) in [("auth", "auth_str"), ("peer", "sni"), ("obfs", "obfs"), ("protocol", "protocol"), ("upmbps", "up"), ("downmbps", "down"), ("alpn", "alpn")] {
        let v = find(param);
        if !v.is_empty() {
            match field {
                "alpn" => { map.insert(key("alpn"), Value::Sequence(v.split(',').map(|x| s(x.trim())).collect())); }
                "up" | "down" => { map.insert(key(field), s(&format!("{v} Mbps"))); }
                _ => { map.insert(key(field), s(&v)); }
            }
        }
    }
    if ["1", "true"].contains(&find("insecure").as_str()) { map.insert(key("skip-cert-verify"), Value::Bool(true)); }
    Ok(Parsed { name: name.unwrap_or(host), map })
}

fn parse_tuic(body: &str) -> Result<Parsed, String> {
    let (body, name) = split_name(body);
    let url = url::Url::parse(&format!("tuic://{body}")).map_err(|_| "tuic 链接无效")?;
    let host = url.host_str().ok_or("tuic 缺少服务器")?.to_owned();
    let port = url.port().ok_or("tuic 缺少端口")?;
    let mut map = Mapping::new();
    map.insert(key("type"), s("tuic"));
    map.insert(key("server"), s(&host));
    map.insert(key("port"), n(port));
    map.insert(key("uuid"), s(url.username()));
    map.insert(key("password"), s(url.password().unwrap_or("")));
    let pairs = query_pairs(url.query().unwrap_or(""));
    for (param, field) in [("sni", "sni"), ("congestion_control", "congestion-controller"), ("udp_relay_mode", "udp-relay-mode"), ("alpn", "alpn")] {
        if let Some((_, v)) = pairs.iter().find(|(k, _)| k == param) {
            if field == "alpn" { map.insert(key(field), Value::Sequence(v.split(',').map(|x| s(x.trim())).collect())); }
            else { map.insert(key(field), s(v)); }
        }
    }
    Ok(Parsed { name: name.unwrap_or(host), map })
}

fn parse_yaml_node(text: &str) -> Result<Parsed, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|_| "节点内容不是链接或有效的 YAML".to_string())?;
    let node = match &doc {
        Value::Mapping(_) if doc.get("type").is_some() => doc.clone(),
        Value::Sequence(items) if items.len() == 1 => items[0].clone(),
        Value::Mapping(_) => {
            let proxies = doc.get("proxies").and_then(Value::as_sequence).ok_or("YAML 中找不到 proxies 节点")?;
            if proxies.len() != 1 { return Err("一次只粘贴一个节点".into()); }
            proxies[0].clone()
        }
        _ => return Err("无法识别的节点内容".into()),
    };
    let map = node.as_mapping().cloned().ok_or("节点必须是键值对")?;
    let name = map.get(key("name")).and_then(Value::as_str).map(str::to_owned).ok_or("节点缺少 name")?;
    if map.get(key("type")).and_then(Value::as_str).map(str::is_empty).unwrap_or(true) { return Err("节点缺少 type".into()); }
    Ok(Parsed { name, map })
}

// Turn a pasted share link or YAML fragment into one outbound proxy mapping.
pub fn parse_node(input: &str) -> Result<Value, String> {
    let text = input.trim();
    if text.is_empty() || text.len() > 64 * 1024 { return Err("节点内容为空或过长".into()); }
    let parsed = if let Some((scheme, body)) = text.split_once("://") {
        match scheme.to_ascii_lowercase().as_str() {
            "ss" => parse_ss(body)?,
            "ssr" => parse_ssr(body)?,
            "vmess" => parse_vmess(body)?,
            "trojan" => parse_trojan(body)?,
            "vless" => parse_vless(body)?,
            "hysteria2" | "hy2" => parse_hysteria2(body)?,
            "hysteria" => parse_hysteria(body)?,
            "tuic" => parse_tuic(body)?,
            _ => return Err(format!("暂不支持的链接类型 {scheme}://，可改用 YAML 粘贴节点")),
        }
    } else { parse_yaml_node(text)? };
    finish(parsed)
}

// Subscriptions may also be delivered as newline separated share links,
// optionally base64 encoded as a whole document.
pub fn parse_link_list(data: &[u8]) -> Result<Vec<Value>, String> {
    let text = String::from_utf8(data.to_vec()).map_err(|_| "订阅内容不是文本".to_string())?;
    let text = if text.contains("://") { text } else { decode_text(&text).unwrap_or(text) };
    let mut nodes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains("://") { continue; }
        let node = parse_node(line).map_err(|e| format!("第 {} 行: {e}", nodes.len() + 1))?;
        let name = node.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        if !seen.insert(name) { return Err("订阅中存在重复的节点名称".into()); }
        nodes.push(node);
    }
    if nodes.is_empty() || nodes.len() > 2000 { return Err("订阅节点为空或数量过多".into()); }
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> { v.as_mapping()?.get(key(k)) }
    #[test]
    fn parses_ss_sip002_and_plain_forms() {
        let link = format!("ss://{}@example.com:8388#%E9%A6%99%E6%B8%AF", STANDARD.encode("aes-256-gcm:pass123"));
        let node = parse_node(&link).unwrap();
        assert_eq!(field(&node, "name").and_then(Value::as_str), Some("香港"));
        assert_eq!(field(&node, "cipher").and_then(Value::as_str), Some("aes-256-gcm"));
        let whole = format!("ss://{}", URL_SAFE_NO_PAD.encode("aes-256-gcm:pass123@example.com:8388"));
        assert!(parse_node(&whole).is_ok());
    }
    #[test]
    fn parses_vmess_and_trojan_links() {
        let json = serde_json::json!({"ps":"节点A","add":"vps.example.com","port":"443","id":"uuid-x","aid":"0","net":"ws","tls":"tls","sni":"sni.example.com","path":"/p","host":"cdn.example.com"});
        let node = parse_node(&format!("vmess://{}", STANDARD.encode(json.to_string()))).unwrap();
        assert_eq!(field(&node, "servername").and_then(Value::as_str), Some("sni.example.com"));
        assert_eq!(field(&node, "network").and_then(Value::as_str), Some("ws"));
        let t = parse_node("trojan://pw@host.example:443?sni=a.example&type=grpc&serviceName=sv#N").unwrap();
        assert_eq!(field(&t, "network").and_then(Value::as_str), Some("grpc"));
    }
    #[test]
    fn parses_vless_reality_and_hysteria2() {
        let v = parse_node("vless://u@h.example:443?security=reality&pbk=KEY&sni=x.example&fp=chrome&type=tcp#R").unwrap();
        assert_eq!(field(&v, "reality-opts").and_then(Value::as_mapping).and_then(|m| m.get(key("public-key"))).and_then(Value::as_str), Some("KEY"));
        let h = parse_node("hysteria2://pw@h.example:443?sni=s.example&insecure=1#H").unwrap();
        assert_eq!(field(&h, "skip-cert-verify"), Some(&Value::Bool(true)));
    }
    #[test]
    fn parses_yaml_fragment_and_link_lists() {
        let y = parse_node("- name: X\n  type: socks5\n  server: h.example\n  port: 1080\n").unwrap();
        assert_eq!(field(&y, "type").and_then(Value::as_str), Some("socks5"));
        let list = parse_link_list(b"ss://YWVzLTI1Ni1nY206cEBoLmNvbTo0NDM=\ntrojan://p@t.example:443\n").unwrap();
        assert_eq!(list.len(), 2);
    }
    #[test]
    fn rejects_garbage() {
        assert!(parse_node("http://example.com/x").is_err());
        assert!(parse_node("ss://!!!").is_err());
        assert!(parse_node("{name: A, type: socks5}").is_err()); // missing server/port
    }
}
