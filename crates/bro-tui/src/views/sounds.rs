//! Sounds view: PeonPing voice lines (see [`bro_core::peon`]) in the sessions bro starts. The switch, the volume
//! and which moments speak; which harnesses get bro's hooks, and the logins whose own config already runs
//! peon-ping (a Claude login's can be handed to bro, so its MP3 packs and the volume work outside bro too); then
//! the packs: the installed ones, and the PeonPing registry to search, preview and install from. Everything that
//! touches the disk or the network runs on a background thread and wakes the pane.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::alerts::Kind;
use crate::pane::{Cx, Pane};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::peon::registry::{self, RegistryPack};
use bro_core::peon::wire::{self, Login, Own};
use bro_core::peon::{self, CATEGORIES, PeonConfig, pack};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Which packs the list shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Tab {
    #[default]
    Installed,
    Browse,
}

/// What background work brought back.
#[derive(Default)]
struct Inbox {
    scan: Option<(PeonConfig, Vec<pack::Installed>, Vec<Login>)>,
    registry: Option<Result<Vec<RegistryPack>, String>>,
    /// pack → (files done, of)
    progress: HashMap<String, (usize, usize)>,
    /// a job ended: (pack, what to say, went wrong)
    done: Vec<(String, String, bool)>,
    /// a preview or test couldn't play
    sound_error: Option<String>,
    /// what's playing ended
    played: bool,
}

/// One row of the list: an installed pack, a registry pack, or both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Entry {
    pub installed: Option<usize>,
    pub listed: Option<usize>,
}

#[derive(Default)]
pub struct SoundsView {
    pub(crate) tab: Tab,
    pub(crate) sel: usize,
    /// lines (terminal UI) or pixels (desktop app)
    pub(crate) scroll: usize,
    pub(crate) query: String,
    /// keys go to the search field
    pub(crate) typing: bool,
    pub(crate) cfg: PeonConfig,
    pub(crate) installed: Vec<pack::Installed>,
    pub(crate) registry: Option<Vec<RegistryPack>>,
    pub(crate) registry_error: Option<String>,
    pub(crate) logins: Vec<Login>,
    /// the first scan came back
    pub(crate) scanned: bool,
    pub(crate) fetching: bool,
    pub(crate) started: bool,
    /// installs running: pack → (done, of)
    pub(crate) jobs: HashMap<String, (usize, usize)>,
    /// the pack whose line is playing (a preview or the test)
    pub(crate) playing: Option<String>,
    stop: Option<Arc<AtomicBool>>,
    /// which of a pack's previews plays next
    preview_n: HashMap<String, usize>,
    /// install this one, then use it
    use_after: Option<String>,
    inbox: Arc<Mutex<Inbox>>,
    /// (screen row, list index) from the last terminal render, for clicks
    hits: Vec<(u16, usize)>,
}

impl SoundsView {
    pub fn new() -> SoundsView {
        SoundsView::default()
    }

    /// The config, the packs and the logins, read again on a background thread.
    pub(crate) fn rescan(&mut self, cx: &Cx) {
        if cx.svc.is_demo() {
            self.scanned = true;
            return;
        }
        let inbox = self.inbox.clone();
        let waker = cx.waker();
        std::thread::spawn(move || {
            let found = (PeonConfig::load(), pack::installed(), wire::logins());
            inbox.lock().scan = Some(found);
            waker.wake();
        });
    }

    /// The registry (from its cache while it's fresh, unless `force`).
    pub(crate) fn fetch(&mut self, force: bool, cx: &Cx) {
        if self.fetching || cx.svc.is_demo() {
            return;
        }
        self.fetching = true;
        let inbox = self.inbox.clone();
        let waker = cx.waker();
        std::thread::spawn(move || {
            let r = registry::load(force).map_err(|e| format!("{e:#}"));
            inbox.lock().registry = Some(r);
            waker.wake();
        });
    }

    /// The first draw starts the scans.
    pub(crate) fn start(&mut self, cx: &Cx) {
        if !self.started {
            self.started = true;
            self.rescan(cx);
            self.fetch(false, cx);
        }
    }

