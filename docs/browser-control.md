# Browser control

Bro Browser lets a phone or the desktop app use the same signed-in tab as an agent. The extension streams the tab through Bro's paired bridge. The browser process, cookies, and page memory stay in the original Chrome or Edge profile.

An agent handoff shows **Browser needs you** in the mobile app and a **Browser needs your input** prompt in the desktop workspace. The desktop prompt names the tab and shows the agent's reason. **Open browser** opens that exact tab in Bro's desktop viewer. **Return control** finishes the handoff directly, including when you already completed sign-in in your regular browser. The desktop window and phone use the same viewer and protocol.

The desktop prompt appears only while a handoff is pending. Use **Next** to switch between requests. If another screen is controlling a tab, finish with **Done** on that screen; the workspace cannot interrupt its input lease. Returning control checks the handoff ID, so a stale button cannot finish a newer request. It does not attach a viewer, resize the page, move the tab, or copy its profile. The Browser command palette entry and Browser and computer settings remain available for manual access.

Requests for protected pages such as `edge://net-internals` also appear in the prompt and mobile tab picker. **View request** shows the agent's instructions and explains that the page needs the desktop browser. Complete the steps there, or through Desktop control, then select **Return control**. Bro keeps these requests visible even when JustBrowse omits the tab from its streamable page list. Opening or closing the instructions keeps the request visible while the agent can continue working.

## Handoff

A handoff asks for help and lets the agent keep working. Agents can inspect the page in every handoff phase and continue as soon as the requested step is complete. **Done** and **Return control** provide an optional completion signal. Agents do not need to wait for either button. `chrome_wait_for_handoff` checks status immediately by default; an explicit timeout can wait for a change.

1. The agent opens its own background tab through JustBrowse and works normally.
2. When the page needs human input, it calls `chrome_request_handoff` with the tab ID and a reason. The request stays visible while the agent continues working and inspecting the page.
3. The user opens Browser and selects the waiting tab. Bro borrows the live tab with a controller lease that prevents a second viewer from taking over. It waits for an in-flight agent action before enabling input.
4. The user taps, scrolls, zooms, or types. Fit-to-screen mode changes the remote viewport and follows rotation and keyboard changes. It does not reload the page. The optional detached-window command also moves the existing tab without reloading it.
5. **Done** waits for an ordered input barrier, drains pending input, stops capture, clears temporary emulation, and returns control. `chrome_wait_for_handoff` then reports `agent`, and Bro can notify the agent. The agent can already continue in the same tab whenever the page is ready. A failed barrier keeps the completion signal pending.

Disconnecting, closing the viewer, backgrounding the phone, or losing the debugger releases the controller while retaining an agent handoff. The user reconnects and taps Done when the interaction is finished. A new controller gets a new lease; old input is rejected. Pending handoffs survive an extension service-worker restart in `chrome.storage.session`. Closing the tab tells the agent to recover its task.

The handoff states are `requested`, `connecting`, `human`, and `agent`. A second screen cannot take over an occupied tab. Agent tool calls cannot invoke the internal paired-viewer command.

## Returning work to the agent

Each handoff has a stable ID that remains the same through viewer disconnects and reconnects. Done records a completion after the final input has drained. Closing the original browser tab records a completion that tells the agent to recover its task. Lease expiry and viewer disconnection keep the handoff waiting.

Bro watches these completions independently of the agent's turn. An agent that already resumed and successfully used the returned tab needs no extra prompt. Otherwise Bro queues a message to that handoff's owning session, asking it to inspect the page and continue the original task. Delivery waits for an idle agent with an empty editor and uses Bro's confirmed paste/Enter queue. Busy agents, questions, and user drafts defer delivery. Another pending browser request does not defer a completion notification. Repeated completion signals share one ID and do not create duplicate turns.

