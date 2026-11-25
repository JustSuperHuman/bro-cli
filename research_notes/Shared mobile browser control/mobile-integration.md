# Mobile and desktop browser takeover

## Where should mobile browser control enter Bro?

### Takeaway

Add a browser screen beside the existing remote desktop screen. A hosted viewer in the existing WebView can share its stream and controls with Bro's desktop GUI without a new native dependency.

### Cited Findings

- The mobile application uses Expo `~56.0.14`, React Native `0.85.3`, and `react-native-webview` `13.16.1`. Its repository instructions require consulting the exact SDK 56 documentation before coding. Expo lists WebView 13.16.1 as the recommended SDK 56 version. [Package](F:/bro-cli-mobile/package.json), [Instructions](F:/bro-cli-mobile/AGENTS.md), [Expo WebView](https://docs.expo.dev/versions/v56.0.0/sdk/webview/)
- `src/TerminalScreen.tsx` owns screen and overlay state. Existing desktop control imports are at lines 62–63, `remoteOpen` and `remoteStatus` are at 179–180, the `ChatsScreen.onTakeControl` callback is at 1370, and the `RemoteDesktopScreen` overlay is at 1565. A browser screen can follow this arrangement. [Terminal screen](F:/bro-cli-mobile/src/TerminalScreen.tsx)
- `ChatsScreen` declares `onTakeControl` and `remoteReadOnly` near line 83 and renders its desktop-control row at 514. This is the current host-wide controls entry point. A browser entry can live beside it, while a session-level entry should pass the active Bro session ID. [Chats screen](F:/bro-cli-mobile/src/components/ChatsScreen.tsx)
- `RemoteDesktopScreen` uses a full-screen React Native `Modal`, only mounts its active session while visible, accepts portrait and landscape orientations, and passes `onRequestClose`. React Native documents `onRequestClose` as the Android hardware-back callback while a modal is open. [Existing desktop screen](F:/bro-cli-mobile/src/components/remote/RemoteDesktopScreen.tsx:108), [React Native modal](https://reactnative.dev/docs/modal)
- The app already loads host-owned web content with a WebView, restricts top-level navigation to the intended host page, and handles messages with `window.ReactNativeWebView.postMessage`. Its `AppCard` is a useful navigation and lifecycle example. [App card](F:/bro-cli-mobile/src/components/focus/AppCard.tsx)
- Current mobile styling uses the graphite Fluent palette and Inter for controls. `colors`, `font`, and `radius` are exported from `src/theme.ts`. The browser screen should extend these existing choices. [Theme](F:/bro-cli-mobile/src/theme.ts)

### Inferences

- Build one hosted browser viewer, then show it in a mobile WebView and a Bro desktop WebView. This keeps touch mapping and stream behavior consistent across clients.
- A mobile wrapper should own close, safe-area treatment, network errors, and app-background cleanup. The hosted viewer should own the remote tab image, navigation, touch input, keyboard input, and takeover/release.
- A persistent Browser entry is useful even before an agent requests help. A second, session-specific entry can go directly to that agent's waiting tab.
- Keep the full desktop-control screen available. Browser-only control has different geometry and does not require a remote desktop session.

### Gaps

- No current browser-specific screen or browser-control API was found in the mobile source during this inspection.
- Physical-device behavior of a new hosted browser viewer has not been tested by this research task.

## Which bridge and desktop paths should carry the protocol?

### Takeaway

Use the existing Bro bridge as the authenticated phone-facing gateway. It can proxy browser metadata, lease commands, and a frame socket to the local Chrome bridge while the phone uses its existing paired endpoint.

### Cited Findings

- `ServerEndpoint` supplies `httpBase`, `wsUrl`, host, and token. `apiUrl()` appends the paired token to paths. The general mobile `authHeaders()` uses `x-terminal-web-token`. The remote desktop API instead sends Bearer credentials and guards request paths to `/api/rmm` and `/rmm/`. A new browser API should have its own path validation. [Endpoint](F:/bro-cli-mobile/src/lib/endpoint.ts), [General API](F:/bro-cli-mobile/src/lib/api.ts:21), [Desktop API](F:/bro-cli-mobile/src/lib/remote/api.ts)
- `crates/bro-bridge/src/server.rs::router` merges protected API routes with separately authenticated `rmm::routes(state.clone())` near line 196. Browser control can use a separate routes module at this same point. [Bridge router](F:/bro-cli-v2/crates/bro-bridge/src/server.rs:190)
- `rmm.rs::routes()` applies `paired()` middleware to every remote-control route. `paired_request()` calls shared authorization with a synthetic nonlocal peer, preventing the ordinary loopback exemption, rejects `sec-fetch-site: cross-site`, and compares an Origin authority to Host. `paired()` adds no-store, no-referrer, and nosniff headers. [Remote pairing middleware](F:/bro-cli-v2/crates/bro-bridge/src/rmm.rs:756)
- `Bridge::local_url()` returns the bound local bridge address. `BridgeStatus` includes the pairing token and access URLs. `Services::bridge_local_url()` exposes the local address to callers. [Bridge methods](F:/bro-cli-v2/crates/bro-bridge/src/lib.rs:863), [Services](F:/bro-cli-v2/crates/bro-tui/src/services/mod.rs:777)
- `Services::open_voice_page()` is a compact existing pattern for a desktop command opening a bridge-owned page. It obtains a bridge URL and sends `Event::OpenLink`, or shows a settings hint when the bridge is unavailable. [Voice page command](F:/bro-cli-v2/crates/bro-tui/src/services/live_voice.rs:60)
- `crates/bro-tui/src/palette.rs` defines `Cmd` and adds an Automation command near line 86. `App::run_palette()` handles it at `app/commands.rs:615`. A Browser command can use this same entry path. [Palette](F:/bro-cli-v2/crates/bro-tui/src/palette.rs), [Command handler](F:/bro-cli-v2/crates/bro-tui/src/app/commands.rs:615)
- The desktop already uses Wry for native web content in `app/gui/apps.rs`; its platform implementation builds a WebView with the Bro webview data directory and manages placement, visibility, navigation, and message delivery. [Native webviews](F:/bro-cli-v2/crates/bro-tui/src/app/gui/apps.rs:451)
- `AppState` is cheap to clone and holds shared state through Arcs. `Inner.sessions` contains the bridge's live sessions. A new browser subsystem can hold its lease registry in shared AppState while validating Bro session existence through this map. [Bridge state](F:/bro-cli-v2/crates/bro-bridge/src/state.rs)

### Inferences

- Recommended route shape: browser status/list API, create or claim takeover API, heartbeat/release API, and a browser frame/input socket. Do not have mobile connect to `127.0.0.1:12306`; that address is the phone's own loopback, and exposing the extension bridge directly bypasses the established Bro pairing boundary.
- Give each control lease a single writer. Multiple viewers can watch, but only the current lease may send input. Renew ownership while connected and release it on disconnect or lease expiry.
- Keep Chrome's protocol behind the gateway. Forward a narrow set of validated control messages rather than permitting arbitrary CDP methods from the phone.
- Share the hosted page and socket protocol between mobile and desktop. Reusing Wry's existing implementation is possible, though its current Apps manager assumes conversation-card placement and MCP app data.

### Gaps

- No browser-control command or fully embedded browser-takeover surface was found in the native GUI during this inspection.
- This task did not test binding changes, proxy upgrades, or credential handling in a running Bro process.

## How should touch, keyboard, and stream geometry work?

### Takeaway

Keep frame dimensions and page viewport dimensions separate. Send pointer coordinates in remote CSS pixels, use text insertion for mobile typing, and acknowledge frames after they are accepted for display.

### Cited Findings

- CDP `Input.dispatchMouseEvent` uses coordinates relative to the main frame's viewport in CSS pixels. Wheel deltas are also CSS pixels. `Input.insertText` is explicitly intended for input that does not come from a physical key press, including emoji keyboards and IMEs. `Input.dispatchKeyEvent` is available for control keys. [Chromium Input protocol source](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Input.pdl)
- `Page.startScreencast` accepts JPEG/PNG format, quality, maximum width/height, and an every-Nth-frame parameter. Frame metadata includes DIP dimensions, page scale, top offset, and CSS scroll offsets. `Page.screencastFrameAck` controls progress. Newer protocol source includes optional in-flight limits; compatibility with the installed Chrome version must be checked before using new parameters. [Chromium Page protocol source](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Page.pdl)
- The existing mobile desktop gestures module calculates fit scale, pan limits, and display-to-remote coordinates. `desktopPoint()` converts an input location using the viewport's fit factor, zoom, and pan. These formulas provide a tested local example, although browser CDP uses CSS pixels rather than desktop pixels. [Existing gesture mapping](F:/bro-cli-mobile/src/lib/remote/gestures.ts)
- `getBoundingClientRect()` reports an element's dimensions and position relative to the local viewport. `VisualViewport` reports visible width/height and pinch scale and emits resize events, which lets a hosted viewer adapt when the mobile keyboard changes the visible area. [Element geometry](https://developer.mozilla.org/en-US/docs/Web/API/Element/getBoundingClientRect), [Visual viewport](https://developer.mozilla.org/en-US/docs/Web/API/VisualViewport)
- Pointer Events define a shared mouse/touch/pen input model and `touch-action` for direct-manipulation behavior. A custom viewer needs explicit keyboard equivalents as well. [W3C Pointer Events](https://w3c.github.io/pointerevents/)

### Inferences

- When a frame is displayed with contain sizing, derive its actual content rectangle, subtract letterboxing, then map through current remote CSS viewport dimensions. Reject taps outside the content rectangle. Do not multiply coordinates by devicePixelRatio.
- A tap sends mouse press/release. A one-finger drag can scroll the page through wheel input, while a clear alternate drag mode handles slider controls and selection. Preserve two-finger pinch as a viewer zoom gesture and account for its transform in pointer mapping.
- Use a local input element to open the phone keyboard, send committed text through `Input.insertText`, and send Enter, Backspace, Tab, and arrows as key events. Treat composition events as a commit boundary to avoid repeated IME text. A paste action should also use text insertion.
- Restrict buffered frames to the newest frame so a slow mobile link does not grow latency. Keep width and JPEG quality bounded for phones. Stop capture when there are no viewers or the app is backgrounded; release input ownership even if rendering cleanup fails.
- Avoid changing a live tab's device-emulation metrics by default. Resizing or emulating mobile layout changes the page the agent is working on and needs explicit restoration if used.

### Gaps

- DIP, CSS-pixel, offsetTop, and pageScaleFactor combinations need a real-browser mapping test at zoom levels other than 100 percent. This research does not establish a universal transform from screencast metadata alone.
- File selection, native browser permission prompts, password-manager UI, and operating-system dialogs are outside a page-only screencast. These interactions need an explicit fallback to the existing remote desktop surface or a separately implemented browser tool.
