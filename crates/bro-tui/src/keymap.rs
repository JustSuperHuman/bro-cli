//! The central keymap: every app action (`Act`) with its default chords — direct alt-bindings that never clash
//! with Claude Code / Codex (they own ctrl+*, shift+tab, esc) and a tmux-style prefix table (ctrl+space) — plus
//! user overrides from `Settings.keys` (`action = "alt+m"`, `"prefix m"`, or `"none"`).
//!
//! The palette and the help overlay are generated from this, so they always show the live bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

/// An app action a key can trigger.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Act {
    NewSession,
    /// A new agent in your home folder on your last-used harness and login: quick system tasks.
    QuickAgent,
    NewShell,
    Split,
    SplitRight,
    SplitDown,
    Close,
    Zoom,
    /// Grow the focused session to a working size (neighbours give way); again: snap back.
    Grow,
    /// Snap a grown session back to its place in the layout.
    Shrink,
    TileAll,
    /// Turn the focused session's card over: what it's working on; again: face up.
    Flip,
    /// Turn every session on screen over, or all face up again.
    FlipAll,
    /// Focus mode (the desktop app): agent sessions as their conversation in big clean type, with a reply box,
    /// instead of their terminal; again: the terminals.
    Focus,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    /// Jump to live session N (1-based) in sidebar order.
    Jump(u8),
    NextSession,
    PrevSession,
    NextProject,
    PrevProject,
    NextTab,
    PrevTab,
    /// Show a project tab: 0 is All (every session tiled), 1–9 the projects in tab order.
    ShowTab(u8),
    /// Back to the project tab you were on before this one.
    LastTab,
    /// The project tabs over the sessions, on / off.
    ToggleProjectTabs,
    Attention,
    ToggleSidebar,
    FocusSidebar,
    Palette,
    Usage,
    Profiles,
    Proxy,
    Bridge,
    /// Token totals per login (the ⓘ on the sidebar's usage).
    Stats,
    /// Claude Code privacy / update / behaviour presets.
    Settings,
    /// Quick Install: JustTools, Git, Node.js, .NET, the agents… as a checklist.
    QuickInstall,
    /// Sounds: PeonPing voice lines when sessions start, finish or need you, and the pack browser.
    Sounds,
    /// Skills: the Skill Issue marketplace to search and install agent skills from.
    Skills,
    /// Local prompt shortcuts in the desktop conversation view.
    Quips,
    /// The setup guide: install an agent, add a login, launch.
    Setup,
    Help,
    Rename,
    Themes,
    ToggleIcons,
    /// Request compression (llmtrim) in bro-proxy on/off.
    ToggleTrim,
    /// Proxy mode on/off: new sessions share every signed-in login of their agent.
    ToggleProxyMode,
    /// Retry agents that stop on an API error, on / off.
    ToggleAutoResume,
    /// Caveman mode (terse replies) for new sessions, on / off.
    ToggleCaveman,
    RefreshUsage,
    UsageDetails,
    Continue,
    Reload,
    /// Go to bro's orchestrator, or start one (the launcher on its tab).
    Orchestrator,
    SwitchLogin,
    CloseProject,
    /// Open the folder whose path is on the clipboard as a project.
    OpenClipboardFolder,
    /// Show the file an agent last opened (`bro open`).
    OpenedFile,
    /// What the focused agent sent you and the files it touched.
    AgentFiles,
    /// The agent team the focused Claude session leads: teammates and their shared task list.
    Team,
    /// Start or look at the focused session's watchdog (milestones, a reviewer that hands it its next task).
    Watchdog,
    /// Edit the instructions every watchdog reviewer starts with.
    WatchdogPrompt,
    /// The desktop app's media panel (the pictures and videos beside the sessions), show / hide.
    Media,
    /// Replace bro's session server with a fresh one (its sessions end; agents come back resumed).
    RestartServer,
    Quit,
}

/// Help/palette grouping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Sessions,
    Panes,
    Navigate,
    Views,
    App,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Sessions, Group::Navigate, Group::Panes, Group::Views, Group::App];
    pub fn label(self) -> &'static str {
        match self {
            Group::Sessions => "sessions",
            Group::Panes => "panes",
            Group::Navigate => "navigate",
            Group::Views => "views",
            Group::App => "app",
        }
    }
}

