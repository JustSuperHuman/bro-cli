# bro desktop app: design system

Windows 11 (WinUI 3 / Fluent) in manner, everywhere bro runs. Calm, precise, content first: the terminals are
the product, the chrome around them is quiet and exact. Every colour comes from the active theme
(`kit::Tokens::from_theme`), so a custom theme file restyles the whole app. Nothing here depends on Windows: the
look is drawn, not borrowed from the OS.

## Principles

1. **WinUI geometry.** Controls (buttons, fields, chips, keycaps, list rows, segmented choices) have **4px**
   corners (`kit::radius::CONTROL`); cards, dialogs, menus, flyouts and the session cards have **8px**
   (`kit::radius::OVERLAY`). Nothing rounder, except the toggle switch's track and circular marks.
2. **Depth by layers.** Window (`t.bg`, like Mica) → content layer (`t.layer`) → cards (`t.surface`) →
   floating (`t.raised`: dialogs, menus, tooltips). Steps are slight luminance changes; strokes are 1px
   (`t.stroke` on controls, `t.hairline`/`t.border` elsewhere); shadows only on floating things. A theme with its
   own side bar colour (VS Code's, `Theme::panel`) is the window, and the cards are its editor colour. The
   window's own surfaces (the content layer, session cards, the cards and panels inside views, scroll-edge fades
   on them) are glass fills (`ui.glass`, `ui.glass_round`, `ui.glass_panel`, `scene.top_glass_quad`): they paint
   their colour without changing how see-through the window is, so over a material (Acrylic, Mica, Glass) the
   whole window, bro's title bar included, is evenly see-through at the tint (`t.window`'s alpha). Over a solid
   window they're ordinary fills. Text, controls, buttons and floating things (dialogs, menus, toasts) are
   ordinary fills and stay solid.
3. **No accent bars, ever.** No coloured stripe on any edge of a card, row, tab, callout or panel, and no
   selection pill beside a list item. Selection is a fill (`t.selected`); keyboard focus is a full 1px outline.
4. **No dots, no badges for identity or counts in lists.** A project is its name in its own colour (the
   terminal UI's `project_color`), with no colour dot and no folder icon; sessions show their agent's real logo; a
   project row doesn't carry "N live". Status is an icon (spinner while working, alert when it needs you, check
   when done), never a coloured dot or a text badge like "working".
5. **Real marks.** Claude, Codex (OpenAI), Gemini, OpenRouter, PowerShell, Bash and the other brands draw their
   actual logos (`kit::brand`: `ui.mark(brand::harness(h), r, size, None)`, `brand::shell(program)`,
   `brand::provider(id)`), in their own colour or the text colour. Pi and omp have no logo: a glyph.
6. **Never an icon on a coloured tile.** Icons and logos stand on the surface they're on: no coloured square,
   circle or rounded tile behind them (settings section icons, list icons, agent marks). An icon may sit on a
   neutral control fill only when it *is* a control (an icon button on hover).
