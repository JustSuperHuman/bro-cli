//! Project icons in the window: each project's picture ([`bro_core::project_icon`]) beside its name in the
//! sidebar, or its letter tile. A worker thread finds and decodes them and keeps each as a small PNG in
//! `~/.bro/project-icons/`, with an index of the picture it came from, so the next start shows every icon at once
//! while a fresh look in the project confirms it. Choosing a picture, the letter or going back to automatic runs on
//! the same worker, and the sidebar shows the change when it's done.
//!
//! A project with no picture (and no letter asked for) gets one drawn while `Settings::draw_project_icons` is on:
//! a second thread hands it to a signed-in agent ([`bro_core::icon_maker`]), one project at a time and only once
//! the window has settled, and the sidebar picks the icon up when it's done.
//!
//! The demo's projects aren't on disk, so each shows a symbol of its own ([`super::demo_icons`]) the way a drawn
//! icon shows: no project there is left with its letter.

use super::pick;
use crate::alerts::Kind;
use crate::app::App;
use crate::pane::Event;
use bro_core::Harness;
use bro_core::icon_maker::{self, Artist};
use bro_core::project_icon::{self, Icon};
use bro_gfx::{ImageId, Renderer};
use bro_media::Rgba;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

/// The side of the pictures kept: sharp for a 24 px icon on a 3x screen.
const SIDE: u32 = 72;
/// How the pictures kept are fitted ([`fitted`]): one a window fitted differently is fitted again.
const FIT: u32 = 1;
/// A symbol filling less of its app icon than this, on a plain background, is brought closer…
const SMALL: f32 = 0.66;
/// …until it fills this much: about what a drawn icon's symbol fills.
const CLOSER: f32 = 0.74;
/// A picture still decoding after this is given up: a broken file doesn't hold up the others.
const DECODE_LIMIT: Duration = Duration::from_secs(6);
/// Pictures offered when you choose one.
const OFFERS: usize = 6;
/// The widest (or tallest) a found picture may be and still be an icon: a social card is 1.9 times as wide.
const WIDEST: f32 = 1.6;
/// Nothing is drawn by itself until the window has been up this long: its own start comes first.
const SETTLE: Duration = Duration::from_secs(20);

/// What to draw for a project.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// not known yet: its letter, quietly
    Loading,
    Letter,
    /// its picture, its width over its height, and what it needs behind it
    Picture(ImageId, f32, Tone),
}

/// How a picture sits on its tile, so a dark logo doesn't vanish on a dark theme (nor a white one on a light theme).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// it fills its square (an app icon): drawn edge to edge
    Opaque,
    /// dark marks on clear: a light tile behind it on a dark theme
    Dark,
    /// light marks on clear: a dark tile behind it on a light theme
    Light,
    /// anything else: a quiet tile
    #[default]
    Mid,
    /// a drawn icon's white symbol: a tile in the project's colour
    Symbol,
}

impl Tone {
    /// As kept in the index ([`project_icon::Kept::tone`]), for the phone.
    fn name(self) -> &'static str {
        match self {
            Tone::Opaque => "opaque",
            Tone::Dark => "dark",
            Tone::Light => "light",
            Tone::Mid => "mid",
            Tone::Symbol => "symbol",
        }
    }
}

/// What `icon`'s picture `px` needs behind it: a drawn one is a white symbol, anything else as [`tone`] reads it.
fn tone_of(icon: &Icon, px: &Rgba) -> Tone {
    if matches!(icon, Icon::Made(_)) { Tone::Symbol } else { tone(px) }
}