impl Act {
    /// Every action, in help order.
    pub fn all() -> Vec<Act> {
        use Act::*;
        let mut v = vec![NewSession, QuickAgent, NewShell, Reload, Rename, Close, Attention, CloseProject, Continue, Orchestrator, SwitchLogin, OpenClipboardFolder, OpenedFile, AgentFiles, Team, Watchdog, NextSession, PrevSession, NextProject, PrevProject];
        v.extend((1..=9).map(Jump));
        v.extend((0..=9).map(ShowTab));
        v.extend([LastTab, NextTab, PrevTab, FocusSidebar, ToggleSidebar]);
        v.extend([Split, SplitRight, SplitDown, TileAll, Zoom, Grow, Shrink, Flip, FlipAll, Focus, FocusLeft, FocusRight, FocusUp, FocusDown, ResizeLeft, ResizeRight, ResizeUp, ResizeDown]);
        v.extend([Usage, UsageDetails, Stats, Profiles, Proxy, ToggleProxyMode, ToggleTrim, Bridge, Settings, WatchdogPrompt, QuickInstall, Sounds, Quips, Skills, Setup, RefreshUsage, Media]);
        v.extend([Palette, Help, Themes, ToggleIcons, ToggleProjectTabs, ToggleAutoResume, ToggleCaveman, RestartServer, Quit]);
        v
    }

    /// Stable name used in `Settings.keys`.
    pub fn name(self) -> String {
        use Act::*;
        match self {
            Jump(n) => format!("jump_{n}"),
            ShowTab(0) => "tab_all".into(),
            ShowTab(n) => format!("tab_{n}"),
            LastTab => "last_tab".into(),
            ToggleProjectTabs => "toggle_project_tabs".into(),
            NewSession => "new_session".into(),
            QuickAgent => "quick_agent".into(),
            NewShell => "new_shell".into(),
            Split => "split".into(),
            SplitRight => "split_right".into(),
            SplitDown => "split_down".into(),
            Close => "close".into(),
            Zoom => "zoom".into(),
            Grow => "grow".into(),
            Shrink => "shrink".into(),
            TileAll => "tile_all".into(),
            Flip => "flip".into(),
            FlipAll => "flip_all".into(),
            Focus => "focus_mode".into(),
            FocusLeft => "focus_left".into(),
            FocusRight => "focus_right".into(),
            FocusUp => "focus_up".into(),
            FocusDown => "focus_down".into(),
            ResizeLeft => "resize_left".into(),
            ResizeRight => "resize_right".into(),
            ResizeUp => "resize_up".into(),
            ResizeDown => "resize_down".into(),
            NextSession => "next_session".into(),
            PrevSession => "prev_session".into(),
            NextProject => "next_project".into(),
            PrevProject => "prev_project".into(),
            NextTab => "next_tab".into(),
            PrevTab => "prev_tab".into(),
            Attention => "attention".into(),
            ToggleSidebar => "toggle_sidebar".into(),
            FocusSidebar => "focus_sidebar".into(),
            Palette => "palette".into(),
            Usage => "usage".into(),
            Profiles => "profiles".into(),
            Proxy => "proxy".into(),
            Bridge => "bridge".into(),
            Stats => "stats".into(),
            Settings => "settings".into(),
            QuickInstall => "quick_install".into(),
            Sounds => "sounds".into(),
            Skills => "skills".into(),
            Quips => "quips".into(),
            Setup => "setup".into(),
            Help => "help".into(),
            Rename => "rename".into(),
            Themes => "themes".into(),
            ToggleIcons => "toggle_icons".into(),
            ToggleTrim => "toggle_trim".into(),
            ToggleProxyMode => "toggle_proxy_mode".into(),
            ToggleAutoResume => "toggle_auto_resume".into(),
            ToggleCaveman => "toggle_caveman".into(),
            RefreshUsage => "refresh_usage".into(),
            UsageDetails => "usage_details".into(),
            Continue => "continue".into(),
            Reload => "reload".into(),
            Orchestrator => "orchestrator".into(),
            SwitchLogin => "switch_login".into(),
            CloseProject => "close_project".into(),
            OpenClipboardFolder => "open_clipboard_folder".into(),
            OpenedFile => "opened_file".into(),
            AgentFiles => "agent_files".into(),
            Team => "agent_team".into(),
            Watchdog => "watchdog".into(),
            WatchdogPrompt => "watchdog_prompt".into(),
            Media => "media".into(),
            RestartServer => "restart_server".into(),
            Quit => "quit".into(),
        }
    }

