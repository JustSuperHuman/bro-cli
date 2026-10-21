# bro-tui inventory: sidebar, chrome, theming and visuals

All `file:line` refs are under `F:\bro-cli-v2\crates\bro-tui\src\`. Nothing was edited and no build was run.

Two reference sets will help the designer:
- **Coloured renders of real frames:** `F:\bro-cli-v2\crates\bro-tui\snapshots\*.html`, written by the tests. Each opens in a browser. Useful ones: `sidebar-demo`, `usage-expanded`, `usage-resets` (alt held), `welcome`, `notifications-top-right`, `tiled-graphite`, `tiled-paper`, `tiles_same_login`, `tiles_shared_borders`, `orchestrator`, `orchestrator-tiled`, `project-tabs`, `rain-working`, `sidebar-session-actions`, and `theme-<name>` for every skin.
- **Assets:** `F:\bro-cli-v2\crates\bro-tui\assets\`. Holds `bro.png`, `bro.ico`, `claude.png`, `codex.png`, `logos\*.png` (64×64) and `fonts\*` with licences.

## 0. Screen composition and draw order (`app/draw.rs:67-127`)

1. A painted theme fills the whole screen with its bg/fg first (70-72).
2. Sidebar sizing (23-25, 73-75):
   - It shows only if `sidebar` is on and the screen is at least 72 columns wide.
   - Width is the saved `sidebar_width`, else `(w/4).clamp(30,40)`.
   - Final width is clamped to `[26, max(26, w/2)]`.
3. Project tab strip (a setting): takes the top row of the body when the body is at least 8×24 (80-89).
4. Sidebar, then its right-edge resize affordance (90-102).
5. Welcome screen (no tabs) or the panes, then tile-edge handles (107-112).
6. Launcher overlay, drawn into the body. The sidebar beside it stays live (115-117).
7. Mouse text selection, in reverse video (118).
8. Backdrops (120-122).
9. Alt-hold hints, then toasts, then the modal overlay, then skin screen effects last (123-126).

## 1. Sidebar

Trimmed from `sidebar-demo` (fallback glyphs swapped in for Nerd glyphs):
```
╭ bro  ⚙ ─────────────────────── 13:51 ╮
│ ▍  bro-cli-v2             ● 3 live  ×│
│ ▍  ~\code\bro-cli-v2                 │
│ ▶1 ◐ ✻ work · opus-5             7m ×│
│ │2 ● ◎ local · gpt-5.2-codex    31m ×│
│ │3 • ❯ shell                     1m ×│
│                                      │
│ ▏  justgains              ● 1 live  ×│
 …                                     ↔   (resize grip at mid-height)
