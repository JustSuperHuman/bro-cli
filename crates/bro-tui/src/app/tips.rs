//! "Did you know?": the tricks that make bro quicker, one at a time on the welcome screen. A tip moves on by itself
//! after a few seconds (not while the pointer rests on it, or while the window is in the background) and by hand:
//! its arrows and dots, the wheel over it, `[` and `]`. Keys come from the live keymap, so a rebound action shows
//! its own chord and an unbound one sits out.

use super::App;
use crate::keymap::{Act, Keymap};
use std::time::{Duration, Instant};

/// How long a tip stays before the next one comes along by itself.
pub(crate) const DWELL: Duration = Duration::from_secs(9);
/// How long once you've moved through them by hand: time to read one and try it.
pub(crate) const DWELL_BY_HAND: Duration = Duration::from_secs(30);
/// A trackpad's burst on the wheel moves one tip.
const WHEEL_GAP: Duration = Duration::from_millis(300);

/// One trick: what to do, what it gets you, and a line more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tip {
    /// keys and mouse together, in the keymap's chord form: "ctrl+click", "alt+I", "hold alt" (see [`gesture`])
    pub how: String,
    pub title: &'static str,
    pub detail: String,
}

/// A piece of a tip's gesture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// a key, as its keycap reads: "Ctrl", "Shift", "I", "↑"
    Key(String),
    /// what the mouse (or the hand) does: "click", "double-click", "hold"
    Word(&'static str),
}

const WORDS: [&str; 8] = ["click", "double-click", "middle-click", "right-click", "drag", "drop", "hold", "scroll"];

/// A gesture in pieces: "ctrl+click" → Ctrl, click; "alt+I" → Alt, Shift, I (a capital letter is shift and its
/// key); "hold alt" → hold, Alt; the prefix then a key ("ctrl+space S") → Ctrl, Space, Shift, S.
pub(crate) fn gesture(how: &str) -> Vec<Part> {
    let mut out = vec![];
    for token in how.split(' ').filter(|t| !t.is_empty()) {
        let keys = split_keys(token);
        let shifted = keys.iter().any(|k| k.eq_ignore_ascii_case("shift"));
        for k in keys {
            if let Some(w) = WORDS.iter().find(|w| **w == k) {
                out.push(Part::Word(w));
                continue;
            }
            let mut cs = k.chars();
            if let (Some(c), None) = (cs.next(), cs.next())
                && c.is_ascii_uppercase()
                && !shifted
            {
                out.push(Part::Key("Shift".into()));
            }
            out.push(Part::Key(keycap(k)));
        }
    }
    out
}

