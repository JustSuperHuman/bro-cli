# C: layout, sidebar, project tabs, pane chrome (LAY, SB, TAB, CHR)

LAY-001 | DONE | gui/layout.rs `panes` runs before drawing each frame and writes body/outer/inner/tile_regions/grown; the window only shows after the first frame, so no input meets a 0×0 body
LAY-002 | DONE | virtual cell grid at the terminal font's cell (layout.rs `panes`, `Layout::to_cells/to_px`); px constants stand in for the cell ones (sidebar cutoff 640, sidebar 220..w/2, × at >160, hints ≥160×90); lock `at` in cells from body start (mouse.rs TileEdge) (DD-003)
LAY-003 | DONE | layout.rs reuses crate::layout (rects, group_regions, stack_rects) and crate::priority (lock, prioritize, want) unchanged
LAY-004 | DONE | shared app::Tab
LAY-005 | DONE | shared App::new_tab
LAY-006 | DONE | shared App::open
LAY-007 | DONE | shared App::open reads `outer` in virtual cells (≈2.1 aspect cells)
LAY-008 | DONE | shared layout.rs Node::split
LAY-009 | DONE | shared Node::remove / App::close
LAY-010 | DONE | Node::rects in layout.rs `panes`; cards inset from the exact tiling
LAY-011 | DONE | shared run_act resize
LAY-012 | FIXED | mouse.rs `divider_near` (deepest first, the gutter between cards is the grab zone); hover cursor was ResizeH for every divider, now ResizeV for horizontal ones (`cursor_at`)
LAY-013 | DONE | mouse.rs `divider_near` filters zoom/stacked/grown; `moved` sets the ratio clamped 0.1..0.9; release ends; the hovered divider lights (DD-014)
LAY-014 | DONE | shared run_act NextTab/PrevTab
LAY-015 | DONE | quirk kept (shared run_act)
LAY-016 | DONE | shared move_focus over `outer` from layout.rs
LAY-017 | DONE | shared move_focus
LAY-018 | DONE | shared run_act Zoom
LAY-019 | PARTIAL | header/frame double-click zooms (mouse.rs `pane_press`, full-card hit in chrome `draw_card`); inside a terminal that doesn't use the mouse a double-click selects a word (`body_press`, SelectionType::Semantic) instead of zooming — needs a design delta for word selection, or a switch in body_press
LAY-020 | DONE | shared focus_pane
LAY-021 | DONE | shared App::close
LAY-022 | DONE | shared state
LAY-023 | DONE | shared tile_ids
LAY-024 | DONE | shared toggle_tiles
LAY-025 | DONE | shared toggle_tiles toast
LAY-026 | DONE | mouse.rs `side_click` → activate_row
LAY-027 | DONE | shared show_project_tiles
LAY-028 | DONE | shared toast; the sidebar/strip switch now reads "▸ minimized N" so the toast's words match (sidebar.rs)
LAY-029 | DONE | shared show_project_tiles
LAY-030 | DONE | shared include_in_tiles
LAY-031 | DONE | shared include_in_tiles/clear_stack
LAY-032 | DONE | shared sessions.rs restore_background_tiles
LAY-033 | DONE | shared go_session
LAY-034 | DONE | shared focus_pane
LAY-035 | DONE | shared focus_pane; mouse.rs `focus` mirrors app/mouse.rs
LAY-036 | DONE | shared App::close
LAY-037 | DONE | layout.rs `panes` (orchestrator_column on the virtual body)
LAY-038 | DONE | shared App::tile_groups
LAY-039 | DONE | layout.rs `panes`, same weights as draw.rs
LAY-040 | DONE | shared group_regions
LAY-041 | DONE | layout.rs `panes` framed = 2+ panes and ≥14×6
LAY-042 | DONE | shared stack_rects
LAY-043 | DONE | shared orchestrator_column on the virtual body
LAY-044 | DONE | layout.rs `panes` (column outside groups, extended after the grow, part of the lock pass)
LAY-045 | DONE | shared minimize_pane; chrome `draw_header` minimize button only in tile_all on terminals
LAY-046 | DONE | shared minimize_pane
LAY-047 | DONE | sidebar.rs tile row and strip switch → toggle_minimized_visibility (tile_all only)
LAY-048 | DONE | chrome `draw_panes` centred "All sessions minimized · show minimized in the sidebar"
LAY-049 | DONE | mouse.rs `side_click` shift/ctrl → toggle_stack; shift+Enter shared
LAY-050 | DONE | shared toggle_stack
LAY-051 | DONE | shared toggle_stack
LAY-052 | DONE | shared go_session/include_in_tiles
LAY-053 | DONE | sidebar.rs `draw_strip` lights `drawn_tab` (showing_tab None for a manual stack)
LAY-054 | DONE | mouse.rs `pane_press`/`body_press` ctrl (no shift, not on a link) → toggle_priority; Grow/Shrink shared
LAY-055 | DONE | shared after_events
LAY-056 | DONE | layout.rs `panes` applies prioritize on a copy
LAY-057 | DONE | shared priority::want(tile_body)
LAY-058 | DONE | shared priority
LAY-059 | DONE | shared priority
LAY-060 | DONE | layout.rs `panes` moves group frames with g.frames
LAY-061 | DONE | shared priority
LAY-062 | FIXED | a folded card's body had no hit (a click there fell to the gutter); chrome `draw_card` now registers the whole card (focus/zoom), header only, PTY untouched
LAY-063 | DONE | divider_near and tile_edge off while grown; zoom skips priority (layout.rs)
LAY-064 | DONE | shared App::tile_edge over virtual cells; the gutter between cards is the grab zone (mouse.rs `tile_edge_at`)
LAY-065 | FIXED | with shared borders only half of a gutter reached the right/lower tile's edge; mouse.rs `tile_edge_at` now reaches across (used by edge_press, cursor_at, chrome draw_edges)
LAY-066 | DONE | mouse.rs `edge_press` anchors to the held cut and drops the others
LAY-067 | FIXED | an edge press now also focuses the pane it belongs to (mouse.rs `edge_press`)
LAY-068 | FIXED | mouse.rs: another button cancels the drag (`pressed`), a drag clears double-click state, and a lock is only made once the edge moves a cell (`moved`), as the terminal's per-cell drag events
LAY-069 | FIXED | double-click on a held edge releases the cut; on an unheld edge it now zooms (mouse.rs `edge_press`)
LAY-070 | DONE | layout.rs `panes` priority::lock pass, frames and column move with it
LAY-071 | DONE | shared follow_tab / after_events pruning
LAY-072 | DONE | tile_edge off when zoomed/grown; presses with an overlay never reach edge_press; chrome `draw_edges` returns with an overlay
LAY-073 | FIXED | the setting did nothing in the window; layout.rs `panes` now joins touching tiles (no gutter, 1px overlap so one line), chrome `draw_card` squares shared corners and draws the focused card last; the shared line is grabbed as the edge (PaneBox::edge_strips)
LAY-074 | N/A | content already sits 10px inside every card (chrome `draw_terminal`), so text never meets a shared line
SB-001 | DONE | gui/mod.rs `draw_all`: sidebar flag and window ≥ 640 px (layout::SIDEBAR_MIN_WINDOW for the 72 cols, DD-003)
SB-002 | DELTA | DD-003: width from gui prefs (default 272 px) clamped 220..w/2 while dragging, w/2 when drawn (layout.rs `frame`); no w/4 auto width
SB-003 | DELTA | DD-020: WinUI navigation pane, no frame; keyboard focus shows as the selected row's 1px accent outline and the filter's "Type to filter"
SB-004 | DELTA | DD-020: "bro" wordmark with an accent dot (sidebar.rs `draw`)
SB-005 | DONE | sidebar.rs gear icon button → SideHit::Settings; sits in the header's right cluster (DD-020)
SB-006 | PARTIAL | sidebar.rs draws util::clock top-right; probe_local_offset at gui::run; with terminals open the 400 ms ticks redraw it, but on an idle welcome screen it can lag up to 30 s — DD-016 needs gui/mod.rs `next_wake` to wake at the next minute
SB-007 | FIXED | live keymap in the filter box: FocusSidebar chord, and "{help} keys" (F1 keys) while focused; the long focused placeholder used to run under the label ("Type to filter" now)
SB-008 | FIXED | DD-012: both read the live keymap (sidebar.rs `draw`)
SB-009 | DONE | 10 px padding (sidebar.rs PAD)
SB-010 | DONE | sidebar.rs `draw`: filter, pinned orchestrator + hairline, list, footer (tile row, New/Continue, usage, status); the filter box always shows (WinUI search box)
SB-011 | DELTA | DD-003: tile row when the strip is off and 2+ terminals; the pixel footer caps its height instead of dropping rows
SB-012 | DELTA | DD-003: New/Continue always shown in the pixel footer; usage lines give way first when short
SB-013 | DELTA | DD-003: footer measured in px and capped (sidebar.rs `draw`), usage lines stop at the room, status rows pinned
SB-014 | DONE | shared crate::sidebar::Row via app.rows()
SB-015 | DONE | shared rows order
SB-016 | DONE | shared rows
SB-017 | DONE | shared numbering; sidebar.rs `draw_row` shows n
SB-018 | DONE | sidebar.rs `row_height` gap above projects, none for the first
SB-019 | DELTA | DD-021: folder icon (open for the current project), grid icon after the name when its grid shows, semibold name in text colour
SB-020 | DELTA | DD-021: alert icon for attention, no "N live", age when idle, × on hover/selection (danger when selected)
SB-021 | DONE | sidebar.rs util::short_path
SB-022 | DONE | sidebar.rs branch icon + ui::fit(branch, 18) from projects::git_branch
SB-023 | DELTA | DD-001: fills for the focused / stacked rows instead of bar glyphs
SB-024 | DONE | sidebar.rs number semibold, text colour when focused, faint otherwise (no project colours, DESIGN §7)
SB-025 | DELTA | DD-021: spinner / alert / check icons, nothing when idle (chrome `status_dot`)
SB-026 | DELTA | DD-022: real logos (chrome `session_mark`)
SB-027 | DONE | sidebar.rs `draw_row` same primary-text order; orchestrator = star mark + "Orchestrator"
SB-028 | DONE | strong (semibold) when focused/selected/orchestrator (gold); else lighter
SB-029 | DONE | secondary "login · model" faint; orchestrator name + login · model
SB-030 | DELTA | DESIGN §8: age, replaced by × on hover/selection (danger when selected)
SB-031 | DELTA | DD-023: no tinted ground behind the gold star; the row is marked by the gold star and gold name
SB-032 | DONE | sidebar.rs note line with pause / recap icon, danger-muted or muted
SB-033 | DONE | right side laid out first, text cut with an ellipsis (ui.rich max width)
SB-034 | DELTA | DD-001: selection = t.selected fill + 1px accent outline
SB-035 | DELTA | quirk gone: the selection fill comes from the theme tokens (works on light themes)
SB-036 | FIXED | the focused session's fill showed while the sidebar had the keyboard (next to the selection); now only when it doesn't (sidebar.rs `draw_row`); hover fill per DD-014
SB-037 | DONE | sidebar.rs `draw_list` pins row 0 above the clip with a hairline; filtered like any row
SB-038 | DONE | WinUI text field with the filter and caret (sidebar.rs); "Nothing matches" when empty
SB-039 | DONE | shared crate::sidebar::build
SB-040 | FIXED | shared sidebar_filter_changed; a filter change now also resets the pixel scroll and manual hold (SideState.filter)
SB-041 | DONE | mouse.rs `pressed` leave_sidebar outside it; FocusLost shared
SB-042 | DONE | shared App::paste
SB-043 | DELTA | DD-003: pixel scrolling; keyboard selection kept in view (sidebar.rs `draw_list`)
SB-044 | FIXED | the wheel only scrolled over sidebar hits; now anywhere over the sidebar (gaps, header, footer), and beside the launcher (mouse.rs `wheel`)
SB-045 | DONE | scroll clamped to content − view
SB-046 | FIXED | DD-014: the scrollbar is draggable now and a press on its track jumps (Action::SideScroll, SideState grab_bar/drag_bar); shows on scroll/move/hover, fades
SB-047 | DONE | sidebar.rs "Tile all · N" + TileAll chord (keytip while alt held)
SB-048 | FIXED | "Tiled · N" + "▸/▾ minimized N" (chevron icon + the terminal UI's words; was "Show/Hide minimized N"); shown while some are minimized
SB-049 | DONE | sidebar.rs New session (primary) and Continue (icon only + tooltip when narrow)
SB-050 | DONE | real-logo icon buttons with name tooltips, dropped below 210 px (DD-022)
SB-051 | DONE | SideHit::Quick → shared quick_launch
SB-052 | DONE | chevron + "Usage left" + info button; heads "5h"/"wk", "resets" while alt held
SB-053 | DONE | info button → Stats (wins over the header); header → toggle_usage_details (persisted)
SB-054 | DONE | shared usage_lines; notes "…"/"unavailable" (the "n✗" partial note isn't shown by either UI, same as the TUI)
SB-055 | FIXED | the row's launch hit was registered after the Fable arrow, so the arrow launched Claude; the arrow now wins (sidebar.rs `draw_footer`)
SB-056 | DONE | shared usage_lines expanded
SB-057 | FIXED | meter capped at 8 segments as the TUI's ▰▱ (was up to 12); fixed name column
SB-058 | DONE | sidebar.rs `left_color` danger/warn/good; empty segments faint
SB-059 | DONE | figures, ≈ prefix, "—", countdown without meter room
SB-060 | DONE | "Resets @ …" / "no reset reported" while alt held
SB-061 | FIXED | hover now shows the "+" at the row's end (figures moved left to keep its room); click → LaunchUsage
SB-062 | DONE | Proxy row, check/spinner/cross (DD-021), click → Proxy
SB-063 | DONE | Phone row, same states, click → Bridge
SB-064 | FIXED | the right edge now stays lit while it's dragged (SideState.resizing); resize cursor (DD-014) instead of the ↔ handle
SB-065 | DONE | Drag::Sidebar captures until release, clamp 220..w/2, saved on release, window shrink doesn't overwrite
SB-066 | FIXED | double-click resets to the default width and now saves it at once (mouse.rs)
SB-067 | DONE | shared FocusSidebar strip/toast via side_area (gui/mod.rs)
SB-068 | FIXED | hits are searched last-first: × and ⓘ already won; the Fable arrow didn't (see SB-055)
SB-069 | DONE | mouse.rs `side_click` focuses the sidebar → activate_row
SB-070 | DONE | mouse.rs `side_click` (ctrl/shift stack, middle ask_close, others swallowed)
SB-071 | DONE | SideHit::Close → ask_close; CloseProject → close_project
SB-072 | FIXED | a blank sidebar spot outside the list (header, footer gaps) left the sidebar; now it focuses it (mouse.rs `pressed`)
SB-073 | DONE | a dialog's scrim takes those clicks (mouse.rs overlay branch)
SB-074 | DONE | quirk kept: no row double-click action
SB-075 | DONE | mouse.rs `launcher_side_click` mirrors the TUI; wheel scrolls the sidebar beside it
SB-076 | DONE | shared start_rename
SB-077 | FIXED | the field's hint ran into the next row; the renaming row is now 48 px and kept in view (sidebar.rs `row_height`, `draw_list`); × off while renaming
SB-078 | DONE | shared rename_key/paste; field shows the text and caret
SB-079 | DONE | quirk kept (shared)
SB-080 | FIXED | with the launcher's project picker open the sidebar and tabs reacted; now they don't (mouse.rs `beside_launcher`)
SB-081 | DONE | the gear sits in the header (stale doc comment is in app/mod.rs)
TAB-001 | DONE | gui/mod.rs strip when tabs are open and the setting is on (window ≥ 320 px)
TAB-002 | DONE | shared project_tabs/strip_items
TAB-003 | DONE | shared showing_tab via drawn_tab
TAB-004 | DONE | shared show_tab
TAB-005 | DONE | shared show_tab
TAB-006 | DONE | shared show_tab
TAB-007 | PARTIAL | DD-012: the toast still hard-codes "alt+O" — app/project_tabs.rs `show_tab` should use keymap.primary(Act::Orchestrator)
TAB-008 | DONE | shared show_tab
TAB-009 | DONE | shared click_tab via strip_click
TAB-010 | DONE | shared step_tab; wheel includes ★
TAB-011 | DONE | shared follow_tab
TAB-012 | DONE | shared follow_tab
TAB-013 | FIXED | logic shared; the keys' pick wasn't drawn at all — now a focus outline with its × (sidebar.rs `draw_strip`)
TAB-014 | FIXED | shared strip_key; the moving pick is now visible
TAB-015 | DONE | shared strip_key
TAB-016 | DONE | shared strip_key
TAB-017 | DONE | shared strip_key; the outline goes with the pick
TAB-018 | DONE | SideHit::Strip(Tab) → strip_click
TAB-019 | DONE | × registered after its tab; middle-click → strip_click(_, true)
TAB-020 | DONE | folds → StripHit::More → palette "tiles "
TAB-021 | DONE | minimized switch and + (sidebar.rs `draw_strip`)
TAB-022 | FIXED | the wheel only stepped over a tab or button; now anywhere over the strip, not while the launcher is open (mouse.rs `wheel`)
TAB-023 | DONE | mouse.rs `launcher_side_click` Strip(Project) picks the launcher's project
TAB-024 | FIXED | "▸/▾ N minimized" at the far end (was Show/Hide), + follows the last tab (DD-027)
TAB-025 | FIXED | All's count included minimized sessions; now tile_ids(None) as the TUI (grid icon + "All N", DD-027)
TAB-026 | FIXED | the ★ tab now shows the orchestrator working (gold spinner) and waiting/done badges (was a fixed "1"); gold star; hairlines instead of " │ " (DD-027)
TAB-027 | DONE | spinner/folder, name, count, alert+N, check+N, × (DD-021)
TAB-028 | FIXED | names now shorten (to 44 px) before tabs fold (sidebar.rs `strip_cap`, tested)
TAB-029 | FIXED | was one trailing "…" that could hide the showing tab; now "‹ n" / "n ›" folds around the pick (else the showing tab) with an alert icon when a folded tab wants you (`strip_window`, tested)
TAB-030 | FIXED | × on hover, on the showing tab and now on the keys' pick; its room always kept
TAB-031 | DONE | spinner while working, folder icon otherwise (DD-021)
TAB-032 | FIXED | DD-027 TabView fills; keys' pick now outlined; the focused session's project name semibold
TAB-033 | DONE | sidebar.rs tile row only when app.strip is off
CHR-001 | FIXED | gui/mod.rs order matches except toasts were drawn before the launcher; now toasts go last unless a dialog is open (toasts.rs `draw_late` from hints.rs)
CHR-002 | FIXED | toasts were hidden under the launcher; now over it and under dialogs; alt hints off with any overlay (app.alt_hints)
CHR-003 | DONE | layout.rs focused = focus && !side_focus && !overlay (the launcher covers the cards anyway)
CHR-004 | FIXED | views: icon + title, muted and lighter when unfocused; the zoomed/grown sub-label now shows on views too (chrome `draw_header`)
CHR-005 | DELTA | DD-002: accent edge when focused, hairline otherwise
CHR-006 | FIXED | the unfocused orchestrator card now keeps a faint gold edge (chrome `draw_card`)
CHR-007 | FIXED | quirk resolved as the comment meant: the orchestrator keeps its colour when unfocused
CHR-008 | DELTA | DD-020/DD-002: 1px edges, 8px corners
CHR-009 | DELTA | DD-002: faintly tinted header instead of a solid band
CHR-010 | DELTA | DD-001/DD-002: logo, title, project as quiet meta (omitted in a group); minimize/close on the right; no ▶ marker
CHR-011 | FIXED | logo, title, login · model, project — cwd; inside a group "n · title" (it repeated login · model as meta, now dropped)
CHR-012 | FIXED | unfocused titles muted and now medium weight (were semibold)
CHR-013 | DONE | chrome `draw_header`: scrollback, remote size, resume note, watchdog, then the indicator, in the TUI's order
CHR-014 | DELTA | DD-024: "watchdog reviewing/done/paused/n/m" as quiet coloured text, no glyphs
CHR-015 | FIXED | the header showed a check when done; the TUI header never says done — now spinner / alert only (DD-024)
CHR-016 | DELTA | DD-024: status at the header's right in every mode
CHR-017 | DONE | chrome `draw_header` reads Term facts as data (scroll_offset, remote_grown, resume_note, svc.watchdog, activity)
CHR-018 | FIXED | chrome `subtitle`: same words and chords, in the card's accent (DD-002); a tab zoomed before tiling all made every grid card read "zoomed" — zoom is now read as the TUI (stack_zoom when stacked, else the tab's)
CHR-019 | FIXED | × caption button in the header when > 160 px and not zoomed, shown on hover/focus; the same stale tab zoom hid it on every grid card (chrome `draw_header`)
CHR-020 | FIXED | hover turns the × the danger colour on a neutral fill, with a "Close · ctrl+w" tooltip; click → ask_close
CHR-021 | DONE | minimize/restore icon button in tile_all on terminals (header right, DD-020)
CHR-022 | DONE | buttons registered after the header hit, so they win
CHR-023 | DELTA | DD-002: group frame is a neutral container
CHR-024 | FIXED | label now draws the agent's real logo instead of the Nerd glyph inside the text (DD-022): project · logo who — cwd · N sessions; focus brightens it (chrome `draw_group`)
CHR-025 | FIXED | gold edge focused (accent) and now faintly unfocused too
CHR-026 | DONE | with shared borders tiles meet on one line with square corners there, so lines join cleanly (no box glyphs to join)
CHR-027 | DONE | chrome `draw_edges` lights the gutter or shared line under the pointer; resize cursor (DD-014)
CHR-028 | DONE | held edges keep a muted grip midway; the dragged edge lights as the pointer stays on it
CHR-029 | FIXED | folded slivers showed only a cut-off header; now the name's letters run down a narrow sliver (chrome `draw_folded`)
CHR-030 | DONE | alt+double-click on header or content flips (mouse.rs); only terminals flip (shared flip_pane)
CHR-031 | DONE | shared Flip/FlipAll over `outer`
CHR-032 | DONE | shared card::Flip; chrome narrows the card by `face`
CHR-033 | FIXED | the card narrows around its middle while it turns (pixel squash: the layer shows at its sides, its own edge stays); the face now also dims while cover < 0.6 (chrome `draw_card`)
CHR-034 | DONE | chrome `draw_panes` retains flips; the back skips the terminal draw, the PTY keeps running
CHR-035 | DONE | shared App::key
CHR-036 | FIXED | the back had no hit, so its links/files didn't open on press and alt+double-click there didn't turn it; chrome `draw_back` registers it, mouse.rs opens rows on press; the wheel no longer scrolls the hidden history and the pointer no longer shows the program's links (`pane_wheel`, `cursor_at`)
CHR-037 | DONE | shared card::lines (title in Inter)
CHR-038 | DONE | shared card::lines
CHR-039 | DONE | shared card::lines
CHR-040 | DONE | chrome `draw_back` links with probe.up
CHR-041 | DONE | shared card::lines
CHR-042 | DONE | shared screensaver_step in after_events
CHR-043 | FIXED | mouse input never counted as a touch (clicks, moves, wheel, drops, IME), so the saver ran under an active mouse and a move didn't turn cards back; mouse.rs now calls App::touched (verified: a move after 60 s turns the cards face up)
CHR-044 | PARTIAL | mouse fixed this pass: the waking click/wheel is swallowed, a move is still handled, drops and IME text still land (mouse.rs); but a ctrl+v paste that wakes the saver is swallowed, because the window pastes through the key, not a Paste event — gui/mod.rs `key` should call App::touched() before handing a paste chord to the workspace
CHR-045 | DONE | gui/mod.rs ModifiersChanged → Event::Alt; keys/FocusLost hide via App::handle; 300 ms hold shared
CHR-046 | DONE | gui/mod.rs alt alone only (AltGr = ctrl+alt excluded)
CHR-047 | DONE | hints.rs `alt_key` strips "alt+"
CHR-048 | DELTA | DD-026: a key bar on cards ≥160×90 px, centred near the bottom, dropping tips from the end
CHR-049 | FIXED | All/★/+ key tips were there; the "K ‹ tab · J tab › · l last" bar after the tabs was missing (hints.rs `draw_tips`)
CHR-050 | DELTA | DD-026: WinUI key tips on session numbers and clickable spots
CHR-051 | DONE | same: no tips without alt chords (hints.rs)
CHR-052 | DONE | hints.rs reads layout and hit metadata (DD-015)
CHR-053 | DELTA | DD-020: content layer, no frame; block centred
CHR-054 | DELTA | DD-004/DD-020: Inter wordmark "bro" instead of the block-letter art
CHR-055 | FIXED | the wordmark now takes the logo's colours: a moving rainbow on animated themes, accent → shine otherwise (welcome.rs)
CHR-056 | DONE | "The agentic terminal workspace" muted
CHR-057 | FIXED | showed the setup CTA while profiles were still loading (TUI shows launch); now only once loaded with nobody signed in; label "Install an agent and sign in" (welcome.rs)
CHR-058 | DONE | welcome.rs 8 rows with live chords, clickable
CHR-059 | FIXED | profiles/proxy/bridge with check/spinner/cross (DD-021); "profiles loading" now in the TUI's words
CHR-060 | DONE | toasts.rs kind → icon and colour (DD-023 icons, no tile)
CHR-061 | DONE | shared Toasts (2, replace, 2 s)
CHR-062 | DELTA | DD-003: top-right, newest on top, wraps to 3 lines at 380 px instead of cutting at 60 columns
CHR-063 | DONE | toasts.rs prefix card bottom-right "{prefix} …" + prefix_hint, replacing toasts
