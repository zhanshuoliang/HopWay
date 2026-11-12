# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## HopWay design direction

The approved main-window reference is `design-reference-workspace.png`. For the desktop widget, use `design-reference-dual-widget.png`: a compact 320 × 56 logical-pixel pill that expands into a 600 × 276 detailed card and can be collapsed again. Keep the dark-blue palette, colorful node icons, avoid unnecessary subtitles and visible rectangular corner artifacts, and show a relay flag only when its source can be identified. Do not show the “上次” label below the flag in the compact widget; retain the source information in the hover text.

The main configuration workflow is a visual canvas: each VPN subscription and static IP is a draggable module, connections define the hop order, and users can save the layout and generate/apply a client configuration. The widget should start compact and reuse the existing dark-blue palette and colored node icons in its expanded view.

The user expects the canvas to show the existing Clash Verge chain. Show it from the runtime configuration as a read-only current view, separate from the editable HopWay workspace; do not imply that a strategy group's actual selection is known when it is not.

The user also wants the selected relay's flag. Read a configured local Mihomo controller using GET only where available; distinguish live readings, last-saved selections, and unknown values. Country flags are inferred from node names and do not establish physical location. Never enable the controller or change a proxy selection for detection.

The current desktop product supports Clash Verge only. Do not restore FlClash discovery or configuration without a new user request. The user has also asked to stop testing to protect their current configuration; do not run tests or interact with their real client configuration unless they explicitly authorize it.