/// "alt+shift+t" → alt, shift, t; a lone "+" or "-" (and a last one: "ctrl++") is a key.
fn split_keys(token: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut rest = token;
    while let Some(i) = rest.find('+').filter(|i| *i > 0 && rest.len() > 1) {
        out.push(&rest[..i]);
        rest = &rest[i + 1..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// How a key's cap reads: "Ctrl", "PgUp", "Q"; arrows, F-keys and symbols as they are.
fn keycap(k: &str) -> String {
    match k {
        "pgup" => "PgUp".into(),
        "pgdn" => "PgDn".into(),
        "esc" => "Esc".into(),
        "del" => "Del".into(),
        _ if k.chars().count() == 1 => k.to_uppercase(),
        _ if k.chars().all(|c| c.is_ascii_lowercase()) => {
            let mut cs = k.chars();
            cs.next().map(|c| c.to_ascii_uppercase().to_string() + cs.as_str()).unwrap_or_default()
        }
        _ => k.to_string(),
    }
}

/// Whether a "+" goes between two pieces of a gesture ("Ctrl + click", "Alt + Shift + I"); between "hold" and its
/// key it's a space.
pub(crate) fn joined(a: &Part, b: &Part) -> bool {
    !matches!((a, b), (Part::Word(_), Part::Key(_)))
}

/// A gesture as it reads in a sentence: "Ctrl + click", "Alt + Shift + I", "hold Alt". Empty for no key.
pub(crate) fn spoken(how: &str) -> String {
    let parts = gesture(how);
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            s.push_str(if joined(&parts[i - 1], p) { " + " } else { " " });
        }
        match p {
            Part::Key(k) => s.push_str(k),
            Part::Word(w) => s.push_str(w),
        }
    }
    s
}

/// Every tip, in the order they come round: the ones that change how you work first. `desktop` adds the desktop
/// app's own (focus mode, dropped files, the media panel).
pub(crate) fn list(km: &Keymap, desktop: bool) -> Vec<Tip> {
    let key = |a: Act| km.primary(a);
    let say = |a: Act| spoken(&km.primary(a));
    let mut v: Vec<Tip> = vec![];
    let mut add = |how: String, title: &'static str, detail: String| {
        if !how.is_empty() {
            v.push(Tip { how, title, detail });
        }
    };
    let grow = say(Act::Grow);
    add(
        "ctrl+click".into(),
        "Make one session bigger",
        if grow.is_empty() {
            "Its neighbours shrink to make room while you work. Do it again and it snaps back.".into()
        } else {
            format!("Its neighbours shrink to make room. Do it again and it snaps back, or press {grow} from the keyboard.")
        },
    );
    if desktop {
        add(key(Act::Focus), "Read the conversation, not the terminal", "Focus mode shows each agent's chat in big, clean type with a reply box underneath. Press it again for the terminals.".into());
    }
    add(key(Act::FlipAll), "See what every session is up to", "Every card turns over to show its task, plan, latest update and the files it touched. Press again to flip back.".into());
    add("hold alt".into(), "Show the keys for everything on screen","Each card shows its jump key, and the sidebar labels everything you can reach, right where it sits.".into());
    add(key(Act::Attention), "Jump to whoever needs you", "Goes straight to the next session that's waiting on you. Press it again for the one after that.".into());
    let zoom = say(Act::Zoom);
    add(
        "double-click".into(),
        "Fill the screen with one session",
        if zoom.is_empty() {
            "Double-click its title bar, and again to drop back into the grid.".into()
        } else {
            format!("Double-click its title bar, and again to drop back into the grid. {zoom} does it from the keyboard.")
        },
    );
    let flip = say(Act::Flip);
    add(
        "alt+double-click".into(),
        "Flip just one card",
        if flip.is_empty() {
            "The same summary, for a single session. Do it again to turn it face up.".into()
        } else {
            format!("The same summary, for a single session. {flip} flips the one you're in.")
        },
    );
    let (second, all) = (say(Act::Jump(2)), say(Act::ShowTab(0)));
    let mut jump = if second.is_empty() {
        "The numbers count down the sidebar from the top, up to 9.".to_string()
    } else {
        format!("{} is the top session in the sidebar, {second} the next, and so on up to 9.", say(Act::Jump(1)))
    };
    if !all.is_empty() {
        jump.push_str(&format!(" {all} shows them all."));
    }
    add(key(Act::Jump(1)), "Jump to any session by its number", jump);
    add("shift+click".into(), "Put sessions side by side", "Shift + click sessions in the sidebar to line them up together. Shift + click one again to take it out.".into());
    add("ctrl+v".into(), "Paste a screenshot straight to an agent", "With a picture on the clipboard, it goes in as a file the agent can open and look at.".into());
    add(key(Act::Continue), "Pick up an old conversation", "Search every earlier session and resume it on any login, even one it never ran on.".into());
    add("middle-click".into(), "Close a session like a browser tab", "Middle-click its tab or its row in the sidebar. If the agent is still busy, bro checks with you first.".into());
    add(km.prefix.display(), "Every action, one key away", "Opens bro's menu, tmux style. One more key runs whatever you pick from it.".into());
    add("drag".into(), "Pin a tile edge where you want it", "Drag the gap between two tiles and the edge stays where you leave it. Double-click it to let go.".into());
    add(key(Act::QuickAgent), "Spin up a quick agent for odd jobs", "It starts in your home folder on your last login. Good for chores like clearing disk space or fixing PATH.".into());
    if desktop {
        add("drop".into(), "Drop files onto a session", "The path lands where you're typing. In focus mode, pictures attach as thumbnails instead.".into());
    }
    add(key(Act::LastTab), "Hop back to where you just were", "Switches between the sessions on screen and the ones you had up before. Like Alt + Tab, for your agents.".into());
    add("click".into(), "Answer an agent's question with a click", "When an agent lists options, click the one you want instead of arrowing down to it.".into());
    add(key(Act::SwitchLogin), "Out of usage? Move the session", "Hand the conversation to another login and carry on right where you left off.".into());
    add(key(Act::Palette), "Forgot a shortcut? Search for it", "The command palette finds every action by name, with its key right beside it.".into());
    add(key(Act::OpenClipboardFolder), "Open the folder you just copied", "Copy a folder's path from anywhere, press this, and it's a project in the sidebar.".into());
    add("shift+drag".into(), "Select text even when the app wants the mouse", "Hold Shift while you drag and the selection is yours, not the program's.".into());
    if desktop {
        add(key(Act::Media), "Keep pictures and videos beside your work", "The media panel collects everything your agents open, send or make, in one place.".into());
    }
    add("double-click".into(), "Reset the sidebar's width", "Drag its edge to make it wider or narrower. Double-click the edge for the automatic width again.".into());
    v
}

