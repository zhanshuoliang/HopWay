import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { addEdge, Background, Controls, Handle, MarkerType, Position, ReactFlow, useEdgesState, useNodesState } from "@xyflow/react";
import { ArrowsClockwise, CheckCircle, CircleNotch, Code, DotsThree, FloppyDisk, Globe, HardDrives, Info, Laptop, LinkSimple, LockKey, PencilSimple, Plus, ShieldCheck, Trash, X } from "@phosphor-icons/react";

const empty = { subscriptions: [], customs: [], links: [] };
const demo = {
  subscriptions: [
    { id: "vpn-demo-1", name: "机场订阅 A", selected: "香港节点 01", choices: [{ name: "香港节点 01", region: "hk" }, { name: "日本节点 02", region: "jp" }], position: { x: 55, y: 105 } },
    { id: "vpn-demo-2", name: "机场订阅 B", selected: "新加坡节点 01", choices: [{ name: "新加坡节点 01", region: "sg" }], position: { x: 325, y: 105 } },
  ],
  customs: [{ id: "node-demo", name: "自建 VPS", protocol: "ss", position: { x: 595, y: 120 } }],
  links: [{ source: "vpn-demo-1", target: "vpn-demo-2" }, { source: "vpn-demo-2", target: "node-demo" }],
};
const edgeStyle = { type: "straight", animated: false, style: { stroke: "#9ebbe9", strokeWidth: 1.7 }, markerEnd: { type: MarkerType.ArrowClosed, color: "#9ebbe9", width: 13, height: 13 } };

// The chain is one linear path; the tail is the egress and any module may be it.
function chainPath(nodes, edges) {
  const linked = new Set();
  edges.forEach((edge) => { linked.add(edge.source); linked.add(edge.target); });
  let order;
  if (!linked.size) {
    if (nodes.length !== 1) throw new Error("请把节点连成一个链，或只留一个节点");
    order = [nodes[0].id];
  } else {
    const heads = nodes.filter((node) => linked.has(node.id) && !edges.some((edge) => edge.target === node.id));
    if (heads.length !== 1) throw new Error("链路需要唯一的起点");
    order = [];
    const seen = new Set();
    let current = heads[0].id;
    while (current) {
      if (seen.has(current)) throw new Error("链路不能形成循环");
      seen.add(current);
      order.push(current);
      current = edges.find((edge) => edge.source === current)?.target;
    }
    if (order.length !== linked.size) throw new Error("只能连接一条链路，其余节点未加入");
  }
  if (!order.length || order.length > 6) throw new Error("链路需要 1 至 6 跳");
  return order;
}
function routeFor(nodes, edges) {
  return chainPath(nodes, edges).map((id) => {
    const node = nodes.find((item) => item.id === id);
    if (node.data.kind === "vpn") {
      if (!node.data.selected) throw new Error("请选择每个订阅的节点");
      return `${node.data.name} · ${node.data.selected}`;
    }
    return node.data.name;
  });
}