/// What `px` needs behind it.
pub fn tone(px: &Rgba) -> Tone {
    let (mut solid, mut lum, mut weight) = (0usize, 0f32, 0f32);
    for c in px.px.chunks_exact(4) {
        let a = f32::from(c[3]) / 255.0;
        if c[3] > 200 {
            solid += 1;
        }
        lum += (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0 * a;
        weight += a;
    }
    let n = (px.w as usize * px.h as usize).max(1);
    if px.w.abs_diff(px.h) * 10 <= px.w.max(px.h) && solid * 100 >= n * 85 {
        return Tone::Opaque;
    }
    match lum / weight.max(f32::EPSILON) {
        _ if weight <= 0.0 => Tone::Mid,
        l if l < 0.3 => Tone::Dark,
        l if l > 0.82 => Tone::Light,
        _ => Tone::Mid,
    }
}

/// A change to a project's icon.
#[derive(Clone, Debug)]
pub enum Change {
    /// a picture from the system's file dialog
    Choose,
    /// this picture
    Use(PathBuf),
    /// the letter tile
    Letter,
    /// back to finding one
    Reset,
    /// one drawn for it now, by the agent (a fresh one when it has one)
    Draw,
}

#[derive(Default)]
struct Entry {
    /// the project's folder (empty for a scripted stand-in)
    root: PathBuf,
    icon: Option<Icon>,
    /// the picture as kept ([`SIDE`])
    full: Option<Arc<Rgba>>,
    /// …and as drawn: shrunk to the icon's size in pixels (the GPU's images have no smaller copies, so a big
    /// one drawn small would shimmer)
    shown: Option<Rgba>,
    tone: Tone,
    tex: Option<ImageId>,
    /// pictures in the project that would make a good icon (looked for when its menu opens)
    offers: Option<Vec<PathBuf>>,
    /// a change is under way
    busy: bool,
    /// an icon is being drawn for it: by whom ("" while it waits its turn)
    drawing: Option<String>,
}

enum Job {
    Look { key: String, root: PathBuf, cached: bool },
    Change { key: String, root: PathBuf, name: String, change: Change },
    Offers { key: String, root: PathBuf },
}

enum Done {
    Icon { key: String, icon: Icon, rgba: Option<Rgba> },
    Offers { key: String, list: Vec<PathBuf> },
    Changed { key: String },
    /// drawing its icon started (by whom) or ended
    Drawing { key: String, by: Option<String> },
}

/// Drawing missing icons, as the window has it: on or off (the setting), and who draws.
#[derive(Clone, Debug, Default)]
struct Drawer {
    on: bool,
    artist: Option<Artist>,
}

/// A project to draw an icon for; `asked`: you asked for it (now, and a fresh one).
struct Draw {
    key: String,
    root: PathBuf,
    asked: bool,
}

pub struct Icons {
    entries: HashMap<String, Entry>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    draws: Sender<Draw>,
    drawer: Arc<Mutex<Drawer>>,
    /// an icon's side in pixels on this screen
    side: u32,
}

/// An icon's side (logical px) the pictures are sized for: the sidebar's.
pub const ICON: f32 = 24.0;

impl Icons {
    /// `wake` runs after each answer (on the worker's thread); `events` takes its toasts; `persist` keeps the
    /// pictures in `~/.bro` (off in the demo); `demo` gives a project with no picture the demo's symbol for it.
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, events: Sender<Event>, persist: bool, demo: bool) -> Icons {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let (draws, drawn) = channel::<Draw>();
        let drawer = Arc::new(Mutex::new(Drawer::default()));
        {
            let (jobs, tx, drawer, wake, events) = (jobs.clone(), tx.clone(), drawer.clone(), wake.clone(), events.clone());
            let _ = std::thread::Builder::new().name("bro-project-icon-drawing".into()).spawn(move || drawing(drawn, jobs, tx, drawer, wake, events));
        }
        {
            let (draws, drawer) = (draws.clone(), drawer.clone());
            let _ = std::thread::Builder::new().name("bro-project-icons".into()).spawn(move || work(rx, tx, wake, events, persist, drawer, draws, demo));
        }
        Icons { entries: HashMap::new(), jobs, done, draws, drawer, side: ICON as u32 }
    }

    /// Whether missing icons are drawn, and by whom (each frame is fine: only a change does anything). Turning it on
    /// or off looks at every project again: a drawn icon shows only while it's on.
    pub fn set_drawing(&mut self, on: bool, artist: Option<Artist>) {
        let flipped = {
            let mut d = self.drawer.lock();
            let flipped = d.on != on;
            d.on = on;
            d.artist = artist;
            flipped
        };
        if flipped {
            for (key, e) in &self.entries {
                if !e.root.as_os_str().is_empty() {
                    let _ = self.jobs.send(Job::Look { key: key.clone(), root: e.root.clone(), cached: false });
                }
            }
        }
    }

    /// Missing icons are drawn (the setting is on).
    pub fn draws(&self) -> bool {
        self.drawer.lock().on
    }

    /// An icon is being drawn for project `key`: by whom ("" while it waits its turn).
    pub fn drawing(&self, key: &str) -> Option<&str> {
        self.entries.get(key)?.drawing.as_deref()
    }

    /// What to draw for project `key` at `root` (the first ask looks it up).
    pub fn look(&mut self, key: &str, root: &Path) -> Look {
        let jobs = &self.jobs;
        let e = self.entries.entry(key.to_string()).or_insert_with(|| {
            let _ = jobs.send(Job::Look { key: key.to_string(), root: root.to_path_buf(), cached: true });
            Entry { root: root.to_path_buf(), ..Entry::default() }
        });
        match (&e.icon, e.tex, &e.full) {
            (None, ..) => Look::Loading,
            (Some(_), Some(tex), Some(px)) => Look::Picture(tex, px.w as f32 / px.h.max(1) as f32, e.tone),
            (Some(_), None, Some(_)) => Look::Loading,
            (Some(_), _, None) => Look::Letter,
        }
    }

    /// What the project's icon is, once known.
    pub fn icon(&self, key: &str) -> Option<&Icon> {
        self.entries.get(key)?.icon.as_ref()
    }

    /// A change to the project's icon is under way.
    pub fn busy(&self, key: &str) -> bool {
        self.entries.get(key).is_some_and(|e| e.busy)
    }

    /// Look for pictures in the project that would make a good icon (a menu opening; the look is fresh each time).
    pub fn ask_offers(&mut self, key: &str, root: &Path) {
        let e = self.entries.entry(key.to_string()).or_default();
        e.root = root.to_path_buf();
        e.offers = None;
        let _ = self.jobs.send(Job::Offers { key: key.to_string(), root: root.to_path_buf() });
    }

    /// The pictures [`Icons::ask_offers`] found, once it has.
    pub fn offers(&self, key: &str) -> Option<&[PathBuf]> {
        self.entries.get(key)?.offers.as_deref()
    }

    /// Change project `key`'s icon (`name` for what's said about it).
    pub fn change(&mut self, key: &str, root: &Path, name: &str, change: Change) {
        if let Change::Draw = change {
            self.entries.entry(key.to_string()).or_default().drawing = Some(String::new());
            let _ = self.draws.send(Draw { key: key.to_string(), root: root.to_path_buf(), asked: true });
            return;
        }
        self.entries.entry(key.to_string()).or_default().busy = true;
        let _ = self.jobs.send(Job::Change { key: key.to_string(), root: root.to_path_buf(), name: name.to_string(), change });
    }

    /// Before drawing at `scale`: take the worker's answers and put new pictures on the GPU, sized for the screen.
    pub fn prepare(&mut self, r: &mut Renderer, scale: f32) {
        self.side = (ICON * scale).ceil().max(8.0) as u32;
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Icon { key, icon, rgba } => {
                    let e = self.entries.entry(key).or_default();
                    let full = rgba.map(Arc::new);
                    if full.as_deref() != e.full.as_deref() || e.icon.as_ref() != Some(&icon) {
                        e.shown = None;
                        e.tone = full.as_deref().map(|px| tone_of(&icon, px)).unwrap_or_default();
                    }
                    e.icon = Some(icon);
                    e.full = full;
                }
                Done::Offers { key, list } => self.entries.entry(key).or_default().offers = Some(list),
                Done::Changed { key } => self.entries.entry(key).or_default().busy = false,
                Done::Drawing { key, by } => self.entries.entry(key).or_default().drawing = by,
            }
        }
        let side = self.side;
        for e in self.entries.values_mut() {
            let Some(full) = &e.full else {
                if let Some(tex) = e.tex.take() {
                    r.remove_image(tex);
                }
                continue;
            };
            let fits = e.shown.as_ref().is_some_and(|s| s.w.max(s.h) == side.min(full.w.max(full.h)));
            if fits && e.tex.is_some() {
                continue;
            }
            if !fits {
                e.shown = Some(shrink(full, side));
            }
            let Some(px) = e.shown.as_ref() else { continue };
            match e.tex {
                Some(tex) => r.update_image(tex, px.w, px.h, &px.px),
                None => e.tex = Some(r.create_image(px.w, px.h, &px.px)),
            }
        }
    }

    /// The window has a new renderer and every texture went with the old one: they go up again.
    pub fn gpu_lost(&mut self) {
        for e in self.entries.values_mut() {
            e.tex = None;
        }
    }

    /// Show `icon` for `key` as if it had been found (the scripted captures' demo projects aren't on disk).
    pub fn inject(&mut self, key: &str, picture: &Path) {
        // known from now on: the sidebar's first look doesn't go looking on disk (and find nothing) after it
        self.entries.entry(key.to_string()).or_default();
        let _ = self.jobs.send(Job::Change { key: key.to_string(), root: PathBuf::new(), name: String::new(), change: Change::Use(picture.to_path_buf()) });
    }
}