    pub fn from_name(s: &str) -> Option<Act> {
        let s = s.trim().to_lowercase().replace('-', "_");
        Act::all().into_iter().find(|a| a.name() == s)
    }

    /// One-line description for the palette and help.
    pub fn describe(self) -> String {
        use Act::*;
        match self {
            Jump(n) => format!("jump to live session {n}"),
            ShowTab(0) => "project tabs: All — every session tiled".into(),
            ShowTab(n) => format!("project tabs: project {n} — its sessions tiled"),
            LastTab => "project tabs: back to the tab you were on before".into(),
            ToggleProjectTabs => "project tabs over the sessions on / off".into(),
            NewSession => "launch an agent session…".into(),
            QuickAgent => "quick agent in your home folder on your last login, for system tasks".into(),
            NewShell => "open a plain shell tab".into(),
            Split => "launch into a split beside this pane…".into(),
            SplitRight => "split right with a shell".into(),
            SplitDown => "split down with a shell".into(),
            Close => "close the focused pane".into(),
            Zoom => "zoom / unzoom the focused pane".into(),
            Grow => "grow the focused session to a working size (neighbours give way) / snap back".into(),
            Shrink => "snap a grown session back".into(),
            TileAll => "tile all sessions / return to the focused session".into(),
            Flip => "turn the focused session over: its task, plan, latest word and files / face up".into(),
            FlipAll => "turn every session on screen over / all face up".into(),
            Focus => "focus mode: agent sessions as their conversation in big type, with a reply box (desktop app) / the terminals".into(),
            FocusLeft => "focus the pane to the left".into(),
            FocusRight => "focus the pane to the right".into(),
            FocusUp => "focus the pane above".into(),
            FocusDown => "focus the pane below".into(),
            ResizeLeft => "move the divider left".into(),
            ResizeRight => "move the divider right".into(),
            ResizeUp => "move the divider up".into(),
            ResizeDown => "move the divider down".into(),
            NextSession => "next session".into(),
            PrevSession => "previous session".into(),
            NextProject => "next project (in a grid: the next project tab)".into(),
            PrevProject => "previous project (in a grid: the previous project tab)".into(),
            NextTab => "next layout (a session opened in its own tab, with its splits)".into(),
            PrevTab => "previous layout".into(),
            Attention => "jump to the next session that needs you (again: the one after)".into(),
            ToggleSidebar => "show / hide the sidebar".into(),
            FocusSidebar => "focus the sidebar (type to filter, arrows to move, enter to open)".into(),
            Palette => "command palette".into(),
            Usage => "usage: meters for every profile".into(),
            Profiles => "profiles: accounts and logins".into(),
            Proxy => "proxy: routes and live requests".into(),
            Bridge => "bridge: phone / web pairing".into(),
            Stats => "stats: tokens per login — lifetime, last 24h / 7d / 30d".into(),
            Settings => "settings: bro options and Claude Code privacy, updates and long-run presets".into(),
            QuickInstall => "quick install: JustTools, Git, Node.js, Bun, Python, .NET and the coding agents".into(),
            Sounds => "sounds: PeonPing voice lines when a session starts, finishes or needs you · browse sound packs".into(),
            Skills => "skills: every skill your agents have, by source and by project; watch repos; find more on Skill Issue".into(),
            Quips => "Quips: quick instructions for your prompt; browse, create, and edit local shortcuts".into(),
            Setup => "setup guide: install Claude Code / Codex, add your logins, launch".into(),
            Help => "keys and help".into(),
            Rename => "rename the focused session".into(),
            Themes => "pick a theme (live preview)".into(),
            ToggleIcons => "toggle nerd font icons".into(),
            ToggleTrim => "compress requests (llmtrim) on / off".into(),
            ToggleProxyMode => "proxy mode on / off: new sessions share all your logins".into(),
            ToggleAutoResume => "auto-resume agents after API errors on / off".into(),
            ToggleCaveman => "caveman mode on / off: terse replies in new sessions".into(),
            RefreshUsage => "refresh usage now".into(),
            UsageDetails => "sidebar usage: totals / every profile".into(),
            Continue => "continue an earlier session (search, resume in any login)".into(),
            Reload => "reload session with a profile…".into(),
            SwitchLogin => "move this session to another login (keeps the conversation)".into(),
            Orchestrator => "go to or start this project's orchestrator".into(),
            CloseProject => "close the current project (and its sessions)".into(),
            OpenClipboardFolder => "open the folder whose path you copied as a project".into(),
            OpenedFile => "show the file an agent last opened (bro open)".into(),
            AgentFiles => "agent files: what it sent you and the files it touched".into(),
            Team => "agent team: this session's teammates, what each is doing and their task list".into(),
            Watchdog => "watchdog: keep this session working through your milestones while you're away".into(),
            WatchdogPrompt => "watchdog prompt: edit what every reviewer is told".into(),
            Media => "media panel: the pictures and videos your agents open, send or make (desktop app)".into(),
            RestartServer => "session server: restart it (its sessions end; Claude and Codex come back resumed)".into(),
            Quit => "quit bro".into(),
        }
    }

