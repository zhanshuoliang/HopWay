"""Create isolated fake client and subscription data for native smoke tests."""
from pathlib import Path

app_root = Path(__file__).resolve().parents[1]
root = (app_root / ".hopway-test").resolve()
if root.parent != app_root or root.exists():
    raise SystemExit("fixture directory already exists or is outside the app")

appdata = root / "appdata"
verge = appdata / "io.github.clash-verge-rev.clash-verge-rev"
for directory in (verge / "profiles", root / "subscriptions"):
    directory.mkdir(parents=True, exist_ok=True)

profile = """mode: rule
mixed-port: 19789
allow-lan: false
tun:
  enable: false
proxies:
  - name: Old relay
    type: socks5
    server: 127.0.0.1
    port: 9
proxy-groups:
  - name: Default
    type: select
    proxies: [Old relay]
rules:
  - MATCH,Default
"""
(verge / "clash-verge.yaml").write_text(profile, encoding="utf-8")
(verge / "profiles" / "base.yaml").write_text(profile, encoding="utf-8")
(verge / "profiles.yaml").write_text("current: P1\nitems:\n- uid: P1\n  type: local\n  file: base.yaml\n  option: {}\n", encoding="utf-8")
for name, node in (("a", "香港 测试 A"), ("b", "日本 测试 B")):
    text = f"proxies:\n  - name: '{node}'\n    type: socks5\n    server: 127.0.0.1\n    port: 9\n"
    (root / "subscriptions" / f"{name}.yaml").write_text(text, encoding="utf-8")
print("Isolated visual fixture created")
