# Remote desktop

Open **Settings → Mobile Companion** and enable **Remote desktop**. Bro downloads Bro RMM from its [separate Voxl fork](https://github.com/JustSuperHuman/voxl), verifies its signature and file hashes, and keeps it outside the Bro installation. The package is never included in Bro’s binaries or release ZIP.

On your paired phone, choose **Take Control**. You can also watch without sending input. The desktop settings can restrict every phone to view only, choose the shared monitor, or disconnect all viewers. Turning remote desktop off ends every connection. Only one device can control the computer at a time, including across separate Bro windows.

The mobile viewer uses native video decoding. Trackpad mode moves the pointer relative to your finger; tap to click, double-tap for a double-click, and hold to drag. Two fingers scroll. The viewer also has zoom, keyboard controls and clipboard actions. The phone releases held inputs when it disconnects or goes into the background.

WebRTC tries a direct connection over UDP, with an optional TURN relay. The Windows WebRTC host currently uses UDP; the relay also accepts TCP and TLS from clients that support them. Auto mode falls back to the native WebSocket viewer if WebRTC cannot deliver video. This TCP fallback uses the same address and pairing token as your Bro sessions.

This preview supports Windows x64. It runs in your Windows session, so it cannot control the lock screen or an elevated UAC prompt. Someone at the computer must handle those screens. The phone still needs a reachable Bro address to pair and negotiate; TURN relays the media connection.

## Package maintenance

The [fork](https://github.com/JustSuperHuman/voxl) owns capture, encoding, input and transport changes. Bro’s gateway uses protocol version 1. Compatible RMM releases can update independently through the signed `bro-rmm-channel-v1` catalog. Bro checks the catalog with its pinned Ed25519 public key, then checks the archive and executable hashes before installing. It retains a verified installed package when the update server is unreachable.

Use **Repair download** in Mobile Companion settings to check the package channel again. This ends active remote connections. Bro starts the streaming process when the next viewer connects and stops it after the last viewer leaves. Existing terminal sessions keep running.

The package lives in `<BRO_DIR>/rmm/packages/protocol-v1`. Optional relay configuration lives in `<BRO_DIR>/rmm/relay.json`; it contains a credential broker URL and a private per-install token. The token stays on the computer. Phones receive temporary TURN credentials, and long connections refresh before those credentials expire. Keep relay configuration out of source control.

Voxl’s source retains its MIT attribution. The separately distributed binary links x264 and includes its GPL license and corresponding source instructions in the RMM release.