    pub fn group(self) -> Group {
        use Act::*;
        match self {
            NewSession | QuickAgent | NewShell | Rename | Close | Attention | Continue | Reload | Orchestrator | SwitchLogin | CloseProject | OpenClipboardFolder | OpenedFile | AgentFiles | Team | Watchdog => Group::Sessions,
            NextSession | PrevSession | NextProject | PrevProject | Jump(_) | ShowTab(_) | LastTab | NextTab | PrevTab | FocusSidebar | ToggleSidebar => Group::Navigate,
            Split | SplitRight | SplitDown | TileAll | Zoom | Grow | Shrink | Flip | FlipAll | Focus | FocusLeft | FocusRight | FocusUp | FocusDown | ResizeLeft | ResizeRight | ResizeUp | ResizeDown => Group::Panes,
            Usage | UsageDetails | Stats | Profiles | Proxy | ToggleProxyMode | ToggleTrim | Bridge | Settings | WatchdogPrompt | QuickInstall | Sounds | Quips | Skills | Setup | RefreshUsage | Media => Group::Views,
            Palette | Help | Themes | ToggleIcons | ToggleProjectTabs | ToggleAutoResume | ToggleCaveman | RestartServer | Quit => Group::App,
        }
    }

    /// Default direct chords.
    fn default_direct(self) -> Vec<&'static str> {
        use Act::*;
        match self {
            NewSession => vec!["alt+n"],
            QuickAgent => vec!["alt+q"],
            NewShell => vec!["alt+t"],
            Split => vec!["alt+enter", "alt+\\"],
            Close => vec!["ctrl+w", "alt+w"],
            Zoom => vec!["alt+z"],
            Grow => vec!["ctrl+up", "alt+="],
            Shrink => vec!["ctrl+down", "alt+-"],
            TileAll => vec!["alt+shift+t"],
            Flip => vec!["alt+i"],
            FlipAll => vec!["alt+I"],
            Focus => vec!["alt+f"],
            FocusLeft => vec!["alt+left"],
            FocusRight => vec!["alt+right"],
            FocusUp => vec!["alt+up"],
            FocusDown => vec!["alt+down"],
            ResizeLeft => vec!["alt+shift+left"],
            ResizeRight => vec!["alt+shift+right"],
            ResizeUp => vec!["alt+shift+up"],
            ResizeDown => vec!["alt+shift+down"],
            Jump(n) => vec![["alt+1", "alt+2", "alt+3", "alt+4", "alt+5", "alt+6", "alt+7", "alt+8", "alt+9"][(n.clamp(1, 9) - 1) as usize]],
            // ctrl+tab reaches bro once the terminal doesn't keep it for its own tabs
            NextSession => vec!["alt+j", "ctrl+tab"],
            PrevSession => vec!["alt+k", "ctrl+shift+tab"],
            NextProject => vec!["alt+J"],
            PrevProject => vec!["alt+K"],
            Attention => vec!["alt+a"],
            ShowTab(0) => vec!["alt+0"],
            LastTab => vec!["alt+l"],
            ToggleSidebar => vec!["alt+s"],
            FocusSidebar => vec!["alt+b"],
            Palette => vec!["alt+p"],
            Usage => vec!["alt+u"],
            UsageDetails => vec!["alt+U"],
            SwitchLogin => vec!["alt+L"],
            Continue => vec!["alt+r"],
            Orchestrator => vec!["alt+O"],
            OpenClipboardFolder => vec!["ctrl+shift+o"],
            Profiles => vec!["alt+o"],
            Proxy => vec!["alt+y"],
            Bridge => vec!["alt+g"],
            Settings => vec!["alt+,"],
            Media => vec!["alt+m"],
            Help => vec!["f1"],
            _ => vec![],
        }
    }

    /// Default keys after the prefix.
    fn default_prefix(self) -> Vec<&'static str> {
        use Act::*;
        match self {
            NewSession => vec!["c"],
            QuickAgent => vec!["Q"],
            NewShell => vec!["!"],
            SplitRight => vec!["%", "|", "\\"],
            SplitDown => vec!["\"", "-"],
            Close => vec!["x"],
            Zoom => vec!["z"],
            Grow => vec!["="],
            Shrink => vec!["_"],
            TileAll => vec!["T"],
            Flip => vec!["d"],
            FlipAll => vec!["D"],
            FocusLeft => vec!["h", "left"],
            FocusRight => vec!["l", "right"],
            FocusUp => vec!["k"],
            FocusDown => vec!["j"],
            ResizeLeft => vec!["H"],
            ResizeRight => vec!["L"],
            ResizeUp => vec!["K"],
            ResizeDown => vec!["J"],
            Jump(n) => vec![["1", "2", "3", "4", "5", "6", "7", "8", "9"][(n.clamp(1, 9) - 1) as usize]],
            NextSession => vec!["n"],
            PrevSession => vec!["p"],
            NextProject => vec!["]"],
            PrevProject => vec!["["],
            NextTab => vec![")"],
            PrevTab => vec!["("],
            ShowTab(0) => vec!["0"],
            LastTab => vec!["tab"],
            Attention => vec!["a"],
            ToggleSidebar => vec!["s"],
            FocusSidebar => vec!["b"],
            Palette => vec![":", "space"],
            Usage => vec!["u"],
            UsageDetails => vec!["U"],
            Stats => vec!["S"],
            Settings => vec!["A"],
            Continue => vec!["C"],
            Reload => vec!["R"],
            Orchestrator => vec!["O"],
            SwitchLogin => vec!["P"],
            CloseProject => vec!["X"],
            OpenedFile => vec!["f"],
            AgentFiles => vec!["F"],
            Team => vec!["M"],
            Watchdog => vec!["w"],
            Profiles => vec!["o"],
            Proxy => vec!["y"],
            Bridge => vec!["g"],
            Help => vec!["?"],
            Rename => vec![","],
            Themes => vec!["t"],
            ToggleIcons => vec!["i"],
            RefreshUsage => vec!["r"],
            Quit => vec!["q"],
            QuickInstall => vec!["I"],
            Setup => vec!["e"],
            Media => vec!["m"],
            Split | Focus | ShowTab(_) | ToggleProjectTabs | ToggleProxyMode | ToggleTrim | ToggleAutoResume | ToggleCaveman | WatchdogPrompt | OpenClipboardFolder | Sounds | Quips | Skills | RestartServer => vec![],
        }
    }
}