/// The worker: one job at a time, in the order asked.
#[allow(clippy::too_many_arguments)]
fn work(rx: Receiver<Job>, tx: Sender<Done>, wake: Arc<dyn Fn() + Send + Sync>, events: Sender<Event>, persist: bool, drawer: Arc<Mutex<Drawer>>, draws: Sender<Draw>, demo: bool) {
    let mut kept = Kept::load(persist);
    let looks = |key: &str, root: &Path, cached: bool, kept: &mut Kept| {
        if look(key, root, cached, kept, &tx, &*wake, drawer.lock().on, demo) {
            let _ = draws.send(Draw { key: key.to_string(), root: root.to_path_buf(), asked: false });
        }
    };
    while let Ok(job) = rx.recv() {
        match job {
            Job::Look { key, root, cached } => looks(&key, &root, cached, &mut kept),
            // a scripted capture's stand-in: shown, not kept
            Job::Change { key, root, change: Change::Use(p), .. } if root.as_os_str().is_empty() => {
                // one from bro's drawn icons shows as drawn
                let icon = if p.parent().is_some_and(|d| bro_core::util::same_path(d, &icon_maker::dir())) { Icon::Made(p) } else { Icon::Set(p) };
                let rgba = icon.picture().and_then(decode).map(|px| fitted(&icon, px));
                let _ = tx.send(Done::Icon { key, icon, rgba });
            }
            Job::Change { key, root, name, change } => {
                match apply(&root, &name, change) {
                    Ok(Some(said)) => {
                        let _ = events.send(Event::Toast(Kind::Info, said));
                    }
                    Ok(None) => {}
                    Err(why) => {
                        let _ = events.send(Event::Toast(Kind::Error, why));
                    }
                }
                looks(&key, &root, false, &mut kept);
                let _ = tx.send(Done::Changed { key });
            }
            Job::Offers { key, root } => {
                let list = project_icon::candidates(&root, OFFERS);
                // the menu also confirms the icon shown
                looks(&key, &root, false, &mut kept);
                let _ = tx.send(Done::Offers { key, list });
            }
        }
        wake();
    }
}

/// Make a change: what to say about it, None when it was called off (the dialog cancelled).
fn apply(root: &Path, name: &str, change: Change) -> Result<Option<String>, String> {
    let set = |picture: &Path| {
        project_icon::set(root, picture)
            .map(|_| Some(format!("{name} has its new icon. It's saved in the project's .bro folder, so it stays put even after you close the project")))
            .map_err(|e| format!("That picture can't be {name}'s icon: {e}"))
    };
    match change {
        Change::Choose => match pick::picture(&format!("Pick an icon for {name}"), root) {
            Some(p) => set(&p),
            None => Ok(None),
        },
        Change::Use(p) => set(&p),
        Change::Letter => project_icon::use_letter(root).map(|_| Some(format!("{name} shows its letter now. Its pictures are all still there if you change your mind"))).map_err(|e| format!("Couldn't save {name}'s letter: {e}")),
        Change::Reset => project_icon::reset(root).map(|_| Some(format!("{name} finds its own icon again: its favicon, or its letter when it has none"))).map_err(|e| format!("Couldn't reset {name}'s icon: {e}")),
        // drawing runs on its own thread ([`Icons::change`])
        Change::Draw => Ok(None),
    }
}