│ ▦ tile all · 5                 alt+T │
│ + New  ↻ Continue            ✻  ◎  ❯ │
│                                      │
│ ▸ usage left ⓘ ───────────  5h   wk  │
│ ✻ claude   ▰▰▰▱▱▱▱▱        46%   39% │
│ ◎ codex    ▰▰▰▰▰▰▱▱        93%   76% │
╰─────────────────────────────── alt+b ╯
```

### 1.1 Frame (`app/side_view.rs:74-92`)
- **Border:** drawn with `ui::frame_ex`. Focused (and no overlay): accent colour with the `skin.focus` set. Otherwise: `frame` colour with the `skin.border` set.
- **Title:** " bro " in bold accent, dressed by the skin (brackets, case, inverted tab).
- **Settings gear:** sits in the top border right after the title. It is 3 cells (" ⚙ ") and shows only if width ≥ 20 + title width.
  - Hover: bold accent on `current_tint` bg. Otherwise muted.
  - Click: opens the Settings view.
- **Clock:** " HH:MM " top-right, muted (`util.rs:163`).
  - The UTC offset is probed once on a thread (powershell / `date`, `util.rs:176-201`).
  - It updates only on redraw. The loop can sleep up to 30 s.
- **Bottom-right subtitle:** "? keys" when focused, else "alt+b". Both strings are hard-coded and ignore rebinding.
- **Inner padding:** 1 column on the left (84).

### 1.2 Inner layout, top to bottom (94-134)
1. Filter line, if filtering.
2. Pinned orchestrator row plus a `╌` hairline.
3. Scrolling list of projects and sessions.
4. Tile row (1 line). Only when the project strip is off, there are ≥2 terminal panes, and height ≥ footer + strip + 10.
5. New-session strip. 2 rows (strip + blank) if height ≥ footer + 10; 1 row if ≥ footer + 6; else hidden.
6. Footer, pinned to the bottom.

**Footer height** (`footer_height` 626-636):
- Full height = 1 header + N usage lines + 2 more when expanded.
- If room ≥ full + 8: full height.
- Else if the inner area ≥ 12: only the 2 status rows (when expanded, otherwise 0).
- Else: 0.

### 1.3 Row model (`sidebar.rs`, pure)
- **Types:** `Row::Project{key,name,root,live,attention,last_age}` and `Row::Live{info,n}` (62-73).
- **Order** (95-115, 181-208):
  - Orchestrator first.
  - Then projects with live sessions, then the rest. Opened-project order is kept within each group.
  - Sessions sort by launch seq.
- **What is a project row:** an opened project (`~/.bro/v2-projects.json`) or any project with a live session. Past sessions are not rows; they only feed `last_age`.
- **Numbering:** 1-9 via `live_order`, orchestrator first. Past 9, no number. The same order drives alt+1..9, next/prev session and the attention jump.
- **Attention:** any session in the project is blocked or done.
- **Filter:** case-insensitive substring match.
  - Orchestrator matches on "orchestrator", name, harness or profile.
  - A project-name match shows all its sessions.
  - A session matches on name, harness label (or "shell"), profile or model.

### 1.4 Project row (`side_view.rs:264-298`)
Two lines, plus a blank gap above every project except the first (`row_lines` 639-655).

Line 1:
- **Bar:** project colour. `▍` if this is the current project (where new sessions start), else `▏`. Same bar on both lines.
- **Mark:** `▦` when the grid shows only this project.
- **Name:** bold, project colour.
- **Right side:**
  - "● " in danger colour if attention.
  - "{N} live " in shine if any sessions run. Otherwise "{age} " muted: the newest past session, as now / 42s / 7m / 3h / 2d / 5w.
  - " ×": danger colour when selected, else frame colour.

Line 2:
- Bar, then the short path, muted. `~` replaces the home dir; long paths compact to `C:\…\a\b`.
- Right side: branch icon (frame colour) and the branch name, cut to 18, in mix(shine, muted, .45).
- The branch is read from `.git/HEAD` and cached for 4 s (bro-core `projects.rs:84`).

### 1.5 Session row (299-359)
One line, two when it has a note.

1. **Bar** (bold, project colour): `▶` for the focused pane, `┃` for a pane in the tiled or stacked set, `│` otherwise.
2. **Number** 1-9: bold project colour when focused, else muted.
3. **Status dot** (`panes/term.rs:984-991`):
   - Blocked: `●` in danger colour.
   - Working: spinner `◐◓◑◒` at 6 fps, accent.
   - Idle and done: `●` in good colour.
   - Idle: `○` muted.
   - Unknown or plain shell: `•` muted.
4. **Harness glyph** in brand colour (`ui.rs:65-88`): Claude `#D97757`, Codex `#2EC49A`, Pi `#A78BFA`, Omp `#60A5FA`, shell muted.
5. **Primary text:**
   - Orchestrator: "★ orchestrator".
   - Else the session name. Priority: your rename, then the agent's OSC title (cleaned), then the starting name.
   - Else "login · model", else the harness label, else "shell".
   - Style: bold project colour when focused, selected or orchestrator; else fg.
6. **Secondary text,** muted: "  login · model" (login = the part after `:`). The orchestrator shows its name plus login · model here.
7. **Right side:** " {age}" muted, then " ×" (danger colour when selected).

Extra cases:
- **Orchestrator row:** gold tint bg always (337-339).
- **Note line** (352-358): bar, then 6 spaces, then the note.
  - `⏸ reason` (waiting on you) in mix(danger, muted, .45).
  - `↳ recap` muted.
  - Built in `app/insights.rs:87-95, 207-270`. Nothing shows while the agent works. Claude panes left waiting unseen get an automatic `/recap` (a setting).
- **Rename mode** (327-332): the row becomes "   ✎ " + "{text}▏" (bold accent, underlined) + "  ⏎ ok · esc" (muted).
  - Max 40 characters. Enter saves (empty restores the agent title). Esc cancels. Paste appends.
  - The × hit is disabled while renaming.
  - Only entry point: prefix `,` (renames the focused session). It forces the sidebar visible and updates the bridge title (`app/input.rs:203-228`, `app/commands.rs:532`).