/// Which tip shows, since when, and the one it took over from (for the slide).
#[derive(Debug)]
pub(crate) struct Tips {
    /// the tip showing, an index into [`list`] (kept in range as the list changes)
    pub index: usize,
    /// the tip it replaced, which way they went (1 on, -1 back) and when
    pub from: Option<(usize, i32, Instant)>,
    /// when the showing tip's time started (moved on while it's paused, so a pause doesn't use it up)
    shown: Instant,
    /// the last look while paused: the pause runs from it
    paused_at: Option<Instant>,
    /// moved by hand: each tip stays longer
    by_hand: bool,
    wheel_at: Option<Instant>,
}

impl Tips {
    pub(crate) fn new(index: usize, now: Instant) -> Tips {
        Tips { index, from: None, shown: now, paused_at: None, by_hand: false, wheel_at: None }
    }

    /// Somewhere new each run, so the same few don't always come first (the first one in tests and the demo).
    pub(crate) fn start(fixed: bool) -> Tips {
        let index = if fixed || cfg!(test) {
            0
        } else {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize ^ d.as_secs() as usize).unwrap_or(0)
        };
        Tips::new(index, Instant::now())
    }

    fn dwell(&self) -> Duration {
        if self.by_hand { DWELL_BY_HAND } else { DWELL }
    }

    /// The tip to show out of `n`, moving on once its time is up (not while `paused`).
    pub(crate) fn current(&mut self, n: usize, now: Instant, paused: bool) -> usize {
        if n == 0 {
            return 0;
        }
        self.index %= n;
        if paused {
            self.shown += now.saturating_duration_since(self.paused_at.unwrap_or(now));
            self.paused_at = Some(now);
            return self.index;
        }
        self.paused_at = None;
        if now.saturating_duration_since(self.shown) >= self.dwell() {
            self.go((self.index + 1) % n, 1, now);
        }
        self.index
    }

    /// How far the showing tip is through its time (0 → 1).
    pub(crate) fn progress(&self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.shown).as_secs_f32() / self.dwell().as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Move `by` tips, round the end, by hand.
    pub(crate) fn step(&mut self, n: usize, by: i32, now: Instant) {
        if n == 0 {
            return;
        }
        self.by_hand = true;
        let to = (self.index as i64 + by as i64).rem_euclid(n as i64) as usize;
        self.go(to, by.signum(), now);
    }

    /// Show tip `i` (its dot), by hand.
    pub(crate) fn show(&mut self, n: usize, i: usize, now: Instant) {
        if i >= n {
            return;
        }
        self.by_hand = true;
        let dir = if i >= self.index % n.max(1) { 1 } else { -1 };
        self.go(i, dir, now);
    }

    /// The wheel over the tips: one tip a burst.
    pub(crate) fn wheel(&mut self, n: usize, by: i32, now: Instant) {
        if self.wheel_at.is_some_and(|at| now.saturating_duration_since(at) < WHEEL_GAP) {
            return;
        }
        self.wheel_at = Some(now);
        self.step(n, by, now);
    }

    fn go(&mut self, to: usize, dir: i32, now: Instant) {
        if to != self.index {
            self.from = Some((self.index, dir, now));
        }
        self.index = to;
        self.shown = now;
        if self.paused_at.is_some() {
            self.paused_at = Some(now);
        }
    }
}

