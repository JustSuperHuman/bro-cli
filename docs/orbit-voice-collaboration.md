# Orbit compatibility review notes

Upstream: https://github.com/Cookiebubba/orbit-voice

Fork: https://github.com/JustSuperHuman/orbit-voice

Inspected base: `f65f437d8df7b5a5b976da6b7bbd79476ebd7232`

Local branches:

- bro: `feat/orbit-voice-integration`
- Orbit: `feat/external-host-compatibility`

The Windows companion is distributed as a [separate compatibility preview](https://github.com/JustSuperHuman/orbit-voice/releases/tag/bro-preview-v1.3.0.1). Its source and portable package retain the upstream license and attribution. No upstream compatibility PR has been submitted.

## Keep the upstream review focused

Review the change in three parts:

1. **Optional OpenAI realtime provider.** Provider event normalization, interrupted playback/truncation, native credential storage and settings. Existing Claude/Grok provider defaults and consent behavior remain covered by their existing regressions.
2. **Optional external workspace backend.** Versioned protected local discovery, capability negotiation, stable session and utterance identities, draft/send consent, correlated non-user results and outcomes. No selected host means the existing Claude Desktop backend; a selected but disconnected host fails closed rather than changing backends.
3. **Native companion attachment and optional workspace stage.** Reuse the original avatar windows, director and throw engine. Explicit desktop/attached placement validates host identity. `avatar.stage.v1` adds a separate, latest-only geometry feed and a persistent client-area stage for Qubit; it does not change voice protocol v1 or execution authority. Old peers retain window-only attachment. Keep voice execution separate from window selection and character navigation.

**Preview packaging is downstream scaffolding**, not a request to rename upstream Orbit. The fork’s app/profile/Keychain identity, update-feed changes and preview-only microphone/hotkey defaults must be kept separate from—or made build-variant-specific in—the compatibility PR. Do not merge those settings as new defaults for ordinary upstream installations.

## Compatibility contract

- Protocol version and negotiated capabilities are authoritative, not a matching package version alone.
- A preview manifest includes the tested base revision and actual payload hashes. Regenerate after changing source or built UI. This is a local integrity/compatibility declaration, not a release signature or evidence that live speech passed.
- Do not rely on private renderer cookies, Claude Desktop metadata impersonation, production test hooks or raw terminal writes. The paired preview's narrowly scoped native system OpenAI resolver does not copy keys into its profile or expose a host credentials endpoint; standalone credential defaults remain unchanged.
- Pin an utterance’s target before speech. Later focus changes may not redirect it; target deletion/restart is an explicit failure.
- Transport receipt, durable queueing, initial acknowledgement and accepted task completion are different states.
- Host results are non-user data. They cannot authorize a send or satisfy a permission prompt.
- Reconnect observes existing work. Unknown delivery is not permission to replay an action.
- A user-owned Orbit instance, profile, provider key or startup preference is not bro’s to replace or terminate.

## Review and acceptance

Run the fork’s compatibility and sandboxed provider/consent suites, typed UI build and source-manifest generation. Validate that manifest through bro-core, then run the strict bro-bridge tests and the bro/App pinned-target regression. Review optional-feature and standalone behavior separately.

Current native preview acceptance is limited. The existing free-avatar harness demonstrated real native avatar visibility, stage geometry, display changes and avatar handover, but its overall run failed a no-child-process check because the standalone-default path attempted stubbed helper/auth probes. It did not run those external processes. Do not represent that harness as a full pass or use it to claim mixed-DPI/virtual-desktop/throw acceptance.

The workspace-stage harness passed separately against an actual isolated bro GUI with three panes and 16 surfaces. Eleven hidden compositor captures show initial preferred-header support and a completed drop onto that exact header, including closely stacked one-way ledges. This validates the live geometry/native-binding/renderer path, not desktop z-order, physical pointer pass-through or mixed-DPI acceptance. See [the local verification record](../reports/orbit-stage/README.md).

Microphone capture and paid provider calls were not approved. A genuine speech → durable bro request → spoken reply remains a manual acceptance gate. Legacy audio startup is disabled in bro, but final removal of the dormant legacy engine is held behind that gate. Existing sounds/cues and the shared durable inbox remain intact.

Native Swift OpenAI/external-host parity and a signed compatible installer are not part of this Windows source preview. Update controls must continue to report unavailable until real tested artifacts exist.
