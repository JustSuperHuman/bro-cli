# The desktop app

`bro` opens your workspace in its own window, on Windows, macOS and Linux. It's the same bro as `bro-cli` (the
terminal UI): the same sessions, sidebar, launcher, views, keys and background services, with the terminals drawn
on the GPU and a few things only a window can do.

```
bro                open it here (this folder becomes the current project)
bro F:\code\app    open it with that folder
bro --new          a second bro, even when one is running
bro --demo         realistic fake data, touching no accounts or settings
bro gui            the window even where bro would pick the terminal UI (over SSH, no display)
```

On Windows the Start menu's **bro** (`bro-app.exe`) opens it without a console window: pin it to the taskbar.
`bro` typed in a terminal hands the window to its own process, so your shell gets its prompt back.

If bro is already running, `bro` opens the folder there instead and brings that window forward. With `--new`
beside a running bro, the new window leaves the orchestrator, the phone bridge and voice to the first one.
When a launch needs translation, the new window starts its own proxy on an unused loopback port and waits
until it is ready. This also handles launches made while the proxy is starting.

## What's different from the terminal

- **Terminals.** Each session is drawn from an alacritty_terminal screen: crisp text in your font (with its
  ligatures, so Fira Code's `->` and `!=` join up), box drawing that joins between cells, colour emoji, 10,000 lines
  of history under the wheel, and selection by drag, double-click (a word) or triple-click (a line). Selections copy when you let go, as in the
  terminal UI. Programs that ask for the kitty keyboard protocol get it, so shift+enter, ctrl+enter and friends
  reach Claude Code and Codex as themselves.
- **Photos and videos.** Pictures and videos your agents open (`bro open`), send you, make, look at or print the
  path of play in the media panel beside your sessions instead of another app: zoom pictures with the wheel,
  drag to pan, play, scrub, loop and unmute videos, then copy the path, @mention it into the session,
  show it in its folder or open it in its own app. The gallery shows the session you're in, or all of them.
  Dropped files and alt+v pastes join it too. Programs and scripts are never run from a click.
  Double-click a recent thumbnail or the preview to fill Bro's window with that media. Press **Esc** or
  double-click again to return to the panel.
- **The mouse everywhere.** Every row, tab, button and option the terminal UI has is clickable, with hover states;
  drag the sidebar's edge, split dividers and tile edges; scroll anything with the wheel.
- **Themes in full.** A VS Code theme looks like itself: its side bar colour around your sessions, its editor
  colour under them, its cursor. SynthWave '84's neon glows, and so does the neon of outrun, vaporwave, cyber and
  nebula.
- **A see-through window.** Settings → Appearance → **Window**: Acrylic frosts the desktop behind bro, Mica washes
  in your wallpaper's colours (both Windows 11), Glass leaves it clear. **Window tint** sets how much of the theme
  covers it, the same everywhere: title bar, sidebar and sessions are one sheet of glass (on Windows bro draws its
  own title bar for it, with Snap Layouts on maximise), while text, buttons and popups stay solid. The palette has
  them too (`window acrylic`). On macOS and KDE the desktop behind is blurred.

## Keys only the window has

| Keys | Does |
|---|---|
| `alt+m` | show or hide the media panel (when you haven't bound alt+m to something else) |
| `ctrl+=` / `ctrl+-` / `ctrl+0` | terminal font size bigger, smaller, back to 14 |
| `ctrl+shift+c` (or `ctrl+c` with a selection) | copy the selection |
| `F11` | full screen |
| mouse back / forward | the history of what you looked at (as in the terminal UI) |

With the media panel focused (click it): `←` `→` previous and next, `space` play or pause, `m` mute, `l` loop,
`+` `-` `0` zoom, `c` copy the path, `@` mention it, `o` open it in its app, `del` take it off the list, `esc`
back to your sessions.

Everything else is the terminal UI's keymap, including your own bindings from `~/.bro/v2.toml`; F1 lists them.

## Settings

The window keeps its size, place, panel widths and font size in `~/.bro/v2-gui.json`. Everything else, from the
theme to the terminal font (Settings → Appearance → Font), is shared with the terminal UI in `~/.bro/v2.toml`, so
both look and behave the same; the window's material (`material`, `material_tint`) lives there too. Custom themes
in `~/.bro/themes/` restyle the window too, live, and can set its side bar (`panel`) and cursor (`cursor`).

Text follows your system: on Windows with ClearType on it's smoothed for LCD screens, as other Windows apps are.
Set `BRO_TEXT_AA=grayscale` (or `rgb`, `bgr`) to choose yourself. Over a see-through window text is always
greyscale: colour fringes would show against what's behind.

## For contributors

How it's built: [gui/ARCHITECTURE.md](gui/ARCHITECTURE.md). The design system: [gui/DESIGN.md](gui/DESIGN.md).
Feature parity with the terminal UI, item by item: [gui/PARITY.md](gui/PARITY.md).

Scripted runs for screenshots and checks (demo mode, safe):

```
BRO_GUI_SCRIPT="wait:2500;key:alt+n;wait:600;shot:F:/tmp/launcher.png;exit" bro gui --gui-child --demo
```

Steps: `wait:<ms>`, `key:<chord>`, `type:<text>`, `alt:down|up`, `move:x,y`, `click:x,y`, `dclick:x,y`,
`rclick:x,y`, `wheel:x,y,lines`, `theme:<name>`, `material:<solid|acrylic|mica|glass>[,<tint %>]`,
`shot:<file.png>`, `exit`. Themes and materials set this way aren't saved. A shot of a see-through window keeps
its alpha; what the system draws behind it isn't in the picture.
