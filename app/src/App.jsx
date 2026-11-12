import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowRight, ArrowsClockwise, CaretDown, CaretUp, Clock, Cube, HardDrives, Laptop, Minus, Monitor, Pulse, ShieldCheck, Square, WifiHigh, X } from "@phosphor-icons/react";
import "@xyflow/react/dist/style.css";
import "flag-icons/css/flag-icons.min.css";
import flagCountries from "flag-icons/country.json";
import CanvasEditor from "./CanvasEditor.jsx";
import globeImage from "./assets/globe.png";

const native = isTauri();
const widgetView = new URLSearchParams(window.location.search).get("view") === "widget";
const knownFlags = new Set(flagCountries.map((country) => country.code));
document.body.dataset.view = widgetView ? "widget" : "main";

const demo = {
  client: "verge", configFound: true, proxyPort: 7897,
  systemProxyEnabled: true, systemProxyMatches: true, routeTarget: "HopWay",
  staticNode: "自建 VPS", chainAttached: true, relay: "香港节点 01", relayKind: "节点",
  relayRegion: "hk", relayRegionSource: "configured",
  relays: [{ name: "香港节点 01", kind: "节点", region: "hk", regionSource: "configured", protocol: "VMess" }, { name: "新加坡节点 02", kind: "节点", region: "sg", regionSource: "configured", protocol: "VMess" }],
  ip: "203.0.113.8", probeState: "ok", egressMatch: true, updatedAt: Math.floor(Date.now() / 1000),
};
const empty = { client: "verge", configFound: false, probeState: "idle", egressMatch: null };

function mergeRelaySelections(snapshot, view) {
  if (!view || snapshot.proxyPort !== view.proxyPort || snapshot.routeTarget !== view.routeTarget ||
      snapshot.staticNode !== view.staticNode || snapshot.chainAttached !== view.chainAttached ||
      (snapshot.relays || []).length !== view.relays.length) return snapshot;
  if (!(snapshot.relays || []).every((relay, index) => relay.name === view.relays[index].name && relay.kind === view.relays[index].kind)) return snapshot;
  const relays = snapshot.relays.map((relay, index) => {
    const resolved = view.relays[index];
    return resolved.selectedName ? { ...relay, selectedName: resolved.selectedName, region: resolved.region, regionSource: resolved.source, selectionSource: resolved.source, selectionCheckedAt: view.checkedAt, protocol: resolved.protocol || null } : relay;
  });
  return { ...snapshot, relays, relayRegion: relays[0]?.region || null, relayRegionSource: relays[0]?.regionSource || null };
}