The extension retains bounded completion metadata, and Bro persists its pending deliveries in `browser-handoffs.json` under the bridge data directory. Kernel file locks prevent two Bro hosts from claiming the same event. A claimed delivery whose process stopped can be retried; a confirmed submission is remembered across restarts. Neither store contains browser images or site credentials.

Managed MCP connections identify the Bro session through `x-bro-session-id`, including SSE connections. Older connections can supply their own `BRO_SESSION_ID` as `broSessionId` to `chrome_request_handoff`. Handoffs without a known Bro session remain available to the viewer. Agents can inspect the page and check status with `chrome_wait_for_handoff`; Bro does not guess which agent to wake.

## Page edit requests

The extension's page review editor lets the user send saved text changes and full comments to a running Bro agent. The user can select a registered Bro session for an ordinary browser tab, or send a review to an agent-owned tab's session. This does not require a browser handoff or a viewer lease. Closing the page after sending does not cancel the request.

Each bridge publishes its running agent sessions to JustBrowse about every five seconds. Registrations contain the session ID, title, and working folder; the extension merges registrations across hosts and expires them after 30 seconds. Shell-only and exited sessions are excluded. Bro accepts a new request only for a known, running agent and never redirects it to another session. Requests for closed or missing sessions stay pending.

Bro polls the private loopback endpoint and persists accepted requests in `browser-page-requests.json` under the bridge data directory. It shares the handoff worker's kernel-lock and atomic-write helper, with a separate outbox and claims. Each request uses the delivery event ID `page-request/<id>`. Delivery uses `BridgeCommand::BrowserResume`, including its idle-agent, empty-editor, draft, and question checks and confirmed paste/Enter submission. A busy agent or a user draft defers the request.

Bro marks the local request handled on disk before acknowledging it to JustBrowse. Failed acknowledgements retry without submitting another turn, including after a bridge restart. The persisted prompt is cleared after confirmed delivery. Pending requests do not expire locally, and an abandoned worker claim can be retried. Bro validates a 256 KiB serialized request limit and a 200 KiB UTF-8 prompt limit, normalizes line endings, and rejects terminal control characters other than newline and tab. The page metadata and comments are untrusted browser context.

The native host exposes `POST http://127.0.0.1:12306/browser/page-review` with a 32 KiB request-body limit and the existing origin gate. It forwards only these operations to the extension's internal `__justbrowse_page_review` tool:

- `{action:'list', broSessionId?:string}` returns `{success:true, requests:[ReviewRequest]}`.
- `{action:'ack', id:string, broSessionId:string}` returns `{success:true, request:ReviewRequest}`.
- `{action:'targets', targets:[{id:string, title:string, folder?:string}]}` returns `{success:true, targets:[ReviewTarget]}`. Bro batches registrations within the route's body limit.

A `ReviewRequest` has `id`, `createdAt`, `broSessionId`, `prompt`, `pages`, and `status:'pending'|'delivered'`. Each page includes its source URL, title, element selectors, before/after text, and full comments. Extension failures return HTTP 409 with `{success:false, error:string}`; an unavailable or incompatible extension returns HTTP 503. The internal tool and native endpoint cannot enqueue requests. Only the extension's runtime UI can send one. Request bodies are kept out of Bro's public `/api/browser` snapshot and paired viewer commands.

## Platforms

The Chrome or Edge browser and JustBrowse extension can run on Windows, macOS, or Linux. The shared viewer uses the same paired protocol on every host. Bro opens a native viewer window on Windows and macOS; on Linux, its Browser command opens the paired viewer in the default browser.

The mobile app supports iOS and Android. The browser screen accounts for safe areas on all four edges, keeps the input row above the iOS keyboard, and uses Android's native resize behavior. Safari's visual viewport also sizes the shared viewer when its keyboard opens. Backgrounding either app pauses browser control without returning the tab to the agent.

Development and internal mobile builds keep a manual Browser shortcut. Production builds show browser access while an agent request is waiting. The global shortcut opens all tabs; a chat's browser action scopes the picker to that chat. Waiting tabs from older agent sessions without a chat ID remain visible in that picker.

