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
    NewShell,
    Split,
    SplitRight,
    SplitDown,
    Close,
    Zoom,
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
    Attention,
    ToggleSidebar,
    FocusSidebar,
    Palette,
    Usage,
    Profiles,
    Proxy,
    Bridge,
    Help,
    Rename,
    Themes,
    ToggleIcons,
    RefreshUsage,
    UsageDetails,
    ShowArchived,
    SwitchLogin,
    OpenProject,
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
        let mut v = vec![NewSession, NewShell, Rename, Close, Attention, OpenProject, SwitchLogin, ShowArchived, NextSession, PrevSession, NextProject, PrevProject];
        v.extend((1..=9).map(Jump));
        v.extend([NextTab, PrevTab, FocusSidebar, ToggleSidebar]);
        v.extend([Split, SplitRight, SplitDown, Zoom, FocusLeft, FocusRight, FocusUp, FocusDown, ResizeLeft, ResizeRight, ResizeUp, ResizeDown]);
        v.extend([Usage, UsageDetails, Profiles, Proxy, Bridge, RefreshUsage]);
        v.extend([Palette, Help, Themes, ToggleIcons, Quit]);
        v
    }

    /// Stable name used in `Settings.keys`.
    pub fn name(self) -> String {
        use Act::*;
        match self {
            Jump(n) => format!("jump_{n}"),
            NewSession => "new_session".into(),
            NewShell => "new_shell".into(),
            Split => "split".into(),
            SplitRight => "split_right".into(),
            SplitDown => "split_down".into(),
            Close => "close".into(),
            Zoom => "zoom".into(),
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
            Help => "help".into(),
            Rename => "rename".into(),
            Themes => "themes".into(),
            ToggleIcons => "toggle_icons".into(),
            RefreshUsage => "refresh_usage".into(),
            UsageDetails => "usage_details".into(),
            ShowArchived => "show_archived".into(),
            SwitchLogin => "switch_login".into(),
            OpenProject => "open_project".into(),
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
            NewSession => "launch an agent session…".into(),
            NewShell => "open a plain shell tab".into(),
            Split => "launch into a split beside this pane…".into(),
            SplitRight => "split right with a shell".into(),
            SplitDown => "split down with a shell".into(),
            Close => "close the focused pane".into(),
            Zoom => "zoom / unzoom the focused pane".into(),
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
            NextProject => "next project".into(),
            PrevProject => "previous project".into(),
            NextTab => "next tab".into(),
            PrevTab => "previous tab".into(),
            Attention => "jump to the session that needs you".into(),
            ToggleSidebar => "show / hide the sidebar".into(),
            FocusSidebar => "focus the sidebar (j/k, enter, f fork, / filter)".into(),
            Palette => "command palette".into(),
            Usage => "usage: meters for every profile".into(),
            Profiles => "profiles: accounts and logins".into(),
            Proxy => "proxy: routes and live requests".into(),
            Bridge => "bridge: phone / web pairing".into(),
            Help => "keys and help".into(),
            Rename => "rename the focused session".into(),
            Themes => "pick a theme (live preview)".into(),
            ToggleIcons => "toggle nerd font icons".into(),
            RefreshUsage => "refresh usage now".into(),
            UsageDetails => "sidebar usage: totals / every profile".into(),
            ShowArchived => "show / hide archived sessions".into(),
            SwitchLogin => "move this session to another login (keeps the conversation)".into(),
            OpenProject => "open a folder as a project".into(),
            Quit => "quit bro".into(),
        }
    }

    pub fn group(self) -> Group {
        use Act::*;
        match self {
            NewSession | NewShell | Rename | Close | Attention | ShowArchived | SwitchLogin | OpenProject => Group::Sessions,
            NextSession | PrevSession | NextProject | PrevProject | Jump(_) | NextTab | PrevTab | FocusSidebar | ToggleSidebar => Group::Navigate,
            Split | SplitRight | SplitDown | Zoom | FocusLeft | FocusRight | FocusUp | FocusDown | ResizeLeft | ResizeRight | ResizeUp | ResizeDown => Group::Panes,
            Usage | UsageDetails | Profiles | Proxy | Bridge | RefreshUsage => Group::Views,
            Palette | Help | Themes | ToggleIcons | Quit => Group::App,
        }
    }

    /// Default direct chords.
    fn default_direct(self) -> Vec<&'static str> {
        use Act::*;
        match self {
            NewSession => vec!["alt+n"],
            NewShell => vec!["alt+t"],
            Split => vec!["alt+enter", "alt+\\"],
            Close => vec!["alt+w"],
            Zoom => vec!["alt+z"],
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
            ToggleSidebar => vec!["alt+s"],
            FocusSidebar => vec!["alt+b"],
            Palette => vec!["alt+p"],
            Usage => vec!["alt+u"],
            UsageDetails => vec!["alt+U"],
            SwitchLogin => vec!["alt+L"],
            OpenProject => vec!["alt+O"],
            Profiles => vec!["alt+o"],
            Proxy => vec!["alt+y"],
            Bridge => vec!["alt+g"],
            Help => vec!["f1"],
            _ => vec![],
        }
    }

    /// Default keys after the prefix.
    fn default_prefix(self) -> Vec<&'static str> {
        use Act::*;
        match self {
            NewSession => vec!["c"],
            NewShell => vec!["!"],
            SplitRight => vec!["%", "|", "\\"],
            SplitDown => vec!["\"", "-"],
            Close => vec!["x"],
            Zoom => vec!["z"],
            FocusLeft => vec!["h", "left"],
            FocusRight => vec!["l", "right"],
            FocusUp => vec!["k", "up"],
            FocusDown => vec!["j", "down"],
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
            Attention => vec!["a"],
            ToggleSidebar => vec!["s"],
            FocusSidebar => vec!["b"],
            Palette => vec![":", "space"],
            Usage => vec!["u"],
            UsageDetails => vec!["U"],
            ShowArchived => vec!["A"],
            SwitchLogin => vec!["P"],
            OpenProject => vec!["O"],
            Profiles => vec!["o"],
            Proxy => vec!["y"],
            Bridge => vec!["g"],
            Help => vec!["?"],
            Rename => vec![","],
            Themes => vec!["t"],
            ToggleIcons => vec!["i"],
            RefreshUsage => vec!["r"],
            Quit => vec!["q"],
            Split => vec![],
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

    /// A short "key action · key action" line of the prefix table, for the armed-prefix hint.
    pub fn prefix_hint(&self) -> String {
        let want = [Act::SplitRight, Act::SplitDown, Act::Close, Act::Zoom, Act::NewSession, Act::FocusSidebar, Act::Palette, Act::Help];
        want.iter()
            .filter_map(|a| self.prefixed.iter().find(|(_, x)| x == a).map(|(c, _)| format!("{} {}", c.display(), short_label(*a))))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn short_label(a: Act) -> &'static str {
    match a {
        Act::SplitRight => "split",
        Act::SplitDown => "split↓",
        Act::Close => "close",
        Act::Zoom => "zoom",
        Act::NewSession => "launch",
        Act::FocusSidebar => "sidebar",
        Act::Palette => "palette",
        Act::Help => "help",
        _ => "",
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
        assert_eq!(km.direct(&ev(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT)), Some(Act::ResizeLeft));
        assert_eq!(km.direct(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)), None, "ctrl keys belong to the agent");
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
        assert_eq!(km.direct(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)), None, "plain tab still goes to the agent");
    }
}
