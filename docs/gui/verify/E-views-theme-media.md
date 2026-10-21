# E: VIEW, THM, MED

Reviewer E's verdicts for `VIEW` (settings, usage, stats, profiles, proxy, bridge, quick install, setup), `THM`
(theming) and `MED` (the media sidebar, new for the window).

How the window reaches them (checked in code):
- `app/gui/views.rs` hands each view pane to its own `views/<name>/gui.rs` (`draw`, `click`, `wheel`) with the
  view's own state.
- Keys reach each view's own `key()` through `App::handle`, as in the terminal.
- The first-draw work the terminal UI did in `render` (stats scan, quick install and setup detection, the settings
  JustTools/JustImagine checks) runs from the view's `gui::draw`.
- Each pane's `tick_every` still feeds `App::next_deadline`, so the views keep polling.
- Theme colours become `kit::Tokens` (`kit/mod.rs Tokens::from_theme`) and the terminal palette (`term/draw.rs
  Palette`). Backdrops are drawn in `app/gui/backdrop.rs`, and the skin's screen effects run as a renderer post-pass
  (`app/gui/mod.rs`).
- The media panel is `app/gui/media/**` on `crates/bro-media`.

Demo checks (`--demo`, scripted) are under `F:/tmp/verify-e/{a,b,c,d}/`:
- every view and settings section, with their keys and clicks;
- twelve themes, a custom theme file edited live (in a temp `BRO_DIR`), the rain setting and the font switch;
- for the media panel, real files dropped on the window: png, gif, mp4, mp3, plus made-up hostile files (a huge-claim
  PNG, a GIF bomb, an svgz bomb, an SVG pointing at local files, a fake .exe).

Fixed in this pass:
- THM-014: `kit/mod.rs` `Tokens::ink_on` now picks the higher WCAG contrast, like `ui::ink_on`. Danger and gold buttons
  with light fills now get dark text.
- VIEW-114: `views/proxy/gui.rs`: live log rows fold onto two lines under 980 px instead of dropping columns.
- VIEW-127: `views/quick_install/gui.rs`: the JustTools "✗ error" / "✓ done" note stays visible while a skill download
  runs.
- MED-046 (`crates/bro-media`, `app/gui/media/loader.rs`):
  - a 1 GiB decode budget for every image decoder; animations stop decoding after 8 s;
  - per-job time limits, and a decoder that panics no longer kills a worker;
  - SVG/svgz files are capped at 2M XML nodes.
- MED-013: SVGs no longer read the local files they point at.
- MED-011: an ffmpeg fallback for AVIF, HEIC and JPEG XL.
- MED-010: loads carry a request number, so stale results are dropped and their textures freed.
- MED-001, MED-007, MED-033, MED-035: gallery entries now cover the following:
  - unwatched `bro open`;
  - moved files and files rewritten on disk;
  - dedup across path spellings;
  - removals stay removed.

Findings outside these files (details in the lines):
- MED-043, a security risk: with no editor configured, `e` on a previewed `.bat`/`.cmd`/`.js`/`.vbs`/`.hta` runs it.
  `app/files.rs` `open_in_editor` `Plan::System` calls `panes::links::open` without the window's executable check.
- Backdrops (THM-031, THM-037): stars, grid, hatch and rain are glyphs on the window's base layer, so they show
  through the cards and over text. With rain set to "working sessions", rain also falls in resting sessions.
- Theme hot reload (THM-020): `app/gui/mod.rs theme_key` leaves out shine, inline, good, danger and user, so editing
  only those keys doesn't recolour the window.
- Skins (THM-022…029): title brackets, upper case, full-width titles and inverted title tabs aren't drawn. This needs a
  decision against DESIGN.md's "no ALL-CAPS" rule.
- The media panel (MED-039, MED-041, MED-009/016/017):
  - alt+m never gives the panel the keyboard;
  - a watched `bro open` opens the file preview overlay for a moment;
  - clicked PDF links and tiff/heic/avi links get the file preview's binary card instead of the panel.
- Demo mode and real data: opening stats with `--demo` scans the real `~/.claude` / `~/.codex` logs and writes
  `~/.bro/token-scan.json`, as the terminal UI's demo does. The fix belongs in `views/stats.rs` or `services/demo.rs`.
- VIEW-076 needs a product call: which v2.toml-only settings the window should expose.


## VIEW