    /// Take what background work brought back.
    pub(crate) fn take_inbox(&mut self, cx: &mut Cx) {
        let mut inbox = self.inbox.lock();
        if let Some((cfg, installed, logins)) = inbox.scan.take() {
            self.cfg = cfg;
            self.installed = installed;
            self.logins = logins;
            self.scanned = true;
        }
        if let Some(r) = inbox.registry.take() {
            self.fetching = false;
            match r {
                Ok(p) => {
                    self.registry = Some(p);
                    self.registry_error = None;
                }
                Err(e) => self.registry_error = Some(e),
            }
        }
        for (name, p) in inbox.progress.drain() {
            if let Some(j) = self.jobs.get_mut(&name) {
                *j = p;
            }
        }
        let done = std::mem::take(&mut inbox.done);
        let sound_error = inbox.sound_error.take();
        if std::mem::take(&mut inbox.played) {
            self.playing = None;
        }
        drop(inbox);
        let mut rescan = false;
        for (name, msg, bad) in done {
            self.jobs.remove(&name);
            if !bad && self.use_after.as_deref() == Some(name.as_str()) {
                self.use_after = None;
                self.cfg = PeonConfig::load();
                self.cfg.use_pack(&name);
                self.save_cfg(cx);
            }
            cx.toast(if bad { Kind::Error } else { Kind::Info }, msg);
            rescan = true;
        }
        if let Some(e) = sound_error {
            self.playing = None;
            cx.toast(Kind::Error, e);
        }
        if rescan {
            self.rescan(cx);
        }
    }

    fn save_cfg(&mut self, cx: &mut Cx) {
        if cx.svc.is_demo() {
            return;
        }
        if let Err(e) = self.cfg.save() {
            cx.toast(Kind::Error, format!("couldn't save the sound settings: {e}"));
        }
    }

    /// Sounds play in the sessions bro starts (and peon-ping's own hooks stay on with them).
    pub(crate) fn on(&self, cx: &Cx) -> bool {
        cx.svc.settings().peon.on && self.cfg.enabled()
    }

    /// The switch: bro's hooks in new sessions and peon-ping's `enabled`, together. Turning on with no pack at
    /// all installs peon-ping's own default.
    pub(crate) fn toggle(&mut self, cx: &mut Cx) {
        let on = !self.on(cx);
        let mut s = cx.svc.settings();
        s.peon.on = on;
        cx.svc.save_settings(s);
        self.cfg.set_enabled(on);
        self.save_cfg(cx);
        if on && self.scanned && self.installed.is_empty() && self.jobs.is_empty() {
            let default = self.registry.as_ref().and_then(|r| r.iter().position(|p| p.name == peon::DEFAULT_PACK));
            match default {
                Some(i) => {
                    let p = self.registry.as_ref().unwrap()[i].clone();
                    self.install(p, true, cx);
                }
                None => cx.toast(Kind::Info, "sounds on · pick a pack to install under Browse"),
            }
        } else {
            cx.toast(Kind::Info, if on { "sounds on · new sessions play them" } else { "sounds off" });
        }
    }

    pub(crate) fn volume(&mut self, delta: f32, cx: &mut Cx) {
        let v = ((self.cfg.volume() + delta) * 10.0).round() / 10.0;
        self.cfg.set_volume(v.clamp(0.0, 1.0));
        self.save_cfg(cx);
    }

    pub(crate) fn toggle_category(&mut self, i: usize, cx: &mut Cx) {
        let Some((cat, _)) = CATEGORIES.get(i) else { return };
        let on = !self.cfg.category_on(cat);
        self.cfg.set_category(cat, on);
        self.save_cfg(cx);
    }

    pub(crate) fn toggle_harness(&mut self, h: Harness, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        let on = !s.peon.harness(h);
        s.peon.set_harness(h, on);
        cx.svc.save_settings(s);
    }

