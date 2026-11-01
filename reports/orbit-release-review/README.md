The Orbit Voice release review covers Bro 2.1.22 and the Windows x64 companion preview `1.3.0-bro-preview.1`. The settings retain Bro's existing typography and controls. This pass shortened repeated instructions, clarified the conversation action and kept setup controls hidden until the connection and key source are ready.

The ready state has one primary action, Start voice, beside the current microphone status. Provider and key-source context remain visible before starting. Model and voice rows share the provider's next-conversation guidance. Companion version details no longer replace a healthy connection status with an update error. Keyboard navigation now follows the available Orbit controls instead of the retired push-to-talk settings.

Verification completed on Windows:

- All 1,389 Bro workspace library and binary tests passed. Another 55 tests remain explicitly ignored because they require separate environments, devices or provider access.
- All 586 enabled Orbit unit and sandboxed tests passed. The typed React UI production build passed.
- The native Electron acceptance test passed against the extracted release package, using its included runtime and production dependencies. It verified hidden startup, native controls, an occupied requested port, owner handoff and recovery after the saved runtime record was removed. It retained one renderer and one simulated provider connection across recovery.
- The portable installation script passed with an isolated Bro data folder and local Programs folder. The resulting registration pointed to an existing executable and a validated compatibility manifest.
- The settings capture matrix produced 24 images: off, connecting, error, setup, ready and talking states at 1120×820 and 760×700, in graphite and paper themes. Recovery controls wrap at narrow widths. These captures use demo state and never open a microphone.
- Separate screenshots from the freshly installed viewer showed the actual connected companion in both themes. They were sent to the user's Telegram DM. The original host and all 18 baseline sessions retained their process identities; the live companion remained ready with the microphone off.
- Both installed Windows executables matched their release-build SHA-256 hashes. A fresh installed Bro process reported version 2.1.22.

The companion archive includes Electron and production dependencies, with a SHA-256 checksum. Its published GitHub asset digest matched the local archive. The preview uses its own profile and chooses an available IPv4 loopback port.

Live microphone and paid-provider acceptance were not performed in this review. Native recovery testing used simulated audio and provider connections. The companion preview targets Windows x64; native macOS and Linux companion support and automatic companion updates remain outside this release.