impl App {
    /// The tips for this screen: the desktop app has a few of its own.
    pub(crate) fn tip_list(&self) -> Vec<Tip> {
        list(&self.keymap, crate::panes::mirror::active())
    }

    /// The next tip (`by` 1) or an earlier one (-1), by hand.
    pub(crate) fn tip_step(&mut self, by: i32) {
        let n = self.tip_list().len();
        self.tips.step(n, by, Instant::now());
    }

    /// Show tip `i`, by hand.
    pub(crate) fn tip_show(&mut self, i: usize) {
        let n = self.tip_list().len();
        self.tips.show(n, i, Instant::now());
    }

    /// A turn of the wheel over the tips: on (`by` 1) or back.
    pub(crate) fn tip_wheel(&mut self, by: i32) {
        let n = self.tip_list().len();
        self.tips.wheel(n, by, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn gestures_read_as_keycaps_and_mouse_words() {
        assert_eq!(gesture("ctrl+click"), vec![Part::Key("Ctrl".into()), Part::Word("click")]);
        assert_eq!(spoken("alt+I"), "Alt + Shift + I", "a capital is shift and its key");
        assert_eq!(spoken("alt+shift+i"), "Alt + Shift + I");
        assert_eq!(spoken("alt+f"), "Alt + F");
        assert_eq!(spoken("hold alt"), "hold Alt");
        assert_eq!(spoken("alt+double-click"), "Alt + double-click");
        assert_eq!(spoken("ctrl+space"), "Ctrl + Space");
        assert_eq!(spoken("ctrl+space S"), "Ctrl + Space + Shift + S");
        assert_eq!(spoken("ctrl+↑"), "Ctrl + ↑");
        assert_eq!(spoken("ctrl++"), "Ctrl + +");
        assert_eq!(spoken("alt+-"), "Alt + -");
        assert_eq!(spoken("F1"), "F1");
        assert_eq!(spoken(""), "");
    }

    #[test]
    fn the_three_that_change_how_you_work_come_first() {
        let km = Keymap::default();
        let tips = list(&km, true);
        let firsts: Vec<String> = tips.iter().take(3).map(|t| spoken(&t.how)).collect();
        assert_eq!(firsts, ["Ctrl + click", "Alt + F", "Alt + Shift + I"]);
        assert!(tips.len() >= 20, "{}", tips.len());
        for t in &tips {
            assert!(!t.how.is_empty() && !t.title.is_empty() && !t.detail.is_empty(), "{t:?}");
            // one line for the headline and two for the rest, in the terminal UI's tip and the window's card
            assert!(t.title.chars().count() <= 46, "{t:?}");
            assert!(t.detail.chars().count() <= 120, "{} chars: {t:?}", t.detail.chars().count());
            assert!(!t.detail.contains('—'), "{t:?}");
        }
        let titles: std::collections::HashSet<&str> = tips.iter().map(|t| t.title).collect();
        assert_eq!(titles.len(), tips.len(), "no tip twice");
    }

    #[test]
    fn the_terminal_ui_leaves_out_the_windows_own() {
        let km = Keymap::default();
        let terminal = list(&km, false);
        let desktop = list(&km, true);
        assert!(terminal.len() < desktop.len());
        for t in &terminal {
            assert!(!["alt+f", "alt+m", "drop"].contains(&t.how.as_str()), "{t:?}");
        }
    }

    #[test]
    fn tips_follow_the_live_keymap() {
        let mut o = BTreeMap::new();
        o.insert("flip_all".to_string(), "alt+y".to_string());
        o.insert("focus_mode".to_string(), "none".to_string());
        o.insert("zoom".to_string(), "none".to_string());
        let km = Keymap::new("ctrl+b", &o);
        let tips = list(&km, true);
        let flip = tips.iter().find(|t| t.title == "See what every session is up to").expect("flip all");
        assert_eq!(flip.how, "alt+y");
        assert!(tips.iter().all(|t| t.title != "Read the conversation, not the terminal"), "an unbound action sits out");
        let menu = tips.iter().find(|t| t.title == "Every action, one key away").expect("the prefix");
        assert_eq!(spoken(&menu.how), "Ctrl + B");
        let zoom = tips.iter().find(|t| t.title == "Fill the screen with one session").expect("zoom");
        assert!(!zoom.detail.contains("Alt + Z") && !zoom.detail.contains("  "), "{zoom:?}");
        let jump = tips.iter().find(|t| t.how == "alt+1").expect("jump");
        assert!(jump.detail.contains("Alt + 2") && jump.detail.contains("Alt + 0"), "{jump:?}");
    }

    #[test]
    fn tips_move_on_by_themselves_and_by_hand() {
        let t0 = Instant::now();
        let mut tips = Tips::new(0, t0);
        assert_eq!(tips.current(5, t0 + Duration::from_secs(1), false), 0);
        assert_eq!(tips.current(5, t0 + DWELL, false), 1, "time's up: the next one");
        assert_eq!(tips.from.map(|f| (f.0, f.1)), Some((0, 1)), "it slides on from the last");
        // a pause doesn't use up its time
        let t1 = t0 + DWELL;
        assert_eq!(tips.current(5, t1 + Duration::from_secs(4), true), 1);
        assert_eq!(tips.current(5, t1 + Duration::from_secs(30), true), 1, "held while the pointer rests on it");
        assert_eq!(tips.current(5, t1 + Duration::from_secs(31), false), 1, "about 5 s of its 9 used");
        assert_eq!(tips.current(5, t1 + Duration::from_secs(36), false), 2);
        // by hand: back round the start, and each stays longer after
        let t2 = t1 + Duration::from_secs(40);
        tips.step(5, -1, t2);
        tips.step(5, -1, t2);
        tips.step(5, -1, t2);
        assert_eq!(tips.index, 4, "back past the first: the last");
        assert_eq!(tips.from.map(|f| f.1), Some(-1));
        assert_eq!(tips.current(5, t2 + DWELL, false), 4, "moved by hand: time to read it");
        assert_eq!(tips.current(5, t2 + DWELL_BY_HAND, false), 0);
        tips.show(5, 3, t2 + DWELL_BY_HAND);
        assert_eq!((tips.index, tips.from.map(|f| f.1)), (3, Some(1)));
        // a list that got shorter keeps the index in range
        assert_eq!(tips.current(2, t2 + DWELL_BY_HAND, false), 1);
    }

    #[test]
    fn a_burst_on_the_wheel_moves_one_tip() {
        let t0 = Instant::now();
        let mut tips = Tips::new(0, t0);
        for ms in [0, 20, 40, 120, 250] {
            tips.wheel(9, 1, t0 + Duration::from_millis(ms));
        }
        assert_eq!(tips.index, 1);
        tips.wheel(9, 1, t0 + Duration::from_millis(400));
        assert_eq!(tips.index, 2, "the next burst, the next tip");
    }
}
