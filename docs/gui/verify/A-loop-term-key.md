LOOP-001 | DONE | gui/mod.rs run(): one mpsc channel; the "bro-gui-events" thread forwards every pane::Event to winit as UserEvent::App through the EventLoopProxy; UserEvent::Wake is the window's own variant
LOOP-002 | FIXED | winit hands over every pending event (user_event / window_event → App::handle), then about_to_wait runs App::after_events, checks quit and asks for the frame (wakes are coalesced per pane, so no 12 ms cap is needed); fixed: mouse handlers asked winit for the frame directly and Windows painted it before about_to_wait, i.e. before after_events (a grown tile snapped back a frame late); Gui::redraw now only marks the window dirty and about_to_wait asks after the housekeeping (gui/mod.rs)
LOOP-003 | DONE | Gui::next_wake starts from App::next_deadline (30 s base, 4 ms floor), 16 ms while something animates
LOOP-004 | DONE | shared App::next_deadline; the window's spinners also set ui.ticking
LOOP-005 | DONE | shared App::next_deadline (110 ms for rain / animated backdrops)
LOOP-006 | DONE | shared deadline; toasts expire in App::after_events
LOOP-007 | DONE | shared deadline (side_view::bar_wake)
LOOP-008 | DONE | shared 16 ms deadline; chrome.rs draw_card also sets ui.ticking while a card turns
LOOP-009 | DONE | shared alt_wake / screensaver_wake; ModifiersChanged (alt alone) sends Event::Alt
LOOP-010 | DONE | shared tick_every (terminals always, others while visible)
LOOP-011 | DONE | Gui::tick sends Event::Tick on ResumeTimeReached, and from about_to_wait once the deadline has passed (so constant output can't starve it); App::handle runs pump_resumes then the due polls
LOOP-012 | DONE | shared Waker/WakeSignal coalescing; App::handle acknowledges then polls; the window marks itself dirty
LOOP-013 | DONE | Gui::after_events calls App::after_events unchanged, then the mirrors' OSC 52 / bell requests, the media gallery and panel requests
LOOP-014 | DONE | input::key returns None for releases and marks repeats KeyEventKind::Repeat; App::handle treats Repeat as Press
LOOP-015 | DONE | WindowEvent::Resized resizes the surface and draws at once; layout uses the new size and chrome.rs draw_terminal resizes visible ptys
LOOP-016 | DONE | shared App::handle(Services)
LOOP-017 | DONE | shared (Term::reregister)
LOOP-018 | DONE | shared thread + error toast; gui/mod.rs app_event also refuses file: links to programs
LOOP-019 | FIXED | gui/mod.rs app_event: OpenProject is handled by the window (request_user_attention, add_project, leave_sidebar) instead of App::handle, which wrote a BEL to stdout (DD-009)
LOOP-020 | DONE | App::new toasts every keymap error (the window builds App::new)
LOOP-021 | DONE | run() → Gui::shutdown: prefs saved, App::shutdown (quitting, close every pane, svc.shutdown); the window now hides as the loop exits (gui/mod.rs exiting) so the bridge's wait doesn't leave it frozen on screen; restoring fonts/terminal doesn't apply
LOOP-022 | FIXED | Focused(false) → App FocusLost (filter, alt, term_focused) plus the window's alt_alone / drag / pressing; fixed: a sidebar or media-panel edge drag cut short now saves its width (gui/mod.rs window_event)
LOOP-023 | DONE | decided: fix; the window ends every drag when it loses focus, a held tile edge too (gui/mod.rs window_event Focused)
LOOP-024 | DONE | Focused(true) → App FocusGained: term_focused + touched()
LOOP-025 | DONE | winit Focused drives App.term_focused, which track_agents and update_insights read for "seen"
LOOP-026 | DONE | shared App::focused
LOOP-027 | DONE | shared App::visible
LOOP-028 | DONE | shared follow_focus / pick_project
LOOP-029 | DONE | shared current_project
LOOP-030 | DONE | shared run_act tail
LOOP-031 | DONE | shared after_events
LOOP-032 | DONE | shared History; record_view in after_events
LOOP-033 | DONE | shared History::record
LOOP-034 | DONE | shared App::navigate
LOOP-035 | DONE | shared App::navigate
LOOP-036 | DONE | shared App::navigate
LOOP-037 | FIXED | mouse Back/Forward buttons → Event::Navigate (gui/mouse.rs pressed); fixed: the keyboard's BrowserBack / BrowserForward keys were dropped by input::key, now gui/mod.rs gui_key sends Event::Navigate on every platform
LOOP-038 | DONE | input reads the last frame: Gui.hits, Gui.layout, overlay_grid, painted, plus App.outer/inner/body set by layout::panes
LOOP-039 | FIXED | draw_all / layout::panes set body, side_area, strip, drawn_tab, outer, inner, tile_regions, grown before painting; chrome.rs keeps flips.retain and card_links; sidebar.rs clamps side_sel; fixed in gui/mod.rs: strip_sel cleared while the strip is off, and a session that left the screen loses its selection after the frame
LOOP-040 | PARTIAL | chrome.rs draw_card applies a fallback view's render Actions (app.apply + Tick) in the middle of the frame; collect them in Painted and apply them in Gui::frame_inner after draw_all (integrator: chrome.rs + mod.rs)
LOOP-041 | DONE | App::key / App::paste and gui/mouse.rs pressed clear clipboard_request; App::handle(ClipboardText) checks the token
LOOP-042 | DONE | CursorMoved updates Gui.pointer, which every hover fade and the pointer shape read; also fixed here (CHR-043): mouse moves, clicks, wheel, drops and IME text now count as a touch (gui/mod.rs window_event), and a click or wheel that wakes the screensaver does only that
LOOP-043 | PARTIAL | gui/mouse.rs follows the order through its hit list; gaps: a double-click inside a terminal selects a word instead of zooming (LAY-019), and a press on a link doesn't start a selection (TERM-044)
LOOP-044 | PARTIAL | gui/mouse.rs pressed clears Gui.link_press, but a drag (moved while pressing) and the wheel don't; clear it in mouse.rs moved (while pressing) and wheel
LOOP-045 | PARTIAL | body presses focus with any button, but gui/mouse.rs pane_press (Header) returns before focus() for a right click; focus first for every button
LOOP-046 | DONE | Gui.drag owns the gesture until release whatever is under the pointer (winit captures the mouse while a button is down)
TERM-001 | DONE | chrome.rs draw_terminal: term.ensure_started at term/draw.rs fit() of the card's content on its first frame
TERM-002 | DONE | shared start_now on App.body (the window's virtual cells are terminal cells)
TERM-003 | DONE | shared panes/term.rs (the window's sessions are the same Term)
TERM-004 | DONE | shared resolve()
TERM-005 | DONE | shared Term::start
TERM-006 | DONE | shared Term::start (BRO_BIN is the bro.exe the window runs as)
TERM-007 | DONE | shared Term::start
TERM-008 | DONE | shared Term::start
TERM-009 | DONE | shared Term::start
TERM-010 | DONE | shared panes/writer.rs
TERM-011 | DONE | shared start_reader (also feeds the mirror)
TERM-012 | DONE | start_reader feeds vt100, the alacritty mirror (gui/term/mirror.rs ScreenMirror::feed) and bridge_output from the same chunk, in order
TERM-013 | DONE | the session answers CPR after releasing the parser lock; gui/term/mirror.rs Listener drops alacritty's own CPR (is_cursor_report) and writes its other answers (DA, colours, kitty) back
TERM-014 | N/A | fixed panes only exist with Opts.fixed_demo (tests); the window's demo runs live shells. If the window ever shows one: Term::fixed (panes/term.rs) must also resize + feed the mirror, or it draws blank
TERM-015 | DONE | chrome.rs draw_terminal resizes visible, unfolded panes every frame (Term::resize → apply_size → mirror resize); hidden and folded ones keep theirs
TERM-016 | DONE | gui/term/draw.rs Palette from the theme's Tokens (term_fg, term_bg, ansi); the mirror's Env answers OSC 10/11/4 with the same colours
TERM-017 | FIXED | gui/term/draw.rs Palette::resolve: SGR 40–47 / 90–107 colours went through `named`, which painted a white background in the theme's text colour and left black backgrounds dark on light themes; they now take the indexed rules (white text → text colour, black background → light tint). Test
TERM-018 | DONE | gui/term/draw.rs Palette::resolve: 232–255 and grey RGB recast between the theme's bg and fg on painted dark themes
TERM-019 | DONE | gui/term/draw.rs draw: cursor at the program's position when SHOW_CURSOR, not Hidden, not scrolled back; card backs and folded cards never draw a screen
TERM-020 | DONE | the terminal font from crate::fonts (Gui::sync_font); WIDE_CHAR cells drawn over two columns
TERM-021 | PARTIAL | vt100 still parses in the window, so scan, permission_prompt, prompt_option_at, screen_stall and early_failure work; screen_urls() reads Term.links, which only Term::render fills, so card backs lose on-screen URLs. Fix in panes/term.rs screen_urls (detect from the vt100 screen when a mirror is set) or chrome.rs draw_back (use PaneState.links)
TERM-022 | DONE | gui/mouse.rs pane_wheel: 3 lines a notch (touchpads accumulate), clamped to the history; the alternate screen gets arrows
TERM-023 | DONE | chrome.rs draw_header badge "↑{n}" in t.inline (DD-024)
TERM-024 | DONE | shared Term::send resets scroll; gui/term/draw.rs follows scroll_offset
TERM-025 | PARTIAL | alacritty Simple selection inside the pane's grid, clamped, scrolling past the edges (gui/mouse.rs); gap: with a remote-grown pty (view_top > 0) Drawn::point_at ignores view_top, so it selects view_top rows too high; add t.view_top() to point.line in mouse.rs body_press and moved (Drag::Select)
TERM-026 | PARTIAL | starts on press when the program doesn't want the mouse or with shift (gui/mouse.rs body_press); a press on a link sets link_press and returns, so no selection starts there
TERM-027 | DONE | gui/term/draw.rs draws the selection colour over the cell (DD-010)
TERM-028 | PARTIAL | copied on release with the toast "copied N character(s)", blank copies nothing (gui/mouse.rs released); not trimmed like the terminal UI: a triple-click (Lines) copies a trailing newline and spaces before the selection's end stay; trim each line's end and trailing newlines in mouse.rs released and copy_selection
TERM-029 | DONE | selection_to_string from the alacritty grid; card backs aren't in it
TERM-030 | FIXED | clicks already cleared it (gui/mouse.rs pressed); fixed in gui/mod.rs: every key (Gui::key), IME text and dropped files now clear selections and the link press, and a session that leaves the screen loses its selection (frame_inner)
TERM-031 | DONE | gui/window.rs set_clipboard (arboard), also for programs' OSC 52 (DD-008)
TERM-032 | FIXED | gui/term/draw.rs find_links scans the visible rows at the display offset, joins WRAPLINE rows, handles wide cells; fixed: after a scroll the links were found at the old offset until the screen changed, so draw() now bumps the mirror's seq and draws once more when the offset moves. Test
TERM-033 | FIXED | gui/term/draw.rs find_links: OSC 8 cells are now taken before URLs and paths, so a URL keeps only free cells and a path over a hyperlink label is skipped whole (it used to survive on the label's other rows). Test
TERM-034 | DONE | shared links::web_urls
TERM-035 | DONE | links::target for OSC 8 and web_urls; links::open checks again
TERM-036 | DONE | bro_bridge find_file_links in gui/term/draw.rs find_links
TERM-037 | DONE | shared FileCache lookup → links::file_uri with #L/#LC
TERM-038 | DONE | shared Lookup::get answers only confirmed paths
TERM-039 | DONE | chrome.rs draw_terminal: term.file_cache().lookup() + finish(waker); PaneState.links_pending looks again once the worker is done
TERM-040 | FIXED | TTLs shared, but the window only looked for links again when the screen changed, so on a still screen a path that became a file never linked (or a deleted one kept its link); gui/mod.rs tick() now re-runs detection every 2 s
TERM-041 | DONE | shared FileCache
TERM-042 | DONE | gui/mouse.rs open_link → App::open_file_link; a folder's preview falls back to OpenLink → links::open (the file manager)
TERM-043 | DONE | gui/term/draw.rs underlines every link (accent at 45 %, full under the pointer)
TERM-044 | PARTIAL | opens on release at the pressed cell (gui/mouse.rs released); shift skips links; gap: a drag that starts on a link selects nothing; mouse.rs body_press should also start Drag::Select there and open only if the selection stayed empty
TERM-045 | DONE | gui/mouse.rs body_press: ctrl needed for a link when the program wants the mouse; ctrl+click elsewhere grows
TERM-046 | DONE | gui/mouse.rs open_link: file: → App::open_file_link (preview; pictures go to the media panel in gui/mod.rs app_event), others → Event::OpenLink → links::open on a thread
TERM-047 | DONE | link rows + view_top (gui/mouse.rs, chrome hover), prompt_option_at adds view_top, Term::mouse adds view_top
TERM-048 | DONE | shared Term::mouse / encode_mouse; gui/mouse.rs passes events only when wants_mouse
TERM-049 | PARTIAL | encoding shared (encode_mouse); gap: gui/mouse.rs moved reports Drag(Left) for every Drag::Program, even a right or middle drag; keep the pressed button in Drag::Program
TERM-050 | DONE | gui/mouse.rs sends grid cells with a (0,0) area; Term::mouse adds view_top
TERM-051 | PARTIAL | press, release, drag, right, middle pass through only inside content, not flipped, without alt or shift (gui/mouse.rs body_press); gaps: pane_wheel passes through with alt held, and moved sends AnyMotion to a turned card's program
TERM-052 | DONE | gui/mouse.rs: plain clicks to the program; alt, shift (selection), ctrl (links, grow) and the header stay bro's
TERM-053 | FIXED | shared encode_key; fixed in gui/input.rs: ctrl+alt+letter on a layout without AltGr has no logical key in winit (Unidentified) and was dropped; it now falls back to the key's own character
TERM-054 | DONE | shared encode_key (ESC CR); kitty-protocol programs get CSI 13;2u (gui/term/mirror.rs kitty_key)
TERM-055 | FIXED | shared encode_key (ESC[Z, ctrl+Backspace 0x08); fixed in gui/term/mirror.rs kitty_key: with the kitty protocol on, shift+tab went out as CSI 9;3u (alt+tab, so Claude Code / Codex mode cycling broke); now CSI Z, or CSI 9;{mods}u with alt / ctrl / report-all. Test
TERM-056 | DONE | shared encode_key with vt100's application-cursor mode
TERM-057 | DONE | shared encode_key; gui/input.rs maps F1–F24 (F13+ dropped by encode_key); F11 is the window's fullscreen (gui_key)
TERM-058 | DONE | shared encode_key; kitty programs get CSI u with the alt bit
TERM-059 | DONE | Ime::Commit → gui/mouse.rs type_text: into the focused terminal, else App::paste (overlays, rename, sidebar filter)
TERM-060 | DONE | shared Term::paste
TERM-061 | DONE | shared Term::paste
TERM-062 | DONE | shared Term::send
TERM-063 | DONE | shared submit / type_command (Enter written from poll)
TERM-064 | DONE | shared config / Services::remote_min
TERM-065 | DONE | shared remote_min
TERM-066 | DONE | shared poll and Term::resize from chrome.rs draw_terminal
TERM-067 | PARTIAL | chrome.rs draw_terminal pins view_top to the last text / cursor row, but only while scroll == 0: scrolling up jumps into history instead of first showing the hidden rows. In chrome.rs: view_top = hidden − min(scroll, hidden) and View.scroll = scroll − min(scroll, hidden); mouse.rs pane_wheel clamp = history + hidden
TERM-068 | DONE | chrome.rs draw_header badge "⇲ {cols}×{rows} remote" in muted (DD-024)
TERM-069 | DONE | shared bridge_command (App::visible)
TERM-070 | DONE | shared clean_title on vt100's OSC title (the window's title follows it)
TERM-071 | DONE | shared Term::poll
TERM-072 | DONE | shared Term::poll / tick_every
TERM-073 | DONE | shared reap in after_events
TERM-074 | DONE | shared early_failure (vt100 rows)
TERM-075 | DONE | chrome.rs draw_terminal shows the error and "enter or esc closes this pane"; shared Term::key closes it
TERM-076 | DONE | shared kill / Drop
TERM-077 | DONE | shared Term::queued; chrome.rs draw_terminal returns before ensure_started
TERM-078 | DONE | chrome.rs draw_queued: spinner, "Resuming {title}", "{who} · starting now / up next / in line · N ahead", the sentence
TERM-079 | DONE | shared launched()
KEY-001 | DONE | shared keymap.rs (now 63 actions with Media, Act::all() 80)
KEY-002 | DONE | shared Keymap
KEY-003 | FIXED | gui/input.rs key(): named keys, Space, F1–F24, Tab+shift → BackTab, AltGr, repeats; fixed: ctrl / alt chords with no layout character (Unidentified, Dead) use the key's own character, and Super (Windows key, Cmd) chords no longer type their plain key into sessions
KEY-004 | DONE | shared run_act (alt+n → Char('n')+ALT); checked in the window: launcher opens
KEY-005 | DONE | shared quick_agent (alt+q, prefix Q)
KEY-006 | DONE | shared open_shell (alt+t, prefix !)
KEY-007 | DONE | shared; Place::Split reads App.outer, which the window keeps in terminal-sized cells (alt+enter = Named Enter+ALT, alt+\ = Char('\\')+ALT)
KEY-008 | DONE | shared (prefix % | \)
KEY-009 | DONE | shared (prefix " -)
KEY-010 | DONE | shared ask_close (ctrl+w, alt+w, prefix x)
KEY-011 | DONE | shared zoom (alt+z, prefix z); checked in the window
KEY-012 | DONE | shared toggle_priority (ctrl+up, alt+=; gui_key only takes ctrl+= / ctrl+-); checked in the window: ctrl+up grows, a click on another tile snaps it back
KEY-013 | DONE | shared (ctrl+down, alt+-)
KEY-014 | DONE | shared toggle_tiles; alt+shift+t arrives as Char('T')+ALT|SHIFT and normalises to the binding; checked in the window
KEY-015 | DONE | shared flip_pane (alt+i, prefix d)
KEY-016 | DONE | shared flip_all (alt+I, prefix D)
KEY-017 | DONE | shared move_focus over App.outer (alt+arrows, prefix h l k j / arrows)
KEY-018 | DONE | shared resize (alt+shift+arrows, prefix H L K J)
KEY-019 | DONE | shared Jump (alt+1..9); checked in the window (alt+3 focused the shell)
KEY-020 | DONE | shared cycle_session (alt+j / alt+k; ctrl+tab = Tab+CONTROL, ctrl+shift+tab = BackTab+CONTROL|SHIFT)
KEY-021 | DONE | shared cycle_project (alt+J / alt+K)
KEY-022 | DONE | shared (prefix ) ()
KEY-023 | DONE | shared show_tab(All) (alt+0, prefix 0)
KEY-024 | DONE | shared show_tab (no defaults)
KEY-025 | DONE | shared LastTab (alt+l, prefix tab)
KEY-026 | DONE | shared ToggleProjectTabs + toast
KEY-027 | DONE | shared attention_target (alt+a, prefix a)
KEY-028 | DONE | shared; draw_all reads App.sidebar (alt+s)
KEY-029 | DONE | shared; draw_all sets App.side_area / App.strip, so the squeezed / strip branches work (alt+b); checked in the window
KEY-030 | DONE | shared open_palette (alt+p, prefix : space); the palette draws natively
KEY-031 | DONE | shared open_view("usage") + refresh (alt+u)
KEY-032 | DONE | shared (alt+o)
KEY-033 | DONE | shared (alt+y)
KEY-034 | DONE | shared (alt+g)
KEY-035 | DONE | shared (prefix S)
KEY-036 | DONE | shared (alt+,)
KEY-037 | DONE | shared (prefix I)
KEY-038 | DONE | shared (prefix e)
KEY-039 | DONE | shared Help overlay (F1, prefix ?); checked in the window
KEY-040 | DONE | shared start_rename; gui/sidebar.rs draws the inline field
KEY-041 | DONE | shared open_palette("theme ")
KEY-042 | DONE | shared toggle + toast; the window always has its bundled symbol font, so only surfaces it paints through the terminal UI's drawing change
KEY-043 | DONE | shared set_trim + toast
KEY-044 | DONE | shared + toast
KEY-045 | DONE | shared set_caveman + toast
KEY-046 | DONE | shared refresh_usage + toast "refreshing usage…"
KEY-047 | DONE | shared toggle_usage_details (alt+U)
KEY-048 | DONE | shared open_continue (alt+r, prefix R)
KEY-049 | DONE | shared open_orchestrator (alt+O)
KEY-050 | DONE | shared open_switch (alt+L, prefix P)
KEY-051 | DONE | shared close_project (prefix X)
KEY-052 | DONE | shared (ctrl+shift+o = Char('O')+CONTROL|SHIFT, normalised)
KEY-053 | DONE | shared show_last_opened (prefix f)
KEY-054 | DONE | shared show_agent_files (prefix F)
KEY-055 | DONE | shared open_watchdog (prefix w)
KEY-056 | DONE | shared open_watchdog_prompt
KEY-057 | DONE | shared Quit (confirm while terminals run); about_to_wait exits on App.quit, the window's close button runs the same act
KEY-058 | DONE | shared Keymap::new
KEY-059 | DONE | shared App::key prefix handling and its toast; gui/toasts.rs shows the armed hint
KEY-060 | DONE | shared App::paste; IME text and drops go through it too while armed
KEY-061 | DONE | shared Chord::new; Super chords never reach the workspace (gui/input.rs)
KEY-062 | DONE | shared Chord::parse
KEY-063 | DONE | shared Chord::parse
KEY-064 | DONE | shared Chord::display
KEY-065 | DONE | shared Keymap::new
KEY-066 | DONE | shared Keymap::new
KEY-067 | DONE | shared Keymap::new
KEY-068 | DONE | shared Keymap::new; App::new toasts them
KEY-069 | FIXED | App::key clears clipboard_request (and the terminal UI's own sel / link_press); fixed: the window's selection and link press weren't cleared, gui/mod.rs Gui::key now clears both on every key
KEY-070 | DONE | shared App::key (ctrl+v, ctrl+shift+v, shift+insert arrive as such from gui/input.rs)
KEY-071 | DONE | shared App::key; only the window's own keys (font size, copy, F11, browser back/forward in gui_key) come first
KEY-072 | DONE | shared rename_key
KEY-073 | DONE | shared alt+v grab; gui/mod.rs app_event also adds the picture to the media gallery
KEY-074 | DONE | shared App::key
KEY-075 | DONE | shared App::key
KEY-076 | DONE | shared App::key
KEY-077 | DONE | shared App::key
KEY-078 | DONE | shared App::key
KEY-079 | DONE | shared welcome_key
KEY-080 | DONE | shared sidebar_key
KEY-081 | DONE | shared sidebar_key (side_nav)
KEY-082 | DONE | shared sidebar_key
KEY-083 | DONE | shared sidebar_key
KEY-084 | DONE | shared sidebar_key
KEY-085 | DONE | shared clipboard_request token
KEY-086 | DONE | shared clip::grab_text
KEY-087 | DONE | shared App::paste
KEY-088 | DONE | shared clip::grab_image
KEY-089 | DONE | shared App::handle(Clipboard) toast; nothing image-like hands the program its alt+v
KEY-090 | DONE | gui/input.rs: Tab with shift → KeyCode::BackTab
KEY-091 | DONE | the window always sends ctrl+space as Char(' ')+CONTROL (Named Space), which the keymap and continue_picker accept
KEY-092 | DONE | alt+p → Char('p')+ALT, alt+shift+p → Char('P')+ALT|SHIFT, alt+o → Char('o') with exactly ALT
KEY-093 | DONE | gui/mod.rs returns before routing for modifier-only presses (input::modifier_key), and input::key has no mapping for them