/// Answer for one project: what's kept from last time first (quick), then a fresh look in the project, sent only
/// when it differs. With `drawn` (drawing is on) a project without a picture shows the icon drawn for it; true when
/// it has none yet, and should get one. In the `demo` such a project shows the demo's symbol for it instead (its
/// folder isn't there to draw from), and nothing is asked for.
#[allow(clippy::too_many_arguments)]
fn look(key: &str, root: &Path, cached: bool, kept: &mut Kept, tx: &Sender<Done>, wake: &dyn Fn(), drawn: bool, demo: bool) -> bool {
    let before = kept.get(key);
    if cached && let Some((icon, rgba)) = before.clone() {
        let _ = tx.send(Done::Icon { key: key.into(), icon, rgba });
        wake();
    }
    let made = || drawn.then(|| icon_maker::made(root)).flatten().map(Icon::Made);
    let mut icon = project_icon::resolve(root);
    if icon == (Icon::Letter { asked: false }) && let Some(m) = made() {
        icon = m;
    }
    if demo && icon == (Icon::Letter { asked: false }) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // it shows as a drawn icon does: a white symbol on a tile in the project's colour (the path names it, no
        // file is there)
        let icon = Icon::Made(PathBuf::from(format!("demo/{name}.svg")));
        if let Some(rgba) = super::demo_icons::picture(&name, SIDE * 2).map(|px| fitted(&icon, px)) {
            let _ = tx.send(Done::Icon { key: key.into(), icon, rgba: Some(rgba) });
            return false;
        }
    }
    let Some(picture) = icon.picture().map(Path::to_path_buf) else {
        if before.as_ref().map(|b| &b.0) != Some(&icon) || !cached {
            let _ = tx.send(Done::Icon { key: key.into(), icon: icon.clone(), rgba: None });
        }
        kept.put(key, &icon, None);
        return drawn && icon == (Icon::Letter { asked: false });
    };
    let stamp = stamp(&picture);
    if cached && before.as_ref().is_some_and(|b| b.0 == icon && b.1.is_some()) && kept.stamp(key) == stamp && kept.current(key) {
        return false;
    }
    // a found picture that won't decode gives way to the next one the project has, then to the one drawn for it
    let mut tries = vec![icon.clone()];
    if matches!(icon, Icon::Found(_)) {
        tries.extend(project_icon::candidates(root, 6).into_iter().map(Icon::Found).filter(|t| *t != icon));
        tries.extend(made());
    }
    for t in tries {
        // a found picture much wider than tall (a banner, a lockup) is no icon: the next one is
        let shaped = |px: &Rgba| !matches!(t, Icon::Found(_)) || (px.w as f32 / px.h.max(1) as f32).clamp(0.0, 9.0) <= WIDEST && (px.h as f32 / px.w.max(1) as f32) <= WIDEST;
        if let Some(rgba) = t.picture().and_then(decode).filter(shaped).map(|px| fitted(&t, px)) {
            kept.put(key, &t, Some(&rgba));
            let _ = tx.send(Done::Icon { key: key.into(), icon: t, rgba: Some(rgba) });
            return false;
        }
    }
    let letter = Icon::Letter { asked: false };
    kept.put(key, &letter, None);
    let _ = tx.send(Done::Icon { key: key.into(), icon: letter, rgba: None });
    drawn
}

/// The drawing thread: one project at a time, what you asked for first, the rest quietly once the window has
/// settled; each project at most once a window unless you ask (and a failure waits days, see
/// [`icon_maker::RETRY_AFTER`]). The icon worker looks again when one is done.
fn drawing(rx: Receiver<Draw>, looks: Sender<Job>, tx: Sender<Done>, drawer: Arc<Mutex<Drawer>>, wake: Arc<dyn Fn() + Send + Sync>, events: Sender<Event>) {
    let started = Instant::now();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<Draw> = VecDeque::new();
    let say = |kind: Kind, text: String| {
        let _ = events.send(Event::Toast(kind, text));
    };
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(d) => queue.push_back(d),
                Err(_) => return,
            }
        }
        while let Ok(d) = rx.try_recv() {
            if d.asked {
                queue.push_front(d);
            } else {
                queue.push_back(d);
            }
        }
        let Some(d) = queue.pop_front() else { continue };
        if !d.asked {
            if seen.contains(&d.key) {
                continue;
            }
            if let Some(wait) = SETTLE.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let name = d.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| d.key.clone());
        let Drawer { on, artist } = drawer.lock().clone();
        let skip = |key: String| {
            let _ = tx.send(Done::Drawing { key, by: None });
            wake();
        };
        // nobody to draw it (yet): a login signed in later draws it on the next look
        let Some(artist) = artist.filter(|_| on) else {
            if d.asked {
                say(Kind::Info, format!("bro draws icons with a signed-in Claude or Codex, and there isn't one: sign in from profiles (alt+o), then ask again for {name}'s"));
            }
            skip(d.key);
            continue;
        };
        if !d.asked {
            seen.insert(d.key.clone());
            if !d.root.is_dir() || project_icon::too_broad(&d.root) || icon_maker::made(&d.root).is_some() || icon_maker::failed_recently(&d.root) {
                skip(d.key);
                continue;
            }
        }
        let _ = tx.send(Done::Drawing { key: d.key.clone(), by: Some(artist.label.clone()) });
        wake();
        let t = Instant::now();
        match icon_maker::draw(&artist, &d.root) {
            Ok(Some(_)) => {
                crate::perf::note(&format!("project icon: {} drew {} in {:.0}s", artist.label, d.root.display(), t.elapsed().as_secs_f32()));
                if d.asked {
                    say(Kind::Info, format!("{name} has a new icon, drawn by {}", artist.label));
                }
            }
            // another window is on it: it shows here once that one's done
            Ok(None) => {}
            Err(why) => {
                crate::perf::note(&format!("project icon: {} couldn't draw {}: {why}", artist.label, d.root.display()));
                icon_maker::note_failure(&d.root, &why);
                if d.asked {
                    say(Kind::Error, format!("Couldn't draw {name}'s icon: {why}"));
                }
            }
        }
        let _ = tx.send(Done::Drawing { key: d.key.clone(), by: None });
        let _ = looks.send(Job::Look { key: d.key, root: d.root, cached: false });
        wake();
    }
}

