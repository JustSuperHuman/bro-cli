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
  Ctrl+Shift+Up/Down scrolls the active session in its terminal or Focus view. Full-screen programs that handle
  the mouse receive a wheel step; ordinary terminal history and Focus move one line. Shift+Up/Down stays
  available to the running program, including Codex's reasoning controls. Wheel notches move two
  terminal rows or one conversation line; touchpads keep their pixel movement, with small movements accumulated
  until the terminal can move a row.
- **Photos and videos.** Pictures and videos your agents open (`bro open`), send you, make, look at or print the
  path of play in the media panel beside your sessions instead of another app: zoom pictures with the wheel,
  drag to pan, play, scrub, loop and unmute videos, then copy it (a still picture as the picture, to paste into
  a chat, a document or an editor; a video, a song, a GIF or a PDF as the file, as Explorer or Finder copies it),
  copy its path (click the path under its name), @mention it into the session, show it in its folder or open it
  in its own app. The gallery shows the session you're in, or all of them.
  Dropped files and alt+v pastes join it too. Programs and scripts are never run from a click.
  Double-click a recent thumbnail or the preview to fill Bro's window with that media. Press **Esc** or
  double-click again to return to the panel.
  Right-click a thumbnail or the preview for its menu: everything above, plus play, mute, loop, fit or actual size
  and filling the window, each with its key.
- **HTML as the page.** An HTML file you click or an agent opens previews as the page itself, styles, scripts,
  pictures and all, in the system's web view (WebView2 on Windows, WebKit on macOS). The scroll keys scroll it,
  `m` swaps to the source and back, `o` opens it in your browser. Click into the page to use it; **Esc** still
  closes. Web links open in your browser. Linux shows the source.
- **The mouse everywhere.** Every row, tab, button and option the terminal UI has is clickable, with hover states;
  drag the sidebar's edge, split dividers and tile edges; scroll anything with the wheel.
- **No title bar** (Windows). The sidebar runs to the top; minimise, maximise and close sit at the end of the tab
  row. Drag the window by the sidebar's header or any empty part of that row; Snap Layouts still open over maximise.
- **Themes in full.** A VS Code theme looks like itself: its side bar colour around your sessions, its editor
  colour under them, its cursor. SynthWave '84's neon glows, and so does the neon of outrun, vaporwave, cyber and
  nebula.
- **A see-through window.** Settings → Appearance → **Window**: Acrylic frosts the desktop behind bro (Windows 11),
  Glass leaves it clear. **Window tint** (shown once the window isn't solid) sets how much of the theme
  covers it, the same everywhere: sidebar, tabs and sessions are one sheet of glass, while text, buttons and popups stay solid. The palette has
  them too (`window acrylic`). On macOS and KDE the desktop behind is blurred.

## Focus mode

`alt+f` (or **focus mode** in the palette) shows every Claude Code and Codex session as its conversation instead
of its terminal, in big, clean type across the whole card. You see what you said, what the agent said, and what
it did in between. Each run of steps is one quiet line ("Read 4 files · Changed 2 files · Ran a command", with the
lines added and removed), led by the skill it used when it used one ("Generate voice · Ran 2 commands"); click it
to see the steps, each with an icon for what it is. Steps read as what they did: a skill by its name, an MCP
server's tool by what it did and to what ("Navigate · figma.com", with the server beside it), and the usual
commands by what they do ("Tested bro-proxy", "Pushed fix/timer-clock", "Committed “fix the timer”") when the agent
didn't describe them itself. The folder the session works in and its git branch head the
conversation. The card's header says what the agent is doing: the step it's running and for how long, or how its
thinking is going ("Thinking", "Still thinking", "Deep in thought · 1m 10s", or Codex's own heading for it), "Needs
you" when it waits on you, "Done" when a turn ends.

A new session, before anything's been said, shows its agent's logo in pixels: Claude's spark or Codex's knot. The
pixels pop in from the middle as the session opens and a shine crosses them. The shine comes round again while the
agent gets ready and whenever the pointer moves onto the logo, and the rest of the time it holds still, so an
empty session doesn't keep the window redrawing. Over a see-through window (Acrylic, Mica, Glass) the pixels are
solid, and the glow under them while they move tints the glass without making it any more or less see-through.

- **Replying.** Type in the box under the conversation and press Enter (shift+Enter for a new line). Your message
  shows at once, marked as sending, until the agent has it. Sent while it works, it's queued and keeps its place
  in the conversation; Claude Code reads a queued message at its next step, and it shows where Claude read it.
  Up on an empty box brings back what you sent before. Pastes and dropped files land in the box.
- **Pictures.** Paste a picture (ctrl+v or alt+v, a screenshot or a copied file) or drop one on the window: it
  waits in the box as a thumbnail the moment you paste it, and goes with your message where you pasted it, held
  there by a mark you don't see. Delete the text around it and the picture stays (it moves to the end); hover
  its thumbnail for a bigger look, **Open** and **Remove**. A pasted path to a picture becomes the picture.
  Pictures in your messages, and the ones the agent looks at or sends you, show as thumbnails too; click one to
  see it in the media panel.
- **Sounds.** When the agent makes a sound file (a voice line, a song) or sends you one, it plays right there: a
  card under the reply that gives its full path, or the step that wrote or sent it, with a play button, the sound's
  own waveform and how long it runs. Click the waveform to play from that point. One sound plays at a time. The
  phone app shows the same card and streams the sound from this computer.
- **Apps.** When Claude uses a tool that comes with its own screen (an MCP App, such as the workout card from
  JustGains' `generate_workout`), that screen shows in the conversation, under the step, as it would on claude.ai
  or in ChatGPT. It's the app itself, live: it scrolls with the conversation, takes clicks and typing, opens its
  links in your browser, and can call its own tools or send Claude a message for you. It matches bro's light or
  dark theme. Nothing changes on the app's side: bro speaks the MCP Apps protocol (and the Apps SDK's
  `window.openai`), and reads the app's page through the session's [bro mod](claude-mod.md), over Claude Code's
  own connection, so claude.ai connectors and servers that need a login just work. The mod has to be on (Settings
  › bro mod) when the session starts. A page read once is kept in `~/.bro/mcp-apps/` and opens at once after
  that, even when its session has ended. The wheel scrolls the conversation unless the app can scroll itself; Esc
  in an app hands the keys back. bro finds an app from the tool's result, so a server has to name its page there
  (`_meta.ui.resourceUri` or `openai/outputTemplate`). One that names it only in its tool list shows as a step,
  as every app does on Linux. The phone app shows the same card in its conversation: bro serves it the app's page
  (`/api/sessions/{id}/apps/{call}`) and passes what the app asks of its server to the session's mod.
- **Model and thinking.** The box's footer says what the agent runs on and how hard it thinks ("Opus 5.5 ·
  thinking high"). Click either for the list to switch from: the models with what each is for, the thinking
  levels the model has. The switch shows at once. Claude Code takes it there and then (`/model`, `/effort`);
  Codex takes it when its turn ends (`/model <model> --reasoning=<level>`, Codex 0.165 and later; an older Codex
  shows its own menu in the terminal). Sessions through bro's proxy switch the same way: proxy mode and the Claude
  pool keep the agent's own models, a session on your other family's logins (Claude Code on your ChatGPT ones) gets
  theirs, and one on an API provider gets that provider's models; type to search a long list.
- **Commands.** Type `/` for the commands the agent takes: its own, the project's and yours (`.claude/commands`,
  Codex prompts) and, for Claude, its skills, best match first as you type. Up and down choose, Tab completes,
  Enter runs (one that needs more after it waits for that), Esc closes the list. A command goes in key by key,
  so the agent's own command menu takes it. One that opens a screen of its own (`/config`, `/resume`, `/model`
  with nothing after it…) shows that session's terminal while you use it.