### 1.6 Row backgrounds (231-241)
- **Selected** (sidebar focused): mix(user, rgb(20,20,24), .35). This mixes toward near-black even on light themes.
- **Current** (the focused pane's row, sidebar not focused): `project_tint` of the project colour.
- No hover highlight on rows.

### 1.7 Sticky orchestrator (156-170)
- Drawn above the scroll region, followed by a `╌` hairline in mix(gold, frame, .7).
- Never scrolls away. The filter can hide it.

### 1.8 Filter (141-153; `app/input.rs:230-334`)
- **Line:** "/ " (bold accent) + text (bold) + `▏` cursor while filtering. "nothing matches" when empty.
- **Input:** any printable character (punctuation and Unicode included) or a paste goes into the filter. A leading `/` is swallowed.
- **Each change:** scroll to top, select the first project, and make it the current project.
- **Backspace** pops.
- **Esc:** clears the filter first, then leaves the sidebar.
- **The filter also clears on:** leaving the sidebar, a click outside it, or the window losing focus (`app/mod.rs:978-988`).

### 1.9 Scrolling and scrollbar (176-225, 19-54; `app/mouse.rs:175-186`)
- **Unit:** one row, not one line. Row heights vary.
- **Keyboard:** selection auto-scrolls into view.
- **Wheel:** 1 row per notch, anywhere over the sidebar (gaps and footer too). The viewport then holds until the next keyboard move (`side_manual_scroll`).
- **Clamp:** the last page is always full (188-194).
- **Scrollbar:**
  - Shows only on overflow, as `┃` drawn over the sidebar's right border.
  - Thumb size and offset come from line spans (`thumb` 50-54).
  - Shows on any scroll or selection move. Solid for 800 ms, fades over 250 ms toward the border colour, redrawn every 40 ms while fading.
  - Not draggable.

### 1.10 Tile row (115-134)
- **Not tiled:** "▦ tile all · N" (hover: bold accent) plus the TileAll key on the right (default alt+T).
- **Tiled:** left "▦ tiled N" toggles back. Right "▸ minimized n" / "▾ minimized n" toggles showing minimized sessions.
- Hidden when the project tab strip is on.

### 1.11 New-session strip (365-417)
- **Left buttons:**
  - "+ New" (bold accent) opens the launcher.
  - "↻ Continue" (bold) opens the launcher's Continue tab. Below 15 columns it shows only `↻`.
  - Hover on either: tint bg + bold accent.
- **Right buttons:** 3-cell icons for Claude, Codex and Terminal (Terminal in shine).
  - Hover: tint bg, and the name appears muted in the gap ("Claude", "Codex", "Terminal").
  - Claude/Codex: quick launch on the usual login, or open the launcher on that harness when a choice is needed (`app/commands.rs:364-380`).
  - Terminal: a shell in the current project.
- **Narrow sidebar:** the icons drop first.

### 1.12 Usage block (420-614)
**Header:**
- "▸ usage left " (or ▾) in accent, then ⓘ (hover: bold accent), then a `─` fill, then the column heads " 5h   wk  ". With alt held the heads read "resets ".
- Click ⓘ: Stats view.
- Click anywhere else on the header: collapse/expand, persisted as `usage_expanded`.

**Collapsed:** one line per family that has a signed-in login ("claude", "codex").
- Figures come from `app_summary` (bro-core `usage/mod.rs:170`).
- Notes: "…" loading, "unavailable", or "n✗" for partial failures.

**Fable allowance:** when any Claude account has one, the Claude label gets its own `▸`/`▾` hit cell.
- Open: adds a muted " fable" line with no glyph. Clicking that line opens the Usage view.
- Not persisted.

**Expanded:** one line per signed-in profile.

**Line layout:**
- Glyph, then the label padded to name width + 2 (name width = longest label, clamped 5-10).
- Meter: `▰▱`, up to 8 segments, showing the tighter of the two windows. Colour by `left_color`: ≤20 danger, ≤50 amber `#ffb86c`, else good. Empty segments use the frame colour.
- Right: 5h and wk figures. "≈" prefix when the 5h figure is capped by the week. "—" when missing.
- Expanded with no room for the meter: a countdown like "2h14m".
- Alt held: the right side reads "Resets @ 3:40pm" / "Fri 3:40pm" / "Oct 21", or "no reset reported".

**Row hover:** tint bg and a `+` in the last cell. Click launches a new session: a family line uses the quick defaults; a profile line uses that exact login. Either way it starts in the current project (`app/commands.rs:411-430`).

**Status lines, expanded only** (last 2 rows, 570-613):
- `proxy`: "● idle" / "● N sessions · port N" (good), "◌ starting", or "○ off" (danger). Click opens the Proxy view.
- `phone`: "● no phones" / "● N connected · port N", "◌ starting", or "○ off". Click opens the Bridge view.

### 1.13 Resize, hide and collapse (`app/draw.rs:90-102`; `app/mouse.rs:115-166, 622-628`)
- **Grip:** the sidebar's right border column, which is also the scrollbar column.
  - Hover or drag: the whole column goes bold accent.
  - A `↔` handle sits at mid-height. Accent when hot, muted otherwise. Hidden under the scrollbar thumb unless hot.
- **Drag:** captures the mouse until release. Width is clamped to [26, max(26, screen/2)] and saved once, on release. Shrinking the window does not overwrite the saved width.
- **Double-click** (400 ms): back to automatic width.
- **Hide/show:** alt+s hides or shows. alt+b shows and focuses; again, leaves.
- **Squeezed** (below 72 columns): alt+b focuses the project strip if it is on. Otherwise it toasts "the window is too narrow for the sidebar · widen it, or turn on project tabs" (`app/commands.rs:106-125`).

### 1.14 Keyboard with the sidebar focused (`app/input.rs:264-334`)

| Key | Action |
|---|---|
| ↑ ↓ / Home End | Move selection |
| PgUp PgDn | Move by 10 |
| ← | Jump to the project header |
| → | Step into the project. On a session row: back to the panes |
| Enter | Activate the row |
| Shift+Enter | Toggle "show alongside" (stack) |
| Delete | Close the session or project (confirms when busy / live) |
| Tab | Back to the panes |

Every move re-shows the scrollbar and sets the current project. alt+← off the leftmost pane enters the sidebar.

### 1.15 Click targets (`SideHit`, `app/mod.rs:77-106`; dispatch in `app/mouse.rs:187-246`)
Hits are recorded during draw and searched in insertion order, so the first match wins: × beats its row, and ⓘ beats the header.

| Hit | Where | Effect |
|---|---|---|
| `Row(i)` | Row rect | Focuses the sidebar. Project row: make it current and tile its sessions (again: All / back). Session row: go to it. |
| `Row(i)` with ctrl/shift | Session row | Toggle stack. |
| `Row(i)` middle-click | Session row | Close the session. |
| `Close(pane)` | × on a session row | Close; confirms when the agent is busy. |
| `CloseProject(root,live)` | × on a project row | Confirms if sessions are live. Then toasts "closed X · choose it from New session". |
| `Settings` | Gear | Settings view. |
| `Launcher` | + New | Launcher. |
| `Continue` | ↻ Continue | Launcher, Continue tab. |
| `Quick(h)` | Strip icons | Quick launch. |
| `TileAll` | Tile row | Toggle tile all. |
| `Minimized` | Minimized switch | Toggle minimized visibility. |
| `UsageToggle` | Usage header | Collapse/expand. |
| `Stats` | ⓘ | Stats view. |
| `Fable` | Claude arrow | Toggle the fable line. |
| `LaunchUsage(h,profile)` | Usage line | New session on that harness/login. |
| `Usage` | Fable line | Usage view. |
| `Proxy`, `Bridge` | Status lines | Their views. |
| `Strip(..)` | Project tab strip | See 2.8. |

Other click rules:
- An empty sidebar click focuses the sidebar. Any click outside leaves it.
- While an overlay is open, sidebar clicks are swallowed; Palette and Help close on any click.
- **Launcher open:** the sidebar stays live (`app/mouse.rs:456-494`).
  - A project row picks the launcher's project.
  - +New / Continue switch the launcher's tab.
  - The toggles leave the launcher open.
  - Anything else closes it first, then acts.
- `open_view` focuses a view that is already open. Pressing its key again while focused returns to the previous tab.

### 1.16 Alt-hold hints (`app/alt_hints.rs:127-170`)
After 300 ms holding alt:
- Session numbers light up in reversed accent.
- Clickable spots show their key: either the matching letter in the label lights up ("+ New" lights N), or a " k " chip appears beside it.

## 2. Pane chrome (`app/draw.rs:213-442`; `ui.rs`)

Excerpts from the snapshots:
```
┏ − ▶ bro-cli-v2 · ✻ work · opus-5 ━━━━━┓      focused
╭ −   bro-cli-v2 · ❯ pwsh ────────────╮       unfocused
┗ ◐ working                     ━━×━━┛  /  ╰ ● needs you   ──×──╯
── ● bro-cli-v2 · ✻ local · 5 sessions ─────   group rule
╔ − ▶ ★ orchestrator · ✻ local ═══╗ … ╚ ○ idle ══×══╝
├ −   4 · Group the tiles    ○ idle ×──┼ −   5   ○ idle ×──┤   shared borders
```

### 2.1 Frame choice
- **Sessions** use `session_frame_as` (`ui.rs:173-222`).
- **Views** (usage, profiles, proxy, bridge, stats, settings, setup, quick-install) use `frame_ex` (143-157).
  - Title " {icon} {title} ".
  - Unfocused: title unbolded and muted (`draw.rs:346-348`).
  - Subtitle bottom-right, muted.
- **Focused** means the pane holds focus, the sidebar is not focused, and no overlay is open (340).

### 2.2 Session frame
- **Border colour:**
  - Focused: the project colour.
  - Unfocused: mix(project, frame, .45).
  - Orchestrator unfocused: mix(gold, frame, .8). That is more muted than a normal session, although the code comment claims it "keeps most of its color".
- **Border set:** orchestrator is always DOUBLE. Focused uses `skin.session_focus` (default THICK). Otherwise `skin.border` (default ROUNDED).
- **Title band:** the whole top edge between the corners.
  - Focused: solid project colour, ink from `ink_on` (a WCAG check picks deep() or white), bold.
  - Unfocused: project-colour text on `project_tint`.
- **Band contents, in order:**
  1. Minimize button " − " / " + ". Tile-all only; reverses on hover.
  2. Marker: `skin.marker` (default `▶`) when focused.
  3. " {project} ·". Omitted inside a group frame.
  4. Title: harness glyph, name, then "login · model", then " — cwd" when the cwd differs from the project root (`panes/term.rs:1000-1010`). Inside a group the title is " {n} · {name}" instead (761-767).
- **Status parts** (1011-1036), in order:
  - " ↑{n} " scrollback depth, inline colour.
  - " ⇲ {cols}×{rows} remote ", muted. Means the pty was grown for a phone/web viewer; the tile shows its bottom-left.
  - Auto-resume note, e.g. "↻ retry 2 in 14s".
  - Watchdog: "◐ watchdog reviewing" (accent), "✓ watchdog done" (good), "‖ watchdog paused" (muted), or "◉ watchdog 2/5" (shine).
  - Activity, bold: "● needs you" / "◐ working" / "○ idle". The header never says "done".
- **Status position:** bottom-left edge, with 7 cells reserved on the right.
  - Shared borders instead put the status right-aligned on the title band, each part keeping its own colour.
- **Subtitle** (349-355): "back · alt+double-click" (card flipped), "zoomed · alt+z", or "grown · ctrl+down restores", in the project colour.
- **Close "×":** hit area is 3 cells at right−4, on the bottom edge (on the band with shared borders).
  - Only if width > 24 and not zoomed. Drawn as `──×──`.
  - Hover: danger colour, bold, reversed. Click asks before closing a busy agent (379-392).
- **Minimize hit:** (x+1, y, 3×1).
  - When every session is minimized, the body shows "All sessions minimized · show minimized in the sidebar".
- **Special pane bodies:**
  - Queued resume: spinner, "Resuming {title}", "· up next / in line · N ahead / starting now", and "Sessions open one at a time so they don't fight over your machine." (`panes/term.rs:292-321`).
  - Spawn error: the error, then "enter or esc closes this pane".

### 2.3 Tile groups (`app/draw.rs:31-53, 232-257, 315-319`; `ui.rs:308-326`)
- Tiles that share project, harness, login, model and cwd form a group (`panes/term.rs:749-757`).
- A group of ≥2 in a region at least 14×6 gets a ring. Only the top rule is drawn, with the skin's horizontal glyph. The other sides stay blank as a 1-cell gutter.
- Rule colour: the project colour when the group holds focus, else mix(colour, frame, .5).
- Rule label: " ● {label} " + "· N sessions".
- Region weights are floored at ceil(tiled/4).

### 2.4 Orchestrator column (`app/draw.rs:57-65`)
When tiled with others: a full-height right column, width (body/3) clamped to [min(44, body/2), 96] and never more than half. Gold double frame.

### 2.5 Shared borders, a setting (`app/draw.rs:148-150, 564-597`; `ui.rs:245-302`)
- Each tile is drawn 1 cell onto its right/below neighbour, so they share one line.
- Overlapping corners become `┬┴├┤┼`.
- 1 blank column/row of padding inside shared lines.

### 2.6 Tile edges (`app/draw.rs:153-206`; `app/mouse.rs:280-306`)
- Held edges and the edge under the mouse light up bold accent.
- Handle `↔` (vertical edge) or `↕` (horizontal) sits mid-span, or under the mouse when lit. Muted when idle.
- Drag holds the edge. Double-click releases it.

### 2.7 Other chrome behaviour
- **Split dividers:** draggable, ratio 0.1-0.9, no visual handle.
- **Folded slivers** (from ctrl+click grow): the title runs vertically in project colour; no clicks inside.
- **Prompt options:** a hovered option row gets a tint and bold; clicking answers the prompt (412-420).
- **Card flip** (alt+double-click): 280 ms squash with `▕▏│` edges, dimming, a 45 ms cascade across cards, and hover tints on the back's links (`card.rs:21-96`). The summary screensaver flips every card after idle.
- **Selection:** reverse video. Copied on release with a toast "copied N characters".
- **Double-click** (400 ms, ±1 cell): zoom. **Ctrl+click:** grow.
- **Alt hints on panes** (`app/alt_hints.rs:59-92`), on the bottom edge: "dbl-click flip", "{n} go" on unfocused panes, and "i flip / I flip all / z zoom / = grow" on the focused one.

### 2.8 Project tab strip, a setting (`app/project_tabs.rs:261-477`)
- **Layout:** " ▦ All N ", then a gold " ★ ◐ ●1 ✓1 " orchestrator tab, then " │ ", then per project " ◐ name 3 ●1 ✓1 × ".
  - ● = sessions waiting on you, ✓ = finished unseen.
  - The × shows on hover; its room is always kept.
- **Tab styles:** the showing tab is solid (band style). Hover or keyboard pick: tint, plus underline for the keyboard pick.
- **Overflow:** names shrink from 24 down to 6 columns, then fold into "◂N" / "+N▸" (with ● if anything folded needs you).
- **Right end:** a "▸ N minimized" switch and " + " for a new session.
- **Mouse:** the wheel switches tabs (once per 150 ms burst). Middle-click closes a project. "+N" opens the palette on "tiles ".
- Tab order and names follow the phone/web client's arrangement.

## 3. Welcome, logo, toasts, confirm

### 3.1 Welcome (`app/welcome.rs:31-94`)
- **Frame:** unfocused and untitled, around a 22-row block centred vertically.
- **Logo** (`ui.rs:493-528`): a 6-row "BRO" in ANSI-shadow style.
  - Animated themes (ultra, nebula, outrun, vaporwave, or a custom file with `rainbow=true`): a moving rainbow, redrawn every 125 ms, only on this screen.
  - Other themes: an accent→shine gradient.
  - `█` cells take the full colour; shadow cells are 45% toward rgb(40,40,50).
  - Too small: "bro" in bold accent.
- **Tagline:** "the agentic terminal workspace", muted.
- **Call to action:** " alt+n " (reversed) + "  to launch your first agent". With nobody signed in: the Setup key + "  to set up: install an agent and sign in".
- **8 key rows**, keys taken from the live keymap: launch an agent, browse projects, palette, usage meters, profiles, setup guide, pair your phone, all keys.
- **Status line:** "● N logged-in profiles", "● proxy :PORT", "● bridge :PORT · N dev". Each has an off/loading variant.
- **Bare-screen keys:** Enter/n, s, ?, p, b/Tab/←, q.
- Setup opens automatically on a bare start when nobody is signed in (12-29).

### 3.2 Toasts (`alerts.rs`; `app/draw.rs:526-561`)

| Kind | Glyph | Colour |
|---|---|---|
| Info | · | accent |
| AgentDone | ● | good |
| NeedsYou | ● | danger |
| Bridge | ⌁ | shine |
| Usage | ◔ | inline |
| Error | × | danger |

- At most 2 show. A toast with the same text replaces the old one. Each lasts 2 s.
- Top-right of the screen from row 1, newest on top. Width = text + 4, cut at 60. No OS notifications.
- Agent finished / needs you appends the reason or recap (`app/sessions.rs:766-799`).
- **Prefix armed:** a 3-row box bottom-right, titled "ctrl+space …", showing the prefix hint (528-536).

### 3.3 Confirm dialog (`app/overlays.rs:84-96, 140-155, 204-222`)
- Popup centred a third of the way down. Width clamp(text + 8, 40, 100), height 7, title "confirm".
- Text in bold danger colour; detail muted.
- Buttons: " Yes  (y) " (danger bg) and " No (esc) " (frame bg); hover underlines.
- y / Y / Enter means yes; any other key means no. Only the buttons take clicks.
- Actions and their copy:
  - Quit: "quit bro and end N sessions?"
  - Close: "close {title}?" / "the agent is still busy — it will be stopped"
  - CloseProject: "close {name} and its N running sessions?"

### 3.4 Overlays that exist (`app/overlays.rs:16-32`)
Launcher (in the body), Palette, Help, Resume picker, Confirm, File preview, AgentFiles, Watchdog, WatchdogPrompt.

## 4. Theme system

### 4.1 `Theme` struct (`theme.rs:26-54`)

| Field | Use |
|---|---|
| `name` | Theme id |
| `bg` | Background. `Reset` = terminal default |
| `fg` | Body text |
| `accent` | Focused frames, titles, highlights |
| `shine` | Secondary highlight |
| `frame` | Unfocused borders |
| `muted` | Hints, metadata |
| `user` | Selection tint |
| `inline` | Warm highlight, warnings |
| `danger`, `good` | Status colours |
| `animated` | Rainbow logo |
| `ansi: Option<[Color;16]>` | Palette for session content |
| `skin: Skin` | Shapes, backdrop, effects |

Helpers:
- `is_light`: bg luma > 140.
- `painted`: bg is not `Reset`.
- `deep`: the dark painted bg, else rgb(18,18,22).
- `swatch`: 5 colours for the theme picker.

Derived colours:
- **`project_color`** (262-279):
  - Orchestrator: gold, rgb(255,200,61) on dark, rgb(178,128,0) on light.
  - mono / terminal themes: the accent.
  - Painted themes: FNV-1a hash of the lowercase folder name picks one of ansi slots [4,5,6,3,2,12,13,14].
  - Others: HSL hue 20-340° (never red); s/l .7/.72 on dark, .55/.38 on light.
- **`project_tint`:** 83% toward deep() on dark, 90% toward bg on light.
- **`current_tint`** (hover bg): accent mixed 84% toward deep() on dark, 90% toward bg on light.

### 4.2 Built-in themes (30, in picker order)
graphite (default), paper, tokyo-night, catppuccin-mocha, rose-pine, kanagawa, gruvbox, nord, everforest, nebula, phosphor, outrun, blueprint, cyber, digital-rain, vaporwave, catppuccin-latte, rose-pine-dawn, ultra, oriel, ember, ocean, forest, sakura, synthwave, matrix, amber, dracula, mono, terminal.

- **Accent-only** (graphite + 11): take bg/fg from the terminal. Frame/muted/user are greyed. Only ultra is animated.
- **Painted** (16 + paper): own bg/fg and a 16-colour palette (paper has none). nebula, outrun and vaporwave use the rainbow.
- **terminal:** named ANSI colours only.
- **Light:** paper, catppuccin-latte, rose-pine-dawn.

### 4.3 Custom themes (461-637)
- **Location:** `~/.bro/themes/<name>.toml`. Built-in names win over files.
- **Keys:**
  - `base`: any theme, default graphite; chains up to depth 4.
  - Colour keys: the 8 colours plus `text`/`fg` and `background`/`bg`.
  - `rainbow`: bool.
  - `ansi`: 16 colours.
  - `skin`: borrow a built-in theme's skin.
  - `borders`: rounded / plain / double / thick / dashed / heavy-dashed / ascii / block.
- **Colour values:** `#rgb`, `#rrggbb`, ANSI names, or `terminal`.
- **Hot reload:** a notify watcher fires `ThemeFilesChanged`. The theme reloads and the first problem is toasted. A parse error mid-edit keeps the last good version.
- **Switching:** palette "theme " (prefix t) shows each theme as a swatch on its own bg, tagged light / dark / yours / current.
  - Moving previews live. Esc reverts. Enter saves and toasts "theme: X".
  - Switching also re-inks the logos (`app/mod.rs:747-755`).

### 4.4 Skins (`skin.rs`)
**Skin fields:** `border`, `focus`, `session_focus` (box-drawing sets), `brackets`, `case` (Keep / Upper / Wide fullwidth), `invert_titles` (title as a solid tab), `marker`, `backdrop`, `fx`.

**Default:** ROUNDED / ROUNDED / THICK borders, marker `▶`.

| Theme | Borders | Title | Marker | Backdrop | Effects |
|---|---|---|---|---|---|
| phosphor | PLAIN / DOUBLE / DOUBLE | `[ ]`, UPPER | `>` | none | phosphor + scanlines + vignette |
| outrun | PLAIN / DOUBLE / DOUBLE | `◢ ◣` | default | Stars | gradient #0a001c→#3a0a4a |
| blueprint | dashed / PLAIN / DOUBLE | `┤ ├`, UPPER | `◇` | Grid | none |
| cyber | PLAIN / THICK / THICK | `▐▌`, UPPER, inverted tab | `◆` | Hatch | none |
| digital-rain | PLAIN / PLAIN / THICK | `[ ]` | `»` | Rain | none |
| vaporwave | DOUBLE ×3 | fullwidth | `✦` | Stars | gradient #2a1048→#0c2d3d |
| nebula | default | default | `✦` | Stars | vignette |

**Backdrops** (279-357) fill only runs of ≥6 blank theme-bg cells, keeping 2-cell margins. That includes empty space inside panes.
- **Stars:** sparse ✦ + ·, twinkling by sine.
- **Grid:** `+` every 16×8, `·` every 4×2.
- **Hatch:** `╱` diagonals.
- **Rain:** a third of the columns fall at 5-13 rows/s with flickering katakana; bright head; redrawn every 110 ms.

**Rain setting** (bro-core `config.rs:285-296`; `app/draw.rs:132-145`):
- off: the theme decides.
- everywhere: rain over everything.
- working: rain everywhere except inside resting sessions.

**Effects** (396-441) run last, over overlays too: gradient, phosphor monochrome, scanlines, vignette.

### 4.5 Icons
- **Icon table** (`ui.rs:19-50`): 30 names, each with a Nerd glyph and a plain fallback (✻ ◎ π ◆ ❯ ⚙ ⓘ ⎇ ⇄ ⌁ ◉ …).
  - The `NERD` global follows the `nerd_font` setting, which defaults to false; `BRO_PLAIN` forces plain.
  - Toggle with prefix i.
- **Real logos** (`icons.rs:118-137`): 18 PNGs, each with a scale and an ink rule.
  - Brand ink: Claude coral, Groq, Sakana.
  - Text ink: white on dark, #1f2328 on light.
  - Own colours: Gemini, DeepSeek, Mistral, Zhipu, Meta, Qwen.
  - In the terminal they draw 2 cells wide via sixel/kitty/iTerm when detected. A GUI can draw the PNGs directly.
  - `provider_glyph` maps provider ids to logo + brand colour (`ui.rs:93-139`); unknown providers get ◇.

### 4.6 Fonts (`fonts.rs`)
- **19 bundled fonts:** jetbrains-mono, fira-code, ibm-plex-mono, maple-mono, victor-mono, monaspace-krypton, monaspace-radon, space-mono, share-tech-mono, comic-mono, departure-mono, vt323, kode-mono, doto, monocraft, ibm-vga, workbench, sixtyfour, press-start-2p. Symbols Nerd Font Mono is always the fallback.
- **When offered:** only in the Just Terminal fork (`--app-window`) or with `BRO_TERMINAL_FONTS=1`.
- **Switching:** the Settings Font row cycles with ←/→ live, starting from the terminal's own font. The file is unpacked atomically to `~/.bro/fonts`, then OSC 50 is sent between frames and restored on exit. Saved as `settings.font`.

### 4.7 Other visual settings
In `~/.bro/v2.toml` (bro-core `config.rs:135-167`): `theme`, `nerd_font`, `usage_expanded`, `sidebar_width`, `icons`, `rain`, `font`, `summary_screensaver_secs` (0 / 60 / 120 / 300 / 600 / 900 / 1800), `shared_borders`, `project_tabs`.

## 5. Model/view separation

- **bro-tui is binary-only.** Cargo.toml has two `[[bin]]` targets and no `lib.rs`, so another crate can import nothing from it. Reuse needs a lib crate (or a `[lib]` target) first.
- **Pure, reusable once moved:**
  - `sidebar.rs`: the whole row model, ordering, numbering, filter and project-tab counts.
  - `alerts.rs`: the toast queue.
  - `util.rs`: time and path formatting.
  - `card::Flip` timing.
  - `layout.rs` and `priority.rs`, but they work on ratatui `Rect` in cells.
  - `insights::note_line`, `status_look`.
  - bro-core's `usage::app_summary` and `headroom_detail` are already in a lib.
- **Ratatui-typed data that needs neutral types:**
  - `Theme` uses ratatui `Color`, including `Reset`, named and indexed colours.
  - `Skin` uses ratatui `border::Set`.
  - Backdrop and effect algorithms work on a ratatui `Buffer`.
  - The `ui` helpers return `Style`/`Span`.
- **Tangled with drawing:**
  - `side_view.rs` computes heights, usage lines and scroll, and records hits, all inside draw. Its helpers are private (`UsageLine`, `footer_height`, `row_lines`, `thumb`, `bar_alpha`).
  - `Pane::header` returns styled `Line`s; the status facts (scroll, remote size, resume note, watchdog, activity) are not exposed as data. The GUI needs a `HeaderInfo`-style struct.
  - `Pane::render` takes a ratatui `Frame`.
- **Sidebar state lives on `App`** (`app/mod.rs:163-182`):
  - `pub(crate)`: `sidebar`, `side_focus`, `usage_expanded`, `show_fable`, `side_sel`, `side`, `side_filtering`, `renaming`, `side_nav_at`.
  - Private: `side_scroll`, `side_manual_scroll`, `side_drawn_scroll`, `side_thumb`, `sidebar_width`, `side_drag`, `side_edge_click`, `side_area`, `side_hits`, `hover`.
- **Hit testing is immediate-mode.** It uses the last frame's rects: `side_hits`, `pane_close`, `pane_minimize`, `outer`, `inner`, `confirm_hits`, `card_links`, `strip`.
- **Action logic** (`activate_row`, `toggle_project_tiles`, `close_project`, `quick_launch`, `launch_usage`, `go_session`, filter handling) is reusable in spirit. It is coupled to `Pane` trait objects, `Term`, `Services` and crossterm events.
- **Globals to replace:** `ui::NERD`, the icons painter, the fonts statics, the theme `LAST_GOOD` cache, `util::OFFSET`, the git-branch cache.
- **Terminal-only code to drop:** the image protocols (keep the `LOGOS` table and ink rule), OSC 50 (keep `FONTS`), and the bare-alt console hack.

## 6. What a GUI port must not miss

1. **Hit priority:** × beats its row, ⓘ beats the header, the Fable arrow is separate from the launch row, and the strip × beats its tab.
2. **Focus:** pane focus styling turns off while the sidebar or an overlay has focus. The "current" and "selected" row highlights depend on sidebar focus and never show together.
3. **"Current project"** (`▍`) is where new sessions start. It follows pane focus (except the orchestrator), the sidebar selection while browsing, and picks made in the launcher.
4. **Scrolling:** variable row heights, scroll unit = row, sticky orchestrator, wheel holds the viewport, keyboard reveals the selection.
5. **Size thresholds:** sidebar hidden under 72 columns; footer, strip and tile row drop by height; icons and the "Continue" word drop by width. Text truncates with "…", and the right side of a row wins when space is tight.
6. **Timers:** spinners 6 fps, clock, ages, countdowns, scrollbar fade, toast expiry, rain 110 ms, rainbow 125 ms, card turn 16 ms frames, alt-hold 300 ms, screensaver.
7. **No terminal defaults in a GUI.** Accent-only themes use `Reset` bg/fg, and the `terminal` theme uses named ANSI colours. The GUI must define a default bg/fg and a 16-colour palette; the HTML snapshots use #0c0c0c / #d8d8d8.
8. **Session colour remapping** (`panes/term.rs:929-954`) must carry over to alacritty_terminal's colour config:
   - Painted themes supply default fg/bg and the 16 ANSI colours.
   - Light themes: white text becomes fg, and dark shading becomes light tints.
   - Dark painted themes: greys are recast between the theme's bg and fg.
9. **Light themes** need `ink_on`, tints toward bg, the dark-gold variant, darker project lightness, and re-inked logos.
10. **Skins are more than colours:** border sets per role, title brackets/case/fullwidth/inverted tabs, the focus marker, backdrops only in empty space, and effects drawn after the overlays.
11. **Mouse:** middle-click closes; ctrl/shift-click stacks; double-click (400 ms) resets the sidebar edge, zooms a pane, or releases a held edge; alt+double-click flips a card; the wheel over the strip switches tabs; back/forward buttons walk history. Drags capture the mouse across panes. An open confirm blocks all other input. The sidebar stays live beside the launcher.
12. **Design rule conflict.** The TUI marks project, current and stack state with coloured side bars (`▍ ▏ │ ┃ ▶`) and full-width title bands. Your design rules forbid accent bars, so the redesign should carry that state through background, a full border or an icon. Your rules also make Inter or Poppins the default for designed UI; the terminal grid stays monospace.
13. **Quirks to settle:**
    - Stale "? keys" subtitle: `?` types into the filter.
    - Hard-coded "alt+b" subtitle ignores rebinding.
    - `panes/term.rs:57` says rename with sidebar `r`; letters actually go to the filter.
    - The selection bg mixes toward near-black on light themes.
    - The orchestrator's unfocused border is more muted than a normal session's, against its comment.
    - The `SideHit::Settings` doc says "strip gear"; the gear is in the top border.
    - Stats has no alt chord, so it gets no alt hint.