The live acceptance script accepts `--engine chromium` or `--engine webkit`. It checks Unicode input, rotation, exclusive control, reconnect, painted keyboard focus, preservation of authentication and page state, and recovery when WebSocket upgrades are blocked. The native wrapper's iOS and Android lifecycle checks run with `bun test scripts/browser-screen.test.mjs` in the mobile repository. WebKit engine testing does not replace testing on a physical iPhone.

## Session continuity and containers

The remote surface contains the existing browser tab. There is no profile export or cookie synchronization. HttpOnly cookies, local storage, session storage, and live JavaScript state remain in that tab. This preserves device-bound authentication by keeping its original browser and machine.

A separate Linux browser container is a different browser session. Copying a Windows profile into it cannot guarantee the same credentials, passkeys, or in-memory page. The research and primary sources are in [Shared mobile browser control](../reports/Shared%20mobile%20browser%20control.md). A separately enrolled container provider would need an explicit storage-transfer policy and site-specific tests; this implementation does not claim that migration.

Page capture does not contain native file pickers, password-manager windows, browser permission prompts, or OS authentication dialogs. The phone's **Desktop** action opens Bro's existing remote desktop feature for these interactions. Keep the browser handoff waiting until the user returns and finishes it.

## Transport

| Endpoint | Purpose |
| --- | --- |
| `GET /browser` | Shared viewer; no browser data is embedded in the HTML |
| `GET /api/browser` | Tabs, ownership metadata, and waiting handoffs |
| `POST /api/browser/command` | Open, take, release, heartbeat, or validated input |
| `GET /api/browser/stream` | WebSocket with JSON metadata and binary JPEG frames |
| `POST 127.0.0.1:12306/browser/command` | JustBrowse loopback backend; not exposed to the phone |

The protected Bro endpoints require the pairing token even on loopback and reject cross-site requests and mismatched origins. The viewer receives its token through a URL fragment, removes the fragment, and sends authenticated requests to its host. Responses use `no-store` and `no-referrer`; the viewer has a restrictive content security policy. The native desktop view and mobile WebView restrict navigation to their viewer origin.

The extension acknowledges each CDP screencast frame and retains only the newest one. Bro sends one unacknowledged frame per screen. The viewer acknowledges after decoding and displaying it; a slow connection skips stale frames. A screenshot fallback supports occluded windows. Focus emulation keeps the page rendering without activating the Chrome or Edge window.

If the socket fails before its first image, or no image arrives within five seconds, the viewer releases that lease and takes a fresh one over HTTP. This uses authenticated `frame` commands on the same paired endpoint, with one image awaiting acknowledgement and an ordered input queue capped at 32 events. Done waits for queued input on either transport. Closing during recovery releases any late lease and keeps the agent waiting. HTTP carries JPEG data as base64, so this recovery path uses more bandwidth than the binary WebSocket stream.

Input coordinates use CSS viewport pixels. The viewer accounts for image scaling, letterboxing, and local zoom. Committed Unicode text uses `Input.insertText`; navigation and supported keys use validated commands. Images and controller leases are kept in memory. Browser credentials are never sent to the phone.

New managed Claude connections identify their Bro session through an interpolated HTTP header. Codex uses an environment-backed header, and `bro browse` supplies the same identity. Existing browser sessions retain their tab ownership.

## Verification

The bridge tests cover pairing, origins, response headers, and public viewer assets. Extension tests cover ownership, simultaneous takeover, in-flight work, stale input, Unicode, frame replacement, lease expiry, service-worker recovery, debugger detachment, and a socket-close/Done race.

The live test opens a disposable form in the real browser, sets a test HttpOnly sign-in cookie, and checks the form through a separate mobile viewer at DPR 3. It verifies touch input, Unicode, reconnect, exclusive control, and restoration of the same page identity and storage. A second client tests frame acknowledgement backpressure. It closes only the fixture tab.

