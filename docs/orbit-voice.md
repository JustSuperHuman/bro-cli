# Orbit Voice

Orbit Voice connects your workspace to Orbit’s native companion. Orbit handles realtime conversation and its moving character; bro keeps ownership of sessions, permissions and durable task delivery.

- [Integration fork](https://github.com/JustSuperHuman/orbit-voice)
- [Original Orbit project](https://github.com/Cookiebubba/orbit-voice)
- [Local protocol](orbit-voice-protocol.md)

## Compatibility preview

The first version uses a separate Windows x64 compatibility preview. Download the [tested companion package](https://github.com/JustSuperHuman/orbit-voice/releases/tag/bro-preview-v1.3.0.1), extract it and run `install-bro-preview.ps1` in PowerShell. The script installs the companion into your local Programs folder and registers its paths with Bro. Enable **Settings → Orbit Voice** to connect. You can also register the extracted `runtime/electron.exe` and `app` folder with `bro orbit register`.

The package includes its runtime, so you do not need Node.js or a source build. Automatic companion updates are not available in this preview. Before installing a newer companion, choose **Quit companion** in Bro, then install and resume it. Stock Orbit downloads use a different connection and remain separate.

The preview must have its own application identity, profile and update channel. It must not replace an existing Orbit installation, import its saved credentials or update itself back to an incompatible upstream build. If another Orbit instance is active, close it yourself before switching previews; bro never terminates an unrelated companion.

Developers can build the fork using its compatibility-preview instructions and point Bro to that build. For an Electron source preview:

```toml
[orbit_voice]
enabled = false
mode = "attached"
allow_escape = false
keep_in_background = true
executable = "C:/path/to/orbit-voice/node_modules/electron/dist/electron.exe"
application_dir = "C:/path/to/orbit-voice"
```

Explicit environment overrides are available as `BRO_ORBIT_EXECUTABLE` and `BRO_ORBIT_APPLICATION_DIR`. The preview’s compatibility manifest must match its actual source. A marker file next to an ordinary Orbit executable is not enough.

For an installation that bro can reload while running, register the built fork:

```powershell
bro orbit register "F:/orbit-voice-bro/node_modules/electron/dist/electron.exe" "F:/orbit-voice-bro"
```

This validates the actual payload and writes an installer handoff at `<BRO_DIR>/orbit/installed-preview.json`, separate from ordinary settings saves. In **Settings → Orbit Voice**, enable the connection or choose **Try connection**. Bro validates the selected build again, merges only its launch paths, and starts its hidden companion. Queued saves and stale settings forms cannot restore old paths. No sessions are restarted and no credentials are imported.

Bro reconnects an already-running managed companion automatically, including when its saved runtime record is missing. It checks the selected executable, application folder, original pairing file and native process lifetime, then waits for an authenticated heartbeat. If that companion is stuck or still closing, Bro restarts it automatically. An old runtime record also cannot prevent an enabled connection from starting again. Recovery does not start listening or open a provider session, and it respects **Quit companion** until you explicitly resume it. Orbit instances paired elsewhere remain under their existing owner's control.

The settings page follows setup order:

1. **Connect Orbit.** Until the runtime is ready, only connection and recovery controls are shown. Connecting does not open a microphone or start a paid provider session.
2. **Set up your voice.** Once connected, choose a realtime provider and use **Set up voice** to select a key source in Orbit’s secure settings. Unavailable conversation and voice controls remain hidden.
3. **Start a conversation.** Once a key is available, Start/Stop and voice preferences appear in bro. Mute appears during an active conversation; it stops sending audio but does not release the microphone. Stop releases the device and provider connection.

**Advanced settings** opens the existing Orbit renderer. Closing settings hides it without ending voice. **Quit companion** stops the managed runtime until you explicitly resume it. Enabled startup and recovered workspace results never start listening on their own. Viewer and secondary windows relay native voice controls to the primary owner rather than launching another runtime.

The paired preview can use your existing system `OPENAI_API_KEY` without copying it into Orbit’s profile. A separately saved Orbit key takes precedence unless you select another source. Native lookup checks the process environment, then Windows user and machine environment values. The UI reports the source, never the key. Removing a saved Orbit key does not change your system key.

### A preview checksum failed

The selected source no longer matches its compatibility manifest. Bro refuses that build rather than bypassing validation. In the explicitly selected preview checkout, rebuild the UI and regenerate the manifest:

```sh
npm --prefix ui run build
npm run compatibility:manifest
```

Then choose **Try connection**. A running companion still uses the code it loaded earlier; rebuilding files does not update that process. Bro waits for its heartbeat and automatically restarts the companion paired with this workspace if its loaded build no longer matches. Another Orbit instance keeps its existing connection.

Standalone Orbit retains its saved-key default. Selecting or refreshing a source does not start the microphone or test a paid connection. If a saved store cannot be decrypted, the preview reports that error rather than silently choosing a different billing account. Source changes apply to the next conversation; an authentication error does not authorize trying another account.

## Realtime stays realtime

The integration retains live conversation, interruption and asynchronous task replies. The optional OpenAI realtime provider belongs in Orbit, not a second audio engine in bro. Existing Orbit Grok and Claude engines keep their standalone behavior. Provider-native event handling is separate from the bro host protocol; bro does not relay audio.

Session context you ask Orbit to read may be sent to your selected voice provider. Credentials remain under Orbit’s control. Live microphone/provider acceptance must be tested explicitly; mocked audio alone is not proof that recognition or playback works on a machine.

## The moving character

Qubit stays inside bro by default. Old Desktop or Automatic preferences do not grant permission to escape. The **Allow Qubit to escape bro** checkbox is the explicit opt-in. With a stage-capable preview, pane headers, frame rails, project tabs and the conversation composer become ledges. Terminal output is not an obstacle. Walking, hopping, dragging and throws retain Orbit’s physics and navigation engine.

The optional `avatar.stage.v1` feed uses the final painted layout and the native client’s origin and scale. Closing or hiding a pane removes its surfaces. Missing, invalid or expired geometry removes the old colliders; modal layers, card flips and visible native child webviews conservatively suspend the character. Geometry travels separately from voice, session context and task results.

With escape enabled, Qubit can move between bro and the desktop within one display’s usable work area. Dragging, throws and reachable walking routes share one continuous world. Crossing the boundary does not create another character or voice runtime. Bro owns the saved permission; Orbit owns the character’s transient position.

Escape requires a peer with `avatar.roam.v1` and explicit containment-permission support. Older peers remain contained, and other selected avatars retain their existing behavior. It does not provide continuous cross-monitor dragging or flight, or discover obstacles in other applications. A host moving underneath a detached character does not pull him into it. When bro is minimized, covered or its geometry expires, its region cannot supply support or accept entry; an outside desktop character need not disappear.

Navigation follows reachable routes, not a coordinate animation through walls. A full-height divider can separate parts of the stage; you can still pick the character up and move it across.

Older compatible peers retain window-only attachment. Layout-aware collisions currently target Qubit; other avatars are not silently replaced or advertised as having equivalent collision support. Native mixed-DPI, stacking, pointer pass-through and live drag/throw acceptance must be checked on the actual desktop before claiming full platform acceptance.

Without a usable native bro window, a contained Qubit is hidden; missing geometry never grants desktop permission. Background voice and accepted-work handling are independent of character visibility. Keeping the companion available after bro closes also respects the existing keep-sessions and keep-working settings. Windows x64 is the current managed preview target. Native macOS attachment and managed lifecycle need their own implementation and acceptance.

## Work and replies

- Each utterance keeps the session selected when speech starts. Switching tabs while speaking does not redirect the request.
- Spoken tasks enter bro’s durable orchestrator inbox with that session as context. They are not blindly pasted into a terminal.
- Transport receipt is not durable acceptance. A queued acknowledgement is sent only after saving the request.
- Initial coordinator replies and accepted task outcomes remain distinct.
- Stopping speech does not cancel work. Cancellation targets one queued request; already-dispatched work is never interrupted through an unscoped stop.
- Reconnects do not replay uncertain actions. Inspect the original conversation before retrying completed or uncertain work.
- A generated reply cannot act as a user confirmation or permission answer.

The local connection has its own authentication and does not enable LAN/mobile access. Disconnecting it does not quit Orbit or close sessions. An intentional Orbit quit is not automatically undone.

## Collaboration and updates

Keep the optional external-host interface and realtime provider changes reviewable upstream. The compatibility work must preserve default standalone Orbit behavior, consent safeguards and native animation. See the fork’s compatibility notes for the tested upstream revision and remaining platform checks.

bro will only offer an install/update when an actual tested artifact advertises compatible protocol, platform, channel and capabilities. Fork and upstream updater feeds must not silently cross over. Publishing a compatibility PR or release is a separate review step.