/// Who draws missing icons: the login you last launched Claude or Codex on (still signed in), else the one with
/// the most usage left (as the watchdog picks its reviewer). None without a signed-in Claude or Codex.
pub fn artist(app: &App) -> Option<Artist> {
    let st = app.svc.state();
    let profiles = st.profiles.ready()?;
    let usable = |p: &bro_core::profiles::Profile| p.authenticated && (p.is_claude() || p.is_codex());
    let recent = app.recents.iter().filter(|r| matches!(r.harness, Harness::Claude | Harness::Codex) && r.provider_id.is_none() && !r.orchestrator).find_map(|r| {
        let claude = r.harness == Harness::Claude;
        // no login named: the machine's own
        let id = r.profile_id.clone().filter(|id| !id.is_empty()).unwrap_or_else(|| if claude { "claude:local".into() } else { "codex:local".into() });
        profiles.iter().find(|p| p.id == id && usable(p) && p.is_claude() == claude).map(|p| (p, r.model.clone()))
    });
    let (p, model) = match recent {
        Some(found) => found,
        None => (crate::watchdog::pick_reviewer(profiles, &st.usage, &bro_core::config::WatchdogSettings::default())?.0, None),
    };
    let launch = crate::watchdog::reviewer_launch(p, &bro_core::config::WatchdogSettings { model: String::new(), ..Default::default() });
    let (harness, family) = if p.is_codex() { (Harness::Codex, "Codex") } else { (Harness::Claude, "Claude") };
    Some(Artist { harness, program: launch.program, env: launch.env, env_remove: launch.env_remove, model, label: format!("{family} · {}", p.name) })
}

/// The picture at `p`, fitted in twice [`SIDE`] (room to trim it, see [`fitted`]); None when it won't decode (or
/// takes too long, or the decoder fails).
fn decode(p: &Path) -> Option<Rgba> {
    let (tx, rx) = channel();
    let path = p.to_path_buf();
    std::thread::Builder::new().name("bro-project-icon-decode".into()).spawn(move || {
        let _ = tx.send(bro_media::thumbnail(&path, SIDE * 2));
    }).ok()?;
    match rx.recv_timeout(DECODE_LIMIT) {
        Ok(Ok(px)) if px.w > 0 && px.h > 0 => Some(px),
        Ok(_) | Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
    }
}