/// A normalized key chord.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    /// Normalize so "alt+J" == alt+shift+j and "?" doesn't care about shift.
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Chord {
        let mut m = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                m |= KeyModifiers::SHIFT;
                KeyCode::Char(c.to_ascii_lowercase())
            }
            // some terminals report ctrl+space as ctrl+@
            KeyCode::Char('@') if m.contains(KeyModifiers::CONTROL) => KeyCode::Char(' '),
            KeyCode::Char(c) if !c.is_ascii_alphabetic() && c != ' ' => {
                m.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c)
            }
            KeyCode::BackTab => {
                m.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            // shift+tab arrives as Tab+SHIFT on some paths (Windows console input): same chord as BackTab
            KeyCode::Tab if m.contains(KeyModifiers::SHIFT) => {
                m.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            c => c,
        };
        Chord { code, mods: m }
    }

    pub fn from_event(k: &KeyEvent) -> Chord {
        Chord::new(k.code, k.modifiers)
    }

    /// Parse "alt+shift+left", "ctrl+space", "alt+J", "f1", "?", "enter".
    pub fn parse(s: &str) -> Result<Chord, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".into());
        }
        let mut mods = KeyModifiers::NONE;
        // split on '+', but a trailing "+" is the plus key itself
        let (head, key) = match s.strip_suffix("++") {
            Some(h) => (h, "+"),
            None => match s.rfind('+') {
                Some(i) if i + 1 < s.len() => (&s[..i], &s[i + 1..]),
                _ => ("", s),
            },
        };
        for m in head.split('+').filter(|m| !m.is_empty()) {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" | "opt" | "option" | "m" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier '{other}' in '{s}'")),
            }
        }
        let code = match key.to_lowercase().as_str() {
            "space" | "spc" => KeyCode::Char(' '),
            "enter" | "return" | "ret" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "esc" | "escape" => KeyCode::Esc,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => KeyCode::F(k[1..].parse().unwrap_or(1)),
            _ => {
                let mut cs = key.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return Err(format!("unknown key '{key}' in '{s}'")),
                }
            }
        };
        Ok(Chord::new(code, mods))
    }

    pub fn matches(&self, k: &KeyEvent) -> bool {
        *self == Chord::from_event(k)
    }

    /// Human form: "alt+n", "alt+J", "alt+shift+←", "ctrl+space", "F1".
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str("alt+");
        }
        let shift = self.mods.contains(KeyModifiers::SHIFT);
        let key = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) if shift && c.is_ascii_alphabetic() => return format!("{s}{}", c.to_ascii_uppercase()),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "del".into(),
            KeyCode::Insert => "ins".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::F(n) => format!("F{n}"),
            _ => "?".into(),
        };
        if shift {
            s.push_str("shift+");
        }
        s + &key
    }
}