function useHopWay() {
  const [snapshot, setSnapshot] = useState(native ? empty : demo);
  const [relayView, setRelayView] = useState(null);
  const [notice, setNotice] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const acceptRelayView = useCallback((value) => {
    setRelayView((previous) => value && previous && value.checkedAt < previous.checkedAt ? previous : value);
  }, []);
  const pollRelay = useCallback(async () => {
    if (!native) return;
    try { acceptRelayView(await invoke("get_relay_selections")); }
    catch { acceptRelayView(null); }
  }, [acceptRelayView]);
  const read = useCallback(async () => {
    if (!native) return demo;
    const current = await invoke("get_snapshot");
    setSnapshot(current);
    void pollRelay();
    return current;
  }, [pollRelay]);
  const refresh = useCallback(async () => {
    if (!native) { setSnapshot((s) => ({ ...s, updatedAt: Math.floor(Date.now() / 1000) })); return; }
    setNotice("");
    setRefreshing(true);
    try {
      const current = await read();
      if (!current.proxyPort) { setNotice("未找到 Clash Verge 本地代理端口"); return; }
      setSnapshot(await invoke("probe_egress"));
    } catch (error) { setNotice(String(error)); }
    finally { setRefreshing(false); }
  }, [read]);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const subscriptions = [];
    read().then(() => { if (widgetView && !disposed) refresh(); }).catch(() => setNotice("无法读取客户端状态"));
    listen("hopway:snapshot", (event) => {
      if (!disposed) {
        setSnapshot(event.payload);
        if (event.payload.probeState === "checking") setNotice("");
      }
    }).then((unlisten) => { if (disposed) unlisten(); else subscriptions.push(unlisten); });
    listen("hopway:relay-selection", (event) => { if (!disposed) acceptRelayView(event.payload); }).then((unlisten) => { if (disposed) unlisten(); else subscriptions.push(unlisten); });
    if (widgetView) listen("hopway:widget-shown", () => refresh()).then((unlisten) => { if (disposed) unlisten(); else subscriptions.push(unlisten); });
    if (!widgetView) getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) read().catch(() => setNotice("无法读取客户端状态")); }).then((unlisten) => { if (disposed) unlisten(); else subscriptions.push(unlisten); }).catch(() => {});
    const timer = widgetView ? window.setInterval(refresh, 60_000) : null;
    const relayTimer = widgetView ? window.setInterval(() => { if (!document.hidden) pollRelay(); }, 15_000) : null;
    return () => { disposed = true; subscriptions.forEach((unlisten) => unlisten()); if (timer) window.clearInterval(timer); if (relayTimer) window.clearInterval(relayTimer); };
  }, [acceptRelayView, pollRelay, read, refresh]);
  const resolvedSnapshot = useMemo(() => mergeRelaySelections(snapshot, relayView), [snapshot, relayView]);
  return { snapshot: resolvedSnapshot, notice, refreshing, refresh, read };
}

function statusFor(s) {
  if (s.probeState === "checking") return { text: "正在探测", kind: "pending" };
  if (s.probeState === "ok" && s.egressMatch === true) return { text: "出口 IP 一致", kind: "good" };
  if (s.probeState === "ok" && s.egressMatch === false) return { text: "出口 IP 不一致", kind: "bad" };
  if (s.probeState === "ok") return { text: "代理可达", kind: "pending" };
  if (s.probeState === "failed") return { text: "探测未通过", kind: "bad" };
  return { text: "待验证", kind: "pending" };
}
function dragWindow(event) {
  if (native && event.button === 0 && !event.target.closest("button, select, input, details")) getCurrentWindow().startDragging();
}
function openMain() { if (native) invoke("show_main"); else window.location.href = "/"; }
function hideWidget() { if (native) invoke("hide_widget"); else window.location.href = "/"; }
function showWidget() { if (native) invoke("show_widget"); else window.location.href = "/?view=widget"; }
function hideMain() { if (native) invoke("hide_main"); }
function minimizeMain() { if (native) invoke("minimize_main"); }

