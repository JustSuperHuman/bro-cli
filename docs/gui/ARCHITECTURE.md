# bro desktop app: architecture

`bro gui` opens the same workspace as the terminal UI in its own window, drawn on the GPU. It is not a
second app with its own logic: every session, view, overlay, key binding and background service is the
terminal UI's, and the desktop app adds the window, pixel layout, drawing and the mouse.

## Crates

| Crate | What it does |
|---|---|
| `crates/bro-gfx` | The renderer. wgpu pipelines for soft shadows, rounded rects (borders, gradients), images and glyphs; text with cosmic-text (shaping, fallback) and swash (rasterising) into glyph atlases; box drawing, blocks, braille and powerline drawn to the cell; runs of punctuation shaped together so font ligatures join up on the cell grid; a theme's effects pass (neon glow from a half-size bloom, CRT phosphor, scanlines, vignette); a see-through swap chain on request (`Options::transparent`: DX12 DirectComposition on Windows, the frame premultiplied). No knowledge of bro. |
| `crates/bro-media` | Decoding for the media sidebar: images (png, jpeg, gif, webp, svg…), video and audio playback (Media Foundation on Windows, ffmpeg elsewhere). |
| `crates/bro-tui` with the `gui` feature (default) | The desktop app itself, in `src/app/gui/`, plus a `gui` child module next to each overlay and view it draws natively. |

## Data flow

```
winit events ──► app/gui/input.rs ──► crossterm KeyEvent ──► App::handle (the terminal UI's key routing)
             └─► app/gui/mouse.rs ──► hit under the pointer ──► App methods / surface `click`
bro threads (services, PTY readers, bridge) ──mpsc──► forwarder thread ──EventLoopProxy──► App::handle
about_to_wait ──► App::after_events ──► request a frame
frame: layout (app/gui/layout.rs) ──► sidebar, tabs, cards, overlays, toasts ──► bro-gfx Scene ──► GPU
```

- Keys are translated to the crossterm events the terminal UI gets, so every binding, the prefix table, overlays'
  key handling and each pane's own keys work unchanged. A few keys are the window's own (font size
  ctrl+= / ctrl+- / ctrl+0, copy ctrl+shift+c, fullscreen F11): `Gui::gui_key`.
- Terminals: each session's PTY output is parsed twice. The vt100 parser stays the model the agent tracking
  reads (activity, prompts, early failures); an alacritty_terminal screen (`app/gui/term/mirror.rs`, plugged in
  through `panes/mirror.rs`) is what the window draws, scrolls and selects in, and it answers the program's
  queries (device attributes, colours, the kitty keyboard protocol). The cursor position report is answered by
  the session, so the mirror drops its own.
- Layout: the terminal UI's layout algorithms (tabs and splits, the all-sessions grid, project groups, the
  orchestrator column, held tile edges, the priority grow) run on a virtual grid of terminal-sized cells and
  are scaled to pixels. The workspace's own geometry (`outer`, `inner`, `body`) stays in those cells so focus
  moves, split directions and tile-edge drags behave as in the terminal.

## Surfaces: overlays and views

Every overlay and view is drawn by a module next to its terminal-UI drawing:

| Surface | Module | State it draws |
|---|---|---|
| Launcher, Continue tab, project picker | `launcher/gui.rs` (+ `continue_picker/gui.rs`, `folder/gui.rs`) | `Launcher` |
| Palette, keys (F1), resume picker, confirm | `app/gui/overlays/{palette,help,resume,confirm}.rs` | `App` |
| File preview, agent files | `file_view/gui.rs`, `agent_files/gui.rs` | `FileView`, `AgentFilesView` |
| Watchdog card, watchdog prompt editor | `watchdog/view/gui.rs`, `watchdog/prompt_editor/gui.rs` | `WatchdogView`, `PromptEditor` |
| Settings, stats, usage, profiles, proxy, bridge, quick install, setup | `views/<name>/gui.rs` | the view's own struct |

The contract (`app/gui/surface.rs`):

- `draw(state, ui, area, cx) -> bool` draws the surface and registers its clickable targets with
  `ui.hit(rect, Action::Overlay(n))` (overlays) or `ui.hit(rect, Action::View(cx.id, n))` (views). `n` is the
  module's own numbering (encode what you need: `ROW + i`, constants for buttons). Returning `false` means "not
  native yet": the terminal UI's drawing is painted cell for cell instead and clicks are translated to its
  cells, so nothing is ever missing.
- `click(state, n, p, mods, count)` runs target `n` and returns the surface's outcome (`launcher::Outcome`,
  `file_view::Outcome`, `watchdog::view::Out`…), which the dispatcher applies exactly as the terminal UI does.
- `wheel(state, n, down)` scrolls (`n` = the target under the pointer, if any).
- Keys never reach these modules: they go to the same state machine through the workspace. Draw the state
  machine's state (selection, filter, focus, error) — never keep a second copy of it.
- Being a child module of the state's own module, a `gui.rs` may read private fields. If it needs to change
  state, prefer the state machine's own methods; add a small `pub(crate)` method there only when none fits.

Dispatchers: `app/gui/overlays/mod.rs` and `app/gui/views.rs`.

## Kit

`app/gui/kit/mod.rs`: `Tokens` (colours from the theme, including WinUI control fills, strokes and the content
layer), `radius::{CONTROL, OVERLAY}` (4 and 8), `Ui` (drawing in logical pixels, text, hits, hover,
eased animations; `round_over`/`outline_over`/`circle_over` draw over pictures but under text), `widgets.rs`
(WinUI-style buttons, icon buttons, control boxes, pills, keycaps and key tips, switch, segmented, progress
meter, usage segments, text field, scrollbar, group heading, card, scrim, list row, tooltip), `brand.rs` (the
real logos of the agents, shells and providers as vector paths drawn through the glyph atlas: `ui.mark`,
`ui.logo`). See `DESIGN.md` for how to use them.

## Checking your work

- Build: `CARGO_TARGET_DIR=target-<you> cargo build -p bro-tui` (each agent its own target dir).
- Run the app scripted, in demo mode (fake data, no real accounts or settings touched):

  ```
  BRO_GUI_SCRIPT="wait:2500;key:alt+n;wait:600;shot:F:/tmp/shots/launcher.png;exit" target-<you>/debug/bro.exe gui --gui-child --demo
  ```

  Steps: `wait:<ms>`, `key:<chord>`, `type:<text>`, `move:x,y`, `click:x,y`, `dclick:x,y`, `rclick:x,y`,
  `wheel:x,y,lines`, `shot:<windows path>.png`, `exit`. Coordinates are logical pixels of the 1440×900
  window. Use Windows-style paths (`F:/…`) for shots: the exe doesn't understand `/f/…`.
- Look at the PNGs (the Read tool shows images).
- `--gui-child` keeps it from detaching from the shell. Never run without `--demo` in automated checks.

## Rules

- The terminal UI must keep working exactly as before; `cargo test -p bro-tui` stays green (one test,
  `app::tests::launcher_open`, fails because of unrelated work in progress).
- Don't touch files other agents own. Shared files (`app/gui/mod.rs`, `kit`, `layout.rs`, `chrome.rs`,
  `sidebar.rs`, dispatchers) belong to the integrator; ask for what you need instead of editing them.
- Copy (labels, help text, toasts) comes from the terminal UI verbatim unless it's about keys or cells that
  don't apply in a window.