/// The live key bindings.
#[derive(Clone, Debug)]
pub struct Keymap {
    pub prefix: Chord,
    direct: Vec<(Chord, Act)>,
    prefixed: Vec<(Chord, Act)>,
    /// Problems in the user's overrides, shown once as a toast.
    pub errors: Vec<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::new("ctrl+space", &BTreeMap::new())
    }
}

impl Keymap {
    /// Defaults, then `overrides` (action name → chord / "prefix <key>" / "none").
    pub fn new(prefix: &str, overrides: &BTreeMap<String, String>) -> Keymap {
        let mut errors = vec![];
        let prefix = match Chord::parse(if prefix.trim().is_empty() { "ctrl+space" } else { prefix }) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("prefix: {e}"));
                Chord::new(KeyCode::Char(' '), KeyModifiers::CONTROL)
            }
        };
        let mut direct = vec![];
        let mut prefixed = vec![];
        for a in Act::all() {
            for c in a.default_direct() {
                if let Ok(ch) = Chord::parse(c) {
                    direct.push((ch, a));
                }
            }
            for c in a.default_prefix() {
                if let Ok(ch) = Chord::parse(c) {
                    prefixed.push((ch, a));
                }
            }
        }
        let mut km = Keymap { prefix, direct, prefixed, errors };
        for (name, value) in overrides {
            let Some(act) = Act::from_name(name) else {
                km.errors.push(format!("keys: no action called '{name}'"));
                continue;
            };
            let v = value.trim();
            if v.eq_ignore_ascii_case("none") || v.is_empty() {
                km.direct.retain(|(_, a)| *a != act);
                km.prefixed.retain(|(_, a)| *a != act);
                continue;
            }
            let (table_is_prefix, chord) = match v.strip_prefix("prefix ").or_else(|| v.strip_prefix("prefix+")) {
                Some(rest) => (true, rest),
                None => (false, v),
            };
            match Chord::parse(chord) {
                Ok(c) => {
                    let table = if table_is_prefix { &mut km.prefixed } else { &mut km.direct };
                    // the override wins: take the chord from whoever had it, and replace this action's own
                    table.retain(|(ch, a)| *a != act && *ch != c);
                    table.push((c, act));
                }
                Err(e) => km.errors.push(format!("keys.{name}: {e}")),
            }
        }
        km
    }

    pub fn is_prefix(&self, k: &KeyEvent) -> bool {
        self.prefix.matches(k)
    }

    /// The action bound directly to this key, if any.
    pub fn direct(&self, k: &KeyEvent) -> Option<Act> {
        let c = Chord::from_event(k);
        self.direct.iter().find(|(ch, _)| *ch == c).map(|x| x.1)
    }

    /// The action bound to this key in the prefix table.
    pub fn after_prefix(&self, k: &KeyEvent) -> Option<Act> {
        let c = Chord::from_event(k);
        // prefix tables ignore shift on letters only when the plain letter isn't bound differently
        self.prefixed.iter().find(|(ch, _)| *ch == c).map(|x| x.1)
    }

    /// Every chord for `act`, direct first then "ctrl+space x" forms.
    pub fn chords_for(&self, act: Act) -> Vec<String> {
        let mut v: Vec<String> = self.direct.iter().filter(|(_, a)| *a == act).map(|(c, _)| c.display()).collect();
        let p = self.prefix.display();
        v.extend(self.prefixed.iter().filter(|(_, a)| *a == act).map(|(c, _)| format!("{p} {}", c.display())));
        v
    }

    /// The first (best) chord for `act`, for compact hints.
    pub fn primary(&self, act: Act) -> String {
        self.chords_for(act).into_iter().next().unwrap_or_default()
    }

    /// One menu entry per action, with its current prefix binding.
    pub fn prefix_actions(&self) -> Vec<(String, Act)> {
        Act::all().into_iter().filter_map(|a| {
            self.prefixed.iter().find(|(_, bound)| *bound == a).map(|(chord, _)| (chord.display(), a))
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn parse_chords() {
        assert_eq!(Chord::parse("alt+n").unwrap(), Chord::new(KeyCode::Char('n'), KeyModifiers::ALT));
        assert_eq!(Chord::parse("alt+J").unwrap(), Chord::parse("alt+shift+j").unwrap());
        assert_eq!(Chord::parse("ctrl+space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(Chord::parse("F1").unwrap().code, KeyCode::F(1));
        assert_eq!(Chord::parse("alt+shift+left").unwrap().mods, KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(Chord::parse("ctrl++").unwrap().code, KeyCode::Char('+'));
        assert_eq!(Chord::parse("?").unwrap(), Chord::new(KeyCode::Char('?'), KeyModifiers::SHIFT), "symbols ignore shift");
        assert!(Chord::parse("hyper+x").is_err());
        assert!(Chord::parse("alt+nope").is_err());
        assert_eq!(Chord::parse("alt+J").unwrap().display(), "alt+J");
        assert_eq!(Chord::parse("alt+shift+left").unwrap().display(), "alt+shift+←");
    }

    #[test]
    fn defaults_match_events() {
        let km = Keymap::default();
        assert_eq!(km.direct(&ev(KeyCode::Char('n'), KeyModifiers::ALT)), Some(Act::NewSession));
        // terminals differ: alt+shift+j arrives as 'J' with or without SHIFT
        assert_eq!(km.direct(&ev(KeyCode::Char('J'), KeyModifiers::ALT)), Some(Act::NextProject));
        assert_eq!(km.direct(&ev(KeyCode::Char('J'), KeyModifiers::ALT | KeyModifiers::SHIFT)), Some(Act::NextProject));
        assert_eq!(km.direct(&ev(KeyCode::Char('3'), KeyModifiers::ALT)), Some(Act::Jump(3)));
        assert_eq!(km.direct(&ev(KeyCode::Char('0'), KeyModifiers::ALT)), Some(Act::ShowTab(0)), "alt+0: the All tab");
        assert_eq!(km.direct(&ev(KeyCode::Char('l'), KeyModifiers::ALT)), Some(Act::LastTab));
        assert_eq!(km.after_prefix(&ev(KeyCode::Tab, KeyModifiers::NONE)), Some(Act::LastTab));
        assert_eq!(km.direct(&ev(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT)), Some(Act::ResizeLeft));
        assert_eq!(km.direct(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)), None, "ctrl keys belong to the agent");
        assert_eq!(km.direct(&ev(KeyCode::Char('w'), KeyModifiers::CONTROL)), Some(Act::Close));
        assert_eq!(km.direct(&ev(KeyCode::Char('w'), KeyModifiers::ALT)), Some(Act::Close));
        assert!(km.is_prefix(&ev(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(km.is_prefix(&ev(KeyCode::Char('@'), KeyModifiers::CONTROL)));
        assert_eq!(km.after_prefix(&ev(KeyCode::Char('%'), KeyModifiers::SHIFT)), Some(Act::SplitRight));
        assert_eq!(km.after_prefix(&ev(KeyCode::Char('H'), KeyModifiers::SHIFT)), Some(Act::ResizeLeft));
        assert_eq!(km.after_prefix(&ev(KeyCode::Char('h'), KeyModifiers::NONE)), Some(Act::FocusLeft));
        assert!(km.errors.is_empty());
    }

    #[test]
    fn overrides() {
        let mut o = BTreeMap::new();
        o.insert("palette".to_string(), "alt+k".to_string()); // steals alt+k from prev_session
        o.insert("usage".to_string(), "prefix m".to_string());
        o.insert("zoom".to_string(), "none".to_string());
        o.insert("bogus".to_string(), "alt+x".to_string());
        o.insert("proxy".to_string(), "alt+nope".to_string());
        let km = Keymap::new("ctrl+b", &o);
        assert_eq!(km.direct(&ev(KeyCode::Char('k'), KeyModifiers::ALT)), Some(Act::Palette));
        assert_eq!(km.direct(&ev(KeyCode::Char('p'), KeyModifiers::ALT)), None, "the old palette chord is gone");
        assert!(km.chords_for(Act::PrevSession).iter().all(|c| !c.contains("alt+k")));
        assert_eq!(km.after_prefix(&ev(KeyCode::Char('m'), KeyModifiers::NONE)), Some(Act::Usage));
        assert_eq!(km.direct(&ev(KeyCode::Char('u'), KeyModifiers::ALT)), Some(Act::Usage), "direct binding kept");
        assert_eq!(km.direct(&ev(KeyCode::Char('z'), KeyModifiers::ALT)), None);
        assert!(km.chords_for(Act::Zoom).is_empty());
        assert!(km.is_prefix(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)));
        assert_eq!(km.errors.len(), 2, "{:?}", km.errors);
        assert_eq!(km.chords_for(Act::Usage), vec!["alt+u".to_string(), "ctrl+b m".to_string()]);
    }

    #[test]
    fn names_round_trip() {
        for a in Act::all() {
            assert_eq!(Act::from_name(&a.name()), Some(a));
            assert!(!a.describe().is_empty());
        }
    }

    #[test]
    fn ctrl_tab_cycles_sessions() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let km = Keymap::new("ctrl+space", &Default::default());
        assert_eq!(km.direct(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)), Some(Act::NextSession));
        // Windows console input reports shift+tab as Tab+SHIFT, other terminals as BackTab
        assert_eq!(km.direct(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Act::PrevSession));
        assert_eq!(km.direct(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Act::PrevSession));
        // terminals report ctrl+shift+o as 'O' or as 'o' with shift
        for c in ['O', 'o'] {
            assert_eq!(km.direct(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Act::OpenClipboardFolder));
        }
        assert_eq!(km.direct(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)), None, "plain tab still goes to the agent");
    }
}