7. **One accent.** The theme's accent marks focus, selection and the primary action. Status colours mean status
   only: `good` done/healthy, `danger` needs you/errors, `inline` warnings, `warn` "running low" (amber, darker on
   light themes), `shine` secondary highlights, `gold` the orchestrator. A project's colour (`t.project(key,
   theme)`) is identity, as in the terminal UI: its name in the sidebar and the tabs, and the name, number and a
   faint wash of the session you're in.
8. **Quiet until touched.** Secondary controls (close ×, minimize) appear on hover or focus; hover fades over
   ~110 ms. Nothing jumps.
9. **Self-explanatory.** No paragraphs explaining the UI. Labels say what a thing is; a short help line only where
   the terminal UI has one (reuse its copy).

## Type

Inter for everything except terminals, code and file paths (the terminal font, `Face::Mono`).

| Use | Size (`kit::size`) | Weight (`kit::weight`) | Colour |
|---|---|---|---|
| Page and dialog titles | `HEADING` 16 – 20 | `SEMIBOLD` | `text` |
| Group headings | `SMALL` 12 (`widgets::section`) | `SEMIBOLD` | `text`·`muted` |
| Card titles, list primary text | `BODY` 13 / `TITLE` 14 | `SEMIBOLD` / `MEDIUM` | `text` |
| Secondary text, metadata | `SMALL` 12 | `REGULAR` | `muted` |
| Captions, counts | `CAPTION` 11 | `MEDIUM` | `faint` |
| Big numbers (stats) | 22–40 | `SEMIBOLD` | `text` |

No ALL-CAPS labels: group headings are sentence case, semibold (WinUI's "Body Strong"). `muted` and `faint` sit
where WinUI's secondary and tertiary text do (about 70% and 50% of the way from the background to `text`), so
captions stay readable.

Glyphs are smoothed per colour channel where Windows has ClearType on (bro-gfx `Antialias::Rgb`/`Bgr`, drawn
with dual-source blending), in greyscale elsewhere, with baselines on whole pixels. `BRO_TEXT_AA=grayscale`,
`rgb` or `bgr` overrides the system's choice; `BRO_TEXT_GAMMA=light,dark` tunes stem weight (default
`0.82,1.12`).

## Spacing and sizes

- Spacing: 4, 8, 12, 16, 20, 24, 32. Pages pad 24; cards pad 16; list rows 8–12 horizontally.
- Controls are 32 high (compact 28); list rows 32 (one line) or 48 (two); hit targets never under 24×24.
- Toggle switch 40×20 (`widgets::SWITCH_W`, `SWITCH_H`); place it from those, right-aligned.
- Shadows: floating things only (`widgets::card`, tooltips, key tips). Cards in a layout don't cast shadows.

## Components (`kit::widgets`)

| Need | Use |
|---|---|
| Action | `button(ui, r, label, Kind::{Primary, Secondary, Ghost, Danger}, action)`; width `button_width`; 32 high |
| Icon-only action | `icon_button(ui, r, glyph, colour, action)` + `tooltip` on hover |
| Any standard control box | `control(ui, r, fill)` (fill `t.control`, stroke, the lifted edge) |
| On/off | `switch(ui, at, on, action)` |
| One of a few choices | `segmented(ui, r, &options, selected, |i| action)` |
| Usage left / used (the terminal UI's ▰▰▱) | `segments(ui, r, fraction, n, colour)`, `n` from `segment_count(w, 7)`; 6–8 px high |
| Progress (loads, scans) | `meter(ui, r, fraction, colour)` (WinUI progress bar) |
| Tag, count | `pill(ui, at, h, text, fg, bg)` (4px corners; bg = colour at ~12% alpha or `t.control`) |
| Key hint | `kbd(ui, at, "alt+n")`: one keycap per key |
| Key tip (alt held) | `keytip(ui, center, "alt+n")` |
| Text input | `field(ui, r, value, Some(caret_byte), placeholder, focused, action)` |
| Long lists | clip with `ui.clip`, keep the selection in view, `scrollbar(...)` |
| Floating dialog | `scrim(...)` then `card(ui, r, radius::OVERLAY)` on its own layer |
| Selectable rows | `row(ui, r, key, selected, action)` |
| Group heading | `section(ui, r, "Appearance")` |
| Logo / agent mark | `ui.mark(mark, r, size, None)`; `ui.logo(Logo::Claude, r, colour)` |

Icons are Nerd Font Material Design glyphs, monochrome, in `text`/`muted` (e.g. settings `F0493`, close `F0156`,
plus `F0415`, history `F02DA`, info `F02FD`, folder `F0256`, folder-open `F0770`, file `F0214`, account `F0004`,
search `F0349`, check `F012C`, chevrons `F0142`/`F0140`, alert `F0026`, bell `F009A`, star `F04CE`, terminal
`F018D`, copy `F018F`, open-in-new `F03CC`, play `F040A`, pause `F03E4`, image `F02E9`, video `F0567`, branch
`E725`).

## Signature pieces

- **Session attention.** Unread input requests use `warn`, unread finishes use `good`: a 4–6% surface mix on
  session title bars, a 7–8% wash in lists, and a static alert/check with a short header label. Session content
  keeps its normal surface and colours. No pulse or accent stripe. Keyboard focus keeps its existing outline
  and project colour. Checking a session removes the status
  wash; a quiet check in the list means the latest event was read. An unresolved prompt remains available.
  The same event identity and acknowledgement apply on mobile. Settings can hide the treatment while receipts
  continue to work.

- **Content layer.** The sessions sit on `t.layer`, a step up from the window, with an 8px top-left corner where
  it meets the sidebar and the tab row, as WinUI's navigation frame.
- **Tabs (project tab row).** WinUI's TabView: tabs on the window colour; the selected tab is filled with the
  content layer's colour, its top corners 8px, joined to the content below; others are text with a hover fill
  and hairline separators between them; × on hover and on the selected tab. A project's tab is its name in its
  colour and its session count, which spins while one of its agents works. The permanent Orchestrators tab has
  a gold star and tiles every coordinator, with their combined count and attention. A project tab includes its
  own coordinator alongside its sessions.
- **Sidebar.** WinUI's navigation pane: rows 32–36 high, 4px corners, hover and selection fills; projects are
  their names in their colours over the path and branch; sessions are logo, name, quiet metadata; usage as
  segments. A project's coordinator leads its session list with a gold star. The root Orchestrator stays
  pinned above the scrolling project groups.
- **Key tips.** Holding alt shows WinUI key tips: small raised boxes (4px corners, stroke, soft shadow) with the
  keys, placed at the bottom centre of what they reach.
- **Address bar.** The launcher's project is an address bar across the top: folder icon, the path as segments,
  a chevron; clicking it picks another folder, including for an orchestrator. A standard Orchestrator button
  beside the bar selects cross-project coordination; a gold outline marks this selection. Choosing a folder
  selects that project. If its coordinator is already running, the primary action is Go to the orchestrator.

## Motion

- Hover and selection fades: `ui.hover_amount(key, r)` (110 ms).
- Dialogs: fade the scrim and lift the card 8 px over ~140 ms.
- Spinners and pulses set `ui.ticking`, so the app draws only while something moves.

## Copy

Reuse the terminal UI's words for every label, help line, empty state and toast (parity). When a sentence is
about cells, characters or terminal keys that don't apply in a window, adapt it minimally. No exclamation marks,
no "Please", no explaining what the UI obviously does.
