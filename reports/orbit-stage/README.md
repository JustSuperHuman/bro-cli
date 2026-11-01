# Orbit workspace-stage verification

Verified on Windows x64 on 2026-10-31. This is a local source preview, not a published Orbit release.

## What passed

- A fresh installed bro process opened an isolated, inactive GUI with three no-profile PowerShell panes. Its authenticated stage feed published 16 painted surfaces, the actual native client bounds, window epoch and active pane header.
- The real Orbit external-host client, native window tracker, avatar binding, persistent avatar host and Qubit renderer consumed that feed. The supplied-host harness passed with 11 compositor captures and no recorded errors.
- Qubit initially rested on the advertised preferred header, was held and released through its throw controller, and settled on that same stable surface ID. The lower-right header was only 9 pixels below another one-way ledge; the earlier incorrect upper-edge contact was reproduced and fixed with workspace-only support/contact rules.
- The persistent stage did not use temporary lift or return-to-strip recovery.
- Unchanged stage heartbeats arrived in about 900 ms with the same revision. Geometry and request history remained separate.
- Bro's targeted Orbit tests passed: 16 ordinary tests plus explicit stage-worker and pinned-request loopback tests. CLI parsing, terminal-only checking and the release build of both binaries passed.
- The generated Orbit manifest passed bro-core's actual-payload validation after regeneration.

## Evidence

- `workspace.png`: bro's own rendered three-pane layout.
- `character-00-perched.png` through `character-10-settled.png`: captures of the isolated Qubit WebContents. They have no bro background; compare their client-local coordinates with the workspace image.
- `captures.json`: selected surface, preferred/home/support identities, real rotated body extents, native geometry and per-frame states.

The captures use the actual bro host and native identity checks, but the character window is hidden/offscreen. They are **compositor evidence**, not screenshots of desktop stacking or a claim that physical mouse interaction, click-through, minimize/restore, mixed-DPI monitors or native drag/throw acceptance passed. No unrelated desktop content was captured.

## Remaining limits

Qubit is the collision-aware acceptance target. Other avatars are not silently replaced. Navigation only uses reachable routes: a full-height solid sidebar divider separates regions rather than allowing the character to walk through it. Modal layers, flips and visible native child views conservatively suspend the stage.

The full bridge library suite still has a mod-job timeout outside the Orbit code: `tests::mod_jobs_fail_when_no_mod_takes_or_answers_them` at `crates/bro-bridge/src/tests.rs:2174` reported `the reply never came: Elapsed(())`. The targeted Orbit/stage tests and explicit stage socket test passed.

No microphone, live provider or paid voice call was used. No user-owned bro window, session or Orbit process was terminated. The running apps must be reloaded normally to load new code. No commit, push, PR or release was published.