    /// Hand every Claude login whose settings run peon-ping's scripts over to bro's hook.
    pub(crate) fn take_over(&mut self, cx: &mut Cx) {
        if cx.svc.is_demo() {
            return cx.toast(Kind::Info, "the demo doesn't change any settings");
        }
        let Ok(bin) = std::env::current_exe() else { return };
        let dirs: Vec<PathBuf> = self.logins.iter().filter(|l| l.harness == Harness::Claude && matches!(l.own, Own::PeonPing(_))).map(|l| l.dir.clone()).collect();
        if dirs.is_empty() {
            return cx.toast(Kind::Info, "no Claude login runs peon-ping's own hooks");
        }
        let mut changed = 0;
        for d in &dirs {
            match wire::take_over_claude(d, &bin) {
                Ok(_) => changed += 1,
                Err(e) => cx.toast(Kind::Error, format!("{}: {e:#}", d.display())),
            }
        }
        if changed > 0 {
            let what = if changed == 1 { "1 Claude login".to_string() } else { format!("{changed} Claude logins") };
            cx.toast(Kind::Info, format!("bro plays the sounds for {what} now, in bro and out · each settings.json kept beside it as settings.json.before-bro-peon"));
        }
        self.rescan(cx);
    }

    /// The rows the list shows: installed packs (with their registry entry), or the registry's, filtered by the
    /// search.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        let listed = |name: &str| self.registry.as_ref().and_then(|r| r.iter().position(|p| p.name == name));
        let q = self.query.trim();
        match self.tab {
            Tab::Installed => self
                .installed
                .iter()
                .enumerate()
                .filter(|(_, p)| q.is_empty() || format!("{} {}", p.name, p.title()).to_lowercase().contains(&q.to_lowercase()))
                .map(|(i, p)| Entry { installed: Some(i), listed: listed(&p.name) })
                .collect(),
            Tab::Browse => self
                .registry
                .iter()
                .flatten()
                .enumerate()
                .filter(|(_, p)| q.is_empty() || p.matches(q))
                .map(|(i, p)| Entry { installed: self.installed.iter().position(|x| x.name == p.name), listed: Some(i) })
                .collect(),
        }
    }

    pub(crate) fn name_of(&self, e: Entry) -> String {
        match (e.installed, e.listed) {
            (Some(i), _) => self.installed[i].name.clone(),
            (None, Some(l)) => self.registry.as_ref().map(|r| r[l].name.clone()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(crate) fn listed(&self, e: Entry) -> Option<&RegistryPack> {
        self.registry.as_ref()?.get(e.listed?)
    }

    pub(crate) fn title_of(&self, e: Entry) -> String {
        match (e.installed, self.listed(e)) {
            (Some(i), _) => self.installed[i].title().to_string(),
            (None, Some(p)) => p.title().to_string(),
            _ => String::new(),
        }
    }

    /// The pack sounds come from unless a rule picks another.
    pub(crate) fn active(&self) -> Option<&str> {
        self.cfg.default_pack().filter(|p| self.installed.iter().any(|x| x.name == *p)).or_else(|| self.installed.first().map(|p| p.name.as_str()))
    }

    pub(crate) fn update_available(&self, e: Entry) -> bool {
        match (e.installed, self.listed(e)) {
            (Some(i), Some(p)) => registry::outdated(self.installed[i].version(), p.version.as_deref()),
            _ => false,
        }
    }

    /// "29 sounds · 430 KB · Marko Jevic · en" for a row.
    pub(crate) fn meta(&self, e: Entry) -> String {
        let mut parts: Vec<String> = vec![];
        let listed = self.listed(e);
        let sounds = e.installed.map(|i| self.installed[i].sounds as u32).or_else(|| listed.and_then(|p| p.sound_count));
        if let Some(n) = sounds {
            parts.push(format!("{n} sound{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(b) = listed.and_then(|p| p.total_size_bytes) {
            parts.push(registry::size_label(b));
        }
        let author = listed.and_then(RegistryPack::author_name).map(str::to_string).or_else(|| e.installed.and_then(|i| self.installed[i].manifest.as_ref()?.author.clone()));
        parts.extend(author);
        if let Some(l) = listed.and_then(|p| p.language.clone()) {
            parts.push(l);
        }
        parts.join(" · ")
    }

    /// The line under a row: the registry's description, else the pack's own.
    pub(crate) fn description(&self, e: Entry) -> Option<String> {
        self.listed(e).and_then(|p| p.description.clone()).or_else(|| e.installed.and_then(|i| self.installed[i].manifest.as_ref()?.description.clone())).filter(|d| !d.trim().is_empty())
    }

    pub(crate) fn selected(&self) -> Option<Entry> {
        let all = self.entries();
        all.get(self.sel.min(all.len().saturating_sub(1))).copied()
    }

    fn install(&mut self, p: RegistryPack, then_use: bool, cx: &mut Cx) {
        if cx.svc.is_demo() {
            return cx.toast(Kind::Info, "the demo doesn't install anything");
        }
        if self.jobs.contains_key(&p.name) {
            return;
        }
        if then_use {
            self.use_after = Some(p.name.clone());
        }
        self.jobs.insert(p.name.clone(), (0, p.sound_count.unwrap_or(0) as usize));
        let inbox = self.inbox.clone();
        let waker = cx.waker();
        std::thread::spawn(move || {
            let name = p.name.clone();
            let r = registry::install(&p, |d, t| {
                inbox.lock().progress.insert(name.clone(), (d, t));
                waker.wake();
            });
            let msg = match &r {
                Ok(_) => (format!("{} installed", p.title()), false),
                Err(e) => (format!("{}: {e:#}", p.title()), true),
            };
            inbox.lock().done.push((name, msg.0, msg.1));
            waker.wake();
        });
    }

    /// Enter on a row: use it (installing it first when it isn't).
    pub(crate) fn activate(&mut self, cx: &mut Cx) {
        let Some(e) = self.selected() else { return };
        let name = self.name_of(e);
        if e.installed.is_some() {
            self.cfg.use_pack(&name);
            self.save_cfg(cx);
            if !self.on(cx) {
                cx.toast(Kind::Info, format!("{} picked · sounds are off (s turns them on)", self.title_of(e)));
            }
        } else if let Some(p) = self.listed(e).cloned() {
            self.install(p, true, cx);
        }
    }

    /// Install the selected pack again (its update).
    pub(crate) fn reinstall(&mut self, cx: &mut Cx) {
        match self.selected().and_then(|e| self.listed(e).cloned()) {
            Some(p) => self.install(p, false, cx),
            None => cx.toast(Kind::Info, "this pack isn't in the registry: nothing to update it from"),
        }
    }

    pub(crate) fn remove(&mut self, cx: &mut Cx) {
        if cx.svc.is_demo() {
            return cx.toast(Kind::Info, "the demo doesn't remove anything");
        }
        let Some(e) = self.selected().filter(|e| e.installed.is_some()) else { return cx.toast(Kind::Info, "not installed") };
        let name = self.name_of(e);
        match pack::remove(&name) {
            Ok(()) => {
                cx.toast(Kind::Info, format!("{} removed", self.title_of(e)));
                self.installed.retain(|p| p.name != name);
                // the default went: the next one installed takes over
                if self.cfg.default_pack() == Some(name.as_str())
                    && let Some(next) = self.installed.first().map(|p| p.name.clone())
                {
                    self.cfg.use_pack(&next);
                    self.save_cfg(cx);
                }
                self.rescan(cx);
            }
            Err(e) => cx.toast(Kind::Error, format!("{e:#}")),
        }
    }

    /// Stop what's playing.
    pub(crate) fn hush(&mut self) {
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        self.playing = None;
    }

    /// Play `path` (or what `find` fetches) for pack `name` on a background thread, stopping what plays now.
    fn play(&mut self, name: String, find: impl FnOnce() -> anyhow::Result<PathBuf> + Send + 'static, cx: &Cx) {
        self.hush();
        if cx.svc.is_demo() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        self.playing = Some(name);
        let volume = self.cfg.volume();
        let inbox = self.inbox.clone();
        let waker = cx.waker();
        std::thread::spawn(move || {
            let r = find().and_then(|p| bro_voice::sound::decode(&p)).and_then(|clip| bro_voice::sound::play_clip(&clip, volume.max(0.05), Some(&stop)));
            let mut i = inbox.lock();
            if let Err(e) = r {
                i.sound_error = Some(format!("couldn't play it: {e:#}"));
            }
            if !stop.load(Ordering::Relaxed) {
                i.played = true;
            }
            drop(i);
            waker.wake();
        });
    }

    /// `p`: a line of the selected pack (each press the next one), fetched first when it isn't installed.
    pub(crate) fn preview(&mut self, cx: &mut Cx) {
        let Some(e) = self.selected() else { return };
        let name = self.name_of(e);
        if self.playing.as_deref() == Some(name.as_str()) {
            return self.hush();
        }
        let n = *self.preview_n.entry(name.clone()).and_modify(|n| *n += 1).or_insert(0);
        if let Some(p) = self.listed(e).cloned() {
            self.play(name, move || registry::preview(&p, n), cx);
        } else if let Some(i) = e.installed {
            let dir = self.installed[i].dir.clone();
            self.play(
                name,
                move || {
                    let m = pack::load_manifest(&dir).ok_or_else(|| anyhow::anyhow!("its manifest is gone"))?;
                    let files = m.files();
                    let s = files.get(n % files.len().max(1)).ok_or_else(|| anyhow::anyhow!("it lists no sounds"))?;
                    pack::sound_path(&dir, &s.file).ok_or_else(|| anyhow::anyhow!("{} is missing", s.file))
                },
                cx,
            );
        }
    }

    /// `t`: what a session sounds like when it finishes, from the pack in use.
    pub(crate) fn test(&mut self, cx: &mut Cx) {
        match peon::sample(Some("task.complete")) {
            Some(p) => {
                let path = p.path.clone();
                self.play(p.pack, move || Ok(path), cx);
            }
            None => cx.toast(Kind::Info, "no pack installed yet · pick one under Browse"),
        }
    }

    pub(crate) fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.sel = 0;
            self.scroll = 0;
        }
    }

    /// The login lines worth a word: every harness's state, and who plays for whom when it isn't bro.
    pub(crate) fn harness_state(&self, h: Harness, cx: &Cx) -> (String, bool) {
        let s = cx.svc.settings().peon;
        let theirs: Vec<&Login> = self.logins.iter().filter(|l| l.harness == h && l.own.covered()).collect();
        let total = self.logins.iter().filter(|l| l.harness == h).count();
        let on = s.wires(h);
        let text = match () {
            _ if !s.harness(h) => "off".to_string(),
            // every login plays by its own config: bro's hook made global, or peon-ping's scripts
            _ if theirs.len() == total && total > 0 => match theirs[0].own {
                Own::Bro(_) => "bro · global".into(),
                _ => "peon-ping".into(),
            },
            _ if !theirs.is_empty() => format!("bro · {}: {}", theirs.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "), if theirs.iter().all(|l| matches!(l.own, Own::Bro(_))) { "global" } else { "peon-ping" }),
            // muted while sounds are off
            _ => "bro".into(),
        };
        (text, on)
    }

    /// Claude logins whose settings run peon-ping's scripts.
    pub(crate) fn peon_ping_logins(&self) -> Vec<&Login> {
        self.logins.iter().filter(|l| l.harness == Harness::Claude && matches!(l.own, Own::PeonPing(_))).collect()
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.entries().len();
        if n == 0 {
            self.sel = 0;
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n as isize - 1) as usize;
    }
}

/// The keys the view takes, for the hint lines.
pub(crate) const HINTS: &[(&str, &str)] = &[("s", "on / off"), ("enter", "use"), ("p", "preview"), ("tab", "installed / browse"), ("/", "search"), ("-/+", "volume"), ("t", "test"), ("d", "remove"), ("u", "update"), ("r", "refresh")];

/// Keys 1–8 flip the categories, c / x / i / m the harnesses.
pub(crate) const HARNESS_KEYS: [(Harness, char); 4] = [(Harness::Claude, 'c'), (Harness::Codex, 'x'), (Harness::Pi, 'i'), (Harness::Omp, 'm')];

pub(crate) fn harness_name(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "Claude Code",
        Harness::Codex => "Codex",
        Harness::Pi => "Pi",
        Harness::Omp => "omp",
    }
}

impl Pane for SoundsView {
    fn title(&self) -> String {
        "sounds".into()
    }
    fn icon(&self) -> &'static str {
        "sound"
    }
    fn view(&self) -> Option<&'static str> {
        Some("sounds")
    }
    fn poll(&mut self, cx: &mut Cx) {
        self.take_inbox(cx);
    }
    fn tick_every(&self) -> Option<Duration> {
        (!self.jobs.is_empty() || self.fetching || self.playing.is_some()).then_some(Duration::from_millis(120))
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        self.start(cx);
        let area = ui::hint_line(f, area, HINTS, t);
        let on = self.on(cx);
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        lines.push((None, vec![
            Span::styled(" SOUNDS ", ui::bold_accent(t)),
            Span::styled(format!("  {}", if on { "enabled" } else { "disabled" }), if on { fg(t.good).add_modifier(Modifier::BOLD) } else { muted(t) }),
            Span::styled(format!(" · volume {}% · pack {}", (self.cfg.volume() * 100.0).round(), self.active().unwrap_or("none")), muted(t)),
            // the demo shows no folder of this machine
            Span::styled(if cx.svc.is_demo() { String::new() } else { format!("  {}", peon::home().display()) }, muted(t)),
        ]));
        lines.push((None, vec![Span::styled("  sounds and packs provided by PeonPing · https://peonping.com", muted(t))]));
        // the moments, four a line
        for (n, chunk) in CATEGORIES.chunks(4).enumerate() {
            let mut cats = vec![Span::styled(if n == 0 { "  plays on " } else { "           " }, muted(t))];
            for (k, (cat, label)) in chunk.iter().enumerate() {
                let on = self.cfg.category_on(cat);
                cats.push(Span::styled(format!("{} {}   ", n * 4 + k + 1, label.to_lowercase()), if on { fg(t.inline) } else { muted(t).add_modifier(Modifier::CROSSED_OUT) }));
            }
            lines.push((None, cats));
        }
        let mut hs = vec![Span::styled("  hooks    ", muted(t))];
        for (h, key) in HARNESS_KEYS {
            let (text, wired) = self.harness_state(h, cx);
            hs.push(Span::styled(format!("{key} {}: ", harness_name(h)), muted(t)));
            hs.push(Span::styled(format!("{text}   "), if wired { fg(t.good) } else { muted(t) }));
        }
        lines.push((None, hs));
        let theirs = self.peon_ping_logins();
        if !theirs.is_empty() {
            let names: Vec<&str> = theirs.iter().map(|l| l.name.as_str()).collect();
            lines.push((None, vec![Span::styled(format!("  Claude Code's {} runs peon-ping's own scripts · o lets bro play them, in bro and out", names.join(", ")), fg(t.shine))]));
        }
        lines.push((None, vec![]));
        let tab = |label: &str, on: bool| Span::styled(format!(" {label} "), if on { ui::bold_accent(t).add_modifier(Modifier::REVERSED) } else { muted(t) });
        let count = self.registry.as_ref().map_or(String::new(), |r| format!(" ({})", r.len()));
        let mut bar = vec![tab(&format!("installed ({})", self.installed.len()), self.tab == Tab::Installed), Span::raw(" "), tab(&format!("browse{count}"), self.tab == Tab::Browse), Span::raw("   ")];
        if self.typing || !self.query.is_empty() {
            bar.push(Span::styled(format!("/ {}{}", self.query, if self.typing { "▏" } else { "" }), fg(t.inline)));
        }
        lines.push((None, bar));
        let entries = self.entries();
        self.sel = self.sel.min(entries.len().saturating_sub(1));
        let active = self.active().map(str::to_string);
        if entries.is_empty() {
            let why = match self.tab {
                _ if !self.scanned => "looking…".to_string(),
                Tab::Installed if self.installed.is_empty() => "no packs yet · tab to browse the registry".into(),
                Tab::Browse if self.registry.is_none() => self.registry_error.clone().map_or("fetching the registry…".into(), |e| format!("couldn't fetch the registry: {e}")),
                _ => "nothing matches".into(),
            };
            lines.push((None, vec![Span::styled(format!("  {why}"), muted(t))]));
        }
        for (i, e) in entries.iter().enumerate() {
            let name = self.name_of(*e);
            let sel = i == self.sel;
            let job = self.jobs.get(&name);
            let mark = match () {
                _ if job.is_some() => crate::views::settings::spinner(Instant::now()).to_string(),
                _ if active.as_deref() == Some(name.as_str()) => "★".into(),
                _ if e.installed.is_some() => "✓".into(),
                _ => " ".into(),
            };
            let detail = match job {
                Some((d, n)) => format!("installing {d}/{n}"),
                None if self.update_available(*e) => "update available".into(),
                None if self.playing.as_deref() == Some(name.as_str()) => "♪ playing".into(),
                None => self.meta(*e),
            };
            lines.push((Some(i), vec![
                Span::raw(format!("{} {mark} ", if sel { "▌" } else { " " })),
                Span::styled(ui::pad(&self.title_of(*e), 34), if sel { ui::bold() } else { ratatui::style::Style::default() }),
                Span::styled(format!(" {}", ui::pad(&name, 22)), muted(t)),
                Span::styled(format!(" {detail}"), if job.is_some() { fg(t.shine) } else { muted(t) }),
            ]));
        }
        if let Some(d) = self.selected().and_then(|e| self.description(e)) {
            lines.push((None, vec![]));
            lines.push((None, vec![Span::styled(format!("  {d}"), fg(t.inline))]));
        }

        let room = area.height as usize;
        let at = lines.iter().position(|(s, _)| *s == Some(self.sel)).unwrap_or(0);
        if at < self.scroll {
            self.scroll = at.saturating_sub(2);
        } else if at >= self.scroll + room {
            self.scroll = at + 1 - room;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        self.hits.clear();
        for (n, (sel, spans)) in lines.into_iter().skip(self.scroll).take(room).enumerate() {
            let r = Rect { y: area.y + n as u16, height: 1, ..area };
            if let Some(s) = sel {
                self.hits.push((r.y, s));
            }
            let spans = if sel == Some(self.sel) { spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect() } else { spans };
            ui::line(f, r, spans);
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        if self.typing {
            match key.code {
                KeyCode::Esc => {
                    self.typing = false;
                    self.query.clear();
                }
                KeyCode::Enter | KeyCode::Down => self.typing = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && self.query.len() < 60 => {
                    self.query.push(c);
                    // search finds packs to install
                    self.set_tab(Tab::Browse);
                    self.sel = 0;
                }
                _ => return false,
            }
            return true;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.move_sel(isize::MAX / 2),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => self.set_tab(if self.tab == Tab::Installed { Tab::Browse } else { Tab::Installed }),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Esc if !self.query.is_empty() => self.query.clear(),
            KeyCode::Enter => self.activate(cx),
            KeyCode::Char('p' | ' ') => self.preview(cx),
            KeyCode::Char('s') => self.toggle(cx),
            KeyCode::Char('t') => self.test(cx),
            KeyCode::Char('-') => self.volume(-0.1, cx),
            KeyCode::Char('+' | '=') => self.volume(0.1, cx),
            KeyCode::Char('d') | KeyCode::Delete => self.remove(cx),
            KeyCode::Char('u') => self.reinstall(cx),
            KeyCode::Char('o') => self.take_over(cx),
            KeyCode::Char('r') => {
                self.rescan(cx);
                self.fetch(true, cx);
            }
            KeyCode::Char(c @ '1'..='8') => self.toggle_category(c as usize - '1' as usize, cx),
            KeyCode::Char(c) if HARNESS_KEYS.iter().any(|(_, k)| *k == c) => {
                let h = HARNESS_KEYS.iter().find(|(_, k)| *k == c).unwrap().0;
                self.toggle_harness(h, cx);
            }
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        match ev.kind {
            MouseEventKind::ScrollDown => self.move_sel(1),
            MouseEventKind::ScrollUp => self.move_sel(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, s)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    if self.sel == s {
                        self.activate(cx);
                    }
                    self.sel = s;
                }
            }
            _ => {}
        }
    }
    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        if self.typing {
            self.query.push_str(text.lines().next().unwrap_or_default());
            while self.query.chars().count() > 60 {
                self.query.pop();
            }
        }
    }
}

impl Drop for SoundsView {
    fn drop(&mut self) {
        self.hush();
    }
}
