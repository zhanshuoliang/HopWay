# 桌面卡片设计验收

Source visual truth: design-reference.png (1581 × 995 px; card crop 1129 × 519 px).
Implementation screenshot: design-widget-final.jpg (860 × 390 px, browser viewport 860 × 390 CSS px, device pixel ratio 1; card crop 840 × 370 px).
Comparison evidence: design-comparison-final.png (full card) and design-focused-final.png (header and upper content). Both crops were normalized to 840 × 386 px for side-by-side inspection; the implementation is vertically scaled by about 4% for comparison.
State: sample proxy reachable, desktop card visible; browser data is explicitly marked as a preview.

## Findings and comparison history

- First comparison: design-comparison-before.png. The four-node layout made icons and labels too small compared with the reference; the configuration-source footer was accidentally hidden at the actual widget width. This was a P2 readability and information issue.
- Fix: increased node icons, titles and footer type; restored the source footer; enlarged the globe. The post-fix full and focused comparisons show the restored footer and improved hierarchy.
- The reference has three nodes and country flags; the implemented card has four nodes because the actual product must distinguish relay, static exit and observed IP. Countries and flags are omitted because the client has not verified location. The globe marker is explicitly labeled as an illustration.
- Fonts and typography: Segoe UI with Microsoft YaHei UI / Microsoft YaHei fallbacks preserves a clear Chinese hierarchy; reference has slightly larger display text, an acceptable consequence of the additional node.
- Spacing and layout: the rounded dark panel, header/status, horizontal chain, right-hand globe and bottom facts follow the reference. Controls fit within the 860 × 390 window without clipping.
- Colors and tokens: dark navy translucent panel, pale blue icon blocks and green reachable state follow the source. The panel is a more uniform navy so desktop content can show through it.
- Images and icons: the generated transparent blue globe is sharp at display size; its placement and illustrative label are visible. Interface icons come from the Phosphor icon library.
- Copy and content: app-specific labels state configured relay/static exit and observed IP, while unknown hop status remains explicit.
- Main-window preview, source selection, refresh and browser console were checked; no console errors were observed. Native Windows app was separately checked with the local FlClash configuration.

Follow-up polish (P3): validate the card on multiple displays and at 125% scaling; the illustration can be refined if a different globe style is preferred.

## Compact card and relay flag revision

Source: design-reference-compact.png (1586 × 992 px; widget crop 527 × 209 px and main-window crop 942 × 544 px). Widget screenshot: design-widget-compact-final.jpg (460 × 190 CSS px, DPR 1, widget crop 444 × 174 px). Main screenshot: design-main-compact-final.jpg (960 × 590 CSS px, DPR 1).

Full comparisons are design-comparison-compact-final.png and design-main-comparison-final.png. Focused evidence is design-flag-comparison-final.png. Widget crops were normalized to 444 × 176 px; the main-window crops were normalized to 800 × 460 px.

- Initial compact comparison: design-comparison-compact.png showed symbols and labels that were too small and thin relative to the chosen mock (P2). The icon strokes and sizes and node labels were increased; the final comparisons show clearer colored icons and a visible relay flag.
- Typography: Chinese primary labels remain readable at the 460 × 190 viewport; decorative English overlines and subordinate captions were removed.
- Layout: the card is approximately half its former width, with the selected header, status, IP and horizontal icon chain. The full IP is intentionally shown once rather than duplicated as in the mock.
- Color and iconography: cyan, violet, amber and mint distinguish the four roles; flags are actual packaged flag assets rather than drawn approximations.
- Content: the browser mock explicitly uses sample data; in the native app a flag is tied to a configured node label or FlClash's saved selection, and unknown cases show a question mark.
- Browser checks covered switching sources, the unknown flag, details, card navigation, refreshed layout and absence of console errors. Native checks covered the real FlClash selection and exit IP, hiding the widget, and resuming its probe.

Follow-up (P3): test multimonitor placement and 125% Windows scaling.

final result: passed

## Editable canvas and smaller widget revision (2026-11-03)

Source visual truth: `design-reference-workspace.png` (the design approved by the user). The main window is now the editable canvas; the widget target is 320 × 76 logical pixels. The old comparison evidence above applies to the earlier design only.

The production frontend and Windows release compiled successfully. Visual capture, interaction checks, and native window comparison are blocked because the user explicitly requested that testing stop to protect the current client configuration. No actual client was launched or modified during this revision. Compilation does not verify appearance or behavior.

final result: blocked

## Current configuration chain revision (2026-11-03)

The canvas now has a read-only current-configuration view and a separate editable workspace. As requested by the user, no app launch, interaction test, or comparison against the real Clash Verge configuration was performed.

final result: blocked

## Read-only relay selection revision (2026-11-03)

The client can query the configured local Mihomo interface and, if unavailable, use validated saved selections. The Windows release compiled successfully. The user requested no testing; neither the running client nor its settings were accessed during development, and visual/runtime verification remains blocked.

final result: blocked

## Expandable desktop widget revision (2026-11-03)

Source visual truth: `design-reference-dual-widget.png`. The widget starts as a 320 × 56 logical-pixel pill and switches to a 600 × 276 detailed card. It presents the identifiable relay, exit IP, protocol when known, and the time of the latest probe; the globe is decorative.

The frontend and Windows release compiled successfully. Per the user's request to stop testing, the application was not launched and no interaction, native-window, or visual comparison was performed. The real Clash Verge client and its configuration were not accessed during this revision. Compilation does not verify appearance or behavior.

final result: blocked

## Compact widget label revision (2026-11-03)

Removed the visible “上次” label below the compact card's relay flag; the hover text retains the selection's provenance. The Windows release compiled successfully. At the user's request, the application was not launched, and neither the real client nor its configuration was accessed; appearance and behavior were not verified.

final result: blocked
