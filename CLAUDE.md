# bro v2

Rust workspace; see PLAN.md for the product plan.

- `crates/bro-core`   blocking data layer (config/profiles/providers/sessions/usage/creds/launch/browser). v1-compatible storage in `~/.bro`, `~/.claude-max-pool`, `~/.bro/codex-profiles`.
- `crates/bro-proxy`  axum proxy, Anthropic <-> OpenAI translation + Claude pool. Own tokio runtime thread.
- `crates/bro-bridge` axum bridge host compatible with Just Terminal mobile/web clients. Own tokio runtime thread.
- `crates/bro-tui`    the `bro` binary (ratatui + vt100 + portable-pty, derived from z4-oriel).

Reference sources (read-only, never edit): `F:\bro-cli` (v1, Node), `F:\z4-oriel` (TUI), `F:\terminal\src\cascadia\TerminalConnection\rust-bridge` (bridge).

Rules
- Public items in each crate's `lib.rs` / module stubs are cross-crate contracts: add freely, never rename/remove without updating all callers.
- Nothing slow on the TUI thread: background threads publish state and wake the loop.
- Tests must not touch the real `~/.bro`, `~/.claude`, `~/.codex`: point `BRO_DIR`, `CLAUDE_POOL_DIR`, `BRO_CODEX_PROFILES_DIR`, `HOME`/`USERPROFILE` at a tempdir. Network tests are `#[ignore]`.
- Atomic writes (temp + rename) for every file bro shares with v1.
- Build a single crate with its own target dir when working in parallel: `CARGO_TARGET_DIR=target-<crate> cargo test -p <crate>`.