function ModuleNode({ data }) {
  const isVpn = data.kind === "vpn";
  const selected = data.choices?.find((choice) => choice.name === data.selected);
  return <div className={"module-card " + (isVpn ? "module-vpn" : "module-exit")}>
    <Handle type="target" position={Position.Left} className="module-handle" aria-label="上游连接点" />
    <span className={"module-icon " + (isVpn ? "vpn" : "exit")}>{isVpn ? <ShieldCheck size={27} weight="bold" /> : <HardDrives size={27} weight="fill" />}</span>
    <div className="module-content" title={data.name}>
      {isVpn ? <>
        {selected?.region && <span className={"fi fi-" + selected.region + " module-flag"} role="img" aria-label="根据配置节点名称识别的地区旗帜" />}
        <select aria-label={data.name + " 的节点"} value={data.selected || ""} onChange={(e) => data.onSelect(data.id, e.target.value)} className="module-node-select nodrag nowheel">
          <option value="">选择节点</option>{(data.choices || []).map((choice) => <option key={choice.name} value={choice.name}>{choice.name}</option>)}
        </select>
      </> : <>
        <strong>{data.name}</strong>
        {data.protocol && <span className="module-proto" title="节点协议">{data.protocol.toUpperCase()}</span>}
      </>}
    </div>
      <details className="module-menu nodrag"><summary className="module-action" aria-label={"管理 " + data.name} title="管理节点"><DotsThree size={19} weight="bold" /></summary>
        <div className="module-menu-popover">
          <label className="module-next">下游
            <select aria-label={data.name + " 的下游"} value={data.next || ""} onChange={(event) => data.onLink(data.id, event.target.value)} className="nodrag nowheel">
              <option value="">未连接</option>{data.targets.map((target) => <option key={target.id} value={target.id}>{target.name}</option>)}
            </select>
          </label>
          <button aria-label={"编辑 " + data.name} onClick={(event) => { event.currentTarget.closest("details").open = false; data.onEdit(data); }}><PencilSimple size={15} /> 编辑</button>
          {isVpn && <button aria-label={"更新订阅 " + data.name} onClick={(event) => { event.currentTarget.closest("details").open = false; data.onRefresh(data.id); }}><ArrowsClockwise size={15} /> 更新订阅</button>}
          <button className="danger" aria-label={"删除 " + data.name} onClick={(event) => { event.currentTarget.closest("details").open = false; data.onRemove(data.id, data.name); }}><Trash size={15} /> 删除</button>
        </div>
      </details>
    <Handle type="source" position={Position.Right} className="module-handle" aria-label="下游连接点" />
  </div>;
}
function EndpointNode({ data }) {
  return <div className={"canvas-endpoint " + data.kind} title={data.kind === "local" ? "本机" : data.ip ? "探测到的出口 IP：" + data.ip : "出口 IP 尚未探测"}>
    {data.kind === "local" ? <><Laptop size={25} weight="bold" /><strong>本机</strong></> : <Globe size={28} />}
    <Handle type={data.kind === "local" ? "source" : "target"} position={data.kind === "local" ? Position.Right : Position.Left} className="endpoint-handle" isConnectable={false} />
  </div>;
}
function CurrentNode({ data }) {
  const isExit = data.kind === "exit";
  const title = data.kind === "group" && data.selectedName
    ? data.name + " → " + data.selectedName + (data.selectionSource === "live" ? " · 最近一次从 Clash Verge 接口读取" : " · 客户端上次保存的选择，当前状态未确认")
    : data.kind === "group" ? data.name + " · 策略组，当前具体选中节点尚未确认"
    : data.kind === "default" ? data.name + " · 运行配置的默认规则目标，实际路由还取决于其他规则"
    : data.name + " · Clash Verge 运行配置中的" + (isExit ? "静态出口候选" : "节点");
  return <div className={"module-card current-node " + (isExit ? "module-exit" : "module-vpn")} title={title}>
    <Handle type="target" position={Position.Left} className="endpoint-handle" isConnectable={false} />
    <span className={"module-icon " + (isExit ? "exit" : "vpn")}>{isExit ? <HardDrives size={27} weight="fill" /> : <ShieldCheck size={27} weight="bold" />}</span>
    <div className="module-content">
      {data.region && <span className={"fi fi-" + data.region + " module-flag"} role="img" aria-label="根据配置节点名称识别的地区旗帜" />}
      <strong>{data.selectedName || data.name}</strong>
      {data.selectionSource === "saved" && <span className="selection-source">上次保存</span>}
    </div>
    <Handle type="source" position={Position.Right} className="endpoint-handle" isConnectable={false} />
  </div>;
}
const nodeTypes = { module: ModuleNode, endpoint: EndpointNode, current: CurrentNode };

// Runtime YAML can identify the configured chain, but not which member of a
// select group is active. Keep it separate from the editable saved workspace.
function currentGraph(snapshot) {
  if (!snapshot?.configFound) return { nodes: [], edges: [] };
  const hasStaticCandidate = snapshot.chainAttached && snapshot.staticNode;
  let relays = hasStaticCandidate ? (snapshot.relays || []) : [];
  if (hasStaticCandidate && !relays.length && snapshot.relay) {
    relays = [{ name: snapshot.relay, kind: snapshot.relayKind, region: snapshot.relayRegion }];
  }
  if (!hasStaticCandidate && snapshot.routeTarget) {
    const knownGroup = (snapshot.relays || []).find((relay) => relay.name === snapshot.routeTarget && relay.kind === "策略组");
    relays = [knownGroup || { name: snapshot.routeTarget, kind: "默认规则" }];
  }
  if (!hasStaticCandidate && !relays.length) return { nodes: [], edges: [] };
  const items = [{ type: "endpoint", data: { kind: "local" }, width: 110 }];
  for (const relay of relays) {
    items.push({ type: "current", data: { name: relay.name, selectedName: relay.selectedName, selectionSource: relay.selectionSource, kind: relay.kind === "策略组" ? "group" : relay.kind === "默认规则" ? "default" : "relay", region: relay.region }, width: 210 });
  }
  if (hasStaticCandidate) items.push({ type: "current", data: { name: snapshot.staticNode, kind: "exit" }, width: 210 });
  items.push({ type: "endpoint", data: { kind: "output", ip: snapshot.ip }, width: 62 });
  let x = 0;
  const nodes = items.map((item, index) => {
    const node = { id: `__hopway_current_${index}`, type: item.type, position: { x, y: item.type === "endpoint" ? 5 : 0 }, data: item.data, draggable: false, selectable: false };
    x += item.width + 45;
    return node;
  });
  const edges = nodes.slice(1).map((node, index) => ({ id: `__hopway_current_edge_${index}`, source: nodes[index].id, target: node.id, ...edgeStyle, selectable: false, deletable: false }));
  return { nodes, edges };
}

