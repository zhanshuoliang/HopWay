// managed-by: sync-proxy
// 贴到 Clash Verge：订阅页右键该配置 → 扩展脚本（不是"全局扩展脚本"）
// 作用：订阅里的分流目标统一改到 静态ISP，末尾补 MATCH，静态ISP 才是真正的默认出口
// 组名/节点名不同的话，把下面三处 静态ISP / LS-ISP-01 改成你的名字
function main(config, profileName) {
  if (!Array.isArray(config.rules)) return config;
  var keep = ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "静态ISP"];

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
    parts[i] = "静态ISP";
    return parts.join(",");
  }

  config.rules = config.rules
    .filter(function (r) { return typeof r === "string"; })
    .map(rewrite)
    .filter(function (r) { return r !== null; });
  config.rules.push("MATCH,静态ISP");

  // 静态节点只能待在 静态ISP 组里；出现在别的组（尤其 url-test）会和中继组成自环
  (config["proxy-groups"] || []).forEach(function (g) {
    if (!g || g.name === "静态ISP") return;
    if (Array.isArray(g.proxies)) {
      g.proxies = g.proxies.filter(function (n) { return n !== "LS-ISP-01" && n !== "静态ISP"; });
    }
    if (g["include-all"] || g["include-all-proxies"]) {
      g["exclude-filter"] = "(?i)(LS-ISP-01|静态ISP)";
    }
  });

  return config;
}