VIEW-001 | DONE | views/mod.rs open() makes all eight; pane header icons via app/gui/chrome.rs view_icon (gauge, account, proxy, bridge, stats, settings, rocket)
VIEW-002 | DONE | shared App::open_view (app/commands.rs) reached through App::handle; checked: alt+o on the focused profiles view goes back to the usage tab
VIEW-003 | DONE | shared App::apply Action::Open (app/mod.rs); app/gui/views.rs with_view applies the views' actions through it
VIEW-004 | DONE | shared App::open_view calls svc.refresh_data / svc.refresh_usage
VIEW-005 | DONE | every view gui.rs draws its hint row at the bottom with no hit (usage::gui::hints, profiles::gui::hints, settings gui); usage drops "click account · new session" for the card's hover "+ session" chip
VIEW-006 | DONE | stats::gui::draw starts the scan and takes a finished report; quick_install::gui::draw / setup::gui::draw run check() once (setup also sets rows); settings::gui::prepare; profiles/QI/setup also poll in draw; tick_every polls run from Gui::about_to_wait (Event::Tick) and wakers (Event::Wake)
VIEW-007 | DONE | step()/reset()/BroOpt::change() reached from settings/gui.rs click()/pick() and keys call shared svc.save_settings (services/mod.rs:362): State update, write on a thread, no-op in demo, error toast
VIEW-008 | DONE | settings/gui.rs draw(): same hints as keycaps along the bottom, + "r reset to preset" (Preset/Knob) / "r bro's default" (Watch); not clickable; left out when the pane is under 380 px tall
VIEW-009 | DONE | settings/gui.rs nav(): from 640 px (~76 cols): "Settings" heading (sentence case), icon + label + summary per section, selection fill, "saved to ~/.bro/v2.toml" at the bottom when there's room; no divider line (WinUI nav pane, DD-001)
VIEW-010 | DONE | settings/gui.rs draw()/lay_out(): under 640 px no list, "N of 7" beside the title; segments fall back to steppers (checked in a split)
VIEW-011 | DONE | lay_out()/lay_item()/item(): page up to 780 px; title, blurb, a card per setting: name, tag ("changed" shine / "+ N changes" muted), control right, wrapped help, notes as text or step segments; the rule is spacing (WinUI)
VIEW-012 | DONE | draw(): first row of a section scrolls to the top, the last to the end (Claude launch preview revealed), else kept in view; ▲/▼ become edge fades + scrollbar
VIEW-013 | DONE | control()/ctl(): switch with On/Off (an inert "on" is a grey switch, gui.rs switch()), ‹ value › = stepper with chevrons or segmented choices, Enter-only rows = link button with chevron
VIEW-014 | DONE | nav() draws SettingsView::summary(): theme, "N of 4 on", level/off, "N idle", preset (+N), spinner/"updating"/"update", spinner/"→ x.y.z"/version; spinner frames become ui.spinner (test running_jobs_show_spinners_and_steps)
VIEW-015 | DONE | keys reach SettingsView::key through App::handle unchanged (app/gui/input.rs maps shift+Tab to BackTab); j/k/g/tab/l/h/enter/r checked in the demo
VIEW-016 | DELTA | settings/gui.rs wheel(): the wheel scrolls the page and the selection follows it into view (standard window scrolling) instead of moving the selection; nav click → go(), card click selects, click on the selected card steps +1 (all checked)
VIEW-017 | DONE | prepare() (from draw) sets tools_busy/update_busy and follows imagine_update; Pane::tick_every (100 ms / 3 s) feeds App::next_deadline and Event::Tick redraws (app/gui/mod.rs tick(), app_event())
VIEW-018 | DONE | prepare(): check_justtools() + check_justimagine(true) on the first draw; upstream Caveman check in SettingsView::new() (views::open)
VIEW-019 | DONE | GUI lays out the shared rows(); Font row always present (fonts::GUI makes available() true), JustWatch skill and Smart Paste rows on Windows
VIEW-020 | DONE | Theme link control: click/enter/←/→ → BroOpt::change → Act::Themes; palette opens on "theme " with live preview (checked)
VIEW-021 | DONE | Font stepper in the terminal face: 19 bundled fonts, no "terminal's own" in a window (DD-011); ←/→ wraps, applied live (app/gui/mod.rs:521 reloads settings.font); font blurb as help (checked: wrap to Press Start 2P, Fira Code)
VIEW-022 | DONE | Digital rain segmented off / everywhere / working sessions (pick()); keys wrap via shared change()
VIEW-023 | DONE | Summary screensaver stepper; shared change(): off/1/2/5/10/15/30 min, wraps, off-list snaps up
VIEW-024 | DONE | Shared tile borders switch → shared change()
VIEW-025 | DONE | Project tabs switch → shared change()
VIEW-026 | DONE | Nerd Font icons switch → Act::ToggleIcons + toast "plain icons (no nerd font)" (checked); the window's own icons always draw from the bundled Symbols font, so the switch only changes ui::icon text (see THM-039)
VIEW-027 | DONE | Auto-resume switch → Act::ToggleAutoResume + toast (checked)
VIEW-028 | DONE | Auto-approve prompts switch → shared change()
VIEW-029 | DONE | Recaps switch → shared change()
VIEW-030 | DONE | Request compression switch → Act::ToggleTrim → svc.set_trim + toast "request compression on (new sessions go through bro-proxy)" (checked)
VIEW-031 | DONE | Caveman mode segmented off/caveman/ultracave/megacave, help per level from blocks(), upstream-plugin note drawn as a text note
VIEW-032 | DONE | Claude subagents too: grey "On" switch while caveman is off (gui.rs switch(), live=false; checked)
VIEW-033 | DONE | Watchdog prompt link "bro's default ›"/"customized ›" → Act::WatchdogPrompt (editor opens, checked); ↺ button / r → None (resettable())
VIEW-034 | DONE | Wake after stepper → shared step_choice (20…1800 s, clamps); ↺ / r → 60 (checked)
VIEW-035 | DONE | Reviewer stepper with the login's logo (login_mark()); shared reviewer_choices() ("auto", signed-in Claude/Codex logins, current), wraps; ↺ / r → auto (checked: auto → claude:local)
VIEW-036 | DONE | Reviewer model segmented opus/fable/sonnet/haiku beside Claude's mark; keys wrap; ↺ / r → opus (checked)
VIEW-037 | DONE | quirk kept: shared step() sends an unknown model to opus either way; the GUI selects no segment for it (option() None, tested) and a segment click lands on the pick
VIEW-038 | DONE | Wake-up budget stepper → shared step_choice (10…250, clamps); ↺ / r → 25
VIEW-039 | DONE | Preset stepper Standard…Marathon (wraps); use_preset clears overrides; tag "+ 1 change"; ↺ / r clears overrides; nav summary "Quiet +1" (checked)
VIEW-040 | DELTA | the knobs are segmented choices under sentence-case headings Privacy / Updates / Long runs / Features (DESIGN: no caps); keys wrap; picking the preset's value drops the override (shared set_choice); r removes it. An override shows the "changed" tag and ↺. The terminal UI's colour code (override bold shine, preset inline, default muted) is replaced by the selected segment, so a value set by the preset looks the same as Claude's default (settings/gui.rs control())
VIEW-041 | DONE | knob error_reporting "Error reports": Claude default / off segments (KNOBS via control())
VIEW-042 | DONE | knob feedback "Feedback prompts": Claude default / off
VIEW-043 | DONE | knob fingerprint "Prompt fingerprint": Claude default / off
VIEW-044 | DONE | knob telemetry "Usage telemetry": Claude default / off
VIEW-045 | DONE | knob network "Nonessential traffic": Claude default / off
VIEW-046 | DONE | knob connectors "claude.ai connectors": Claude default / off
VIEW-047 | DONE | knob secrets "Secrets in tool shells": Claude default / scrubbed
VIEW-048 | DONE | knob mcp_env "MCP server env": Claude default / minimal
VIEW-049 | DONE | knob history "Transcripts & history": Claude default / off
VIEW-050 | DONE | knob memory "Auto memory": Claude default / off
VIEW-051 | DONE | knob screen_share "Screen-share safe": Claude default / on
VIEW-052 | DONE | knob updates "Claude updates": Claude default / manual / frozen
VIEW-053 | DONE | knob retry "Overload errors": Claude default / ride out
VIEW-054 | DONE | knob shell_timeout "Shell command time limit": Claude default / long
VIEW-055 | DONE | knob cache_ttl "Prompt cache lifetime": Claude default / 1 hour / 5 minutes
VIEW-056 | DONE | knob compact "Auto-compact": Claude default / early (70%) / off
VIEW-057 | DONE | knob tool_search "Tool search via proxy": auto / Claude default (proxy-only effect is bro-core launch logic, shared)
VIEW-058 | DONE | knob agent_teams "Agent teams": Claude default / on
VIEW-059 | DONE | knob focus "Focus view": on / Claude default; --settings viewMode shows in the launch preview
VIEW-060 | DONE | knob spinner_tips "Spinner tips": off / Claude default; spinnerTipsEnabled shows in the launch preview
VIEW-061 | DONE | lay_out() flush(): "Next Claude launch sets" heading, each K=V then --settings {json} in a read-only mono well, muted "more variables: [claude_env.extra] in ~/.bro/v2.toml" (checked)
VIEW-062 | DONE | Status control from integrations(): spinner + installing/updating, checking…, not installed, "v → new" (shine), offline, up to date (tests running_jobs_show_spinners_and_steps, a_skill_download_spins_on_its_own_row)
VIEW-063 | DONE | notes: progress → Note::Progress (spinner, "N of 5 · step · Ns", one segment per step, tested), ✗ error (danger), ✓ done (good), PATH note, GitHub/SHA-256 note (checked)
VIEW-064 | DONE | button() mirrors SettingsView::tool(): Install / Update / Check / Try again, none while a job runs or a look is under way; Install opens Quick Install (checked)
VIEW-065 | DONE | Quick install link "open ›" → Act::QuickInstall
VIEW-066 | DONE | JustTools skill switch; "⠋ adding" → spinner + "adding" (tested); off/refusal toasts from shared tool()
VIEW-067 | DONE | Auto-update JustTools switch → shared tool()
VIEW-068 | DONE | JustWatch skill switch (Windows) → shared tool(), toast on off
VIEW-069 | DONE | Smart Paste: switching spinner (tested), checking…, switch, needs JustTools / update JustTools / not in this JustTools; notes as text
VIEW-070 | DONE | click/enter → shared tool(SmartPaste); button() offers Install / Update only where enter does that
VIEW-071 | DONE | JustImagine status values and notes from integrations() (muted URL note, danger service note)
VIEW-072 | DONE | button() Install / Update / Open / Check / Try again → imagine(); the update tab is followed every 3 s by prepare()
VIEW-073 | DONE | bro version status: spinner "updating" + 3-step progress (tested), "x installed", checking…, no prebuilt, "cur → new", offline, newest
VIEW-074 | DONE | button Install / Check / Try again → shared step(): install_bro_update or check_bro_update(false)
VIEW-075 | DONE | Check for updates switch → shared step(): turning on runs check_bro_update(true)
VIEW-076 | MISSING | no decision recorded on which v2.toml-only settings the window exposes; the GUI shows the terminal's rows only. Needs a product call; new rows go in views/settings.rs rows()/blocks() (shared, not this reviewer's file), settings/gui.rs control() then draws them
VIEW-077 | DONE | shared StatsView::start (thread, dedup by login dir, counters, drops empty logins) called from stats::gui::draw once profiles are ready
VIEW-078 | DONE | stats::gui::loading: "tallying every token", meter, "X of Y" / "finding logs…", "only new logs next time"; "waiting for your logins…"; `shown` kept during a rescan (screenshot)
VIEW-079 | DONE | stats::gui::draw: segmented tokens/profiles (clickable, checked), spinner + status() text, plus a rescan icon button
VIEW-080 | DONE | stats::gui::draw_tokens headline: lifetime (≥ floor), last 30 days, each provider's % with its logo
VIEW-081 | DONE | draw_tokens columns login / lifetime / last 30d / share segments + % / sparkline with "Mon d … Mon d" axis; px thresholds hide share and sparkline when narrow
VIEW-082 | DONE | draw_tokens: group per harness (logo + label, DD-022), subtotal when >1 login, rule, total, ≥ footnote
VIEW-083 | DONE | shared token_figures, used by stats::gui::tokens_tab
VIEW-084 | DONE | stats::gui::draw_profiles headline 24h/7d/30d, per-login figures (· for zero), mix_bar in the TUI colours (screenshot)
VIEW-085 | DONE | draw_profiles total row, legend, "X% … re-read from cache · Y% was written by the model"; Body::NoLogins copy verbatim
VIEW-086 | DONE | super::compact used for every figure
VIEW-087 | DONE | keys via StatsView::key (tab toggle checked); stats::gui::click (chips, rescan) and wheel (2 steps, clamped in scrolled()); 250 ms tick_every + ui.ticking while scanning
VIEW-088 | DONE | usage::gui::draw title "Usage" + "{ago} · every 60s"; "profiles {why}" while loading
VIEW-089 | DONE | usage::gui::draw_card: logo (DD-022), id, plan · email, large/small task pills, spinner while fetching; "+ session" shows on hover of signed-in cards (checked)
VIEW-090 | DONE | usage::gui::meter: segments (DD-025), pct_color thresholds, "resets in <countdown>", "—" when missing
VIEW-091 | DONE | headroom_spans (now ≈X%, week Y%), "capped by the weekly ratio", up to 2 scoped windows as meters, "last refresh failed: …" in danger
VIEW-092 | DONE | why_not() and empty() copy verbatim
VIEW-093 | DONE | usage::gui::click CARD → pane Action::LaunchUsage (checked: launched a session on claude:work)
VIEW-094 | DONE | r / j / k via UsageView::key; Refresh button toasts "refreshing usage…"; j/k step one card row of the grid, clamped in draw
VIEW-095 | DONE | usage::gui::wheel clamps and draw clamps to the last row that fills the view (DD-013; checked, no blank)
VIEW-096 | DONE | profiles::gui::draw polls a pending save; "Saving profile…" + spinner in the dialog; keys (ProfilesView::key) and clicks swallowed while pending; 50 ms tick_every
VIEW-097 | DONE | "Enlarge this pane to manage profiles" under 240×220 px; title "Profiles" + "Claude and Codex logins"
VIEW-098 | DONE | key buttons + Claude (a), + Codex (c), Log in (l), Remove (d, hidden for machine logins), plus Reload (r)
VIEW-099 | DONE | profile_row: logo, id, "machine login" tag, check / outline + email / Logged in / Not logged in; selection fill (DD-001)
VIEW-100 | DONE | footer: folder icon, short_path of the dir, the machine-login / remove note verbatim
VIEW-101 | DONE | add_card: "Claude|Codex profile name:", text field with caret, error / missing-CLI / rule text (checked)
VIEW-102 | DONE | add_card buttons Create + log in (enter) / Cancel (esc); validation, 40-byte cap, ctrl chords and paste through shared ProfilesView::key / paste (App::paste)
VIEW-103 | DONE | shared ProfilesView::poll (selects, toasts, opens the login)
VIEW-104 | DONE | shared open_login (tab label "login · <id>", install tab when the CLI is missing), reached by l / Enter / Log in button
VIEW-105 | DONE | shared svc.watch_login; App::close calls login_closed for "login · " tabs
VIEW-106 | DONE | remove_card: "Remove X?", "Deletes login, settings and saved sessions.", full dir, Remove (y) / Cancel (esc); "kept it" toast (checked)
VIEW-107 | DONE | shared ProfilesView::key / poll toasts and the machine-login refusal
VIEW-108 | DONE | keys via ProfilesView::key; profiles::gui::wheel (ScrollUp/Down in List mode), click sends the buttons' keys, row click selects
VIEW-109 | DONE | shared ProfilesView::adding; drawn as the add dialog
VIEW-110 | DONE | proxy::gui::header: "Proxy", check + :port, base URL, "N routes · M requests seen"; alert + why when not ready (DD-021)
VIEW-111 | DONE | proxy::gui::compression card: switch with On/Off, level pill, "~X tokens saved in N requests (estimate) · …" or the off copy, verbatim
VIEW-112 | DONE | route_row (≤6): id (shine), label, upstream logo + upstream_label, default model or "client's model"; empty copy verbatim
VIEW-113 | DONE | pool_row (≤6): check / clock / cross icons, name, ready / cooling <countdown> / unavailable, 5h req · in · out · $, last error
VIEW-114 | FIXED | proxy/gui.rs draw: live-log rows now fold onto two lines below 980 px (new LOG_NARROW) instead of dropping the cache and saved columns between 820 and 980 px; Cols::new always has in/out/cache/saved (checked at ~920 px and full width)
VIEW-115 | DONE | c / j / k / g via ProxyView::key; switch (no toast) and "newest" button in proxy::gui::click; wheel 3 rows; services EVENT_CAP 400
VIEW-116 | DONE | bridge::gui::header: running on :port + clients (shine when >0), stopped + error; off() card: "off — t turns it on" or why, spinner while starting
VIEW-117 | DONE | blurb verbatim
VIEW-118 | DONE | url_row: this machine / tailscale / lan with icons, selection fill (DD-001), show_url masking
VIEW-119 | DONE | token field masked / revealed with a Reveal / Hide token button (r), status error below (checked)
VIEW-120 | DONE | bridge::gui::modules decodes state.bridge.qr (pairing_qr = the QR of pairing_url) losslessly and draw_qr draws native modules + "scan to pair" (DD-008); new test pairing_qr_rows_decode_to_the_same_modules
VIEW-121 | DONE | r / t / j / k / c / Enter via BridgeView::key; Turn on/off button toasts like t (checked "bridge off")
VIEW-122 | DONE | bridge::gui::click: power, reveal, URL select, copy button, double-click copies (DD-013; checked)
VIEW-123 | DONE | services/workers.rs spawn_bridge status loop every 2 s
VIEW-124 | DONE | quick_install::gui::draw runs check() once; pick_defaults ticks the recommended missing tools (screenshot)
VIEW-125 | DONE | title "Quick install", OS icon + describe() or spinner + "looking at this machine…"; sentence-case group headings
VIEW-126 | DONE | rows() from bro_core Tool::here() (Windows-only tools there)
VIEW-127 | FIXED | tool_row: spinner / check / checkbox, mark, name (JustTools semibold shine), ★, detail (TUI logic), blurb; quick_install/gui.rs draw: the JustTools "✗ error" / "✓ done" note now also shows while a skill download runs (the TUI's filter)
VIEW-128 | DONE | install bar: Install N (i) + names (6, then "N more") or "nothing ticked to install", both notes, "<Tool> · how: …"
VIEW-129 | DONE | shared install / poll (JustTools, JustPaste, tab, watch, JustWatch skill, JustImagine check); draw also polls while watching
VIEW-130 | DONE | shared toggle / install toasts (demo toast checked)
VIEW-131 | DONE | keys via QuickInstallView::key; wheel through mouse(); row click selects + activates; a / n / r buttons; busy set in draw drives the 100 ms tick, 3 s while watching
VIEW-132 | DONE | shared App::offer_setup on Event::Services (never in demo, so code-checked only)
VIEW-133 | DONE | setup::gui::draw "Set up bro" + status icon and words (ready / start here / next: sign in)
VIEW-134 | DONE | step card 1: agent_row (looking…, ✓ + path, installing in another tab…, not installed + Install button), note, quick install footnote
VIEW-135 | DONE | step card 2: explanation, login_row (≤6: check / outline, email or what to do, machine login), "N more in profiles", Add buttons, note
VIEW-136 | DONE | step card 3: "Launch an agent" (primary), "Done · don't open this at startup again"
VIEW-137 | DONE | build_rows in TUI order + SetupView::activate via click / Enter (Launch opens the launcher, checked)
VIEW-138 | DONE | shared check / poll (3 s for 15 min, toasts); draw also polls while watching
VIEW-139 | DONE | keys via SetupView::key over v.rows set in draw (End scrolls to Done, checked); wheel through mouse(); row click selects + activates
VIEW-140 | DONE | labels, help, notes, toasts and hints come straight from blocks()/summary()/Section::blurb()/BroOpt::help() and knob help; window-only words: On/Off, button labels Install/Update/Check/Try again/Open, sentence-case headings (DESIGN)

## THM

THM-001 | PARTIAL | kit/mod.rs Tokens::from_theme reads bg, fg, accent, shine, inline, good, danger, ansi; skin via backdrop.rs/mod.rs effects; animated via welcome.rs. frame, muted and user are never read (muted/faint/hairline/border/selected are derived from text/bg/accent), so those three theme fields don't reach the window
THM-002 | DONE | is_light → Tokens.light; painted → term/draw.rs Palette.painted; swatch → overlays/palette.rs swatch(); deep() is mirrored by Tokens::ink_on's dark ink
THM-003 | DONE | kit/mod.rs Tokens::from_theme: Reset bg/fg → #0e1015/#e4e6eb (light: #f4f5f7/#1f2328) plus DARK_ANSI/LIGHT_ANSI; the terminal theme's named colours map through kit::color (values differ from the #0c0c0c/#d8d8d8 snapshots on purpose); checked with theme terminal
THM-004 | DONE | kit::color + kit::indexed (Reset → default, 16 named → palette, 6×6×6 cube, grey ramp, RGB); terminal cells through term/draw.rs Palette::resolve/named
THM-005 | DONE | palette "theme" lists theme::names(): 30 in picker order (screenshot c/picker1.png)
THM-006 | DONE | theme.rs builtin() (shared); accent-only themes take the window's default bg/fg (Tokens::from_theme); only ultra has animated
THM-007 | PARTIAL | painted bg/fg/ansi reach the window (Tokens::from_theme, term Palette, checked on several themes); the rainbow is drawn nowhere: welcome.rs only fades the wordmark dot accent→shine on animated themes (theme::rainbow_at unused)
THM-008 | DONE | is_light → Tokens.light (light fills, strokes, title bar via window::style_title_bar); paper and catppuccin-latte screenshots
THM-009 | DONE | Tokens.gold = theme::gold (rgb 255,200,61 dark / 178,128,0 light / accent on mono / LightYellow on terminal); used for the orchestrator card and launcher ★; other project colours aren't drawn (DD-021)
THM-010 | DELTA | DD-021: projects are folder icons with no colour; Tokens::project exists but nothing draws project colours
THM-011 | DELTA | DD-021, as THM-010
THM-012 | DELTA | DD-021/DD-002: no project-tinted bands; the focused header gets a neutral tint
THM-013 | DELTA | DD-001/DD-020: hover and selection are the t.hover / t.selected fills, not current_tint
THM-014 | FIXED | kit/mod.rs Tokens::ink_on picked white below luminance 0.42, so mid colours like graphite's danger #ef7b7b got white text (2.7:1). It now picks whichever of the theme's dark ink and white contrasts more (WCAG), like ui::ink_on. Test kit::tests::ink_on_picks_the_higher_contrast_like_the_terminal_ui; checked on the profiles Remove button
THM-015 | DONE | light Tokens (fills toward white, darker strokes), ink_on (fixed), dark gold, logos in their own colour or t.text (kit/brand.rs Mark::color); project lightness n/a (DD-021); paper and latte screenshots
THM-016 | DONE | logos are drawn every frame from the current Tokens (Gui::theme_changed rebuilds them; icons::retint is also called); the OpenAI mark turns dark on paper
THM-017 | DONE | shared theme.rs; a custom file in a temp BRO_DIR showed in the picker tagged "yours" and applied
THM-018 | PARTIAL | every key is parsed (shared theme.rs) but the window ignores muted, frame and user (Tokens::from_theme derives them), and borders has nothing to act on (DD-020); base, the colour keys that reach Tokens, rainbow, ansi and skin (backdrop/fx) work
THM-019 | DONE | shared theme::color; named colours → the ANSI palette via kit::color; terminal/Reset → the window default
THM-020 | PARTIAL | watcher → ThemeFilesChanged → reload + first-problem toast works, and a parse error keeps the last good theme (checked with an atomic save). But app/gui/mod.rs theme_key covers only name/bg/fg/accent/frame/muted/ansi, so editing shine, inline, good, danger or user alone doesn't rebuild Tokens: checked, a danger edit toasted but the colour stayed. Fix: add those fields to theme_key. Shared quirk: a truncate-then-write save parses the empty file as plain graphite and stores that as LAST_GOOD (theme.rs custom())
THM-021 | DONE | overlays/palette.rs: light/dark/yours/current tags and a swatch on the theme's bg; Enter toasts "theme: X"; Esc reverts the live preview (screenshots c/picker*.png, c/esc-*.png)
THM-022 | PARTIAL | the window reads skin.backdrop and skin.fx; border sets give way to WinUI cards (DD-020/DD-002); brackets, case (Upper/Wide) and invert_titles aren't applied anywhere (chrome.rs draw_header, view headers); the marker gives way to the DD-002 focus border
THM-023 | DELTA | DD-020/DD-002/DD-001: 8px cards with an accent border on focus replace the rounded/thick border sets and the ▶ marker
THM-024 | PARTIAL | phosphor effects work: monochrome, 2px scanlines and vignette over everything, toasts included (c/th-phosphor.png); "[ ]" UPPER titles not drawn (borders/marker: DD-020/DD-002)
THM-025 | PARTIAL | outrun: Stars and the #0a001c→#3a0a4a gradient work (c/th-outrun.png); "◢ ◣" title brackets not drawn
THM-026 | PARTIAL | blueprint: Grid works (c/th-blueprint.png); "┤ ├" UPPER titles not drawn (◇ marker: DD-002)
THM-027 | PARTIAL | cyber: Hatch works (c/th-cyber.png); "▐▌" UPPER inverted title tab not drawn (◆ marker: DD-002)
THM-028 | PARTIAL | digital-rain: Rain works (c/th-digital-rain.png); "[ ]" title brackets not drawn (» marker: DD-002)
THM-029 | PARTIAL | vaporwave: Stars and the #2a1048→#0c2d3d gradient work (c/th-vaporwave.png); fullwidth titles not drawn (✦ marker: DD-002)
THM-030 | DONE | nebula: default borders and titles, Stars plus vignette (c/th-nebula.png); the ✦ marker gives way to the DD-002 focus border
THM-031 | PARTIAL | app/gui/backdrop.rs draws the backdrop as glyphs on the window's base layer. bro-gfx draws a layer's glyphs after its quads, so stars, grid, hatch and rain show through the opaque cards and land on or beside text: blueprint "+" on "gpt-5.2-codex" and "~\code\terminal", rain over terminal text and settings rows (c/z-blueprint-side.png, c/th-digital-rain.png). The terminal UI's rule (only runs of 6 or more blank cells, 2-cell margins) isn't applied. Fix: in app/gui/mod.rs draw_all, push ui.layer() right after backdrop::draw so the content layer and cards cover it. To keep backdrops in the empty space inside panes, draw them per card over blank terminal cells only (chrome.rs draw_terminal knows the grid), with the 6-blank/2-cell rule
THM-032 | DONE | backdrop.rs Stars mirrors skin.rs (1 in 89 cells, ✦/+/·, sine twinkle, shine/accent/text tints). Note: it sets ui.ticking, so the window redraws at 60 fps while stars show
THM-033 | DONE | backdrop.rs Grid: + every 16×8, · every 4×2 (c/th-blueprint.png)
THM-034 | DONE | backdrop.rs Hatch: ╱ where (x+y)%6==0 (c/th-cyber.png)
THM-035 | PARTIAL | backdrop.rs Rain matches skin.rs (a third of the columns, 5-13 rows/s, flickering kana, bright head), but it redraws every 16 ms through ui.ticking instead of 110 ms. Cells only move 5-13 times a second, so that's wasted GPU work. Fix: wake on a 110 ms deadline in app/gui/mod.rs next_wake instead of ticking
THM-036 | DONE | app/gui/mod.rs sets renderer.effects (phosphor monochrome, vignette 0.4, scanlines 0.12 every 2px) as a post-pass over the whole frame, overlays included; the gradient is the window's background quad (zoomed c/z-phos.png)
THM-037 | PARTIAL | backdrop::choice reads the setting (off = the theme's own, everywhere, working), and chrome.rs adds rain inside busy cards. But the window-level rain shows through every card (THM-031), so resting sessions get rain too: checked with rain = working sessions, rain over the resting openrouter·kimi-k2 session (c/rain-working.png)
THM-038 | PARTIAL | effects after overlays work; backdrops aren't confined to empty space (THM-031); title brackets/case/fullwidth/inverted tabs aren't drawn (THM-022); border sets and the focus marker give way to DD-020/DD-002
THM-039 | DELTA | DD-011: the window always has Symbols Nerd Font as a fallback face and draws Material glyphs itself (DESIGN.md icons). nerd_font and BRO_PLAIN only switch the few terminal UI glyphs it reuses (ui::provider_glyph fallbacks)
THM-040 | PARTIAL | the window draws vector logos (kit/brand.rs, DD-022) for Claude, OpenAI, Gemini, OpenRouter, Anthropic, PowerShell, Bash, Ollama, DeepSeek and Mistral, not the PNGs. Groq, Moonshot, Kimi, xAI, Grok, Z.ai, Zhipu, Meta, LM Studio, Qwen and Sakana have no logo: the launcher shows the terminal UI's text glyphs (ϟ ☾ ✕ ℤ ∞ ▤ ◭ ∝), the proxy view a cloud. Fix: add Logo variants in kit/brand.rs, or draw assets/logos/*.png through the image pipeline
THM-041 | DONE | launcher/gui.rs provider_mark: brand::provider logo, else ui::provider_glyph (glyph + brand colour, unknown ◇ muted); proxy/gui.rs upstream_cell: brand::provider, else a cloud icon
THM-042 | DONE | assets present; window icon from assets/bro.png (window.rs ICON_PNG); fonts with licences in assets/fonts/licenses; logos/*.png not used by the window (THM-040)
THM-043 | DONE | fonts.rs FONTS (19 + symbols) loaded by Gui::sync_font; the settings font row cycles them
THM-044 | DONE | app/gui/mod.rs Text::new(mono, fonts::SYMBOLS.data, …): Symbols Nerd Font Mono is the fallback face
THM-045 | DONE | settings.font is applied live by Gui::sync_font (compares font_id every frame) and at window creation; no OSC 50 (fonts::GUI). Switching to IBM Plex Mono redrew the terminals (c/font-1.png, c/font-2.png)
THM-046 | DONE | all read from the shared settings: theme, font, rain, usage_expanded and project_tabs are honoured; sidebar width is kept in px in ~/.bro/v2-gui.json (prefs.rs), leaving the cell width alone; nerd_font/icons don't apply (THM-039/040); shared_borders and the screensaver are LAY/CHR items (app/gui never reads shared_borders)
THM-047 | DONE | globals stay process-wide, which fits one workspace per window process: fonts::GUI marks the window, the icons painter is never started (logos are kit/brand.rs vectors), ui::NERD only affects reused terminal UI glyphs, and theme LAST_GOOD and util::OFFSET are shared logic

## MED

MED-001 | FIXED | watching: the file preview's load is intercepted in app/gui/mod.rs app_event (Payload::Media -> media.add Opened + show_media). Not watching: the TUI only toasts "{who} opened {name} · {prefix f} to view"; media/mod.rs collect() now also adds app.last_opened to the gallery (owner = that pane); test a_picture_an_agent_opens_while_you_look_elsewhere_joins_the_gallery
MED-002 | DONE | prefix f -> App::show_last_opened -> open_file_view -> the same app_event intercept puts it in the panel
MED-003 | DONE | media/mod.rs collect(): facts.sent -> Source::Sent with the SendUserFile caption (now at the fact's own time, fact_time())
MED-004 | DONE | app/gui/mouse.rs open_link -> App::open_file_link -> file preview -> intercept -> panel; checked in the demo (clicked a fractal.png link: panel opened, "opened by …", keyboard stayed on the session)
MED-005 | DONE | turned-card links -> mouse.rs open_link -> same file-preview intercept (code path)
MED-006 | DONE | app/gui/overlays/mod.rs -> App::agent_files_outcome(Open) -> open_file_view -> intercept
MED-007 | FIXED | collect() skipped Move facts: a move's destination now joins as Made (Read joins as "looked at", Delete skipped). A file rewritten on disk now reloads in place: mtime check in add_seen + new check_changes() poll (selected + newest 12, every 1.5 s, from collect) + refresh() keeps the picture and zoom until the new pixels arrive; test a_file_rewritten_on_disk_loads_again
MED-008 | DONE | app/gui/mod.rs app_event Event::Clipboard(pane, Some(paths)) -> media.add(Pasted, that pane's owner) (not run: alt+v writes the real ~/.bro/paste)
MED-009 | PARTIAL | links, bro open and prefix f resolve with the preview's rules (file_view::read) and the bridge's image_mime/media_mime decide Payload::Media; facts, drops and pastes use bro_media::kind_of (a superset + magic sniffing). Gap: a clicked link to a PDF or a type only bro-media knows (tiff, heic, avi…) gets the file preview's "binary file" card, not the panel — app/gui/mod.rs app_event: route Err((_, Some(path))) where media::is_media(path) && !bro_media::is_executable(path) to media.add(Opened) + show_media
MED-010 | FIXED | loads run off the UI thread (media/loader.rs) with a spinner while View::Loading; stale results were dropped by path only (a same-path duplicate replaced the view and leaked its texture): Job/Done now carry a request number (Media.req, bumped in select/refresh) and only the newest is shown; replaced textures are freed
MED-011 | FIXED | png, jpg/jpeg, bmp, ico (largest entry, image crate best_entry: 64x64 of a 16/32/64 ico) and avif decode (probed + screenshots). AVIF was Windows-only (WIC + Store AV1 codec): added an ffmpeg fallback for AVIF/HEIC/HEIF/JPEG XL (bro-media still.rs ffmpeg_still, ffmpeg.rs still); test ffmpeg_reads_stills_the_image_crate_cant. Linux/macOS without ffmpeg: clear "install ffmpeg" error
MED-012 | DONE | bro-media still.rs gif/webp/apng collect_frames keep each frame's delay; media/mod.rs prepare() advances frames; life.gif (36 frames) and an animated webp animate in the demo
MED-013 | FIXED | resvg runs no scripts and fetches nothing, but usvg read local files an SVG pointed at (resources_dir = its folder; script.svg showed transparent.png): still.rs load_svg now resolves data: URLs only, plus 64 MB SVG / 1 MB svgz caps (gzip bombs); test svg_never_reads_the_files_it_points_at; re-checked in the demo
MED-014 | DONE | mp4, m4v, mov, webm (VP9), mkv, ogv (Theora) all play here through Media Foundation (GPU above 720p), ffmpeg fallback; decoding on Player threads; clip.mp4 plays (muted) in the demo
MED-015 | DONE | mp3, wav, ogg, oga, opus, m4a, aac, flac probed OK; compact player in draw_viewer (record mark, play, seek, time, loop, mute, volume); tone.mp3 in the demo
MED-016 | PARTIAL | in the panel a PDF is a card: icon, "PDFs open in their own app: Open, below", "PDF · application/pdf · size", explicit Open (screenshot). A clicked PDF link gets the file preview's "This binary file cannot be previewed." card without size — fixed by MED-009's route in app/gui/mod.rs
MED-017 | PARTIAL | non-text files: file preview card "This binary file cannot be previewed." + copy path + "system app" (explicit; the GUI drops v.apply's Outcome::System), nothing launches (checked with data.dat). Card shows no size: file_view/gui.rs failed card needs a size line. Note: .bin is in bro-media's EXECUTABLE list, so .bin data files get "bro doesn't run programs" instead of the card
MED-018 | DONE | View::Failed shows the decoder's reason over the action row (Copy path, Mention, Open, Folder, Remove); corrupt.png checked
MED-019 | DONE | no file-size cap; stills decode to fit 2560 (thumbnails 256), and now decode again sharper up to 8192 when zoomed (sharpen); a picture over the decode budget goes to Windows' WIC, which scales while decoding (60000x60000 PNG: 42 MB peak); video streams from disk
MED-020 | PARTIAL | fixed: the info line lacked the mime (draw_info + mime()): name; "PNG · image/png · 1600×1000 · 730 KB" / "H.264 · video/mp4 · 1280×720 · 0:06 · 2.3 MB"; path relative to the session cwd; "sent by / opened by / made by / looked at by / pasted / dropped on … · age". Left: a clicked link reads "opened by" — app/gui/mod.rs's file-preview intercept can't tell a click from bro open (FileView.fallback is private: add an accessor and a Clicked source)
MED-021 | DONE | fit() + checker() under pictures with alpha (transparent.png screenshot)
MED-022 | PARTIAL | +/−/=, 0 (fit), 1 (100%), double-click fit <-> 100% at the pointer, wheel and ctrl+wheel (a Windows touchpad pinch arrives as ctrl+wheel) checked. Fixed: for pictures bigger than the 2560 decode, 1/double-click/badge were 100% of the decoded copy: now of the original (actual_zoom, decoded_share), the zoom cap reaches it (max_zoom) and the picture is decoded again sharper (sharpen; 12000x9000 JPEG checked). Left: macOS trackpad pinch (winit WindowEvent::PinchGesture) isn't handled in app/gui/mod.rs
MED-023 | DONE | drag pans when zoomed (Target::Viewer -> DragKind::Pan, mouse.rs moved -> media.dragged); arrows pan while zoomed (checked); select() resets zoom and pan
MED-024 | DONE | play/pause, seek bar with elapsed/total, volume slider (bar wider than 320 px; wheel over mute otherwise), mute, loop (screenshot)
MED-025 | DONE | video opens muted (muted = has_video), audio waits for play; set_active(panel shown && window focused) pauses video/audio and stops animations, resumes after
MED-026 | DONE | positions map: saved in drop_view, restored when the player opens (unless near the end), for this run
MED-027 | DONE | c / Copy path -> copy(): window::set_clipboard + toast "copied {path}"
MED-028 | DONE | @ / i / Mention -> Ask::Mention -> app/gui/mod.rs media_ask: owner pane (else focused), pastes "@rel " or '@"…" ' with whitespace, focuses it; closed -> "that session has closed"; checked with "with space.png"
MED-029 | DONE | o / Open -> Ask::Open -> is_executable check -> Event::OpenLink(file uri); never automatic
MED-030 | DONE | items keep their owner after the session closes and still open (project scope / nothing focused); mention toasts "that session has closed"
MED-031 | DONE | gallery thumbnails newest first with a source badge and a play mark on videos; caption and age in the hover tooltip and the info block (ages now the facts' own times)
MED-032 | DONE | Session/Project segmented control -> whole_project; shown() follows the focused session; test the_scope_follows_the_session_or_its_project
MED-033 | FIXED | dedup was exact PathBuf equality: now key() (verbatim prefix, slashes, case on Windows); a newer event (sent again, opened, pasted, dropped) moves an entry to the top by its own time, and re-reading the same facts no longer reorders or re-adds anything (add_seen); tests one_entry_per_file_however_it_is_spelled, newest_first_once_each_and_stronger_sources_win
MED-034 | DONE | ←/→ (↑/↓) step with wrap, hover ‹ › buttons, thumbnail click selects; checked
MED-035 | FIXED | Remove/Delete dropped the entry but the next transcript revision re-added it: removals are remembered (Media.hidden) until something newer (opened, dropped, sent again later); the file is never touched; test removing_hides_without_touching_the_file
MED-036 | DONE | scan_pastes() (last 12 h of ~/.bro/paste, not in demo) + collect() from the open sessions' transcript facts; nothing written
MED-037 | DONE | layout::frame splits the panel off the right; panes reflow and PTYs resize (checked opening/closing); edge drag (Action::MediaResize, mouse.rs Drag::Media, 280 px..60%); width and open state saved in v2-gui.json (prefs.rs)
MED-038 | DONE | layout::frame: sessions keep >= 360 px; a panel squeezed under 240 px hides
MED-039 | PARTIAL | Act::Media (alt+m, prefix m, rebindable, no collision) is in help and the palette (Act::all) and toggles the panel (checked). Gaps: it never gives the panel the keyboard — app/gui/mod.rs after_events Request::MediaPanel always sets media.focus = false (want: closed -> open + focus, open + unfocused -> focus, focused -> close); no alt key tip on the sidebar's media button — app/gui/hints.rs draw handles only Action::Side hits (add Action::MediaToggle -> Act::Media)
MED-040 | DONE | Media::key while focused: Esc, ←/→, space, m, l, +/−/0/1, c, @/i, o (and Delete); checked +, 1, arrows, 0, Esc, @
MED-041 | PARTIAL | sent/touched files and unwatched bro open only join the gallery; clicked links keep the keyboard on the session. Gap: a watched bro open still opens the file preview overlay until its load returns (app/files.rs agent_opened_file -> open_file_view), so keys typed meanwhile go to the overlay; for media paths (bro_media::kind_of needs no disk read for known extensions) skip the overlay and hand the path to the panel (app/files.rs agent_opened_file or app/gui/mod.rs)
MED-042 | DONE | tokens from the theme (graphite and paper checked), Inter chrome, keyboard focus and selection are full outlines, no side bars (the left hairline is the resize edge)
MED-043 | PARTIAL | blocked: panel Open (media_ask), file: OpenLink (app_event), executable preview targets (toast "bro doesn't run programs · …", checked with setup.exe); bro-media's EXECUTABLE list covers the listed types. Gap: app/files.rs open_in_editor Plan::System (no editor configured) calls panes::links::open directly on a worker, bypassing the GUI check: 'e' on a text-previewed .bat/.cmd/.js/.vbs/.hta runs it via ShellExecute "open" — refuse executables there or in panes/links.rs open()
MED-044 | DONE | panes/links.rs target(): http/https with a host, mailto, file only; no control chars; <= 8192; open() passes the URI as data (ShellExecuteW / open / xdg-open argument, no shell)
MED-045 | DONE | folder -> file_view::read Err(no path) -> Outcome::Fallback(uri) -> Event::OpenLink -> app_event (not executable) -> file manager
MED-046 | FIXED | bro-media still.rs: every image decoder now runs under a 1 GiB decode budget (limits(), check_size(), total_bytes check before from_decoder; PngDecoder/GifDecoder ran unlimited: a 75-byte PNG claiming 60000x60000 allocated 14 GB, a 35-byte GIF with a 65535x65535 canvas hung > 2 min), animations stop decoding after 8 s, SVG/svgz size caps; too-large stills fall to WIC (scaled, 42 MB peak). media/loader.rs: each job runs on its own thread under a time limit (25 s images, 45 s players) and a panicking decoder no longer kills a worker. Player keeps its 30 s open limit and bounded frame queue; ffmpeg runs have 20 s limits. Tests a_tiny_file_claiming_a_huge_picture_fails_fast_without_allocating_it, a_stuck_or_crashing_decoder_reports_instead_of_hanging. Also: SVG/svgz parsing is capped at 2M XML nodes (still.rs parse_svg, MAX_SVG_NODES). Before, a 0.9 MB svgz of <g/> (922 MB unpacked) would build a tree of gigabytes; now it's refused with "the SVG is too complex to preview" (checked in the window). Test an_svg_with_too_many_nodes_is_refused_and_svgz_still_reads