/// `src` fitted inside `side` x `side`, each pixel the average of what it covers (alpha-weighted, so a transparent
/// edge doesn't darken); as it was when it fits already.
fn shrink(src: &Rgba, side: u32) -> Rgba {
    let k = (side as f32 / src.w.max(src.h).max(1) as f32).min(1.0);
    let (w, h) = (((src.w as f32 * k).round() as u32).max(1), ((src.h as f32 * k).round() as u32).max(1));
    if (w, h) == (src.w, src.h) {
        return src.clone();
    }
    let (sx, sy) = (src.w as f32 / w as f32, src.h as f32 / h as f32);
    let mut px = vec![0u8; Rgba::byte_len(w, h)];
    for y in 0..h {
        let (y0, y1) = (y as f32 * sy, (y + 1) as f32 * sy);
        for x in 0..w {
            let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
            let (mut acc, mut cover) = ([0f32; 4], 0f32);
            for yy in (y0.floor() as u32)..(y1.ceil() as u32).min(src.h) {
                let wy = (y1.min(yy as f32 + 1.0) - y0.max(yy as f32)).max(0.0);
                for xx in (x0.floor() as u32)..(x1.ceil() as u32).min(src.w) {
                    let wt = wy * (x1.min(xx as f32 + 1.0) - x0.max(xx as f32)).max(0.0);
                    let p = src.pixel(xx, yy);
                    let a = f32::from(p[3]) / 255.0 * wt;
                    for c in 0..3 {
                        acc[c] += f32::from(p[c]) * a;
                    }
                    acc[3] += a;
                    cover += wt;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            if acc[3] > 0.0 {
                for c in 0..3 {
                    px[i + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            px[i + 3] = (acc[3] / cover.max(f32::EPSILON) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Rgba { w, h, px }
}

/// `icon`'s decoded picture as it's kept: fitted in [`SIDE`], and trimmed unless it was drawn (a drawn symbol sits
/// where bro put it).
fn fitted(icon: &Icon, px: Rgba) -> Rgba {
    let px = if matches!(icon, Icon::Made(_)) { px } else { trim(px) };
    shrink(&px, SIDE)
}

/// A found or chosen picture at its best. The see-through margin around it goes, so a logo fills its room and an
/// app icon with see-through corners fills its tile. And an app icon whose symbol sits small on a plain background
/// comes in closer, so the symbol reads as large as a drawn icon's.
fn trim(px: Rgba) -> Rgba {
    let Some((x0, y0, x1, y1)) = bounds(&px, |p| p[3] >= 128) else { return px };
    let px = crop(&px, x0, y0, x1 - x0, y1 - y0);
    if tone(&px) != Tone::Opaque {
        return px;
    }
    let Some(bg) = plain_edge(&px) else { return px };
    let Some((cx0, cy0, cx1, cy1)) = bounds(&px, |p| p[3] > 200 && p[..3].iter().zip(bg).any(|(a, b)| a.abs_diff(b) > 40)) else { return px };
    let side = px.w.min(px.h);
    let symbol = (cx1 - cx0).max(cy1 - cy0);
    if symbol as f32 >= side as f32 * SMALL {
        return px;
    }
    let want = ((symbol as f32 / CLOSER).ceil() as u32).clamp(1, side);
    let x = ((cx0 + cx1) / 2).saturating_sub(want / 2).min(px.w - want);
    let y = ((cy0 + cy1) / 2).saturating_sub(want / 2).min(px.h - want);
    let mut out = crop(&px, x, y, want, want);
    // its own rounded corners may reach in: the background fills them, and the tile rounds it again
    for c in out.px.chunks_exact_mut(4) {
        let a = u16::from(c[3]);
        for i in 0..3 {
            c[i] = ((u16::from(c[i]) * a + u16::from(bg[i]) * (255 - a)) / 255) as u8;
        }
        c[3] = 255;
    }
    out
}

/// The smallest box (x0, y0, x1, y1; the ends past it) around the pixels that are `wanted`.
fn bounds(px: &Rgba, wanted: impl Fn([u8; 4]) -> bool) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..px.h {
        for x in 0..px.w {
            if wanted(px.pixel(x, y)) {
                b = Some(b.map_or((x, y, x + 1, y + 1), |(x0, y0, x1, y1)| (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1))));
            }
        }
    }
    b
}

/// The `w` x `h` part of `px` at (`x`, `y`).
fn crop(px: &Rgba, x: u32, y: u32, w: u32, h: u32) -> Rgba {
    if (x, y, w, h) == (0, 0, px.w, px.h) {
        return px.clone();
    }
    let mut out = Vec::with_capacity(Rgba::byte_len(w, h));
    for row in y..y + h {
        let at = ((row * px.w + x) * 4) as usize;
        out.extend_from_slice(&px.px[at..at + (w * 4) as usize]);
    }
    Rgba { w, h, px: out }
}

/// The colour all round `px`'s edge, when it's one plain colour (an app icon's background, not a photo's).
fn plain_edge(px: &Rgba) -> Option<[u8; 3]> {
    let mut ring = vec![];
    for y in 0..px.h {
        for x in 0..px.w {
            let edge = x < 2 || y < 2 || x + 2 >= px.w || y + 2 >= px.h;
            let p = px.pixel(x, y);
            if edge && p[3] > 200 {
                ring.push([p[0], p[1], p[2]]);
            }
        }
    }
    if ring.len() < 16 {
        return None;
    }
    let median = |i: usize| {
        let mut v: Vec<u8> = ring.iter().map(|p| p[i]).collect();
        v.sort_unstable();
        v[v.len() / 2]
    };
    let bg = [median(0), median(1), median(2)];
    let near = ring.iter().filter(|p| p.iter().zip(bg).all(|(a, b)| a.abs_diff(b) <= 24)).count();
    (near * 10 >= ring.len() * 9).then_some(bg)
}

/// When the file at `p` last changed (seconds), 0 when unknown.
fn stamp(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// One project's icon as kept in `~/.bro/project-icons/index.json` (the phone's bridge reads it too).
type Record = project_icon::Kept;

/// The icons kept from last time (nothing kept when not persisting).
struct Kept {
    dir: Option<PathBuf>,
    index: HashMap<String, Record>,
}

impl Kept {
    fn load(persist: bool) -> Kept {
        let dir = persist.then(|| crate::util::bro_dir().join(project_icon::KEPT_DIR));
        let index = dir.as_deref().map(project_icon::read_kept).unwrap_or_default();
        Kept { dir, index }
    }

    /// What was kept for `key`: its icon, and its picture.
    fn get(&self, key: &str) -> Option<(Icon, Option<Rgba>)> {
        let r = self.index.get(key)?;
        let icon = match (r.kind.as_str(), &r.source) {
            ("set", Some(p)) => Icon::Set(p.clone()),
            ("found", Some(p)) => Icon::Found(p.clone()),
            ("made", Some(p)) => Icon::Made(p.clone()),
            ("asked", _) => Icon::Letter { asked: true },
            _ => Icon::Letter { asked: false },
        };
        if icon.picture().is_none() {
            return Some((icon, None));
        }
        let px = bro_media::thumbnail(&self.dir.as_ref()?.join(r.file.as_ref()?), SIDE).ok()?;
        Some((icon, Some(px)))
    }

    fn stamp(&self, key: &str) -> u64 {
        self.index.get(key).map_or(0, |r| r.stamp)
    }

    /// Whether `key`'s picture was kept the way this window keeps them: with its tone (the phone needs it) and
    /// fitted as [`fitted`] fits them now. An older window's is redone.
    fn current(&self, key: &str) -> bool {
        self.dir.is_none() || self.index.get(key).is_some_and(|r| r.tone.is_some() && r.fit == FIT)
    }

    /// Keep `icon` (and its picture) for `key`.
    fn put(&mut self, key: &str, icon: &Icon, rgba: Option<&Rgba>) {
        let Some(dir) = self.dir.clone() else { return };
        let (kind, source) = match icon {
            Icon::Set(p) => ("set", Some(p.clone())),
            Icon::Found(p) => ("found", Some(p.clone())),
            Icon::Made(p) => ("made", Some(p.clone())),
            Icon::Letter { asked: true } => ("asked", None),
            Icon::Letter { asked: false } => ("letter", None),
        };
        let file = format!("{:016x}.png", fnv(key));
        let mut record = Record { kind: kind.into(), stamp: source.as_deref().map_or(0, stamp), source, ..Record::default() };
        match rgba.and_then(|px| Some((png_bytes(px)?, tone_of(icon, px)))) {
            Some((bytes, tone)) if std::fs::create_dir_all(&dir).is_ok() && crate::util::atomic_write(&dir.join(&file), &bytes).is_ok() => {
                record.file = Some(file);
                record.tone = Some(tone.name().into());
                record.fit = FIT;
            }
            _ => {
                let _ = std::fs::remove_file(dir.join(&file));
            }
        }
        let same = self.index.get(key) == Some(&record);
        self.index.insert(key.to_string(), record);
        if !same && let Ok(text) = serde_json::to_vec_pretty(&self.index) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = crate::util::atomic_write(&project_icon::kept_index(&dir), &text);
        }
    }
}

/// A picture as PNG.
fn png_bytes(px: &Rgba) -> Option<Vec<u8>> {
    let mut out = vec![];
    {
        let mut enc = png::Encoder::new(&mut out, px.w, px.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&px.px).ok()?;
    }
    Some(out)
}

/// A stable name for a project's kept picture (FNV-1a of its key).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_averages_what_each_pixel_covers() {
        // a 4x2 picture: the left half opaque white, the right half see-through red
        let mut px = vec![];
        for _ in 0..2 {
            px.extend([255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0]);
        }
        let src = Rgba { w: 4, h: 2, px };
        let out = shrink(&src, 2);
        assert_eq!((out.w, out.h), (2, 1), "fitted, keeping its shape");
        assert_eq!(out.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(out.pixel(1, 0)[3], 0, "nothing but see-through stays see-through");
        // half white, half clear: half as solid, and still white (not darkened by the clear pixels)
        let mixed = Rgba { w: 2, h: 1, px: vec![255, 255, 255, 255, 0, 0, 0, 0] };
        assert_eq!(shrink(&mixed, 1).pixel(0, 0), [255, 255, 255, 128]);
        // already small enough: as it was
        assert_eq!(shrink(&src, 8), src);
    }

    #[test]
    fn a_picture_gets_the_tile_it_reads_on() {
        let fill = |w: u32, h: u32, f: &dyn Fn(u32, u32) -> [u8; 4]| {
            let mut px = vec![];
            for y in 0..h {
                for x in 0..w {
                    px.extend(f(x, y));
                }
            }
            Rgba { w, h, px }
        };
        // an app icon: solid edge to edge
        assert_eq!(tone(&fill(10, 10, &|_, _| [200, 40, 40, 255])), Tone::Opaque);
        // a black mark on clear (a wordmark logo), a white one, a coloured one
        let mark = |c: [u8; 4]| fill(10, 10, &move |x, _| if x < 4 { c } else { [0, 0, 0, 0] });
        assert_eq!(tone(&mark([10, 10, 10, 255])), Tone::Dark);
        assert_eq!(tone(&mark([250, 250, 250, 255])), Tone::Light);
        assert_eq!(tone(&mark([40, 160, 200, 255])), Tone::Mid);
        // solid but a wide banner, not a square: on its tile
        assert_eq!(tone(&fill(30, 10, &|_, _| [10, 10, 10, 255])), Tone::Dark);
        assert_eq!(tone(&fill(4, 4, &|_, _| [0, 0, 0, 0])), Tone::Mid);
    }

    fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Rgba {
        let mut px = vec![];
        for y in 0..h {
            for x in 0..w {
                px.extend(f(x, y));
            }
        }
        Rgba { w, h, px }
    }

    #[test]
    fn pictures_are_trimmed_to_fill_their_tile() {
        let inside = |x: u32, y: u32, at: u32, side: u32| x >= at && x < at + side && y >= at && y < at + side;
        // an app icon with a see-through margin: just the icon, edge to edge
        let tile = trim(picture(100, 100, |x, y| if inside(x, y, 10, 80) { [200, 40, 40, 255] } else { [0, 0, 0, 0] }));
        assert_eq!((tile.w, tile.h, tone(&tile)), (80, 80, Tone::Opaque));
        // a logo on clear: just the logo, still on clear
        let logo = trim(picture(100, 100, |x, y| if x >= 20 && x < 80 && y >= 40 && y < 60 { [10, 10, 10, 255] } else { [0, 0, 0, 0] }));
        assert_eq!((logo.w, logo.h, tone(&logo)), (60, 20, Tone::Dark));
        // a small symbol on a plain background: brought closer, to about what a drawn symbol fills
        let small = trim(picture(100, 100, |x, y| if inside(x, y, 30, 40) { [255, 255, 255, 255] } else { [9, 10, 13, 255] }));
        assert_eq!((small.w, small.h), (55, 55));
        assert_eq!(small.pixel(0, 0), [9, 10, 13, 255], "on its own background");
        assert_eq!(bounds(&small, |p| p[0] > 128).map(|(x0, _, x1, _)| x1 - x0), Some(40), "all of the symbol");
        // its own rounded corners fall away as it comes closer: the background fills what's left of them
        let rounded = trim(picture(100, 100, |x, y| if inside(x, y, 12, 10) { [255, 255, 255, 255] } else if x + y < 30 { [0, 0, 0, 0] } else { [9, 10, 13, 255] }));
        assert_eq!((rounded.w, rounded.pixel(0, 0)), (14, [9, 10, 13, 255]));
        assert!(rounded.px.chunks_exact(4).all(|c| c[3] == 255));
        // a symbol that already fills its icon, and a photo (no plain edge), are as they were
        let big = picture(100, 100, |x, y| if inside(x, y, 15, 70) { [255, 255, 255, 255] } else { [9, 10, 13, 255] });
        assert_eq!(trim(big.clone()), big);
        let photo = picture(100, 100, |x, y| if inside(x, y, 45, 10) { [255, 255, 255, 255] } else { [(x * 2) as u8, (y * 2) as u8, 90, 255] });
        assert_eq!(trim(photo.clone()), photo);
        // nothing there: as it was
        let clear = picture(4, 4, |_, _| [0, 0, 0, 0]);
        assert_eq!(trim(clear.clone()), clear);
    }

    #[test]
    fn a_project_without_a_picture_shows_the_icon_drawn_for_it_while_drawing_is_on() {
        crate::testkit::isolate();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let (tx, rx) = channel();
        let mut kept = Kept::load(false);
        let answer = |drawn: bool, kept: &mut Kept| {
            while rx.try_recv().is_ok() {}
            let wants = look("k", root.path(), false, kept, &tx, &|| {}, drawn, false);
            let Ok(Done::Icon { icon, rgba, .. }) = rx.try_recv() else { panic!("an answer") };
            (wants, icon, rgba)
        };
        // nothing in it, nothing drawn yet: its letter, and one wanted (but not while drawing is off)
        assert_eq!(answer(true, &mut kept).0, true);
        assert_eq!(answer(false, &mut kept).0, false);
        assert_eq!(answer(true, &mut kept).1, Icon::Letter { asked: false });
        // drawn: shown, a white symbol for the project's colour to go behind, where bro put it (not trimmed)
        let svg = icon_maker::compose(&icon_maker::Drawing { shapes: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>".into() });
        let at = icon_maker::store(root.path(), &svg).unwrap();
        let (wants, icon, rgba) = answer(true, &mut kept);
        assert!(!wants);
        assert_eq!(icon, Icon::Made(at));
        let px = rgba.expect("its picture");
        assert_eq!(tone_of(&icon, &px), Tone::Symbol);
        assert_eq!((px.w, px.h), (SIDE, SIDE));
        assert_eq!(px.pixel(1, 1)[3], 0, "clear round the symbol");
        // off: the letter again; the letter you asked for wins too, and wants nothing drawn
        assert_eq!(answer(false, &mut kept).1, Icon::Letter { asked: false });
        project_icon::use_letter(root.path()).unwrap();
        assert_eq!(answer(true, &mut kept), (false, Icon::Letter { asked: true }, None));
        // a favicon of its own beats it
        project_icon::reset(root.path()).unwrap();
        std::fs::write(root.path().join("favicon.svg"), icon_maker::compose(&icon_maker::Drawing { shapes: "<path d=\"M4 4h16\"/>".into() })).unwrap();
        assert!(matches!(answer(true, &mut kept).1, Icon::Found(_)));
        icon_maker::forget(root.path());
    }

    #[test]
    fn kept_icons_round_trip_through_the_index_and_their_pngs() {
        let d = tempfile::tempdir().unwrap();
        let mut kept = Kept { dir: Some(d.path().join("project-icons")), index: HashMap::new() };
        let px = Rgba { w: 2, h: 1, px: vec![255, 0, 0, 255, 0, 0, 255, 128] };
        let src = d.path().join("favicon.png");
        std::fs::write(&src, b"x").unwrap();
        kept.put("f:/site", &Icon::Found(src.clone()), Some(&px));
        kept.put("f:/plain", &Icon::Letter { asked: true }, None);
        // a new window reads what the last one kept
        let again = Kept { dir: kept.dir.clone(), index: serde_json::from_str(&std::fs::read_to_string(d.path().join("project-icons").join("index.json")).unwrap()).unwrap() };
        let (icon, back) = again.get("f:/site").expect("kept");
        assert_eq!(icon, Icon::Found(src.clone()));
        assert_eq!(back.as_ref().map(|b| (b.w, b.h, b.px.clone())), Some((2, 1, px.px.clone())));
        assert_eq!(again.stamp("f:/site"), stamp(&src));
        assert_eq!(again.get("f:/plain").map(|k| k.0), Some(Icon::Letter { asked: true }));
        assert!(again.get("f:/never").is_none());
        // the letter later: its picture goes
        kept.put("f:/site", &Icon::Letter { asked: false }, None);
        assert!(!d.path().join("project-icons").join(format!("{:016x}.png", fnv("f:/site"))).exists());
        // the demo and tests keep nothing
        let mut none = Kept::load(false);
        none.put("f:/site", &Icon::Found(src), Some(&px));
        assert!(none.get("f:/site").is_none());
    }
}
