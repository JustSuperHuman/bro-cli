# Orbit external host protocol v1

Orbit owns realtime audio, provider credentials, consent and the native character. The external host owns sessions and execution. This protocol carries control and results, never PCM audio. Standalone Orbit stays unchanged without an explicitly selected host.

## Discovery and trust

The host writes a same-user protected descriptor to `<BRO_DIR>/orbit/connection.json`:

```json
{"protocol":1,"instanceId":"uuid","url":"http://127.0.0.1:port","token":"random-secret"}
```

The native Orbit process receives the descriptor **path**, not the token, via `--external-host-file <path>` (or `ORBIT_EXTERNAL_HOST_FILE` for development). Tokens stay out of renderer state, arguments, URLs and logs. Every endpoint requires `Authorization: Bearer <token>`, including loopback. Reject browser Origins, non-loopback peers and incorrect Host headers. No CORS. Bound request bodies and retained events. Rotate credentials per host launch and remove only the descriptor this instance owns. Windows files require a current-user ACL; Unix files require mode 0600 in a private directory.

## Endpoints

All paths below begin `/orbit/v1`.

- `GET /host`: protocol/capabilities and safe host snapshot.
- `POST /utterances`, `{id}`: latch the current target at audio/utterance start. Returns `{id,target:{sessionId,generation}|null,instanceId}`. Reject conflicting reuse. A new selected tab never retargets an existing utterance.
- `POST /requests`, `{id,utteranceId,text}`: submit explicit user work referencing the latched utterance. Returns request state. `received` is transport receipt only; **`queued` means durably saved**. A retry of the same ID/content never executes again; changed content conflicts.
- `GET /requests/:id`: request status, progress and result. States: `received`, `queued`, `dispatched`, `completed`, `failed`, `cancelled`, `uncertain`. A recovered completion may omit result text rather than rerun work.
- `POST /requests/:id/cancel`: cancel this request only. A host may reject cancellation it cannot safely target; never convert it to an unscoped stop.
- `POST /focus`, `{sessionId}`: focus an existing advertised session.
- `GET /events?after=<sequence>`: bounded ordered event batch `{events,cursor,gap}`. On a gap, refresh host/request snapshots. Polling this control channel does not replace the provider's realtime audio WebSocket.

Initial advertised capabilities: `sessions.read`, `sessions.focus`, `orchestrator.submit`, `requests.observe`, `requests.cancel-queued`, `avatar.desktop`, `avatar.attached`. Other operations are unsupported unless explicitly negotiated. No raw terminal writes, generic shell, filesystem or administration endpoint.

## Host snapshot

```json
{"protocol":1,"instanceId":"uuid","name":"bro","capabilities":[],"mode":"desktop","window":{"handle":"123","pid":123},"focusedSessionId":"session-id","sessions":[{"id":"session-id","generation":"opaque","title":"Name","agent":"claude","activity":"working","context":"bounded plain text"}]}
```

`window` is null for a non-native/background host. Native host identity must be validated against its PID before attaching; a reused handle is not authority. Session IDs/generations, not labels, identify targets. Mode is `desktop` or `attached`. The character uses Orbit's actual native animation/drag/throw runtime.

## Replies and consent

Events contain `{sequence,type,requestId,...}`. Use typed request state/progress/result fields; host results are **non-user data** and may never enter Orbit's `ui.text`, authorize a send, or answer a permission prompt. Stop speaking, cancel work and interrupt a session are separate actions. Final task acceptance is not an initial coordinator acknowledgement.

The bro adapter routes spoken work through its durable orchestrator inbox with the latched viewed session as context. It does not paste into the currently focused terminal. It preserves schema-1 inbox state and accepted coordination outcomes. Uncertain sent work is observed, never blindly replayed after reconnect.

## Realtime compatibility

The bro integration requires realtime conversation (including interruption and live tool/delegation replies), not a replacement record/transcribe/playback pipeline. Orbit's existing Grok and Claude engines remain available and unchanged. Add the existing OpenAI realtime behavior as an optional Orbit provider, with credentials owned by Orbit and provider protocol handling tested independently from host/session transport. The renderer consumes one normalized audio/tool event interface; provider-native event shapes and secrets stay server-side.

## Optional workspace stage: `avatar.stage.v1`

This capability is independent of audio/session protocol v1. Old peers ignore it and retain window-only attachment. Geometry grants no execution authority. It is never placed in the request-event ring or lost when the normal host/session snapshot is replaced.

