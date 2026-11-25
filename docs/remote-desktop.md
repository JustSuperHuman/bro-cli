# Remote desktop

Open **Settings → Remote Control** and enable **Remote desktop**. Bro downloads Bro RMM from its [separate Voxl fork](https://github.com/JustSuperHuman/voxl), verifies its signature and file hashes, and keeps it outside the Bro installation. The package is never included in Bro’s binaries or release ZIP.

On your paired phone, choose **Take Control**. You can also watch without sending input. The desktop settings can restrict every phone to view only, choose the shared monitor, or disconnect all viewers. **Connected now** on the same page lists every phone and browser connected to Bro (bro's app names its device; a browser is read from its user agent), and marks the one watching or controlling the desktop. Turning remote desktop off ends every connection. Only one device can control the computer at a time, including across separate Bro windows.

The mobile viewer uses native video decoding on iPhone and Android. Trackpad mode moves the pointer relative to your finger; tap to click, double-tap for a double-click, and hold to drag. Two fingers scroll, and a flick keeps scrolling. The viewer also has zoom, a row of shortcuts spelled for the computer on the other end (Ctrl+Enter on Windows and Linux, ⌘ Return on a Mac), keyboard controls and clipboard actions. The phone releases held inputs when it disconnects or goes into the background.

WebRTC tries a direct connection over UDP, with an optional TURN relay. The host currently uses UDP; the relay also accepts TCP and TLS from clients that support them. Auto mode falls back to the native WebSocket viewer if WebRTC cannot deliver video. This TCP fallback uses the same address and pairing token as your Bro sessions. The phone still needs a way to reach Bro to pair and negotiate: its address on your network, or your Just Superhuman account from anywhere (see [JustConnect](justconnect.md)). TURN relays the media connection. A computer signed in to Just Superhuman gets its relay from the account, so nothing has to be set up on it.

## Systems

Bro RMM has a package for Windows (x64 and Arm), macOS (Apple silicon and Intel) and Linux (x64 and Arm). Each one has its own signed catalog, so a package only installs on the system it was built for. Until a system's package is published, the settings row says so and **Try again** looks again.

- **Windows.** Bro RMM runs in your Windows session, so it cannot control the lock screen or an elevated UAC prompt. Someone at the computer must handle those screens. Pick the shared monitor in settings.
- **macOS.** macOS lets an app see the screen only with Screen Recording, and move the pointer and type only with Accessibility. Both belong to Bro, which starts Bro RMM. When you turn remote desktop on, Bro shows macOS's own prompts, then opens System Settings for whatever is still off; the **Screen and input** row shows where they stand. After turning on Screen Recording you may need to quit and reopen Bro. A phone that connects before Screen Recording is allowed gets a message saying so, and nothing on the Mac waits for an answer. The lock screen and password prompts need someone at the Mac.
- **Linux.** Bro RMM shares the X11 session in `$DISPLAY`, all of its monitors as one screen. A Wayland desktop would show only its X11 windows, so Bro says to sign in to an Xorg session instead (on Ubuntu, “Ubuntu on Xorg”). Start Bro from the desktop rather than over SSH, so it has a display to share.

## Firewall

A firewall that asks before a program may take connections stops a phone's first connection behind a prompt nobody is there to answer. Bro adds its rules before that can happen, asking for administrator rights once. It asks when you turn on phone access or remote desktop, and once when a window opens with either already on. Turning phone access on writes the rules before the bridge starts listening, so the system never shows its own prompt. If you decline, Bro doesn't ask again on later starts; choose **Allow** on the firewall row in Remote Control settings when you're ready.

- **Windows Firewall.** Bro adds "Bro phone access" for `bro.exe` and "Bro remote desktop" for `voxl.exe`, for every network type, in the rule group "Bro", after one UAC prompt. It also switches off rules that block these two programs, which Windows adds when its prompt is dismissed, because a block rule overrides any allow rule. Rules can't help when Windows blocks all incoming connections, or when group policy manages the firewall; the settings row says which.
- **macOS firewall.** It's off by default. When it's on, Bro adds and unblocks `bro` and `voxl` with `socketfilterfw` after macOS's password dialog. macOS treats an updated program as a new one, so Bro asks again after Bro RMM updates.
- **Linux.** ufw and firewalld don't prompt; they block. When one is on, the settings row says so and gives the command that allows the Bridge page's port.

`bro firewall` shows where each program stands, and `bro firewall allow` adds the rules from a shell.

## Package maintenance

The [fork](https://github.com/JustSuperHuman/voxl) owns capture, encoding, input and transport changes. Bro’s gateway uses protocol version 1. Compatible RMM releases can update independently through the signed `bro-rmm-channel-v1` catalogs: `catalog.json` for Windows x64 (the name earlier Bro releases read) and `catalog-<system>.json` for the others, for example `catalog-macos-aarch64.json`. Bro checks the catalog with its pinned Ed25519 public key, then checks the archive and executable hashes before installing. Windows packages are ZIPs and the others `.tar.gz`, which each system's own `tar` reads. Bro retains a verified installed package when the update server is unreachable.

The line under **Remote desktop** in Remote Control settings shows the installed version. **Check for updates** there (`u` in the terminal) asks the channel for a newer package without touching the running one, so viewers stay connected. A newer package installs straight away when nobody is on the desktop; otherwise the link becomes **Install now**, which ends the desktop connections first. A download that failed shows **Try again**. Bro starts the streaming process when the next viewer connects and stops it after the last viewer leaves. Existing terminal sessions keep running.

The package lives in `<BRO_DIR>/rmm/packages/protocol-v1`. Optional relay configuration lives in `<BRO_DIR>/rmm/relay.json`; it contains a credential broker URL and a private per-install token. Without that file a signed-in Bro asks its Just Superhuman account for relay credentials instead. The token stays on the computer. Phones receive temporary TURN credentials, and long connections refresh before those credentials expire. Keep relay configuration out of source control.

Voxl’s source retains its MIT attribution. The separately distributed binary links x264 and includes its GPL license and corresponding source instructions in the RMM release.