function endpointDecorations(nodes, edges) {
  let order;
  try { order = chainPath(nodes, edges); } catch { return { nodes: [], edges: [] }; }
  const first = nodes.find((node) => node.id === order[0]);
  const tail = nodes.find((node) => node.id === order[order.length - 1]);
  if (!first || !tail) return { nodes: [], edges: [] };
  const local = "__hopway_visual_local", output = "__hopway_visual_output";
  return {
    nodes: [
      { id: local, type: "endpoint", position: { x: first.position.x - 145, y: first.position.y + 5 }, data: { kind: "local" }, draggable: false, selectable: false },
      { id: output, type: "endpoint", position: { x: tail.position.x + 227, y: tail.position.y + 5 }, data: { kind: "output" }, draggable: false, selectable: false },
    ],
    edges: [
      { id: "__hopway_visual_in", source: local, target: first.id, ...edgeStyle, selectable: false, deletable: false },
      { id: "__hopway_visual_out", source: tail.id, target: output, ...edgeStyle, selectable: false, deletable: false },
    ],
  };
}

function Dialog({ title, children, close, busy }) {
  useEffect(() => {
    const onKey = (event) => { if (event.key === "Escape" && !busy) close(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, busy]);
  return <div className="canvas-dialog-backdrop"><section className="canvas-dialog" role="dialog" aria-modal="true" aria-label={title}>
    <header><h2>{title}</h2><button className="icon-button" onClick={close} disabled={busy} aria-label="关闭"><X size={18} /></button></header>
    {children}
  </section></div>;
}

export default function CanvasEditor({ native, snapshot, reloadSnapshot, onApplied, lastApplied, live, onRetry }) {
  const [nodes, setNodes, onNodesChange] = useNodesState([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState([]);
  const [flow, setFlow] = useState(null);
  const target = "verge";
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [form, setForm] = useState(null);
  const [plan, setPlan] = useState(null);
  const [result, setResult] = useState(null);
  const [viewMode, setViewMode] = useState(null);
  const current = useMemo(() => currentGraph(snapshot), [snapshot]);
  const showingCurrent = viewMode !== "draft" && current.nodes.length > 0;

  const showWorkspace = useCallback((workspace) => {
    const data = workspace || empty;
    setNodes([
      ...data.subscriptions.map((sub) => ({ id: sub.id, type: "module", position: sub.position, data: { ...sub, kind: "vpn" } })),
      ...(data.customs || []).map((node) => ({ id: node.id, type: "module", position: node.position, data: { ...node, kind: "custom" } })),
    ]);
    setEdges(data.links.map((edge) => ({ ...edge, id: `${edge.source}->${edge.target}`, ...edgeStyle })));
    setDirty(false);
  }, [setNodes, setEdges]);
  useEffect(() => {
    let active = true;
    (native ? invoke("load_workspace") : Promise.resolve(demo)).then((workspace) => { if (active) showWorkspace(workspace); }).catch((e) => { if (active) setError(String(e)); });
    return () => { active = false; if (native) invoke("cancel_chain").catch(() => {}); };
  }, [native, showWorkspace]);
  useEffect(() => {
    if (!flow || !(showingCurrent ? current.nodes.length : nodes.length)) return;
    const timer = window.setTimeout(() => flow.fitView({ padding: 0.18, maxZoom: 1.05, duration: 180 }), 50);
    return () => window.clearTimeout(timer);
  }, [flow, nodes.length, current.nodes.length, showingCurrent]);

  function layout() {
    return {
      positions: nodes.map((node) => ({ id: node.id, position: node.position })),
      selections: nodes.filter((node) => node.data.kind === "vpn").map((node) => ({ id: node.id, selected: node.data.selected || null })),
      links: edges.map((edge) => ({ source: edge.source, target: edge.target })),
    };
  }
  async function persist() {
    if (!dirty) return;
    if (native) showWorkspace(await invoke("save_workspace_layout", { layout: layout() }));
    else setDirty(false);
  }
  async function saveLayout() {
    setBusy(true); setError(""); setMessage("");
    try { await persist(); setMessage(native ? "配置已保存" : "示意画布已保存到当前页面"); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function reloadCurrent() {
    setBusy(true); setError(""); setMessage("");
    try {
      const value = await reloadSnapshot();
      if (!currentGraph(value).nodes.length) setMessage("未从 Clash Verge 运行配置中识别到链路");
      else setViewMode("current");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  function selectNode(id, value) {
    setViewMode("draft");
    setNodes((old) => old.map((node) => node.id === id ? { ...node, data: { ...node.data, selected: value } } : node));
    setDirty(true); setMessage("");
  }
  function connect(source, targetId) {
    setViewMode("draft");
    if (!nodes.some((node) => node.id === source)) { setError("节点已不存在"); return; }
    const remaining = edges.filter((edge) => edge.source !== source);
    if (!targetId) { setEdges(remaining); setDirty(true); setError(""); return; }
    if (source === targetId || !nodes.some((node) => node.id === targetId) || remaining.some((edge) => edge.target === targetId)) { setError("每个节点只能连接一个上游和下游"); return; }
    let current = targetId;
    const seen = new Set([source]);
    while (current) {
      if (seen.has(current)) { setError("链路不能形成循环"); return; }
      seen.add(current); current = remaining.find((edge) => edge.source === current)?.target;
    }
    setEdges(addEdge({ id: `${source}->${targetId}`, source, target: targetId, ...edgeStyle }, remaining));
    setDirty(true); setError(""); setMessage("");
  }
  async function removeNode(id, name) {
    if (!window.confirm(`删除“${name}”和关联连线？`)) return;
    setBusy(true); setError("");
    try {
      await persist();
      if (native) showWorkspace(await invoke("remove_workspace_node", { id }));
      else { setNodes((old) => old.filter((n) => n.id !== id)); setEdges((old) => old.filter((e) => e.source !== id && e.target !== id)); setDirty(false); }
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function refreshNode(id) {
    setBusy(true); setError(""); setMessage("");
    try {
      await persist();
      if (native) showWorkspace(await invoke("refresh_subscription", { id }));
      setMessage(native ? "订阅已更新，请保存并重新下发链路" : "浏览器演示不会下载订阅");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  function editNode(data) {
    setError("");
    setForm(data.kind === "vpn"
      ? { kind: "vpn", id: data.id, name: data.name, url: "" }
      : { kind: "custom", id: data.id, name: data.name, mode: "uri", config: "", server: "", port: "", username: "", password: "" });
  }
  const decorated = useMemo(() => nodes.map((node) => ({ ...node, data: {
    ...node.data, native, next: edges.find((edge) => edge.source === node.id)?.target || "",
    targets: nodes.filter((other) => other.id !== node.id).map((other) => ({ id: other.id, name: other.data.name })),
    onSelect: selectNode, onLink: connect, onEdit: editNode, onRefresh: refreshNode, onRemove: removeNode,
  } })), [nodes, edges, dirty, native]);
  const endpoints = useMemo(() => endpointDecorations(nodes, edges), [nodes, edges]);

  async function submitForm(event) {
    event.preventDefault(); setError(""); setMessage("");
    if (form.kind === "vpn" && !form.name.trim()) { setError("请填写名称"); return; }
    if (form.kind === "vpn" && !form.id && !form.url.trim()) { setError("请填写订阅地址"); return; }
    const port = form.port ? Number(form.port) : null;
    if (form.kind === "custom") {
      if (form.mode === "uri" && !form.config.trim() && !form.id) { setError("请粘贴节点链接或 YAML"); return; }
      if (form.mode === "socks5" && !form.id && (!form.server.trim() || !port)) { setError("请填写有效的服务器和端口"); return; }
      if (port !== null && (!Number.isInteger(port) || port < 1 || port > 65535)) { setError("端口必须在 1 至 65535 之间"); return; }
    }
    setBusy(true);
    try {
      await persist();
      if (native) {
        let updated;
        if (form.kind === "vpn") updated = form.id
          ? await invoke("edit_subscription", { id: form.id, name: form.name.trim(), url: form.url.trim() })
          : await invoke("add_subscription", { name: form.name.trim(), url: form.url.trim() });
        else updated = await invoke("save_custom_node", { input: { id: form.id || null, name: form.name.trim(), server: form.mode === "socks5" ? form.server.trim() : "", port: form.mode === "socks5" ? port : null, username: form.mode === "socks5" ? form.username : "", password: form.mode === "socks5" ? form.password : "", config: form.mode === "uri" ? form.config.trim() : "" } });
        showWorkspace(updated);
      } else {
        const id = form.id || `${form.kind}-demo-${Date.now()}`;
        setNodes((old) => form.id ? old.map((n) => n.id === id ? { ...n, data: { ...n.data, name: form.name.trim() } } : n)
          : [...old, { id, type: "module", position: { x: 80 + old.length * 120, y: 220 }, data: form.kind === "vpn" ? { id, name: form.name.trim(), kind: "vpn", selected: "香港节点 01", choices: [{ name: "香港节点 01", region: "hk" }] } : { id, name: form.name.trim() || "自定义节点", kind: "custom", protocol: form.mode === "socks5" ? "socks5" : "ss" } }]);
      }
      setForm(null); setMessage(native ? "节点已保存" : "演示节点已添加，未下载订阅或保存凭证");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function prepare() {
    setBusy(true); setError(""); setMessage("");
    try {
      const route = routeFor(nodes, edges);
      await persist();
      const prepared = native ? await invoke("prepare_visual_chain", { target }) : {
        id: "demo", target, route, exitNode: `__hopway_示例_exit::${route[route.length - 1]}`,
        files: ["Verge 增强层"], scriptPreview: "proxies:\n  - name: 中继\n  - name: 出口\n    dialer-proxy: 中继\nrules:\n  - MATCH,HopWay", warnings: ["浏览器仅展示流程，配置和校验请使用桌面程序。"],
      };
      setPlan(prepared);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function cancelPlan() { if (native) await invoke("cancel_chain").catch(() => {}); setPlan(null); setError(""); }
  async function apply() {
    if (!native || !plan) return;
    setBusy(true); setError("");
    try {
      const applied = await invoke("apply_chain", { id: plan.id });
      setPlan(null); setResult(applied); onApplied(applied);
    } catch (e) { setPlan(null); setError(String(e)); }
    finally { setBusy(false); }
  }

  return <div className="canvas-editor">
    <div className="canvas-toolbar">
      <div className="canvas-tab"><LinkSimple size={19} /> 链路画布</div>
      <button className="canvas-tool" disabled={busy} onClick={() => { setViewMode("draft"); setError(""); setForm({ kind: "vpn", name: "", url: "" }); }}><Plus size={16} /> 订阅</button>
      <button className="canvas-tool" disabled={busy} onClick={() => { setViewMode("draft"); setError(""); setForm({ kind: "custom", name: "", mode: "uri", config: "", server: "", port: "", username: "", password: "" }); }}><Plus size={16} /> 节点</button>
      <button className="canvas-tool" disabled={busy || showingCurrent || !dirty} onClick={saveLayout} title={dirty ? "有未保存的更改" : "画布已保存"}><FloppyDisk size={16} /> 保存{dirty && <span className="unsaved-dot" aria-label="有未保存更改" />}</button>
      {lastApplied && <button className="canvas-tool canvas-last" disabled={busy} onClick={() => setResult(lastApplied)} title="上次检测" aria-label="上次检测"><Globe size={17} /></button>}
      <div className="canvas-toolbar-spacer" />
      {!native && <span className="preview-label">示意</span>}
      <button className="primary-button canvas-generate" onClick={prepare} disabled={busy || showingCurrent || !nodes.length}>{busy ? <><CircleNotch size={17} className="spin" /> 处理中…</> : <><Code size={17} /> 生成配置</>}</button>
    </div>
    {(error || message) && !form && !plan && !result && <div className={"canvas-banner " + (error ? "error" : "")} role="status"><Info size={15} />{error || message}</div>}
    <div className={"canvas-body " + (busy ? "busy" : "")} inert={busy ? true : undefined}>
      <div className="canvas-view-switch" role="group" aria-label="画布内容">
        <button className={showingCurrent ? "selected" : ""} aria-pressed={showingCurrent} disabled={!current.nodes.length} title="Clash Verge 运行配置，只读；策略组的当前选中项可能未知" onClick={() => setViewMode("current")}>当前链路</button>
        <button className={!showingCurrent ? "selected" : ""} aria-pressed={!showingCurrent} onClick={() => setViewMode("draft")}>编辑画布</button>
        <button className="canvas-reload" onClick={reloadCurrent} aria-label="重新读取当前链路" title="重新读取 Clash Verge 运行配置"><ArrowsClockwise size={15} /></button>
        {showingCurrent && <span title={!snapshot.systemProxyEnabled ? "系统代理未开启；TUN 等其他代理方式是否生效未知" : !snapshot.systemProxyMatches ? "Windows 系统代理未指向 Clash Verge；这里显示的是 Verge 运行配置" : "配置中识别的链路仅供查看，策略组实际选中项未确认"}>{!snapshot.systemProxyEnabled ? "系统代理未开启" : !snapshot.systemProxyMatches ? "系统代理未指向" : "只读"}</span>}
      </div>
      {!showingCurrent && !nodes.length && <div className="canvas-empty"><span><LinkSimple size={25} /></span><strong>添加订阅或节点</strong><p>拖动连接点连成链，末端即为出口</p></div>}
      <ReactFlow nodes={showingCurrent ? current.nodes : [...decorated, ...endpoints.nodes]} edges={showingCurrent ? current.edges : [...edges, ...endpoints.edges]} nodeTypes={nodeTypes} onNodesChange={(changes) => { if (showingCurrent) return; const actual = changes.filter((change) => !(change.id || change.item?.id || "").startsWith("__hopway_")); onNodesChange(actual); if (actual.some((change) => change.type === "position" && change.dragging)) setViewMode("draft"); if (actual.some((change) => change.type === "position" && change.dragging === false)) { setDirty(true); setMessage(""); } }}
        onEdgesChange={(changes) => { if (!showingCurrent) onEdgesChange(changes.filter((change) => !(change.id || change.item?.id || "").startsWith("__hopway_"))); }} onConnect={(connection) => { if (!showingCurrent) connect(connection.source, connection.target); }} onEdgeDoubleClick={(_, edge) => { if (!showingCurrent && !edge.id.startsWith("__hopway_")) { setEdges((old) => old.filter((item) => item.id !== edge.id)); setDirty(true); } }}
        onInit={setFlow} nodesDraggable={!busy && !showingCurrent} nodesConnectable={!busy && !showingCurrent} elementsSelectable={!busy && !showingCurrent} deleteKeyCode={null} minZoom={0.4} maxZoom={1.4} fitView fitViewOptions={{ padding: 0.18, maxZoom: 1.05 }} proOptions={{ hideAttribution: true }}>
        <Background color="#506a99" gap={22} size={1} /><Controls showInteractive={false} />
      </ReactFlow>
    </div>
    {form && <Dialog title={form.kind === "vpn" ? (form.id ? "编辑 VPN 订阅" : "添加 VPN 订阅") : (form.id ? "编辑节点" : "添加节点")} close={() => { setForm(null); setError(""); }} busy={busy}>
      <form className="canvas-form" onSubmit={submitForm}>
        <label>名称<input autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder={form.kind === "custom" && form.mode === "uri" ? "留空使用链接里的名称" : ""} autoComplete="off" /></label>
        {form.kind === "vpn" ? <label>订阅地址<input type="password" value={form.url} onChange={(e) => setForm({ ...form, url: e.target.value })} placeholder={form.id ? "已保存，留空保持不变" : "输入订阅链接（Clash YAML 或节点链接列表）"} autoComplete="off" /></label> : <>
          <div className="canvas-form-modes" role="group" aria-label="节点配置方式">
            <button type="button" className={form.mode === "uri" ? "selected" : ""} onClick={() => setForm({ ...form, mode: "uri" })}>粘贴链接 / YAML</button>
            <button type="button" className={form.mode === "socks5" ? "selected" : ""} onClick={() => setForm({ ...form, mode: "socks5" })}>SOCKS5 表单</button>
          </div>
          {form.mode === "uri" ? <label>节点配置<textarea className="canvas-config-input" rows="4" value={form.config} onChange={(e) => setForm({ ...form, config: e.target.value })} placeholder={form.id ? "留空保持当前配置；粘贴 ss:// vmess:// trojan:// vless:// hysteria2:// 或 YAML 可替换" : "粘贴 ss:// vmess:// trojan:// vless:// hysteria2:// 链接，或单节点 YAML"} autoComplete="off" spellCheck="false" /></label>
          : <div className="canvas-form-grid">
            <label>服务器<input type="password" value={form.server} onChange={(e) => setForm({ ...form, server: e.target.value })} placeholder={form.id ? "已保存，留空不修改" : "IP 或域名"} autoComplete="off" /></label>
            <label>端口<input type="number" min="1" max="65535" value={form.port} onChange={(e) => setForm({ ...form, port: e.target.value })} placeholder={form.id ? "留空不修改" : "1–65535"} autoComplete="off" /></label>
            <label>账号<input type="password" value={form.username} onChange={(e) => setForm({ ...form, username: e.target.value })} placeholder={form.id ? "已保存，留空不修改" : "可留空"} autoComplete="off" /></label>
            <label>密码<input type="password" value={form.password} onChange={(e) => setForm({ ...form, password: e.target.value })} placeholder={form.id ? "已保存，留空不修改" : "可留空"} autoComplete="new-password" /></label>
          </div>}
        </>}
        <div className="canvas-form-note"><LockKey size={15} /> {native ? "本机账户加密保存" : "示意模式：不会下载或保存"}</div>
        {error && <div className="editor-error" role="alert"><Info size={16} />{error}</div>}
        <footer><button type="button" className="secondary-button" disabled={busy} onClick={() => setForm(null)}>取消</button><button className="primary-button" disabled={busy}>{busy ? "处理中…" : form.kind === "vpn" && (!form.id || form.url.trim()) ? "保存并获取节点" : "保存节点"}</button></footer>
      </form>
    </Dialog>}
    {plan && <Dialog title="确认配置" close={cancelPlan} busy={busy}>
      <div className="canvas-review">
        <div className="verified-line"><CheckCircle size={20} /> {native ? "已通过 mihomo 配置校验" : "预览示意，未执行内核校验"}</div>
        <div className="review-route">{plan.route.map((part, index) => <span className="review-stop" key={index}>{index > 0 && <span>→</span>}{part}</span>)}</div>
        <div className="review-row"><span>写入目标</span><strong>Clash Verge</strong></div>
        <div className="review-row"><span>出口节点</span><strong>{plan.exitNode}</strong></div>
        <details className="script-preview"><summary><Code size={15} /> 查看配置脚本（敏感内容已隐藏）</summary><pre>{plan.scriptPreview}</pre></details>
        {plan.warnings.map((warning) => <div className="review-warning" key={warning}><Info size={16} />{warning}</div>)}
      </div>
      <footer className="dialog-footer"><button className="secondary-button" onClick={cancelPlan} disabled={busy}>返回修改</button><button className="primary-button" onClick={apply} disabled={busy || !native}>{busy ? "写入并校验出口…" : native ? "一键配置并校验" : "仅桌面程序可配置"}</button></footer>
    </Dialog>}
    {result && <Dialog title="配置已写入" close={() => setResult(null)} busy={busy}>
      <div className="canvas-result">
        <div className={"probe-status " + (result.probe?.state || "failed")}><Globe size={21} /><div><strong>{result.probe?.message || "出口未校验"}</strong>{result.probe?.ip && <span>出口 IP：{result.probe.ip}</span>}</div></div>
        <div className={"live-status " + (result.activation?.state || "waiting")}><span className="connection-dot" />{result.nextStep}</div>
        {result.activation?.state !== "ok" && <p>在“{result.group || "HopWay"}”组中选择 <strong>{result.exitNode}</strong>。</p>}
        <div className={"live-status " + (live?.state || "waiting")}><span className="connection-dot" />{live?.message || "等待客户端确认链路"}{live?.ip ? " · " + live.ip : ""}</div>
        <button className="text-button" onClick={onRetry}><ArrowsClockwise size={15} /> 重新校验客户端</button>
      </div>
      <footer className="dialog-footer"><button className="primary-button" onClick={() => setResult(null)}>完成</button></footer>
    </Dialog>}
  </div>;
}