`GET /orbit/v1/stage?after=<revision>` uses the same strict local authentication. It returns immediately on change, or a heartbeat within one second with the current revision. One independent, abortable stage request may be outstanding per native client. Updates are latest-only, coalesced to at most 30 Hz; successful unchanged heartbeats renew freshness. A stage without a successful heartbeat for two seconds expires independently of the voice/control connection.

```json
{"schema":1,"instanceId":"uuid","revision":12,"available":true,"frame":{"window":{"handle":"123","pid":123,"epoch":"window-1"},"viewport":{"width":1200,"height":800,"scaleFactor":1.5,"physicalOrigin":{"x":-1200,"y":45}},"suspended":false,"surfaces":[{"id":"pane-example-header","kind":"oneWay","rect":{"x":250,"y":80,"w":700,"h":34}}],"exclusions":[],"preferredSurfaceId":"pane-example-header"}}
```

`frame:null,available:false` clears old geometry. An invalid frame also clears/suspends stage data without invalidating valid session context. `/host` advertises the optional capability and safe `stage:{schema:1,revision,available}` metadata, not the full geometry.

Coordinates are **client-local logical pixels**, y down. `physicalOrigin` is the actual client origin on the physical desktop, not an outer-window or saved placement origin. `width/height × scaleFactor` match the current native client size; native tracking rebases ordinary movement and rejects stale DPI/viewport identities. Negative screen origins are valid. The window handle/PID must match the current host and native owner; `epoch` distinguishes replacements. No fresh native-helper roundtrip is needed per geometry frame.

Limits: 96 surfaces, 32 exclusions, 64 KiB encoded stage frame; nonempty stable IDs up to 160 safe ASCII characters; finite bounds and positive dimensions within the viewport. Kinds are `solid` (sides/top/underside) and `oneWay` (descending top contact). Exclusions are no-occupancy regions; suspend rather than cover a modal or unsupported native child. `preferredSurfaceId` is optional and must name a current surface. It affects character navigation, never session targeting. Unknown stage schemas must not be treated as usable collision data.

Bro publishes the final painted layout through a typed latest-value mailbox. The native shell keeps a persistent host-client stage distinct from temporary drag/throw lift. The same sanitized surface list feeds navigation, walking, collision/throw physics and valid landing/recovery. Missing/expired geometry removes invisible colliders; Desktop and standalone defaults stay unchanged.

## Optional Automatic placement: `avatar.roam.v1`

This extension uses the existing stage feed; it adds no movement, credentials or execution endpoint. A host may advertise `avatar.roam.v1` and publish `avatarRoam:{"schema":1,"policy":"auto"}`. The capability remains optional. Unknown/malformed policy data is ignored independently of valid session context.

`mode` retains its protocol-v1 values, `desktop` or `attached`. It is the fixed fallback for old clients. A roaming-capable client applies a valid Auto policy ahead of that fallback; unchanged host snapshots must not restore initial placement. Capability/manifest advertisement is discovery, not a bilateral acknowledgement or proof that an already-running companion loaded this feature.

Automatic keeps one character/controller and voice identity in a persistent selected-display work area. Fresh bro surfaces are translated into that world; the native client edge is not an invisible collision wall. Dragging, swept throws and physically reachable navigation can cross between bro and the desktop without saving a different mode or recreating the character. Other application windows are not discovered as obstacles.

This increment is one-display only. The full body stays within the selected display's usable work area. Moving bro to another monitor does not carry a detached character there. Continuous cross-monitor drag/flight is unsupported; display loss or invalid DPI triggers safe recovery of the same runtime, not a claim of seamless trajectory preservation.

The character owner is distinct from the optional bro collision region. Missing, expired, occluded, excluded or identity-mismatched geometry revokes support and entry eligibility without destroying an outside character. Geometry reappearing beneath a detached body does not authorize a forced home placement. Bounds/origin/scale and local movement reports are revisioned; stale reports are ignored. Active-gesture-only native cursor/button observations reconcile capture loss and release. Fallback/cancellation ends the hold safely and never triggers click-to-talk or grants execution consent.

## Availability

Stock Orbit currently does not implement this protocol. The first integration uses a local compatibility preview with isolated app/profile identity and updates disabled or pointed at its own compatible channel. Never claim a stock release compatible, invent an unpublished asset URL, replace the user's upstream installation, or terminate an unrelated Orbit instance. A compatible release must advertise protocol and capabilities before bro offers it as an update.
