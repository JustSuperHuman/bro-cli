# JustBrowse remote browser integration

## Which existing paths should carry the feature?

### Takeaway

Keep the signed-in tab in its current Chrome process and expose its pixels and input through Bro. JustBrowse already has tab ownership, CDP attachment sharing, native transport, and movement of an existing tab into a separate window.

### Cited findings

- The working repository is `F:/chromemcp/JustBrowseMCP`; its native Fastify server binds to `127.0.0.1:12306`. The top-level `F:/chromemcp` directory is not a Git repository. [Server configuration](/F:/chromemcp/JustBrowseMCP/app/native-server/src/constant/index.ts)
- Native commands use `NativeMessagingHost.sendRequestToExtensionAndWait(payload, type, timeout)`. The extension receives `CALL_TOOL` in `entrypoints/background/native-host.ts`, invokes `handleCallTool`, and returns a wrapper with `payload: {status: 'success', data: ToolResult}`. Consumers must inspect the tool's `isError` as well as the outer status. [Native host](/F:/chromemcp/JustBrowseMCP/app/native-server/src/native-messaging-host.ts), [Extension dispatch](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/native-host.ts)
- `tools/index.ts` strips private `__justBrowse` context, calls `browserSessionManager.prepare` before effects, executes the tool, then records its result. Explicit tab selection claims the tab before execution; another MCP session cannot claim it. Context-free calls currently bypass that session manager. [Tool dispatcher](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/index.ts), [Browser session manager](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/browser-session-manager.ts)
- Session leases persist in `chrome.storage.session`, which survives service-worker suspension and resets with the browser. Logical tab selection does not change Chrome focus. `releaseSession` releases leases without closing tabs. [Browser session manager](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/browser-session-manager.ts)
- `utils/cdp-session-manager.ts` shares per-tab debugger attachments by owner and reference count, serializes commands with a per-tab lock, applies 15-second timeouts, and clears state on detach. [CDP manager](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/utils/cdp-session-manager.ts)
- `tools/browser/move-tab.ts` uses `chrome.windows.create({tabId, focused})` or `chrome.tabs.move` to move an existing tab. Chrome documents tab moves between normal windows. [Existing move tool](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/browser/move-tab.ts), [Chrome tabs API](https://developer.chrome.com/docs/extensions/reference/api/tabs#method-move)
- Before routes execute, the native server rejects untrusted browser Origins. Its allowlist accepts extension Origins and `http://127.0.0.1` with any port; requests without Origin are allowed. [Request hook](/F:/chromemcp/JustBrowseMCP/app/native-server/src/server/index.ts), [Origin validation](/F:/chromemcp/JustBrowseMCP/app/native-server/src/server/cors-origin.ts)

### Inferences

- Add explicit remote-browser commands to the native transport, and proxy them behind Bro's existing authenticated device connection. Retain the loopback-only native listener. Avoid exposing the entire MCP HTTP server to mobile.
- Preserve the originating MCP session's tab lease throughout takeover. Add a separate human-control lease checked by the dispatcher, including context-free tools, so the agent cannot click or navigate while the user is interacting.
- Record original window ID, tab position, and active state before optional detachment. Restore the same tab on return when that window still exists. A dedicated Chrome window provides a bounded browser surface, but it is not an operating-system container.

### Gaps

- No existing remote-browser stream endpoint or human-control lease was found in the inspected bridge paths. Implementation and live browser verification remain required.

## What protocol supports mobile streaming and input?

### Takeaway

Use a tab-scoped CDP screencast with bounded buffering and explicit client acknowledgments. Mobile and desktop can consume one protocol while the browser retains all site state.

### Cited findings

- Chrome's debugger API allows the Page, Input, Emulation, and Runtime domains. Detach events occur when a tab closes or DevTools opens; enterprise host restrictions can also detach it. [Chrome debugger API](https://developer.chrome.com/docs/extensions/reference/api/debugger)
- `Page.startScreencast` accepts JPEG/PNG, quality, maximum dimensions, and frame sampling. Current tip-of-tree schema also includes `maxFramesInFlight` and `sendLastFrame`; the default frames-in-flight limit is three. These are experimental commands, so installed Chrome compatibility must be tested. Frame events contain base64 image data, metadata, and the frame number used in `Page.screencastFrameAck`. [Official protocol schema](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/json/browser_protocol.json)
- Existing screenshot capture has a one-frame `Page.startScreencast` fallback and calls `Page.stopScreencast` in cleanup. Concurrent use could stop a long-lived remote stream. [Screenshot fallback](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/browser/screenshot.ts)
- DevTools' own screencast uses JPEG and scales images according to `metadata.deviceWidth`. Its input mapping uses `x = localX / screenZoom` and `y = localY / screenZoom - offsetTop`; viewport mouse input does not add document scroll offsets. Wheel deltas are scaled by the same zoom. [DevTools ScreencastView](https://raw.githubusercontent.com/ChromeDevTools/devtools-frontend/main/front_end/panels/screencast/ScreencastView.ts), [DevTools InputModel](https://raw.githubusercontent.com/ChromeDevTools/devtools-frontend/main/front_end/panels/screencast/InputModel.ts)
- Native messaging carries length-prefixed JSON. Host-to-extension messages have a 1 MB maximum; extension-to-host messages have a 64 MiB maximum. A persistent `connectNative` port keeps the native process running. [Chrome native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
- Extension WebSocket traffic within Chrome's 30-second activity window keeps a service worker alive on Chrome 116 and later. Chrome's example uses a 20-second keepalive. [Chrome service-worker WebSockets](https://developer.chrome.com/docs/extensions/how-to/web-platform/websockets)
- Existing browser input already uses `Input.dispatchMouseEvent` and `Input.insertText` through the shared CDP manager. The official protocol provides touch dispatch and scroll gestures as well. [Existing input](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/entrypoints/background/tools/browser/computer.ts), [CDP Input](https://chromedevtools.github.io/devtools-protocol/tot/Input/)

### Inferences

- Start with JPEG quality 65 and bounded dimensions; these are proposed defaults, not measured requirements. Cache one latest frame, send a frame only when the viewer has capacity, and discard superseded frames. Acknowledge or discard CDP frames deliberately so a slow mobile connection cannot accumulate an unbounded queue.
- Persist session metadata independently from frame data. Stop screencast and release its CDP owner when the final viewer disconnects or a short reconnect grace expires. Closing the viewer must leave the tab open.
- Permit one active controller and multiple viewers. Include a monotonically changing control token and frame sequence in input messages; reject stale or unauthorized input. Serialize input with takeover/release so an in-flight agent action cannot race the handoff.
- For text, use `Input.insertText` so mobile IMEs and emoji do not require synthetic key sequences. Send control keys separately. Touch coordinates should be normalized against the displayed image rectangle, with letterboxing removed before mapping to viewport coordinates.
- Reuse one long-lived screencast owner and arbitrate the screenshot tool's one-frame fallback against it. Clear any temporary Emulation overrides on release.

### Gaps

- Minimized, occluded, and inactive-tab rendering needs live Chrome testing. The existing CDP screenshot tests use mocks and establish focus behavior only at the API-call level.
- Native browser UI, passkey prompts, file pickers, and permission bubbles are outside page pixels and require a separate native interaction path when encountered.
- A Docker-managed Chromium browser can implement the same viewer protocol, but loading a Windows user's live profile into it is a separate session-transfer problem. This bridge inspection does not establish that transfer as possible.

## What has been tested and what should be verified next?

### Takeaway

The existing isolation and transport tests pass. They provide a useful baseline; none proves a complete mobile takeover yet.

### Cited findings

- Ran `node node_modules/vitest/vitest.mjs run tests/browser-session-manager.test.ts tests/cdp-session-manager.test.ts tests/background-screenshot.test.ts tests/move-tab.test.ts` from the extension directory: all four files and 19 tests passed. Coverage includes preserving Chrome focus, tab claims, timeout cleanup, and existing-tab moves. [Extension tests](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/tests/browser-session-manager.test.ts), [Screenshot tests](/F:/chromemcp/JustBrowseMCP/app/chrome-extension/tests/background-screenshot.test.ts)
- Ran `node node_modules/jest/bin/jest.js --runInBand --coverage=false src/server/cors.test.ts src/server/server.test.ts src/native-message-reader.test.ts` from the native-server directory: three suites and 52 tests passed with exit code zero. The initial selected-suite run passed the same tests but failed unrelated whole-project coverage thresholds; disabling coverage for the selected baseline resolved that command issue. [Native server tests](/F:/chromemcp/JustBrowseMCP/app/native-server/src/server/server.test.ts), [Origin tests](/F:/chromemcp/JustBrowseMCP/app/native-server/src/server/cors.test.ts), [Framing tests](/F:/chromemcp/JustBrowseMCP/app/native-server/src/native-message-reader.test.ts)
- `pnpm` is absent from this shell's PATH; installed package-local Node entry points work. [Local package scripts](/F:/chromemcp/JustBrowseMCP/package.json)

### Inferences

- The acceptance fixture should create a page with a cookie, localStorage, IndexedDB, sessionStorage, an in-memory counter, an unsent form, scroll position, and a popup. Record tab ID and state before detachment, interact through the remote viewer, then return and compare all values in the original tab.
- Exercise takeover during an agent action, a second viewer claiming control, disconnect/reconnect, browser navigation, DevTools detach, extension restart, closed source window, and frame backpressure. Verify that stale input is rejected and another agent's leased tabs remain untouched.
- Measure mobile frame size, input-to-visible-update delay, and memory during a deliberately slow connection. Test portrait and landscape plus a reduced-motion preference. These measurements are still outstanding.

### Gaps

- Research notes are the only file authored by this research subtask. The parent must perform the repository-required Bro rebuild/install/hash verification after implementation and documentation changes before reporting completion.