- **Links.** Web addresses and file paths in the conversation open as they do in the terminal: a file in the
  preview (a picture in the media panel), at its line when it says one, the rest in your browser. A step about a
  file, and the folder at the top, open the same way.
- **Questions and permissions.** When the agent asks (a permission prompt, a question with options), the choice
  shows above the box with its options as buttons. Click one, press its number, or move with the arrows and press
  Enter, as in the terminal; Esc cancels. What you've written in the box never goes into a choice.
- **Stopping.** Esc with an empty box stops a working agent, as in the terminal.
- **The terminal is still there.** The session keeps running at its card's size. The card's header has a terminal
  button. Pressing `alt+f` on a session that shows its terminal brings its conversation back; on one that shows
  its conversation, it turns focus mode off. Shells, Pi and omp keep their terminals.
- Copy a reply with the button at its corner.

Focus mode is remembered in `~/.bro/v2-gui.json`. The terminal UI doesn't have it.

## Keys only the window has

| Keys | Does |
|---|---|
| `alt+f` | focus mode: agent sessions as their conversation, with a box to reply in (again: the terminals) |
| `alt+m` | show or hide the media panel (when you haven't bound alt+m to something else) |
| `ctrl+shift+↑` / `ctrl+shift+↓` | scroll the active session up or down (one line, or a wheel step for programs handling scrolling) |
| `ctrl+=` / `ctrl+-` / `ctrl+0` | terminal font size bigger, smaller, back to 14 |
| `ctrl+shift+c` (or `ctrl+c` with a selection) | copy the selection |
| `F11` | full screen |
| mouse back / forward | the history of what you looked at (as in the terminal UI) |

With the media panel focused (click it): `←` `→` previous and next, `space` play or pause, `m` mute, `l` loop,
`+` `-` `0` zoom, `c` (or ctrl+c) copy it, `shift+c` copy the path, `@` mention it, `o` open it in its app, `del` take it off the list,
`shift+F10` its menu (arrows, `enter`, `esc` in it), `esc` back to your sessions.

Everything else is the terminal UI's keymap, including your own bindings from `~/.bro/v2.toml`; F1 lists them.

## Settings

Session status indicators use a faint amber wash for an unread input request and green for an unread finish.
The header and sidebar use an alert or check icon as well, so colour is not the only signal. Focusing a
session marks that event checked and removes its wash; merely tiling a session beside another does not.
A checked input request still shows its prompt. New work clears the old event, and a later finish or question
can ask for attention again. Provider limits read as paused, rather than asking for an answer.

Desktop Settings → Sessions → Session status indicators turns the treatment off. Mobile has the same switch
under Chats → More → Settings. The desktop switch applies to its connected clients too; the mobile switch can
hide the treatment just on that phone. Neither switch resets checked events. Mobile foreground chat focus
acknowledges the same event on the desktop, with its identity included so a delayed receipt cannot clear a
newer question or finish.

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

Steps: `wait:<ms>`, `key:<chord>`, `type:<text>`, `paste:<text>`, `drop:<file>`, `alt:down|up`, `move:x,y`, `click:x,y`, `dclick:x,y`,
`rclick:x,y`, `wheel:x,y,lines`, `theme:<name>`, `material:<solid|acrylic|glass>[,<tint %>]`,
`shot:<file.png>`, `exit`. Themes and materials set this way aren't saved. A shot of a see-through window keeps
its alpha; what the system draws behind it isn't in the picture.
