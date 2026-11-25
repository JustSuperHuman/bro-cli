# Browser session continuity

Research checked on November 7, 2026. This note describes source evidence and a proposed implementation. It does not claim that the implementation has passed a live test.

## Can a signed in Windows browser move into Linux Docker?

### Takeaway

Preserving the existing tab in the existing browser is the strongest route to session continuity. A copied profile or exported storage snapshot cannot promise a faithful transfer of the live session.

### Cited findings

- Chromium stores cookies and other profile data in its user data directory. Its documentation explicitly says two running instances cannot share that directory. A Docker mount is therefore not a supported way to have two browsers concurrently use one profile. [Chromium user data directory](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/user_data_dir.md)
- Windows Chrome introduced App-Bound Encryption for cookies in Chrome 127. Google states that its key is strongly bound to the machine and does not function correctly when profiles roam between machines. [Google Chrome cookie encryption](https://security.googleblog.com/2024/07/improving-security-of-chrome-cookies-on.html?m=1)
- Chromium's encryption implementation validates the Windows user and, for path validation protection, the calling application's path. Copying the profile does not reproduce that environment. [Chromium encryption implementation notes](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/chrome/browser/os_crypt/README.md)
- Chrome 136 stopped honoring remote debugging port and pipe flags against the default data directory. A custom data directory is required for that launch path. [Chrome remote debugging changes](https://developer.chrome.com/blog/remote-debugging-port?hl=en)
- Device Bound Session Credentials bind sessions to a private key protected by the Windows TPM; the server can require proof before issuing fresh cookies. A copied cookie does not provide that proof. [Chrome DBSC announcement](https://developer.chrome.com/blog/dbsc-windows-announcement?hl=en)
- Google announced general availability and default enablement of DBSC for Workspace users in May 2026. This is a production compatibility concern. [Google Workspace DBSC rollout](https://workspaceupdates.googleblog.com/2026/05/prevent-account-takeovers-with-DBSC-now-generally-available-in-the-Chrome-browser-for-Windows.html)
- Playwright storage snapshots cover cookies and local storage, with explicit options for IndexedDB, OPFS, and virtual WebAuthn credentials. The credentials option exports virtual authenticator private keys, and restoring them installs a virtual authenticator that prevents real authenticators from working in that context. It does not export a real Windows TPM credential. [Playwright BrowserContext storageState](https://playwright.dev/docs/api/class-browsercontext#browser-context-storage-state)
- Playwright documents manual sessionStorage save and initialization because its storage-state API does not persist sessionStorage. Its storage state is authentication bootstrap data, not an open renderer checkpoint. [Playwright authentication](https://playwright.dev/docs/auth#session-storage)

### Inferences

- The default backend should be an attached browser tab running on the user's existing host. A dedicated window can contain that same tab while the mobile and desktop clients display its stream. This retains the original storage environment and active page objects.
- A Linux browser container can be a separate provider when isolation is explicitly needed, with a persistent profile volume and its own enrollment. It should not advertise automatic parity with the user's Windows profile. Initial login, device challenges, and server-side token rotation can differ.
- Cookie export plus origin storage replay may work for selected applications under controlled conditions. Bidirectional automatic profile synchronization would require conflict handling and still could not transfer arbitrary page memory or device-bound credentials.

### Gaps

- No primary source provides a general supported Chrome process checkpoint transfer from Windows to a Linux container.
- Whether a particular service accepts copied authentication state depends on that service. The sources do not support a claim that every sign-in can be reused across devices.

## Which protocol preserves the original browser session?

### Takeaway

Use the existing extension as the CDP transport and keep one stable tab identifier throughout handoff. Both clients can use the same authenticated Bro connection and control messages.

### Cited findings

- The extension debugger API attaches to a tab and transports CDP commands and events. Page, Input, Emulation, Runtime, and Storage are supported domains. The manifest needs the debugger permission. The API has an onDetach event when the tab closes or DevTools takes over; enterprise host policy can also block attachment. [Chrome debugger API](https://developer.chrome.com/docs/extensions/reference/api/debugger)
- A new window can be created with the existing tab's tabId. Its type can be normal, with a requested size and focus state. [Chrome windows API](https://developer.chrome.com/docs/extensions/reference/api/windows#method-create)
- tabs.move moves an existing tab to a specified window and index. Movement is restricted to normal windows, which affects later restoration from a dedicated window. [Chrome tabs API](https://developer.chrome.com/docs/extensions/reference/api/tabs#method-move)
- Page.startScreencast emits compressed JPEG or PNG frames and accepts quality, maximum dimensions, and frame sampling. Each frame carries metadata and a sessionId for Page.screencastFrameAck. Metadata includes page scale and screen dimensions; a compressed frame's pixel size is not an input coordinate space. The latest protocol adds optional maximum in-flight frames and sendLastFrame, but these are experimental and may be absent in the installed browser. [CDP Page protocol definition](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Page.pdl)
- Mouse, wheel, and touch input coordinates are relative to the main frame viewport in CSS pixels. Wheel deltas also use CSS pixels. Input.insertText supports text that comes from an IME or emoji keyboard. Touch end and cancel require an empty touch point list. [CDP Input protocol definition](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Input.pdl)
- Emulation.setDeviceMetricsOverride can change page viewport dimensions and the device scale factor. Its mobile option changes viewport meta handling and layout behavior. clearDeviceMetricsOverride removes the override. [CDP Emulation protocol definition](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Emulation.pdl)

### Inferences

Recommended handoff process:

1. The agent creates or claims one owned tab. Bro records its session, tab, and original window identifiers.
2. The agent requests human interaction with the tab identifier and a short reason. Mark that tab as awaiting the user and reject agent mutations while human control is held. The extension or native bridge must enforce this; a prompt alone cannot prevent a race.
3. The mobile user opens that interaction. Move the existing tab into a normal dedicated window if requested. Retain its original window, index, zoom, and focus settings for restoration.
4. Issue an authenticated controller lease. Start the stream for this tab; observers may watch, while only the active lease can send input. Desktop Bro uses the identical lease and event protocol.
5. Prefer a mobile-sized viewport when requested, without overriding the user agent or reloading the page. Report the actual CSS viewport and the frame dimensions separately. Map pointer coordinates through the fitted image rectangle, rejecting taps on letterboxing.
6. Bound frame queues and drop stale frames. Acknowledge consumed or intentionally dropped frames so the browser does not stall. Stop streaming while there are no viewers. Treat the latest protocol options as negotiated improvements.
7. Send gestures, text, and navigation only to the lease's tab. Handle disconnect by retaining the page and permitting reconnection to the same handoff.
8. The user selects Done. Revoke human input, clear any viewport override, stop unused streaming, and restore the tab when requested. Only then signal that the agent can resume against the same tab.

These are design recommendations based on the APIs above, not claims that Bro currently implements them.

### Gaps

- Page streaming does not promise that native browser permission sheets, OS passkey dialogs, file pickers, or DRM content will be visible or actionable. UI should expose a clear recovery route for these cases.
- Visibility, occlusion, minimized windows, display lock, and the particular installed Chrome version need direct testing. A dedicated window is a placement mechanism, not OS or container isolation.

## How should continuity be verified?

### Takeaway

Test continuity with a controlled origin and a live original tab before testing third-party sign-in workflows. The source evidence above establishes constraints; it does not replace runtime validation.

### Cited findings

- Chrome exposes tab discard and freeze state; discarded content reloads when reactivated. Keep this condition visible in the handoff lifecycle. [Chrome tabs API](https://developer.chrome.com/docs/extensions/reference/api/tabs)
- A debugger can detach when DevTools opens, requiring an explicit stream error and recovery state. [Chrome debugger API](https://developer.chrome.com/docs/extensions/reference/api/debugger#event-onDetach)

### Inferences

Suggested acceptance tests:

- Seed a localhost page with a server cookie, localStorage, sessionStorage, IndexedDB data, unsent form text, and a random JavaScript variable that is never persisted. Move the same tab, take control on mobile, change values, finish, and verify them through the resumed agent. Check that the tab identifier remains stable and no navigation or reload occurred.
- Confirm another tab in the same host profile observes cookie and origin storage changes subject to the browser's normal rules. It should retain its own sessionStorage.
- Exercise taps at image edges, letterboxing, orientation changes, wheel scrolling, Unicode input, Enter, Backspace, and a short drag. Check the page's recorded events and resulting DOM.
- Run simultaneous human and agent input attempts. Expect an explicit ownership error, with no page mutation from the losing controller. Repeat across two clients.
- Disconnect the mobile client during an edit, reconnect, and verify the edit and tab remain. Verify the agent stays paused until Done or explicit release.
- Open DevTools, close the handoff tab, and restart the bridge independently. Verify bounded reconnect behavior, clear errors, and no control of a replacement or unrelated tab.
- Measure displayed frame latency and bytes transferred with slow-network simulation. Ensure frame memory does not grow with an unattended client.

### Gaps

- These are proposed tests. Live measurements and pass/fail evidence must come from the implementation work.