```powershell
cargo test --locked -p bro-bridge --lib
cargo test --locked -p bro-core justbrowse --lib
cargo run --locked -p bro-bridge --example browser_live -- C:/Temp/bro-browser-live.json 600
```

While the example runs, execute the live test in another shell with a locally installed Playwright package:

```powershell
node crates/bro-bridge/tests/browser-live.mjs --info C:/Temp/bro-browser-live.json --playwright C:/path/to/playwright-core
```

The example uses an isolated bridge. Restarting it with the same connection file preserves its test port and pairing token. Screenshots and live results go to `.impeccable/review/browser/`. The JustBrowse extension and native host must be rebuilt and reloaded after changes in `F:/chromemcp/JustBrowseMCP`. Mobile changes are in `F:/bro-cli-mobile`; its existing WebView dependency supplies the viewer.

After installation, existing Bro processes keep running their previous code. Reopening the primary Bro window loads the new bridge and desktop entry; terminal sessions kept by the session server remain separate from that window.

### Validation on November 7, 2026

The full bridge suite passed 224 tests with nine existing skips. The real Edge browser passed 13 live checks, including immediate typing followed by Done, screenshot pixel checks for keyboard focus, and decoded portrait/landscape frame sizing. The selected extension and native-host suites passed 29 and 26 checks. Mobile TypeScript checks and the Android debug build passed.

Android emulator testing confirmed native text entry, touch confirmation, larger text, and handback with authentication intact. The installed Windows app also controlled the fixture through its native Wry window; after Done, its agent read the updated confirmation, preserved draft, and valid HttpOnly authentication. Both installed Bro executables matched the release SHA-256 hashes, and existing Bro processes stayed running.

Software keyboard layout, native Android rotation, physical-device gestures, and iOS still need device testing. The Android emulator restarted during the rotation attempt. The full extension TypeScript check still reports pre-existing errors outside the browser-control implementation; extension bundling, native-host type checking, and the selected tests passed.

### Connection recovery validation on November 8, 2026

Chromium and WebKit each passed 15 live checks. A further WebKit run with an older agent session lacking a Bro chat ID passed 16 checks, including finding its waiting tab inside the chat picker. All three runs exercised a blocked WebSocket upgrade, HTTP image delivery, Unicode typing immediately followed by Done, and preservation of the same tab's authentication and JavaScript state. They also delayed image decoding until after disconnect and verified that the late image could not reactivate the controls. Results and screenshots are in `.impeccable/review/browser-connect-fixed/`.

Seven transport tests cover failed and stalled sockets, backpressure, input ordering, failed input, and closing during recovery. The full bridge suite passed 225 tests with nine existing skips. Mobile TypeScript checks, the browser screen's iOS and Android lifecycle tests, build-profile visibility checks, and the existing focus and attention suites passed.

Run transport checks with `node --test crates/bro-bridge/tests/browser-connection.test.mjs`. Add `--legacy true` to the live acceptance command to exercise older agent sessions. In the mobile repository, run `bun test scripts/browser-visibility.test.mjs` to check production visibility and the internal build flags.

### Desktop handoff validation on November 8, 2026

Nine live checks passed through the real browser extension and an isolated bridge. Windows GUI testing used the workspace's **Return control** button, then **Open browser** and the native viewer's **Done** button. Each path returned the exact tab to its owning agent with its draft intact. The workspace prompt fit 1100px and 650px windows and disappeared after completion. The tests also covered a stale return button, an occupied viewer lease, reconnecting the agent, and duplicate completion signals.

The selected extension and native-host suites passed 32 and 27 tests. Eight bridge browser tests and the native viewer-link test passed. To include the desktop buttons in the live acceptance run, pass `--desktop-ui` to `crates/bro-bridge/tests/browser-handoff-live.mjs`; it pauses for each native GUI action against its disposable fixture.