// A saved canvas selection is useful as a label, but it is not proof of the
// currently selected proxy inside Clash Verge.
function workspaceRelay(workspace) {
  const links = workspace?.links || [];
  const nodes = [...(workspace?.subscriptions || []), ...(workspace?.customs || [])];
  const tails = nodes.filter((node) => links.some((link) => link.target === node.id) && !links.some((link) => link.source === node.id));
  if (tails.length !== 1) return null;
  const incoming = links.find((link) => link.target === tails[0].id);
  const relay = (workspace.subscriptions || []).find((sub) => sub.id === incoming?.source);
  const choice = relay?.choices?.find((item) => item.name === relay.selected);
  return choice?.region ? { region: choice.region, name: choice.name, source: "workspace" } : null;
}
function useWorkspaceRelay() {
  const [relay, setRelay] = useState(null);
  useEffect(() => {
    if (!native) return;
    let active = true, unlisten;
    const read = () => invoke("load_workspace").then((workspace) => {
      if (active) setRelay(workspaceRelay(workspace));
    }).catch(() => { if (active) setRelay(null); });
    read();
    listen("hopway:workspace-changed", read).then((dispose) => { if (active) unlisten = dispose; else dispose(); });
    return () => { active = false; if (unlisten) unlisten(); };
  }, []);
  return relay;
}
function relayDetails(snapshot, saved) {
  const runtime = snapshot.relays?.at(-1);
  const hasSelection = Boolean(runtime?.selectedName);
  const hasRuntimeFlag = runtime?.region && knownFlags.has(runtime.region);
  const region = hasRuntimeFlag ? runtime.region : hasSelection ? null : saved?.region;
  const source = hasSelection ? runtime.selectionSource : hasRuntimeFlag ? "configured" : saved?.source;
  const name = hasSelection ? runtime.selectedName : hasRuntimeFlag ? runtime.name : saved?.name || runtime?.name;
  const title = source === "live" ? `${name} · 最近一次从 Clash Verge 接口读取；地区根据节点名称识别`
    : source === "saved" ? `${name} · 客户端上次保存的选择，当前状态未确认`
    : source === "workspace" ? `${name} · 画布配置节点，未确认实际中继`
    : source === "configured" ? `${name} · 根据运行配置中的节点名称识别` : "中继地区未知";
  return { region: region && knownFlags.has(region) ? region : null, source, name, title, protocol: source === "workspace" ? null : runtime?.protocol };
}
function RelayMark({ relay }) {
  return <span className="widget-relay" title={relay.title} role="img" aria-label={relay.title}>
    {relay.region ? <span className={"fi fi-" + relay.region} /> : <ShieldCheck size={21} weight="bold" />}
  </span>;
}
function regionLabel(region) {
  return ({ hk: "香港", jp: "日本", sg: "新加坡", tw: "台湾", us: "美国", gb: "英国", kr: "韩国", de: "德国", fr: "法国", ca: "加拿大", au: "澳大利亚", nl: "荷兰" })[region] || (region ? region.toUpperCase() : "地区未知");
}
function protocolLabel(protocol) {
  if (!protocol) return "未知";
  return ({ ss: "Shadowsocks", shadowsocks: "Shadowsocks", vmess: "VMess", vless: "VLESS", socks5: "SOCKS5", hysteria2: "Hysteria2", tuic: "TUIC", trojan: "Trojan" })[protocol.toLowerCase()] || protocol;
}
function lastProbeTime(value) {
  if (!value) return "尚未检测";
  return new Date(value * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}
function Widget({ state }) {
  const { snapshot: s, notice, refresh, refreshing } = state;
  const [menu, setMenu] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [resizing, setResizing] = useState(false);
  const [resizeError, setResizeError] = useState("");
  const savedRelay = useWorkspaceRelay();
  const status = statusFor(s);
  const relay = relayDetails(s, savedRelay);
  const sourceTag = relay.source === "saved" ? "上次" : relay.source === "workspace" ? "配置" : "";
  const statusKind = resizeError || notice ? "bad" : status.kind;
  const statusText = resizeError || notice || status.text;
  const statusTitle = resizeError || notice || (s.systemProxyEnabled && !s.systemProxyMatches ? `${status.text}；系统代理未指向 Clash Verge` : status.text);
  useEffect(() => {
    if (!menu) return;
    const close = (event) => { if (event.key === "Escape") setMenu(false); };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [menu]);
  async function changeSize(next) {
    if (resizing || next === expanded) return;
    setResizing(true); setResizeError("");
    try {
      if (native) await invoke("set_widget_expanded", { expanded: next });
      setExpanded(next); setMenu(false);
    } catch (error) { setResizeError(String(error)); }
    finally { setResizing(false); }
  }
  return <div className={"widget-stage " + (native ? "native" : "preview")}>
    <section className={"widget-card " + (expanded ? "widget-expanded" : "widget-mini")} onMouseDown={dragWindow} aria-label="网络链路桌面卡片">
      {!expanded && menu ? <div className="widget-menu" role="group" aria-label="桌面组件操作">
        <button onClick={() => { setMenu(false); openMain(); }}><Monitor size={16} /> 打开</button>
        <button onClick={() => { setMenu(false); refresh(); }} disabled={refreshing}><ArrowsClockwise size={16} /> 刷新</button>
        <button onClick={hideWidget}><X size={16} /> 隐藏</button>
        <button className="widget-menu-close" onClick={() => setMenu(false)} aria-label="关闭菜单"><X size={16} /></button>
      </div> : !expanded ? <div className="mini-content">
        <button className="mini-brand" onClick={openMain} aria-label="打开主窗口" title={statusTitle}>
          <Pulse size={19} weight="bold" /><span className={"mini-status-dot " + statusKind} />
        </button>
        <span className="mini-divider" aria-hidden="true" />
        <button className="mini-route" onClick={() => changeSize(true)} disabled={resizing} aria-label="展开网络链路卡片" title={relay.title}>
          <span className="mini-relay"><RelayMark relay={relay} />{relay.source === "workspace" && <span className="mini-source">配置</span>}</span>
          <ArrowRight size={17} className="mini-arrow" />
          <span className="mini-ip">{resizeError ? "无法展开" : s.ip || "未探测"}</span>
        </button>
        <span className="mini-divider" aria-hidden="true" />
        <button className={"mini-menu-button " + statusKind} onClick={() => setMenu(true)} aria-label="组件菜单" title={statusTitle}><WifiHigh size={21} weight="bold" /></button>
        <button className="mini-expand" onClick={() => changeSize(true)} disabled={resizing} aria-label="展开网络链路卡片" title="展开"><CaretUp size={18} weight="bold" /></button>
      </div> : <>
        <header className="expanded-header">
          <button className="expanded-brand" onClick={openMain} aria-label="打开主窗口" title="打开主窗口"><Pulse size={21} weight="bold" /></button>
          <strong className="expanded-title">网络链路</strong>
          <span className={"expanded-status " + statusKind} title={statusTitle}><span className={"connection-dot " + statusKind} />{statusText}</span>
          {!native && <span className="preview-label">示意</span>}
          <div className="expanded-actions">
            <button className="icon-button" onClick={refresh} disabled={refreshing} aria-label="刷新出口" title="刷新出口"><ArrowsClockwise size={17} className={refreshing ? "spin" : ""} /></button>
            <button className="icon-button" onClick={hideWidget} aria-label="隐藏组件" title="隐藏组件"><X size={17} /></button>
          </div>
        </header>
        <div className="expanded-content">
          <div className="expanded-chain">
            <div className="expanded-node"><span className="expanded-node-icon local"><Laptop size={30} weight="bold" /></span><span className="expanded-node-label">本地网络</span><span className="expanded-node-detail" aria-hidden="true" /></div>
            <ArrowRight className="expanded-arrow" size={19} />
            <div className="expanded-node"><span className="expanded-node-icon vpn"><ShieldCheck size={32} weight="bold" /></span><span className="expanded-node-label">VPN 节点</span>
              <span className="expanded-node-detail" title={relay.title}><RelayMark relay={relay} /><span>{relay.region ? regionLabel(relay.region) : "地区未知"}</span>{sourceTag && <small>{sourceTag}</small>}</span>
            </div>
            <ArrowRight className="expanded-arrow" size={19} />
            <div className="expanded-node exit"><span className="expanded-node-icon exit"><HardDrives size={30} weight="fill" /></span><span className="expanded-node-label">出口 IP</span><span className="expanded-ip" title="通过 Clash Verge 探测的出口 IP">{s.ip || "未探测"}</span></div>
          </div>
          <div className="expanded-globe" title="地球图形仅为装饰，不代表节点实际位置"><img src={globeImage} alt="" aria-hidden="true" /></div>
        </div>
        <footer className="expanded-footer">
          <div className="expanded-fact"><Clock size={19} /><span><small>最近检测</small><strong>{lastProbeTime(s.updatedAt)}</strong></span></div>
          <span className="expanded-divider" aria-hidden="true" />
          <div className="expanded-fact protocol"><Cube size={20} /><span><small>协议</small><strong title={relay.name || "中继节点未知"}>{protocolLabel(relay.protocol)}</strong></span></div>
          <button className="collapse-button" onClick={() => changeSize(false)} disabled={resizing} aria-label="收起网络链路卡片" title="收起"><CaretDown size={19} weight="bold" /></button>
        </footer>
      </>}
    </section>
  </div>;
}

function Dashboard({ state }) {
  const { snapshot: s, notice, read } = state;
  const [lastApplied, setLastApplied] = useState(null);
  const [live, setLive] = useState(null);
  const [retry, setRetry] = useState(0);
  const [maximized, setMaximized] = useState(false);
  const status = statusFor(s);
  useEffect(() => {
    if (!native || !lastApplied?.probe) return;
    let active = true, timer, attempts = 0;
    const check = async () => {
      try {
        const value = await invoke("verify_visual_live", { target: lastApplied.target, exitNode: lastApplied.exitNode, expectedIp: lastApplied.probe.expectedIp });
        if (!active) return;
        setLive(value);
        if (value.state === "waiting" && attempts++ < 40) timer = window.setTimeout(check, 3000);
      } catch (error) { if (active) setLive({ state: "failed", message: String(error) }); }
    };
    check();
    return () => { active = false; window.clearTimeout(timer); };
  }, [lastApplied, retry]);
  useEffect(() => {
    if (!native) return;
    let active = true, dispose;
    const window = getCurrentWindow();
    const update = () => window.isMaximized().then((value) => { if (active) setMaximized(value); }).catch(() => {});
    update();
    window.onResized(update).then((unlisten) => { if (active) dispose = unlisten; else unlisten(); }).catch(() => {});
    return () => { active = false; if (dispose) dispose(); };
  }, []);
  async function toggleMaximized() {
    if (!native) { setMaximized((value) => !value); return; }
    try { setMaximized(await invoke("toggle_maximize_main")); } catch { /* Preserve current window state. */ }
  }
  function handleApplied(result) {
    setLive(result?.activation?.state === "ok"
      ? { state: "ok", message: result.nextStep || "链路已生效" }
      : { state: "waiting", message: result?.nextStep || "等待客户端确认链路" });
    setLastApplied(result);
  }
  function retryLive() {
    setLive({ state: "waiting", message: "正在检查客户端…" });
    setRetry((value) => value + 1);
  }
  return <div className={"dashboard-stage " + (native ? "native" : "preview")}>
    <main className={"main-window " + (maximized ? "maximized" : "")}>
      <header className="main-header" onMouseDown={dragWindow}>
        <div className="main-brand"><span className="main-brand-icon"><Pulse size={23} weight="bold" /></span><strong>HopWay</strong></div>
        <div className="main-header-right">
          <span className={"main-status " + status.kind} title={notice || status.text}><span className={"connection-dot " + status.kind} />{notice || status.text}</span>
          <div className="window-controls">
            <button className="icon-button" onClick={showWidget} aria-label="显示桌面组件" title="显示桌面组件"><Monitor size={16} /></button>
            <button className="icon-button" onClick={minimizeMain} aria-label="最小化" title="最小化"><Minus size={17} /></button>
            <button className="icon-button" onClick={toggleMaximized} aria-label={maximized ? "还原窗口" : "最大化"} title={maximized ? "还原窗口" : "最大化"}><Square size={15} /></button>
            <button className="icon-button" onClick={hideMain} aria-label="隐藏主窗口" title="隐藏主窗口"><X size={17} /></button>
          </div>
        </div>
      </header>
      <CanvasEditor native={native} snapshot={s} reloadSnapshot={read} onApplied={handleApplied} lastApplied={lastApplied} live={live} onRetry={retryLive} />
    </main>
  </div>;
}
export function App() {
  const state = useHopWay();
  return widgetView ? <Widget state={state} /> : <Dashboard state={state} />;
}
