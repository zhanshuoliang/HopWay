//! Push an applied chain into the running kernel through the configured local
//! Mihomo controller. Runs only inside the apply flow; status polling elsewhere
//! stays read-only.
use serde::Serialize;
use serde_yaml::Value;
use std::{fs::OpenOptions, io::{Read, Write}, net::TcpStream, path::Path, time::{Duration, Instant}};

const TIMEOUT: Duration = Duration::from_secs(3);
const RELOAD_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activation { pub state: String, pub message: String }

fn encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

struct Body { status: u16, bytes: Vec<u8> }
trait Channel: Read + Write {}
impl Channel for TcpStream {}
impl Channel for std::fs::File {}

fn call(stream: &mut dyn Channel, method: &str, path: &str, body: Option<&[u8]>, secret: Option<&str>) -> Result<Body, String> {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(secret) = secret { request.push_str(&format!("Authorization: Bearer {secret}\r\n")); }
    if let Some(body) = body { request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len())); }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).and_then(|_| stream.write_all(body.unwrap_or(&[]))).map_err(|_| "控制器连接失败".to_string())?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => { raw.extend_from_slice(&chunk[..n]); if raw.len() > 4 * 1024 * 1024 { return Err("控制器响应过大".into()); } }
            Err(e) if raw.is_empty() => return Err(format!("控制器读取失败：{e}")),
            Err(_) => break,
        }
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..pos]).to_lowercase();
                if let Some(length) = head.split("content-length:").nth(1).and_then(|v| v.trim().split(|c: char| c == '\r' || c == '\n').next()).and_then(|v| v.trim().parse::<usize>().ok()) {
                    if raw.len() - pos - 4 >= length { break; }
                }
            }
        }
    }
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("控制器响应无效")?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let status = head.split_whitespace().nth(1).and_then(|v| v.parse::<u16>().ok()).ok_or("控制器响应无效")?;
    Ok(Body { status, bytes: raw[head_end + 4..].to_vec() })
}

fn with_controller<T: Send>(runtime: &Value, f: impl Fn(&mut dyn Channel, Option<&str>) -> Result<T, String> + Send) -> Result<T, String> {
    // A named-pipe read cannot be timed out directly, so it runs on a scoped
    // thread and the caller gives up after a deadline.
    if let Some(pipe) = super::relay_status::local_pipe(runtime) {
        let outcome = std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            scope.spawn(move || {
                let out = OpenOptions::new().read(true).write(true).open(&pipe)
                    .map_err(|_| "控制器连接失败".to_string())
                    .and_then(|mut file| f(&mut file, None));
                let _ = tx.send(out);
            });
            rx.recv_timeout(Duration::from_secs(8))
        });
        match outcome {
            Ok(result) => return result,
            Err(_) => return Err("控制器响应超时".into()),
        }
    }
    if let Some((address, secret)) = super::relay_status::loopback_controller(runtime) {
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, TIMEOUT) {
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let _ = stream.set_write_timeout(Some(TIMEOUT));
            return f(&mut stream, secret.as_deref());
        }
    }
    Err("控制器不可达".into())
}

fn json_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> { value.get(key) }

// Reload the rewritten runtime file and select the chain tail in our group.
// Returns a user-facing outcome; never brings down the caller on failure.
pub fn activate(runtime_path: &Path, group: &str, node: &str) -> Activation {
    match run(runtime_path, group, node) {
        Ok(message) => Activation { state: "ok".into(), message },
        Err((state, message)) => Activation { state: state.into(), message },
    }
}

fn run(runtime_path: &Path, group: &str, node: &str) -> Result<String, (&'static str, String)> {
    let raw = std::fs::read(runtime_path).map_err(|_| ("partial", "运行配置已写入但无法读取，请重启 Clash Verge".to_string()))?;
    let doc: Value = serde_yaml::from_slice(&raw).map_err(|_| ("partial", "运行配置已写入但无法解析，请重启 Clash Verge".to_string()))?;
    if super::relay_status::local_pipe(&doc).is_none() && super::relay_status::loopback_controller(&doc).is_none() {
        return Err(("manual", "未检测到可用的内核控制器；请重启 Clash Verge 使链路生效".to_string()));
    }
    // Reload the kernel on the file we just committed.
    let path = runtime_path.canonicalize().map_err(|_| ("partial", "无法解析运行配置路径，请重启 Clash Verge".into()))?;
    let body = serde_json::to_vec(&serde_json::json!({ "path": path.to_string_lossy() })).map_err(|_| ("partial", "无法生成重载请求".into()))?;
    with_controller(&doc, |stream, secret| call(stream, "PUT", "/configs?force=true", Some(&body), secret))
        .map_err(|e| ("partial", format!("内核重载失败（{e}），请重启 Clash Verge")))?;
    // Wait for the group with our node to appear after reload.
    let group_uri = format!("/proxies/{}", encode(group));
    let start = Instant::now();
    let mut found = false;
    while start.elapsed() < RELOAD_WAIT {
        let reply = with_controller(&doc, |stream, secret| call(stream, "GET", &group_uri, None, secret));
        if let Ok(Body { status: 200, bytes }) = reply {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let members = json_field(&value, "all").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                if members.iter().any(|m| m.as_str() == Some(node)) { found = true; break; }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !found { return Err(("partial", "内核已重载但链路组未出现；请在客户端刷新后手动选择出口".into())); }
    // Select the chain tail.
    let select = serde_json::to_vec(&serde_json::json!({ "name": node })).map_err(|_| ("partial", "无法生成选择请求".into()))?;
    let reply = with_controller(&doc, |stream, secret| call(stream, "PUT", &group_uri, Some(&select), secret))
        .map_err(|e| ("partial", format!("出口选择失败（{e}）；请在客户端 {group} 组中手动选择")))?;
    if !(200..300).contains(&reply.status) { return Err(("partial", format!("出口选择被拒绝（HTTP {}）；请在客户端 {group} 组中手动选择", reply.status))); }
    // Confirm the selection actually took effect.
    let reply = with_controller(&doc, |stream, secret| call(stream, "GET", &group_uri, None, secret));
    match reply {
        Ok(Body { status: 200, bytes }) => {
            let now = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| json_field(&v, "now").and_then(|v| v.as_str()).map(str::to_owned));
            if now.as_deref() == Some(node) { Ok(format!("内核已重载，{group} 组已选中链尾，链路已生效")) }
            else { Err(("partial", format!("已重载但未能确认选中项（当前 {now:?}）；请在客户端 {group} 组中手动选择"))) }
        }
        _ => Err(("partial", format!("出口已请求切换但无法确认；请在客户端 {group} 组中检查"))),
    }
}
