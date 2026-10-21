# bro-tui app core + layout engine: parity inventory

All paths are relative to `F:\bro-cli-v2\crates\bro-tui\src\`.

**What I read:**
- All 15 assigned files, in full.
- `app/draw.rs`, in full. The layout pass lives in `draw_panes`, so it was needed.
- The relevant parts of `app/sessions.rs`, `app/overlays.rs`, `app/side_view.rs`, `sidebar.rs`, `pane.rs`, `main.rs` and `panes/term.rs`.
- `app/tests.rs`, skimmed for behaviours the source reading missed.

---

## 1. Feature checklist

### 1.1 Event loop, timing, housekeeping
- **One channel:** everything arrives on one mpsc channel of `pane::Event`. `pane.rs:17-59`
- **Loop:** draw, then `recv_timeout(next_deadline)`, then handle, then drain more events for at most 12 ms, then `after_events`, then check quit. `app/mod.rs:792-836`
- **next_deadline:**
  - Base 30 s, minimum 4 ms. `app/mod.rs:891-936`
  - 125 ms while any pane is Working, the Watchdog overlay is open, a watchdog is running, or an animated theme shows on the welcome screen.
  - 110 ms while rain is on, or the theme backdrop is animated.
  - Toast expiry plus 10 ms.
  - Sidebar scrollbar fade.
  - 16 ms while a card is turning.
  - Alt-hint wake and screensaver wake.
  - Each pane's `tick_every`: terminals always (even hidden), other panes only when visible.
- **Tick:** polls panes whose `tick_every` has elapsed, and pumps queued resumes. `app/mod.rs:995-1008`
- **Wake:** a per-pane redraw signal that coalesces to one pending wake per pane. It is acknowledged, then the pane is polled. `app/mod.rs:991-994`, `pane.rs:120-151`
- **after_events order:** `app/mod.rs:839-859`
  1. screensaver step
  2. drop the priority grow if focus moved
  3. follow_tab
  4. prune locks of closed panes
  5. follow_focus
  6. record_view (history)
  7. `svc.set_focused_session(sid)`
  8. reap
  9. track_agents
  10. watchdogs
  11. insights
  12. voice
  13. toast expiry
- **Key kinds:** Release is ignored; Repeat is handled like a press. `app/mod.rs:950`
- **Resize:** there is no handler; the next draw uses the new size. `app/mod.rs:989`
- **FocusLost:**
  - Clears the sidebar filter and alt state.
  - Ends a sidebar drag and saves the width.
  - Cancels a divider drag. It does **not** cancel a tile-edge drag. `app/mod.rs:978-988`
- **FocusGained:** sets `term_focused` and counts as a touch (it wakes the screensaver). `app/mod.rs:974-977`
- **OpenProject** (another `bro` was started in a folder): rings BEL, `add_project`, leaves the sidebar. `app/mod.rs:1066-1076`
- **OpenLink:** opens on a background thread; an error becomes a toast. `app/mod.rs:1034-1043`
- **ThemeFilesChanged:** reloads a custom theme and toasts its first problem. `app/mod.rs:1085-1093`
- **Startup:** each keymap error is shown as an Error toast. `app/mod.rs:382-384`
- **Shutdown:** closes every pane with `quitting = true`, which keeps the orchestrator state for next time. `app/mod.rs:881-888`
- **Terminal modes:** alt screen, mouse capture, bracketed paste, focus events, kitty DISAMBIGUATE (non-Windows). `main.rs:217-221`
- **Input thread:** runs `paste_burst::pump`; raw Nav and Alt become `Event::Navigate` and `Event::Alt`. `main.rs:232-243`

### 1.2 Tabs, splits, zoom
- **Tab model:** a Tab is a `Node` tree plus `focus` plus `zoom`. `app/mod.rs:46-50`
- **new_tab:** pushes a tab, sets `cur` to it, then calls `include_in_tiles`. `app/mod.rs:401-407`
- **open(p, place):** `app/mod.rs:591-616`
  - No focused pane, or `Place::Tab`: new tab.
  - `Place::Split`: splits Right if the focused pane's **last-drawn** outer rect has width > height×2.2, else Down. Falls back to `body`.
  - `cur` switches to the tab holding the focused pane.
  - If the target leaf is missing, the root is wrapped in a new split.
  - The new pane gets focus and the tab is unzoomed.
- **Node operations:** `layout.rs:52-148`
  - split: turns a leaf into a 0.5 split.
  - remove: the sibling takes the parent's place; a root leaf cannot be removed, so the tab is removed instead.
  - rects: tile the area exactly, with no gaps.
  - resize: changes the deepest matching split; ratio clamped to 0.1..0.9.
- **split_rect:** rounds, and gives each side at least 1 cell when possible. `layout.rs:152-165`
- **Resize keys:** ±0.05 per press; no-op while stacked or tiled. `app/commands.rs:62-65,245-252`
- **Divider drag:** `app/mouse.rs:264-279,308-322`
  - The grab zone is 2 cells: the last column (or row) of child a and the first of child b.
  - Borders are tested deepest first.
  - Only works when not zoomed, not stacked and not grown.
  - Live ratio = (pos − start + 0.5) / len, clamped to 0.1..0.9. Release ends the drag.
- **Zoom:** toggles `tab.zoom`, or `stack_zoom` when stacked. The frame sub-label reads "zoomed · {primary Zoom chord}". `app/commands.rs:38-44`, `app/draw.rs:351-352`
- **Double-click a pane to zoom:**
  - Same pane, within 400 ms and ±1 cell: focus it and toggle zoom. `app/mouse.rs:413-431,571-580`
  - Always works on the frame.
  - Inside the content it works unless the program uses the mouse; shift or alt brings it back.
- **Unzoom rules:**
  - `focus_pane` unzooms a tab that is zoomed on a different pane. `app/mod.rs:718-720`
  - Closing a pane unzooms its tab. If it was focused, focus goes to the first leaf. If it was the last pane, the tab is removed and `cur` is adjusted. `app/mod.rs:671-685`
- **NextTab / PrevTab:** cycle `cur` and leave the sidebar. They are a no-op with no tabs. `app/commands.rs:77-85`
- **Alt+arrows:** `app/commands.rs:221-243`, `layout.rs:168-191`
  - Uses `neighbor()` over the **last-drawn** outer rects: nearest centre, overlap required, 1-cell tolerance.
  - Moving left off the edge focuses the sidebar (when it is on).
  - Right while the sidebar has focus leaves the sidebar.

### 1.3 All-sessions grid, project grids, groups, orchestrator column, minimize
- **State:** `tile_all`, `tile_project`, `stack`, `stack_focus`, `stack_zoom`, `minimized`, `show_minimized`. `app/mod.rs:208-217,232`
- **stacked()** is `tile_all || stack.len() >= 2`. `app/mod.rs:418-420`
- **tile_ids** (which sessions a grid shows): `app/mod.rs:433-441`, `sidebar.rs:95-121`
  - Live terminals in sidebar order: orchestrator first, then projects with live sessions in opened order, then by launch seq.
  - **Ignores the sidebar filter.**
  - Can be limited to one project.
  - Excludes minimized sessions unless `show_minimized` is on.
- **TileAll (toggle_tiles):** `app/mod.rs:496-517`
  - In All: returns to the focused session's own tab.
  - From a project grid or a single view: widens to All, keeping focus.
  - With fewer than 2 sessions and nothing minimized: toast "open two sessions to tile them", and nothing changes.
- **Project row Enter or click:** picks the project, then:
  - If that project's tab is already showing: toggle_tiles, which goes to All.
  - Otherwise: show_project_tiles. `app/commands.rs:624-635`, `app/mod.rs:520-526`
- **show_project_tiles:** `app/mod.rs:530-552`
  - 2 or more sessions, or a grid is already showing: that project's grid, unzoomed.
  - Exactly 1 session: it shows on its own.
  - All minimized: toast "every {name} session is minimized · ▸ minimized shows them".
  - Focus goes to the given pane, else the current focus if it is in the project, else the first.
- **include_in_tiles** (runs on every new pane): `app/mod.rs:479-494`
  - If in tile_all and the pane is a terminal, it joins the grid.
  - If it belongs to another project: with project tabs on, the grid switches to its project; with them off, the grid widens to All.
  - It takes focus and unzooms the grid.
  - A **non-terminal pane (any view), or any pane while not in tile_all, calls `clear_stack`.** That exits the grid or dissolves a manual stack.
- **Remote (bridge) launches:** restore the previous grid, `cur` and focus. They only join the grid. `app/sessions.rs:147-158,317-328`
- **go_session:** `app/commands.rs:255-275`
  - Keeps the grid in tile_all.
  - If the target is in another project's grid, the grid follows it to its project.
  - Outside tile_all it clears a manual stack.
  - Always sets `cur_project`.
- **focus_pane:** `app/mod.rs:699-724`
  - On a minimized session: un-minimizes it, widens to All if its project differs, then retiles.
  - On a session in the stack: only moves `stack_focus`.
  - Otherwise: clears the stack and focuses it in its tab.
- **close in a grid:** removes it from `stack` and `minimized`. If fewer than 2 remain and nothing is minimized, the grid exits and the survivor gets focus. `app/mod.rs:622-636,686-688`
- **Draw-time layout pass:** `app/draw.rs:213-258`
  - `body` minus the orchestrator column.
  - Groups are keyed by `Term::tile_group`: project, harness, profile, model and cwd. `panes/term.rs:749-757`
  - With 2 or more groups, or one group of 2 or more: `group_regions` with weight = max(len, ceil(n/4)).
  - A region is framed (1-cell ring) only if its group has 2 or more panes and the region is at least 14×6.
  - `stack_rects` runs inside each region; otherwise plain `stack_rects`.
- **stack_rects:** `layout.rs:228-262`
  - 1 pane: full area.
  - 2: side by side when w ≥ h×2.2, else over/under.
  - 3: one big pane plus two stacked.
  - 4 or more: a grid with cols = round(√(n·w/(2.2h))); the last row's panes widen to fill it. Always covers the area exactly.
- **group_regions:** cuts the list where the two weights are most even; side by side if wide, else stacked; recursive; weight 0 counts as 1. `layout.rs:196-224`
- **Orchestrator column:** `app/draw.rs:57-65,226-231,305`
  - Only in a grid with 2 or more sessions that is not zoomed.
  - On the right; width = body/3 clamped to [min(44, body/2), 96], and at most body/2. Shown only if at least 20 wide.
  - It is in no group, and it stays put while another tile grows.
- **Group frame:** label "{project} · {glyph} {who}[ — cwd]" plus a session count. Tiles inside are titled "{n} · {own title}", where n is the sidebar number (up to 9). `app/draw.rs:315-322,342-345`, `panes/term.rs:761-767`
- **Focused pane:** `id == focus && !side_focus && !overlay_open`. Unfocused titles are muted and not bold. `app/draw.rs:340-348`
- **Minimize button:** `app/draw.rs:360-370`, `ui.rs:199-201`
  - Reads " − ", or " + " while minimized and shown.
  - Sits at x+1..x+3 on the frame's top row.
  - Only in tile_all and when the pane is at least 5 wide; reverse video on hover.
- **Close button "×":** `app/draw.rs:379-392`
  - At right−4, on the bottom row (the title row with shared borders).
  - Only when the pane is wider than 24 and not zoomed.
  - Hover: danger color, reversed, bold.
- **minimize_pane:** `app/mod.rs:452-470`
  - tile_all and terminals only; toggles.
  - Minimizing while `show_minimized` is on clears the earlier minimized set and goes back to hidden mode.
  - Then it retiles, clears the selection, leaves the sidebar and resets double-click.
- **Show/hide minimized:** a toggle, tile_all only. Reached from the sidebar's "▸/▾ minimized N" or the strip's switch. `app/mod.rs:472-477`
- **Everything minimized:** a centered "All sessions minimized · show minimized in the sidebar". `app/draw.rs:307-314`
- **Sidebar tile row:** shown only with no strip, 2 or more sessions, and room. `app/side_view.rs:105-134`
  - In a grid: "▦ tiled N" plus "▸/▾ minimized N".
  - Otherwise: "▦ tile all · N" plus the primary chord.
- **Project row ▦ mark:** shown while its grid is on screen. `app/side_view.rs:268-269`
- **Shared-borders setting** (grid, not zoomed): `app/draw.rs:148-150,326-335,374,439-441,565-597`
  - Each tile is drawn 1 cell onto its right and lower neighbours, so they share one line.
  - The status moves up to the title band.
  - Content gets 1 col/row of padding on shared sides, only when inner is larger than 20×4.
  - Corners are joined. `outer` is unchanged.

### 1.4 Manual stack
- **Triggers:** shift+click or ctrl+click a sidebar Live row, or shift+Enter on it, calls `toggle_stack`. `app/mouse.rs:201-206`, `app/input.rs:294-299`
- **toggle_stack:** `app/mod.rs:555-588`
  - Exits tile_all, project grid and minimize state.
  - Seeds the stack with the current focused terminal.
  - Toggles the target in or out; focus goes to the added pane, or to the last one when the focused pane is removed.
  - With fewer than 2 left, it dissolves to the survivor.
- **Leaving it:** a plain sidebar click (`go_session`) or opening any new pane dissolves it. `app/commands.rs:257-259`
- **Strip:** `showing_tab` is None for a manual stack, so no strip tab lights. `app/project_tabs.rs:97-99`

### 1.5 Priority grow / shrink
- **Toggle:** ctrl+left-click a pane (not on a link, no shift), or Grow, toggles the priority on that pane and focuses it. Shrink clears it. `app/mouse.rs:379-386,554-568`, `app/commands.rs:45-50`
- **Auto snap-back:** after any event batch where focus ≠ priority. `app/mod.rs:842-844`
- **Applied at draw time; the base layout is never mutated.** `app/draw.rs:292-303`
- **Algorithm:** `priority.rs:20-68,272-365,480-582`
  - Target = min(102×32, 4/5 of body), in outer cells.
  - The rects are read as a guillotine tree (rows first).
  - Each axis grows through its innermost cuts first.
  - Neighbours give spare room above USEFUL (32×7) in proportion to spare/distance.
  - If that runs out, the nearest neighbours fold to FOLDED (10×2); the surplus a fold frees goes to the grown pane.
  - Frames resize with their panes; it works on tab splits too.
- **No-op cases:** a lone pane, already big enough, not on screen, an empty rect, or not straight cuts. `priority.rs:46-55`
- **Sub-label:** "grown · {Shrink primary} restores". `app/draw.rs:354`
- **Folded neighbour:** `app/draw.rs:375-403`
  - Frame only; the program is not rendered, so its PTY keeps its size.
  - It gets no content hit rect. A click focuses it, which drops the priority and unfolds it.
  - In a narrow strip, the title letters are drawn vertically.
- **Interactions:** while grown, divider drag and tile edges are disabled; zoom ignores priority. `app/mouse.rs:264,584`, `app/draw.rs:154,293`

### 1.6 Tile edge locks (grid only)
- **Hover handle:** only when not zoomed, not grown and no overlay. ↔ for a vertical edge, ↕ for a horizontal one; drawn at the pointer, bold accent, with both sides of the edge lit. `app/draw.rs:153-206`
- **tile_edge:** `app/mouse.rs:583-610`
  - A border cell of a pane with another tile across it: touching, or up to 2 cells across project-frame gutters.
  - With shared borders, the line belongs to the right/lower pane, so the left pane's last column is content.
- **Press (no modifiers) on an edge:** `app/mouse.rs:133-151,282-306`
  - Anchors to an existing lock on the same cut, else to this edge. Other locks on that cut are dropped: one lock per cut.
  - **The press also continues as a normal click** (focus, double-click tracking).
- **Drag:** replaces `Lock{pane, ax, far, at}` live, with `at` = pointer + grab offset − body start.
  - It also clears double-click state.
  - Release ends it; any other button down cancels it.
- **Double-click:** on a **held** edge (exact same cell, 400 ms) it releases every lock on that cut and does not zoom. On an unheld edge it zooms as usual. `app/mouse.rs:287-292`
- **Applied at draw time:** `priority.rs:89-113,391-456`, `app/draw.rs:269-289`
  - Outer cuts move first; the later of two locks on one cut wins.
  - Neighbours squeeze to at least 16×5 (LOCK_MIN); once one is at the minimum the next gives way, so an edge can pass its neighbour.
  - Frames and the orchestrator column move with it.
- **Held-edge marker:** a muted handle midway along the shared span. `app/draw.rs:194-205`
- **Scope:** locks belong to the tab that was showing when they were dragged (All, a project, or ★). They are saved and restored on tab switch and pruned when their pane closes. `app/project_tabs.rs:243-256`, `app/mod.rs:846-848`

### 1.7 Card flip
- **Alt+double-click** anywhere on a terminal pane, even with a mouse-mode program, focuses it and turns it over. A single alt+click does nothing. `app/mouse.rs:413-431,497-508`
- **Flip** acts on the focused card. **FlipAll** sends every on-screen card to its back if any is face up, else turns all face up. Cards go in reading order of outer (y, x), 45 ms apart. `app/commands.rs:52-57`, `app/mouse.rs:512-550`
- **Animation:** `card.rs:21-96`, `app/mod.rs:917-920`
  - TURN is 280 ms with smoothstep; edge-on at the halfway point.
  - Turning again mid-turn swings back from the current width.
  - Squash: sides blanked to the theme background; edges drawn ▕ ▏ (│ when zero width) in the project color; DIM when cover < 0.6.
- **Retention:** a flip is kept only while on its back or turning. `app/draw.rs:325`
- **Back:** the program keeps running underneath; the sub-label reads "back · alt+double-click". `app/draw.rs:349-350,421-433`
- **Typing** into a flipped focused card turns it face up, and the key is still delivered. `app/input.rs:92-95`
- **Mouse on a flipped card:** `app/mouse.rs:351-363,368-370`, `app/draw.rs:426-430`
  - No passthrough, no `link_at`, no prompt clicks.
  - A non-alt left **press** on a link or file row opens it immediately (not on release). The row lights on hover.
- **Back content:** `card.rs:152-401`
  - Title; then who · cwd · ⎇ branch · open {duration}.
  - **now**: ● needs you / spinner working / ● done / ○ idle, plus what it is doing, plus "· N ago".
  - **asked**, **plan** (✓ ▸ ○, "✓ N of M done", "+n to go"), **latest** (markdown stripped).
  - **links**: ranked running-local, remote, unchecked, down.
  - **files**: + − → ~ marks, path relative to cwd, then a tally.
  - Uses a tighter layout when the card is too short.
- **Only terminals** can flip. `app/mouse.rs:498`

### 1.8 Summary screensaver
- **Setting** `summary_screensaver_secs` (0 = off). `app/screensaver.rs:27-43`
  - When idle for that long with no overlay open, every on-screen card turns over, cascaded, and is recorded in `saver`.
  - If nothing turned, the wait restarts.
- **touched():** `app/screensaver.rs:11-25`, `app/mod.rs:950-977`
  - Fires on any key, mouse event, paste, bare-alt down, or focus gained.
  - Resets the idle clock.
  - If the saver is showing, it turns back only the cards still on their back, so cards you flipped yourself stay over.
- **The waking key or click is swallowed.** A mouse Move is still handled and a paste is still delivered. `app/mod.rs:952-966`

### 1.9 Project tab strip (setting `project_tabs`)
- **Placement:** the body's top row, shown whenever any tab is open (views too, so sessions don't jump), and only if the body is at least 24×8. `app/draw.rs:82-89`, `app/project_tabs.rs:61-63`
- **Order:** All, then ★ (if the orchestrator runs), then projects with live sessions in sidebar order. Phone/web names and ordering override that. `app/project_tabs.rs:66-89`
- **showing_tab:** `app/project_tabs.rs:93-109`
  - tile_all: All, or that project.
  - Manual stack: None.
  - The orchestrator alone: ★.
  - Exactly all of one project's sessions on screen and nothing else: that project.
  - Otherwise None.
- **show_tab:** `app/project_tabs.rs:112-137`
  - Already showing: only leaves the sidebar.
  - All: `toggle_tiles`.
  - ★: focuses the orchestrator, or toasts "the orchestrator isn't running · alt+O starts it".
  - A project: `pick_project` then `show_project_tiles`, or toasts "nothing is running in that project".
- **click_tab:** clicking All while it is showing toggles back to the focused session. `app/project_tabs.rs:140-146`
- **step_tab:** wraps; ★ is included only for the wheel. `app/project_tabs.rs:149-159`
- **Strip keyboard** (FocusSidebar when there is no sidebar): `app/project_tabs.rs:167-200`
  - ←/→ wrap, Home/End.
  - 0-9 is ShowTab.
  - Enter or Space clicks the tab; Delete closes the project.
  - Esc, ↑, ↓ or Tab release the strip.
  - Other keys are swallowed.
- **Mouse:** `app/project_tabs.rs:203-229`, `app/mouse.rs:168-173,232-236`
  - Left-click a tab clicks it.
  - The × or a middle-click closes the project, with a confirm when sessions run.
  - A "+N" fold opens the palette prefilled with "tiles ".
  - The minimized switch toggles; + opens the launcher.
  - The wheel steps tabs, once per 150 ms burst.
- **tab_before:** set only on a direct tab-to-tab change; used by LastTab. `app/project_tabs.rs:233-242`
- **Drawing:** `app/project_tabs.rs:261-477`
  - Right end: + (if the strip is at least 40 wide), then " ▸/▾ N minimized " (tile_all, needs room).
  - " ▦ All N " in bold; " ★ glyph badges " in gold; then " │ ".
  - Project tabs " ◐ name N ●b ✓d × ". Names are capped between 6 and 24 cols and shrink to fit.
  - Overflow folds into " ◂N" / " +N▸" around the selected tab, else the showing one. A fold gets a red ● when anything in it wants attention.
  - The × is visible only on hover or keyboard pick, but its room is always kept.
  - The spinner runs at 6 fps when anything works, else ○.
  - Styles: the showing tab is a solid band; hover/pick is a tint (reverse for named colors); keyboard pick is underlined; the focused project's name is bold.

### 1.10 Navigation history (mouse back/forward, browser keys)
- **Entries:** `Seen::{Pane(id), Grid(project, id), NewSession}`, capped at 200. Recorded after **every** event batch; duplicates dropped; a new view truncates forward entries. `app/history.rs:12-48,63-76`
- **Cancelled launcher:** a new-session screen closed without launching (returning to the entry before) is erased. `app/history.rs:36-41`
- **navigate(back):** `app/history.rs:87-139`
  - Any non-launcher overlay just closes, and nothing else happens.
  - Otherwise it records the current view, then steps to the nearest acceptable entry.
  - In your own (unzoomed) manual stack, it accepts only stacked panes and no grids.
  - Grid entries re-show the project grid or All, then focus.
  - Pane entries focus the pane and re-zoom if you were full screen and the tab has splits.
  - NewSession entries reopen the launcher.
- **Input source:** Windows only, from console mouse buttons 0x08/0x10 and VK_BROWSER_BACK/FORWARD. Other platforms never get them in the TUI. `console_nav.rs:30-73`

### 1.11 Alt hints
- **Showing:** alt held at least 300 ms (HOLD) and no overlay open. Any key, Release, paste or FocusLost hides them. `app/alt_hints.rs:20-41`, `app/mod.rs:951-982`
- **Alt state sources:** a mouse event carrying ALT sets it; one without ALT clears it. On Windows, a bare `Event::Alt` also sets it. `app/mod.rs:957,968-973`
- **Only alt chords are shown,** with "alt+" stripped. `app/alt_hints.rs:44-46`
- **Cards** (terminals at least 12×3), on the bottom edge after the status label, centered, avoiding the last 5 cells: `app/alt_hints.rs:60-92`
  - "dbl-click flip".
  - On other cards: their jump key with "go" (top 9 only).
  - On the focused card: "i flip", "I flip all", "z zoom", "= grow".
  - Tips are dropped from the end until they fit.
- **Strip:** each tab's key over its first cell (All "0", ★ "O", + "n", projects only if `tab_N` is bound to an alt chord), then "K ‹ tab · J tab › · l last" if there is blank room. `app/alt_hints.rs:95-125`
- **Sidebar:** `app/alt_hints.rs:128-170`
  - Session numbers light up.
  - Clickable spots show their key, each once. The key's own letter lights in place when the text has it (uppercase matches only uppercase), else a " k " badge sits beside it.
- **Implementation:** reads back the rendered ratatui buffer (`past_label`, `letter_in`, `blank`, `beside`). `app/alt_hints.rs:176-227`

### 1.12 Keymap and key routing
- **Routing order in `key()`:** `app/input.rs:14-108`
  1. Clear the selection, the pending link press and any pending clipboard read.
  2. Paste chords (ctrl+v, ctrl+shift+v, shift+insert) when the prefix is not armed: start an async clipboard read and return. **This applies everywhere, including terminals and overlays.**
  3. An open overlay gets the key.
  4. Inline rename gets the key.
  5. alt+v on a focused terminal (prefix not armed, sidebar not focused): image/file grab on a thread, then paste.
  6. Prefix armed: disarm. The prefix again sends the prefix key to the program. A bound key runs its Act. Esc does nothing. Anything else toasts "{chord} isn't bound after the prefix — ? lists them".
  7. The prefix key arms the prefix.
  8. alt+↑ on a focused Codex terminal goes to Codex (prefix+↑ still navigates).
  9. Direct chords.
  10. The strip, if it has the keys.
  11. The sidebar, if it has the keys.
  12. No panes: welcome keys.
  13. A flipped focused card turns face up.
  14. The pane's own key handler. If a **non-terminal** pane doesn't use the key: '?' opens Help; Esc focuses the sidebar on the focused row.
- **Welcome keys:** Enter or n = NewSession; s = NewShell; ? = Help; p = Palette; b, Tab or ← = sidebar focus; q = Quit. `app/input.rs:110-122`
- **Rename editor:** `app/input.rs:203-228`
  - Enter saves the trimmed name (empty clears it), sets `renamed`, and pushes the title to the bridge.
  - Esc cancels; Backspace deletes.
  - Characters without ctrl are added, up to 40.
  - Opening it forces the sidebar visible. `app/commands.rs:532-539`
- **Chord normalization:** `keymap.rs:415-440`
  - Modifiers are masked to CTRL, ALT and SHIFT; SUPER, META and HYPER are dropped.
  - An ASCII uppercase letter becomes lowercase plus SHIFT.
  - ctrl+@ is treated as ctrl+space.
  - Non-letter, non-space characters drop SHIFT.
  - BackTab drops SHIFT, and Tab with SHIFT becomes BackTab.
- **parse:** `keymap.rs:447-496`
  - Modifier aliases: `ctrl|control|c`, `alt|meta|opt|option|m`, `shift|s`.
  - A trailing `++` means the plus key.
  - Named keys: `space|spc`, `enter|return|ret`, `tab`, `backtab`, `esc|escape`, `backspace|bs`, `delete|del`, `insert|ins`, `home`, `end`, `pageup|pgup`, `pagedown|pgdn`, `up|down|left|right`, `f1..f24`. Any single character is also accepted.
- **display:** for example "alt+J", "alt+shift+←", "ctrl+space", "F1", "shift+tab", "del", "pgup". `keymap.rs:503-538`
- **Prefix setting:** `settings.prefix` (empty means ctrl+space); a bad value records the error "prefix: …" and falls back. `keymap.rs:559-567`
- **Overrides** (`settings.keys`, a BTreeMap, so **applied in alphabetical order**): `keymap.rs:583-607`
  - Keys are action names, case-insensitive, with `-` read as `_`.
  - `"none"` or `""` removes the action from both tables.
  - `"prefix X"` or `"prefix+X"` sets the prefix-table binding; anything else sets the direct table.
  - An override replaces that action's bindings in **that table only**, and steals the chord from any other action there.
  - Errors: "keys: no action called '{name}'" and "keys.{name}: {err}".
- **Keymap API:** `is_prefix`, `direct`, `after_prefix`, `chords_for` (direct first, then "ctrl+space x"), `primary`. `keymap.rs:611-639`
- **prefix_hint:** a bottom-right box titled "{prefix} …" that replaces toasts while the prefix is armed. `keymap.rs:642-648`, `app/draw.rs:528-536`
- **Sidebar keys** (sidebar focused): `app/input.rs:264-334`
  - Printable characters (shift allowed) go to the filter. A leading '/' only starts filtering.
  - Filtering selects the first project row and resets scroll.
  - ↑/↓, Home/End, PgUp/PgDn (±10).
  - ← goes to the project header; → steps into a project, or on a Live row returns to the panes.
  - Enter activates the row; shift+Enter stacks it.
  - Del closes the session or the project.
  - Backspace edits the filter.
  - Esc clears the filter first, then leaves the sidebar.
  - Tab leaves the sidebar.
  - Navigation shows the scrollbar, and the selection drives `pick_project`.

### 1.13 Mouse handling, in precedence order (`app/mouse.rs:12-451`)
1. Any non-Move event cancels a pending clipboard read. Every event sets `hover`. Any button down outside the sidebar leaves the sidebar (clears the filter and strip keys). Down, drag and wheel clear `link_press`. `:13-23`
2. **The launcher fills the body but the sidebar and strip beside it keep working** (the project picker is the exception). Sidebar edge drags work too. `:25-26`
3. **Other overlays own the mouse:** `:27-112`
   - Confirm: only the Yes/No buttons react.
   - File and AgentFiles: events are forwarded.
   - Launcher: left-click and wheel.
   - Watchdog: left-click.
   - Resume: a click selects a row; clicking the selected row runs it.
   - Palette and Help: **any button-down closes them** (the palette restores the theme). No inside-click exemption.
4. **Sidebar edge drag in progress:** captures the whole gesture; width = x − side.x + 1, clamped to [26, max(26, screen/2)]; saved once on release. `:115-131`
5. **Tile-edge drag in progress.** `:133-151`
6. **Sidebar edge press:** double-click within 400 ms resets to adaptive width (w/4, clamped 30..40); otherwise starts a drag. `:152-166`
7. **Wheel over the strip:** steps tabs. **Wheel over the sidebar:** scrolls 1 row and keeps that viewport until keyboard navigation. `:168-186`
8. **Sidebar and strip hits:** `:187-246`
   - Left-click acts. Shift or ctrl on a Live row stacks it. Middle-click on a strip hit closes the project; on a Live row it asks to close the session.
   - Other buttons are swallowed.
   - Sidebar double-click is computed but unused (`:208`).
9. Left-click on blank sidebar focuses the sidebar. `:247-250`
10. Pane minimize button, then pane close button. `:255-262`
11. Divider grab, then tile-edge press. `:264-306`
12. Divider drag and release. Selection drag (clamped to the pane's inner rect at press). Release: copy if the selection is active; else open a pending link if released on the same cell in the same pane. `:308-349`
13. Card-back link press. `:351-363`
14. **Pane under the cursor:** `:365-376`
    - `wants` = the program is in mouse mode and the card is not flipped.
    - `link` = left press, not flipped, inside content, no shift, and (not `wants`, or ctrl), then `pane.link_at`.
15. ctrl+press (no shift, not on a link): priority toggle. `:379-386`
16. **Prompt option click:** `:389-409`
    - Left press, not on a link, not flipped, no alt, inside content, and (not `wants`, or shift).
    - Only when the agent is Blocked and the terminal is not scrolled back.
    - Sends "{n}\r". The row lights on hover (`app/draw.rs:412-420`, `panes/term.rs:583-592`).
17. Double-click handling: zoom, or flip with alt. `:413-431`
18. Selection start: inside content and (not `wants`, or on a link, or shift). `:432-435`
19. **Any button-down focuses the pane** and leaves the sidebar. `:436-443`
20. A link press is recorded; the link opens on release. `:444-447`
21. **Passthrough to the pane** (all kinds, including wheel, right and middle): inside content, not flipped, no alt, and not (shift while `wants`). `:448-450`

### 1.14 Selection, copy, links
- **Selection:** linear in reading order inside one pane's inner rect. `app/mod.rs:53-74`
- **Copy:** happens on the **next draw**, from the rendered buffer cells. Trailing spaces and newlines are trimmed; a blank selection copies nothing. Toast "copied N character(s)". `app/draw.rs:488-523`
- **Clearing:** any key, any Act, a paste, or the pane leaving the screen clears the selection. `app/input.rs:15`, `app/commands.rs:19`, `app/mod.rs:1099`, `app/draw.rs:490-492`
- **Terminal links:** `app/mouse.rs:333-349,371-376`
  - Plain click opens them; mouse-mode programs need ctrl+click.
  - They open on release at the same cell; a drag starting on a link becomes a selection; shift means select only.
  - `file:` links open the preview; others go to the browser.

### 1.15 Paste
- **Routing** (`app/mod.rs:1097-1131`):
  - First it clears the pending clipboard read and the selection.
  - Launcher, Watchdog and WatchdogPrompt overlays receive it.
  - Palette: the query gets the text without control characters, the selection resets, and the theme previews live.
  - Other overlays drop it.
  - Rename: text without control characters, up to 40 chars.
  - Sidebar focused (prefix not armed): goes to the filter.
  - Sidebar focused or prefix armed otherwise: dropped.
  - Else the focused pane's `paste`.
- **Async clipboard read:** guarded by a request token; it is dropped if any key, click or paste happened since. `app/input.rs:18-31`, `app/mod.rs:1046-1053`
- **alt+v:** a clipboard image or copied files are pasted as paths, with toast "pasted image|N files as a path · Claude Code and Codex attach it". With nothing image-like, the original alt+v key goes to the program. `app/mod.rs:1054-1065`
- **Windows console paste folding:** keys arriving under 8 ms apart, in bursts of 3 or more text keys (Enter becomes \r, Tab becomes \t), become one Paste. A non-text event ends the burst and is emitted after it. A lone key passes through with no delay. `paste_burst.rs:10-117`

### 1.16 Focus, current project, attention
- **focused():** `stack_focus` (or the first in the stack) when stacked, else `tabs[cur].focus`. `app/mod.rs:410-415`
- **visible():** the stack, or just the focus when stack_zoom; the zoomed focus; or all leaves of the tab. `app/mod.rs:727-736`
- **follow_focus:** when focus changes to a non-orchestrator session, `cur_project` becomes its project. **pick_project** holds a project until focus moves. `app/mod.rs:863-878`
- **current_project:** the sidebar selection while browsing, else `cur_project`, else the focused session's project, else the first opened project. `app/sessions.rs:430-441`
- **open_launcher** computes its directory **before** leaving the sidebar, so a filtered sidebar selection carries over. `app/commands.rs:322-328`
- **Attention:** the next Blocked session in sidebar order after the focused one, else the next done-unseen one. Pressing it again moves on. `app/sessions.rs:803-811`
- **Done tracking:** `app/sessions.rs:766-799`
  - Working to Idle while not visible, or while the window is unfocused: marked done (✓), with toast "X finished · …".
  - Becoming Blocked while unseen: toast "X needs you".
  - Visible sessions clear their done flag.
- **FocusSidebar:** `app/commands.rs:112-125`
  - If the strip has the keys: releases them.
  - If the strip is shown and the sidebar is hidden or squeezed (window under 72 cols): the strip takes the keys.
  - If the sidebar is squeezed: toast "the window is too narrow for the sidebar · widen it, or turn on project tabs".
  - Otherwise: shows the sidebar and toggles its keyboard focus.

### 1.17 Close, quit, reap, views
- **ask_close:** confirms when activity is not Idle (Blocked included), with "close {title}?" / "the agent is still busy — it will be stopped". `app/commands.rs:211-219`
- **close():** `app/mod.rs:619-696`
  - Stops the watchdog and kills the process.
  - Fires the login-closed hook.
  - Removes the proxy route unless another session shares it.
  - Cleans staged files on a background thread.
  - Updates tab and stack bookkeeping.
  - When the orchestrator closes, clears its state unless bro is quitting.
- **reap:** an exited process closes its pane, with "{name} exited: {why}" (Error) or "{name} exited". `app/mod.rs:1134-1145`
- **Quit:** with zero terminals it quits at once. Otherwise it confirms "quit bro and end N session(s)?" / "running agents are stopped; past sessions stay resumable". `app/commands.rs:196-203`
- **Confirm keys:** y, Y or Enter means yes; anything else means no. `app/overlays.rs:140-155`
- **open_view:** views are singletons. Pressing the key again on a focused view with more than one tab goes back one tab. A view opened from a pane replaces the existing one. `app/commands.rs:455-477`, `app/mod.rs:773-781`

### 1.18 Perf log
- **What is logged:** a pass longer than 50 ms, or input that waited 50 ms or more. It goes to `~/.bro/v2-perf.log` with events, worst event or pane, housekeeping, render, slowest pane render and terminal write. Written on a background thread; when over 512 KB it is cut to the newest half. `perf.rs:12-191`

---

## 2. Model/view separation

### Pure, reusable as-is
These need only a cell `Rect`, currently ratatui's, which is easy to replace with a local u16 rect.

- **`layout.rs`:** all `pub`. `PaneId`, `Dir`, `Node` (all methods), `split_rect`, `neighbor`, `group_regions`, `stack_rects`.
- **`priority.rs`:** `pub` items: `want`, `Grown`, `prioritize`, `Lock{pane, ax, far, at}`, `lock`, `same_cut`, `COMFORT`, `USEFUL`, `FOLDED`, `LOCK_MIN`. `Tile`, `Kind`, `reduce`, `spread` and `share` are private. Deterministic and pure, as tests confirm.
- **`sidebar.rs`:** `build`, `live_order`, `project_tabs`, `Row`, `LiveInfo`, `ProjectTab`, all pure.
- **`keymap.rs`:** `pub enum Act`, `pub enum Group`, `pub struct Chord{pub code, pub mods}`, `pub struct Keymap{pub prefix, direct (private), prefixed (private), pub errors}`. Pure, but typed on **crossterm** `KeyCode`/`KeyModifiers`/`KeyEvent`. A GUI needs a winit-to-crossterm adapter, or a generic chord type. `Chord::new` is public.
- **`app/history.rs`:** `pub(crate) History`; its fields and `record`/`step` are module-private. Pure.
- **`card::Flip`:** `pub(crate)`, with `back`, `at`, `toggled`, `turning`, `face`. Pure.
- **`card::lines`:** the content logic is reusable, but it returns ratatui `Line`s.
- **`perf::Pass`:** framework-agnostic.
- **`paste_burst::pump`:** console-only.
- **`console_nav`:** Windows console-only; winit replaces it.

### App
- **Structure:** `pub struct App` is a single module boundary (`app::*`). Every submodule sees its private fields, so the file split is organizational only.
- **`pub(crate)` fields:**
  - panes, tabs, cur, theme, keymap, svc, toasts, prefix_armed, overlay
  - sidebar, side_focus, side_sel, side, side_filtering, renaming
  - stack, stack_focus, tile_project, strip, drawn_tab, strip_sel, tab_before
  - priority, flips, alt_down, last_input, saver, locks, done, notes
  - recents, archive, persist, open_projects, cur_project, watch, orchestrator_pending
- **Private fields:**
  - tile_all, minimized, show_minimized, stack_zoom, grown
  - outer, inner, body, side_area, side_hits, pane_close, pane_minimize
  - drag, tile_drag, tile_regions, saved_locks, lock_scope, tab_now
  - sel, copy_pending, clipboard_request, hover
  - last_click, last_pane_click, link_press, card_links, confirm_hits
  - history, term_focused, and others
- **Other types:** `Tab`, `Sel`, `SideHit`, `StripTab` and `StripHit` are `pub(crate)`. `Opts` and `Overlay` are `pub`.
- **State-machine methods reusable if App stays the model:** `focus_pane`, `toggle_tiles`, `toggle_project_tiles`, `show_project_tiles`, `toggle_stack`, `minimize_pane`, `include_in_tiles`, `open`, `close`, `go_session`, `run_act`, `navigate`, `show_tab`, `step_tab`, `follow_tab`, `touched`, `screensaver_step`, `toggle_priority`, `flip_pane`, `turn_card`.

### Tangled with ratatui or crossterm
- `mouse()`: entirely hit-tests against last-draw caches and takes a crossterm `MouseEvent`.
- `key()`, `strip_key`, `sidebar_key`: take crossterm `KeyEvent`.
- **The `Pane` trait:** `render(&mut Frame)`, `header() -> Line`, `key(KeyEvent)`, `mouse(MouseEvent, Rect)`. `pane.rs:190-246`
- `draw_project_tabs`: the fold and name-cap algorithm is mixed with drawing and hit registration.
- Alt hints and selection copy read the rendered `Buffer`.

### Logic that depends on geometry from the previous draw
**`outer`** (written `app/draw.rs:306`). Read by:
- `open(Place::Split)` direction (`app/mod.rs:599`)
- `move_focus` (`app/commands.rs:227`)
- `cards_on_screen`, which orders flip-all and the screensaver (`app/mouse.rs:518`)
- `tile_edge`, `across`, `same_cut`, `held_cut`, and the tile-drag anchor (`app/mouse.rs:296,583-620`)
- pane hit-testing (`app/mouse.rs:365`)
- alt hints (`app/alt_hints.rs:60`)
- tile-edge handles (`app/draw.rs:174`)
- selection validity (`app/draw.rs:490`)
- rain exclusion (`app/draw.rs:141`)

**`inner`** (written `app/draw.rs:377`; folded panes omitted). Read by:
- link release (`app/mouse.rs:340`)
- content hits (`app/mouse.rs:366`)
- `working_rects`

**`body`** (`app/draw.rs:76,85`). Read by:
- Split fallback (`app/mod.rs:599`)
- sidebar clamp (`app/mouse.rs:122`)
- tile-drag `at` (`app/mouse.rs:136`)
- `sidebar_squeezed` (`app/project_tabs.rs:163`)
- `start_now` initial PTY size, body−2 (`app/sessions.rs:169`)

**Other caches:**
- `side_area`: `app/mouse.rs:18,26,152,175,247`
- `side_hits` (includes the strip's hits): `app/mouse.rs:187`, `app/alt_hints.rs:99,133`
- `pane_close` / `pane_minimize`: `app/mouse.rs:255-262`
- `strip`: `app/mouse.rs:26,168`, `app/commands.rs:114`
- `tile_regions`: `app/mouse.rs:614`
- `grown`: `app/mouse.rs:264,584`
- `card_links`: `app/mouse.rs:353`
- `confirm_hits`: `app/mouse.rs:30`
- `drawn_tab`: strip and sidebar ▦ highlight
- `hover`: every hover effect
- Launcher and Resume `hits`

### State the draw pass mutates
- `body`, `side_area`, `strip`, `strip_sel` (cleared or clamped), `drawn_tab`, `side_hits`, `outer`, `inner`, `tile_regions`, `grown`, `card_links`
- `flips.retain`, `sel` (cleared), copy plus toast, `side_sel` clamp (`app/side_view.rs:137`)
- **Pane render can return Actions that App applies mid-draw** (open or close panes), and it sends a Tick. `app/draw.rs:434-437`
- **The PTY size is set by render:** `Term::render` calls `ensure_started` and resizes the PTY to the inner rect every frame. Hidden and folded panes keep their last size. `panes/term.rs:1102-1122`

### Recommended seam for the GUI
Extract `draw_panes`' first half (`app/draw.rs:216-306`) into a pure layout function: (model, body) → {outer, regions, groups, folded, column, grown}. Store its result as the "last layout" that hit-testing, alt+arrows, Split direction and tile edges read.

---

## 3. All `Act` variants
There are 62 variants. `Jump(1..9)` and `ShowTab(0..9)` expand `Act::all()` to 79 entries.

- **Prefix:** ctrl+space.
- **Name:** the key used in `settings.keys`.
- **Groups:** Sessions, Navigate, Panes, Views, App. `keymap.rs:281-290`

| Act (name) | Direct | Prefix | What it does |
|---|---|---|---|
| NewSession (new_session) | alt+n | c | Launcher overlay, New tab, in current project |
| QuickAgent (quick_agent) | alt+q | Q | Agent in home dir on last non-orchestrator harness/login; launches at once if nothing to pick |
| NewShell (new_shell) | alt+t | ! | Shell tab in focused session's cwd |
| Split (split) | alt+enter, alt+\ | — | Launcher with Place::Split (longer side) |
| SplitRight (split_right) | — | %, \|, \ | Shell split right |
| SplitDown (split_down) | — | ", - | Shell split down |
| Close (close) | ctrl+w, alt+w | x | ask_close focused |
| Zoom (zoom) | alt+z | z | Toggle tab.zoom / stack_zoom |
| Grow (grow) | ctrl+up, alt+= | = | Toggle priority grow on focused |
| Shrink (shrink) | ctrl+down, alt+- | _ | Clear priority |
| TileAll (tile_all) | alt+T | T | All grid toggle; project grid widens to All |
| Flip (flip) | alt+i | d | Flip focused card |
| FlipAll (flip_all) | alt+I | D | All on-screen cards over / face up |
| FocusLeft/Right/Up/Down (focus_*) | alt+←→↑↓ | h l k j, ←→↑↓ | Geometric neighbour; left edge to sidebar |
| ResizeLeft/Right/Up/Down (resize_*) | alt+shift+arrows | H L K J | ±0.05 split ratio (not when tiled) |
| Jump(1-9) (jump_N) | alt+1..9 | 1..9 | Nth live session (sidebar order), else toast |
| NextSession / PrevSession | alt+j, ctrl+tab / alt+k, ctrl+shift+tab | n / p | Cycle live order (wraps) |
| NextProject / PrevProject | alt+J / alt+K | ] / [ | First session of next project; in a grid, next tab (skips ★) |
| NextTab / PrevTab | — | ) / ( | Cycle layout tabs (`cur`) |
| ShowTab(0) (tab_all) | alt+0 | 0 | All tab |
| ShowTab(1-9) (tab_N) | — | — | Nth project tab, else toast |
| LastTab (last_tab) | alt+l | tab | Tab before |
| ToggleProjectTabs | — | — | Setting plus toast |
| Attention (attention) | alt+a | a | Next blocked, else done-unseen session |
| ToggleSidebar | alt+s | s | Show/hide sidebar |
| FocusSidebar | alt+b | b | Sidebar keys toggle / strip keys / squeezed toast |
| Palette | alt+p | :, space | Command palette |
| Usage | alt+u | u | Usage view (refresh) |
| Profiles | alt+o | o | Profiles view (refresh data) |
| Proxy | alt+y | y | Proxy view |
| Bridge | alt+g | g | Bridge view |
| Stats | — | S | Stats view |
| Settings | alt+, | A | Settings view |
| QuickInstall | — | I | Quick-install view |
| Setup | — | e | Setup view |
| Help | f1 | ? | Help overlay |
| Rename | — | , | Inline rename (terminals only), shows sidebar |
| Themes | — | t | Palette prefilled "theme " |
| ToggleIcons | — | i | Nerd font icons plus toast |
| ToggleTrim | — | — | Proxy compression plus toast |
| ToggleAutoResume | — | — | Auto-resume plus toast |
| ToggleCaveman | — | — | Caveman for new sessions plus toast |
| RefreshUsage | — | r | Refresh, toast "refreshing usage…" |
| UsageDetails | alt+U | U | Sidebar usage totals / every profile |
| Continue | alt+r | R | Launcher on Continue tab |
| Orchestrator | alt+O | O | Focus orchestrator, or launcher ★ tab |
| SwitchLogin | alt+L | P | Move focused session to another login |
| CloseProject | — | X | Close current project (confirm if live) |
| OpenClipboardFolder | ctrl+shift+o | — | Clipboard path becomes project, then launcher |
| OpenedFile | — | f | Preview last `bro open` file |
| AgentFiles | — | F | Agent files overlay |
| Watchdog | — | w | Watchdog overlay |
| WatchdogPrompt | — | — | Reviewer prompt editor |
| Quit | — | q | Quit (confirm if terminals) |

Act source: `keymap.rs:12-97,293-403`; behaviours: `app/commands.rs:18-208`.

---

## 4. Things a GUI port must not miss
1. **Layout is computed inside draw and reused by input.** Before the first draw, `outer` is empty: alt+arrows does nothing, and Split falls back to `body`, which is 0×0 before the first draw, so it splits Down. The GUI needs an explicit layout pass that runs before input is handled.
2. **Units are terminal cells and assume a 2.2 cell aspect.** This affects `stack_rects`, `group_regions` and `Place::Split`, plus every constant: COMFORT 102×32, USEFUL 32×7, FOLDED 10×2, LOCK_MIN 16×5, frame rings, the 14×6 frame minimum, the 72-col sidebar cutoff, sidebar 26..half, the 20/44/96 column, and the 5/24/12 button and hint minimums. Lock `at` is also in cells, measured from the body start. A pixel layout must keep a virtual cell grid or rescale all of these.
3. **Grow and locks never mutate the base layout.** Priority drops automatically whenever focus moves, from any source. Locks are kept per tab (All, a project, ★).
4. **Shared borders change hit geometry:** the line belongs to the right or lower tile.
5. **Paste chords are intercepted globally,** including in terminals, and read the clipboard asynchronously. A late result is discarded after any other input. alt+v pastes images and files as paths.
6. **The screensaver swallows the waking key or click,** but not a paste or a mouse move. A bare alt down and focus-gained also wake it.
7. **Mouse-mode programs keep plain clicks.** bro keeps alt (always), shift (selection), ctrl (links and grow), and frame clicks. A ctrl+click on a link opens the link instead of growing.
8. **Links open differently:** terminal links on release at the same cell; card-back links on press.
9. **The Codex alt+↑ exception:** it goes to the program.
10. **Palette and Help close on any mouse-down,** even one inside them.
11. **NextTab/PrevTab change `cur` while stacked** with no visible effect.
12. **A hard-coded toast string:** "the orchestrator isn't running · alt+O starts it" ignores rebinding. `app/project_tabs.rs:125`
13. **Sidebar row double-click is computed but unused.**
14. **Opening any view while tiled exits the grid.** Opening any pane dissolves a manual stack.
15. **FocusLost does not cancel a tile-edge drag.**
16. **The prefix-armed state drops pastes;** a second prefix press sends the prefix to the program.
17. **Overrides apply in alphabetical order.** One chord per override; it steals the chord from other actions in that table. Cmd/Super can never be bound.
18. **`ShowTab(1..9)` has no default chords,** so alt hints show nothing for project tabs.
19. **Remote and bridge launches never steal focus or change tabs;** local launches into another project do switch tabs.
20. **The window-focus flag affects done tracking:** a session working while the window is unfocused counts as unseen.
21. **History records after every event batch.** A launcher closed without launching is erased from it. Back/forward input exists only on Windows today; the GUI should use mouse X1/X2 everywhere.
22. **Copy text comes from rendered cells,** so it can include card-back text. The GUI should copy from the alacritty grid, but keep the "copied N characters" toast.
