//! The media sidebar: photos, animations, video and audio that agents open, send or make — in a panel beside the
//! sessions instead of another app.
//!
//! Where items come from: an agent's `bro open`, a clicked file link and prefix f on a media file (the file preview
//! hands media here), files an agent sends you (Claude's SendUserFile) or writes, creates or looks at, images pasted
//! with alt+v (and the last half day of them on start), files dropped on the window. Newest first, once each. The
//! panel shows the selected one large (zoom and pan for images, a player for video and audio) and the rest as a
//! gallery, for the session in focus or its whole project. Nothing here ever runs a file.

mod loader;

use super::actions::Action;
use super::kit::{Cursor, Style, Ui, size, weight, widgets};
use crate::layout::PaneId;
use bro_gfx::text::Align;
use bro_gfx::{Color, ImageId, Point, Rect, Renderer};
use bro_media::{MediaKind, Player, Rgba};
use loader::{Done, Job, Loader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Where an item came from. Lower is a stronger reason to have it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `bro open`, a clicked link, prefix f
    Opened,
    /// the agent sent it to you
    Sent,
    /// the agent wrote, created or edited it
    Made,
    /// the agent looked at it
    Viewed,
    /// alt+v
    Pasted,
    /// dropped on the window
    Dropped,
}

impl Source {
    fn verb(self) -> &'static str {
        match self {
            Source::Opened => "opened by",
            Source::Sent => "sent by",
            Source::Made => "made by",
            Source::Viewed => "looked at by",
            Source::Pasted => "pasted into",
            Source::Dropped => "dropped on",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Source::Opened => "\u{F03CC}",
            Source::Sent => "\u{F048A}",
            Source::Made => "\u{F03EB}",
            Source::Viewed => "\u{F0208}",
            Source::Pasted => "\u{F0147}",
            Source::Dropped => "\u{F01DA}",
        }
    }
}

/// The session an item belongs to.
#[derive(Clone, Debug, Default)]
pub struct Owner {
    pub pane: Option<PaneId>,
    /// bridge session id
    pub sid: String,
    /// "claude · work · opus-5" (or the session's name)
    pub who: String,
    /// project key and display name
    pub project: String,
    pub place: String,
    pub cwd: PathBuf,
}

/// The owner of a session pane, when it is one.
pub fn owner(app: &crate::app::App, id: PaneId) -> Option<Owner> {
    let t = app.panes.get(&id)?.as_term_ref()?;
    Some(Owner {
        pane: Some(id),
        sid: t.meta.sid.clone(),
        who: t.display_name(),
        project: t.meta.project.key.clone(),
        place: t.meta.project.name.clone(),
        cwd: t.meta.cwd.clone(),
    })
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub kind: MediaKind,
    /// None: not from a session (dropped, or pasted before this run)
    pub owner: Option<Owner>,
    pub source: Source,
    pub at: SystemTime,
    pub caption: Option<String>,
    /// the file's modification time when last seen (a rewrite refreshes the thumbnail and view)
    mtime: Option<SystemTime>,
}

enum Thumb {
    Loading,
    Ready(ImageId, u32, u32),
    Failed,
}

enum View {
    None,
    Loading(PathBuf),
    /// `asked`: the largest size requested for it (a zoomed-in still is decoded again, sharper)
    Image { path: PathBuf, frames: Vec<(Rgba, Duration)>, frame: usize, next: Instant, tex: Option<ImageId>, dirty: bool, original: (u32, u32), format: String, asked: u32 },
    Media { path: PathBuf, player: Box<Player>, tex: Option<ImageId>, size: (u32, u32) },
    Failed { path: PathBuf, message: String },
}

impl View {
    fn path(&self) -> Option<&Path> {
        match self {
            View::None => None,
            View::Loading(p) | View::Image { path: p, .. } | View::Media { path: p, .. } | View::Failed { path: p, .. } => Some(p),
        }
    }
}

/// What a click or key asks of the workspace.
pub enum Ask {
    None,
    Toast(String),
    /// type `@path ` into this pane (else the focused one)
    Mention(PathBuf, Option<PaneId>),
    /// open with the system's app
    Open(PathBuf),
    Reveal(PathBuf),
    Close,
}

/// Clickable things in the panel this frame.
#[derive(Clone, Debug)]
enum Target {
    Close,
    Collapse,
    Scope(bool),
    Thumb(PathBuf),
    Viewer(Rect),
    Play,
    Seek(Rect),
    Volume(Rect),
    Mute,
    Loop,
    Copy,
    Mention,
    Open,
    Reveal,
    Remove,
    Prev,
    Next,
    Gallery,
}

#[derive(Clone, Copy)]
enum DragKind {
    Seek(Rect),
    Volume(Rect),
    Pan(Point, Point),
}

pub struct Media {
    pub items: Vec<Item>,
    pub sel: Option<PathBuf>,
    /// the whole project's media (else the focused session's)
    pub whole_project: bool,
    /// keys go to the panel (it was clicked last)
    pub focus: bool,
    /// The selected media fills the workspace until Escape or another double-click.
    pub expanded: bool,
    /// new items since the panel was last looked at
    pub unseen: usize,
    thumbs: HashMap<PathBuf, (Thumb, Option<SystemTime>)>,
    view: View,
    loader: Loader,
    /// 1 = fit; above zooms in around `pan`
    zoom: f32,
    pan: Point,
    muted: bool,
    looping: bool,
    volume: f32,
    /// shown and the window focused: animations and video run only then
    active: bool,
    /// playing when the panel went away: play again when it's back
    resume: bool,
    /// where each video or song was left, for this run
    positions: HashMap<PathBuf, Duration>,
    targets: Vec<Target>,
    gallery_scroll: f32,
    gallery_view: Rect,
    gallery_content: f32,
    /// insights revision per pane already read for sent / touched files
    revs: HashMap<PaneId, u64>,
    free: Vec<ImageId>,
    drag: Option<DragKind>,
    /// The first viewer click's playback state, restored when it becomes a double-click.
    viewer_was_playing: Option<bool>,
    viewer: Rect,
    /// logical → physical pixels, as of the last draw
    scale: f32,
    /// wakes the window (a decoded frame is ready)
    wake: Arc<dyn Fn() + Send + Sync>,
    /// numbers the view's loads: only the newest one's result is shown
    req: u64,
    /// removed from the gallery (path key → when): the same file comes back only with something newer
    hidden: HashMap<String, SystemTime>,
    /// the last `bro open` already seen
    opened: Option<(PaneId, PathBuf)>,
    /// when the shown files were last checked for changes on disk
    checked: Instant,
}

/// Pictures are first decoded to fit this many pixels; zooming in on a bigger one decodes it again, up to
/// [`SHARP_SIDE`] (the largest texture).
const VIEW_SIDE: u32 = 2560;
const SHARP_SIDE: u32 = 8192;
/// Zooming in stops here, or at twice the picture's own pixels when that's further.
const MAX_ZOOM: f32 = 16.0;

/// Media the panel can show: pictures, video, audio, and PDFs (a card that opens them in their app).
pub fn is_media(path: &Path) -> bool {
    matches!(bro_media::kind_of(path), MediaKind::Image | MediaKind::Video | MediaKind::Audio | MediaKind::Pdf)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// One file however it was spelled (`\\?\`, slashes, case on Windows).
fn key(path: &Path) -> String {
    bro_core::util::path_key(&crate::util::strip_verbatim(path.to_path_buf()))
}

/// The file's type as the bridge names it (`image/png`, `video/mp4`), when it has one.
fn mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext == "pdf" {
        return Some("application/pdf");
    }
    bro_bridge::file_preview::image_mime(&ext).or_else(|| bro_bridge::file_preview::media_mime(&ext))
}

/// A transcript fact's time.
fn fact_time(at_ms: Option<i64>) -> Option<SystemTime> {
    at_ms.filter(|ms| *ms > 0).map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms as u64))
}

impl Media {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Media {
        Media {
            items: vec![],
            sel: None,
            whole_project: false,
            focus: false,
            expanded: false,
            unseen: 0,
            thumbs: HashMap::new(),
            view: View::None,
            loader: Loader::new(wake.clone()),
            zoom: 1.0,
            pan: Point::default(),
            muted: true,
            looping: true,
            volume: 1.0,
            active: true,
            resume: false,
            positions: HashMap::new(),
            targets: vec![],
            gallery_scroll: 0.0,
            gallery_view: Rect::ZERO,
            gallery_content: 0.0,
            revs: HashMap::new(),
            free: vec![],
            drag: None,
            viewer_was_playing: None,
            viewer: Rect::ZERO,
            scale: 1.0,
            wake,
            req: 0,
            hidden: HashMap::new(),
            opened: None,
            checked: Instant::now(),
        }
    }

    /// Add `path` (newest first, once each). Returns true when it's new.
    pub fn add(&mut self, path: PathBuf, source: Source, owner: Option<Owner>, caption: Option<String>) -> bool {
        self.add_at(path, source, owner, caption, SystemTime::now())
    }

    fn add_at(&mut self, path: PathBuf, source: Source, owner: Option<Owner>, caption: Option<String>, at: SystemTime) -> bool {
        self.add_seen(path, source, owner, caption, Some(at))
    }

    /// Add what happened to `path` at `at` (None: a transcript fact without a time, which never counts as newer
    /// than what's listed). Something newer than the entry (sent again, opened again) brings it back to the top;
    /// re-reading the same facts changes nothing. Returns true when it's new to the list.
    fn add_seen(&mut self, path: PathBuf, source: Source, owner: Option<Owner>, caption: Option<String>, at: Option<SystemTime>) -> bool {
        let k = key(&path);
        // something you did just now (opened, pasted, dropped) always counts as the newest
        let explicit = matches!(source, Source::Opened | Source::Pasted | Source::Dropped);
        if let Some(i) = self.items.iter().position(|it| key(&it.path) == k) {
            // a file written again shows its new pixels
            let now = mtime(&self.items[i].path);
            if now.is_some() && now != self.items[i].mtime {
                self.items[i].mtime = now;
                let p = self.items[i].path.clone();
                self.refresh(&p);
            }
            let it = &mut self.items[i];
            // a stronger reason to have it (sent beats looked at) shows; a caption sticks
            if (source as u8) < (it.source as u8) {
                it.source = source;
            }
            if caption.is_some() {
                it.caption = caption;
            }
            let Some(at) = at.filter(|at| explicit || *at > it.at) else { return false };
            let mut it = self.items.remove(i);
            it.at = at;
            if owner.is_some() {
                it.owner = owner;
            }
            let at_i = self.items.iter().position(|o| o.at <= at).unwrap_or(self.items.len());
            self.items.insert(at_i, it);
            return false;
        }
        // removed from the gallery: only something newer than the removal brings it back
        if let Some(removed) = self.hidden.get(&k) {
            if !explicit && at.is_none_or(|at| at <= *removed) {
                return false;
            }
            self.hidden.remove(&k);
        }
        if !is_media(&path) || !path.is_file() {
            return false;
        }
        let at = at.unwrap_or_else(SystemTime::now);
        let kind = bro_media::kind_of(&path);
        let m = mtime(&path);
        // newest first: something older than the top goes below the newer ones
        let at_i = self.items.iter().position(|it| it.at <= at).unwrap_or(self.items.len());
        self.items.insert(at_i, Item { path, kind, owner, source, at, caption, mtime: m });
        self.items.truncate(500);
        self.unseen += 1;
        true
    }

    /// The file changed on disk: its thumbnail and (when shown) the view load again.
    fn refresh(&mut self, path: &Path) {
        if let Some((Thumb::Ready(id, ..), _)) = self.thumbs.remove(path) {
            self.free.push(id);
        }
        if self.sel.as_deref() != Some(path) {
            return;
        }
        match &self.view {
            // the picture on screen stays until the new one is in (no flash), zoomed where you were
            View::Image { path: shown, asked, .. } if shown == path => {
                let asked = *asked;
                self.req += 1;
                self.loader.ask(Job::Image(path.to_path_buf(), asked, self.req));
            }
            _ => {
                let (zoom, pan) = (self.zoom, self.pan);
                self.sel = None;
                self.select(path);
                (self.zoom, self.pan) = (zoom, pan);
            }
        }
    }

    /// Show `path` large (loading it).
    pub fn select(&mut self, path: &Path) {
        if self.sel.as_deref() == Some(path) && self.view.path() == Some(path) {
            return;
        }
        self.sel = Some(path.to_path_buf());
        self.zoom = 1.0;
        self.pan = Point::default();
        self.drop_view();
        // a load still on its way for something shown before is stale from now on
        self.req += 1;
        match bro_media::kind_of(path) {
            MediaKind::Image => {
                self.view = View::Loading(path.to_path_buf());
                self.loader.ask(Job::Image(path.to_path_buf(), VIEW_SIDE, self.req));
            }
            MediaKind::Video | MediaKind::Audio => {
                self.view = View::Loading(path.to_path_buf());
                self.loader.ask(Job::Player(path.to_path_buf(), 1920, self.req));
            }
            MediaKind::Pdf => {
                self.view = View::Failed { path: path.to_path_buf(), message: "PDFs open in their own app: Open, below".into() };
            }
            MediaKind::Other => {
                self.view = View::Failed { path: path.to_path_buf(), message: "Not a picture, video or song".into() };
            }
        }
    }

    fn drop_view(&mut self) {
        self.viewer_was_playing = None;
        match std::mem::replace(&mut self.view, View::None) {
            View::Image { tex: Some(t), .. } => self.free.push(t),
            View::Media { path, player, tex, .. } => {
                if !player.ended() {
                    self.positions.insert(path, player.position());
                }
                if let Some(t) = tex {
                    self.free.push(t);
                }
            }
            _ => {}
        }
        self.resume = false;
    }

    /// The items this panel shows now: the focused session's (or its project's), plus ones from no session.
    fn shown(&self, focus: Option<&Owner>) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|it| match (focus, &it.owner) {
                (None, _) | (_, None) => true,
                (Some(f), Some(o)) if self.whole_project => o.project == f.project,
                (Some(f), Some(o)) => o.sid == f.sid,
            })
            .collect()
    }

    /// Shown and the window focused (else video pauses and animations stop).
    pub fn set_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        self.active = active;
        if let View::Media { player, .. } = &mut self.view {
            if !active {
                self.resume = player.is_playing() && !player.ended();
                if self.resume {
                    player.pause();
                }
            } else if std::mem::take(&mut self.resume) {
                player.play();
            }
        }
    }

    /// The window has a new renderer (its material changed) and every texture went with the old one: thumbnails
    /// load again as they're drawn, the picture or video frame on view uploads again.
    pub fn gpu_lost(&mut self) {
        self.free.clear();
        self.thumbs.retain(|_, (t, _)| !matches!(t, Thumb::Ready(..)));
        match &mut self.view {
            View::Image { tex, dirty, .. } => {
                *tex = None;
                *dirty = true;
            }
            View::Media { tex, .. } => *tex = None,
            _ => {}
        }
    }

    /// The last half day's alt+v pastes (they live in `~/.bro/paste/`), so the gallery has them after a restart.
    pub fn scan_pastes(&mut self) {
        let dir = crate::util::bro_dir().join("paste");
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        let since = SystemTime::now() - Duration::from_secs(12 * 3600);
        let mut found: Vec<(SystemTime, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let t = e.metadata().ok()?.modified().ok()?;
                (t >= since && bro_media::kind_of(&p) == MediaKind::Image).then_some((t, p))
            })
            .collect();
        found.sort();
        for (t, p) in found.into_iter().rev().take(60) {
            self.add_at(p, Source::Pasted, None, None, t);
        }
        self.unseen = 0;
    }

    /// GPU work before drawing: upload what the loader finished, advance animations and video.
    pub fn prepare(&mut self, r: &mut Renderer) {
        for id in self.free.drain(..) {
            r.remove_image(id);
        }
        while let Ok(done) = self.loader.done.try_recv() {
            match done {
                Done::Thumb(p, Ok(img)) => {
                    let id = r.create_image(img.w, img.h, &img.px);
                    let m = mtime(&p);
                    if let Some((Thumb::Ready(old, ..), _)) = self.thumbs.insert(p, (Thumb::Ready(id, img.w, img.h), m)) {
                        r.remove_image(old);
                    }
                }
                Done::Thumb(p, Err(_)) => {
                    self.thumbs.insert(p, (Thumb::Failed, None));
                }
                Done::Image(p, req, res) => {
                    if req != self.req || self.sel.as_deref() != Some(p.as_path()) {
                        continue;
                    }
                    // a sharper copy for a zoomed picture replaces the one shown (a failed one leaves it be)
                    let (asked, sharper) = match &self.view {
                        View::Image { path, asked, .. } if *path == p => (*asked, true),
                        _ => (VIEW_SIDE, false),
                    };
                    match res {
                        Ok(img) if !img.frames.is_empty() => {
                            if let View::Image { tex: Some(t), .. } = &self.view {
                                self.free.push(*t);
                            }
                            let delay = img.frames[0].1;
                            self.view = View::Image { path: p, frames: img.frames, frame: 0, next: Instant::now() + delay, tex: None, dirty: true, original: img.original, format: img.format, asked };
                        }
                        _ if sharper => {}
                        Ok(_) => self.view = View::Failed { path: p, message: "The picture has nothing in it".into() },
                        Err(e) => self.view = View::Failed { path: p, message: e },
                    }
                }
                Done::Player(p, req, res) => {
                    if req != self.req || self.sel.as_deref() != Some(p.as_path()) {
                        continue;
                    }
                    self.view = match res {
                        Ok(mut player) => {
                            let wake = self.wake.clone();
                            player.set_waker(move || wake());
                            let info = player.info().clone();
                            // video plays by itself, muted; a song waits for you
                            self.muted = info.has_video;
                            player.set_muted(self.muted);
                            player.set_volume(self.volume);
                            player.set_looping(self.looping);
                            if let Some(at) = self.positions.get(&p).copied()
                                && info.duration.is_none_or(|d| at + Duration::from_secs(1) < d)
                            {
                                player.seek(at);
                            }
                            if info.has_video {
                                if self.active {
                                    player.play();
                                } else {
                                    self.resume = true;
                                }
                            }
                            let size = info.size.unwrap_or((0, 0));
                            View::Media { path: p, player: Box::new(player), tex: None, size }
                        }
                        Err(e) => View::Failed { path: p, message: e },
                    };
                }
            }
        }
        let now = Instant::now();
        let active = self.active;
        match &mut self.view {
            View::Image { frames, frame, next, tex, dirty, .. } => {
                if active && frames.len() > 1 && now >= *next {
                    *frame = (*frame + 1) % frames.len();
                    *next = now + frames[*frame].1.max(Duration::from_millis(20));
                    *dirty = true;
                }
                if *dirty {
                    let (img, _) = &frames[*frame];
                    match tex {
                        Some(id) => r.update_image(*id, img.w, img.h, &img.px),
                        None => *tex = Some(r.create_image(img.w, img.h, &img.px)),
                    }
                    *dirty = false;
                }
            }
            View::Media { player, tex, size, .. } => {
                if let Some(f) = player.take_frame() {
                    *size = (f.w, f.h);
                    match tex {
                        Some(id) => r.update_image(*id, f.w, f.h, &f.px),
                        None => *tex = Some(r.create_image(f.w, f.h, &f.px)),
                    }
                }
            }
            _ => {}
        }
    }

    /// When the panel needs the next frame by itself (animations, video, a load in progress).
    pub fn next_wake(&self) -> Option<Instant> {
        match &self.view {
            View::Loading(_) => Some(Instant::now() + Duration::from_millis(80)),
            _ if !self.active => None,
            View::Image { frames, next, .. } if frames.len() > 1 => Some(*next),
            View::Media { player, .. } => player.next_wake(),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ drawing

    /// The panel in `r`, following `focus` (the session in focus, when one).
    pub fn draw(&mut self, ui: &mut Ui, r: Rect, focus: Option<&Owner>, toggle_key: &str) {
        self.targets.clear();
        self.scale = ui.scale;
        let t = ui.t;
        self.unseen = 0;
        // the panel's edge: drag to resize
        let edge = Rect::new(r.x - 3.0, r.y, 6.0, r.h);
        let eh = ui.hover_amount("media-edge", edge);
        ui.vline(r.x, r.y, r.h, t.hairline.mix(t.accent, eh * 0.8));
        ui.hit_with(edge, Action::MediaResize, Cursor::ResizeH);
        ui.block(Rect::new(r.x + 3.0, r.y, r.w - 3.0, r.h));
        // it has the keyboard: a quiet outline says so
        let k = ui.ease("media-focus", if self.focus { 1.0 } else { 0.0 }, 140.0);
        if k > 0.01 {
            ui.outline(Rect::new(r.x + 5.0, r.y + 5.0, r.w - 10.0, r.h - 10.0), super::kit::radius::OVERLAY, 1.0, t.accent.with_alpha(0.5 * k));
        }
        let inner = Rect::new(r.x + 14.0, r.y + 10.0, r.w - 26.0, r.h - 20.0);
        // header: title and count, the scope, close
        let head = Rect::new(inner.x, inner.y, inner.w, 34.0);
        let title = ui.label(Point::new(head.x, head.y + 8.0), "Media", Style::new(size::TITLE, weight::SEMIBOLD, t.text));
        let shown_n = self.shown(focus).len();
        if shown_n > 0 {
            widgets::pill(ui, Point::new(head.x + title.w + 8.0, head.y + 8.0), 18.0, &shown_n.to_string(), t.muted, t.text.with_alpha(0.07));
        }
        let close = Rect::new(head.right() - 26.0, head.y + 4.0, 26.0, 26.0);
        let n = self.target(Target::Close);
        if widgets::icon_button(ui, close, "\u{F0156}", t.muted, Action::Media(n)) {
            widgets::tooltip(ui, close, &format!("Hide · {toggle_key}"));
        }
        let seg_w = 156.0f32.min(head.w - title.w - 90.0);
        if seg_w > 100.0 && focus.is_some() {
            let seg = Rect::new(close.x - 8.0 - seg_w, head.y + 3.0, seg_w, 28.0);
            let a = self.target(Target::Scope(false));
            let b = self.target(Target::Scope(true));
            widgets::segmented(ui, seg, &["Session", "Project"], usize::from(self.whole_project), |i| Action::Media(if i == 0 { a } else { b }));
        }
        let mut y = head.bottom() + 10.0;
        let shown: Vec<Item> = self.shown(focus).into_iter().cloned().collect();
        if shown.is_empty() {
            self.empty(ui, Rect::new(inner.x, y, inner.w, inner.bottom() - y), focus);
            return;
        }
        // keep a selection among what's shown
        if self.sel.as_ref().is_none_or(|s| !shown.iter().any(|it| &it.path == s)) {
            let first = shown[0].path.clone();
            self.select(&first);
        }
        let sel = self.sel.clone().unwrap_or_default();
        let item = shown.iter().find(|it| it.path == sel).cloned();
        // the picture, video or song
        let viewer_h = (inner.w * 0.78).clamp(160.0, (inner.h * 0.55).max(160.0));
        let viewer = Rect::new(inner.x, y, inner.w, viewer_h);
        self.viewer = viewer;
        self.draw_viewer(ui, viewer, shown.len());
        y = viewer.bottom() + 12.0;
        // about it
        if let Some(it) = &item {
            y = self.draw_info(ui, Rect::new(inner.x, y, inner.w, 140.0), it);
        }
        // what you can do with it
        let actions: [(&str, &str, &str, Target); 5] = [
            ("\u{F018F}", "Copy path", "c", Target::Copy),
            ("\u{F0065}", "Mention", "@", Target::Mention),
            ("\u{F03CC}", "Open", "o", Target::Open),
            ("\u{F0256}", "Folder", "", Target::Reveal),
            ("\u{F0A7A}", "Remove", "del", Target::Remove),
        ];
        let aw = (inner.w - 4.0 * 6.0) / 5.0;
        for (i, (icon, label, key, target)) in actions.into_iter().enumerate() {
            let b = Rect::new(inner.x + i as f32 * (aw + 6.0), y, aw, 44.0);
            let n = self.target(target);
            let h = ui.hover_amount(("media-act", i), b);
            ui.round(b, super::kit::radius::CONTROL, t.text.with_alpha(0.04 + 0.06 * h));
            ui.icon(Rect::new(b.x, b.y + 4.0, b.w, 20.0), icon, size::BODY, t.text.mix(t.muted, 0.3 - 0.3 * h));
            ui.label_in(Rect::new(b.x + 2.0, b.y + 24.0, b.w - 4.0, 16.0), label, Style::new(size::CAPTION, weight::MEDIUM, t.muted), Align::Center);
            ui.hit(b, Action::Media(n));
            if h > 0.5 && !key.is_empty() {
                widgets::tooltip(ui, b, &format!("{label} · {key}"));
            }
        }
        y += 56.0;
        // everything else
        widgets::section(ui, Rect::new(inner.x, y, inner.w, 18.0), &format!("Recent · {}", shown.len()));
        y += 26.0;
        let gallery = Rect::new(inner.x, y, inner.w, (inner.bottom() - y).max(0.0));
        self.draw_gallery(ui, gallery, &shown);
    }

    /// Draw the selected media over the workspace, keeping the same image or player loaded.
    pub fn draw_expanded(&mut self, ui: &mut Ui, r: Rect, focus: Option<&Owner>) {
        let prev = ui.layer();
        self.targets.clear();
        self.scale = ui.scale;
        self.viewer = r;
        ui.fill(r, ui.t.bg);
        self.draw_viewer(ui, r, self.shown(focus).len());
        let hint = Rect::new(r.x + 12.0, r.y + 12.0, 256.0_f32.min((r.w - 64.0).max(0.0)), 28.0);
        ui.round_over(hint, super::kit::radius::CONTROL, Color::rgba(0.0, 0.0, 0.0, 0.65));
        ui.label_in(hint, "Esc or double-click to close", Style::new(size::SMALL, weight::MEDIUM, Color::WHITE), Align::Center);
        let close = Rect::new(r.right() - 40.0, r.y + 12.0, 28.0, 28.0);
        let n = self.target(Target::Collapse);
        widgets::icon_button(ui, close, "\u{F0156}", ui.t.text, Action::Media(n));
        ui.set_layer(prev);
    }

    fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
        self.zoom = 1.0;
        self.pan = Point::default();
        self.drag = None;
        self.viewer_was_playing = None;
    }

    fn target(&mut self, t: Target) -> usize {
        self.targets.push(t);
        self.targets.len() - 1
    }

    fn empty(&mut self, ui: &mut Ui, r: Rect, focus: Option<&Owner>) {
        let t = ui.t;
        let c = Point::new(r.x + r.w / 2.0, r.y + r.h * 0.3);
        ui.circle(c, 30.0, t.text.with_alpha(0.05));
        ui.icon(Rect::new(c.x - 20.0, c.y - 20.0, 40.0, 40.0), "\u{F02E9}", 26.0, t.faint);
        let title = match focus {
            Some(f) if self.whole_project => format!("Nothing in {} yet", f.place),
            Some(f) => format!("Nothing from {} yet", f.who),
            None => "No pictures or videos yet".to_string(),
        };
        let tb = ui.line(&title, Style::new(size::TITLE, weight::SEMIBOLD, t.text), Some(r.w - 24.0));
        ui.draw(&tb, Point::new(r.x + (r.w - tb.w) / 2.0, c.y + 44.0));
        let p = ui.para(
            "When an agent opens, sends or makes a picture or a video, it shows up here, right beside the session. Drop a file on the window to look at it too.",
            Style::new(size::SMALL, weight::REGULAR, t.muted),
            (r.w - 40.0).min(300.0),
            Some(5),
        );
        ui.draw(&p, Point::new(r.x + (r.w - p.w) / 2.0, c.y + 72.0));
        // other sessions have some
        if !self.whole_project && focus.is_some() && !self.items.is_empty() {
            self.whole_project = true;
            let more = self.shown(focus).len();
            self.whole_project = false;
            if more > 0 {
                let label = format!("Show the project's {more}");
                let bw = widgets::button_width(ui, &label);
                let b = Rect::new(r.x + (r.w - bw) / 2.0, c.y + 84.0 + p.h + 8.0, bw, 32.0);
                let n = self.target(Target::Scope(true));
                widgets::button(ui, b, &label, widgets::Kind::Secondary, Action::Media(n));
            }
        }
    }

    fn draw_viewer(&mut self, ui: &mut Ui, r: Rect, count: usize) {
        let t = ui.t;
        ui.glass_round(r, super::kit::radius::OVERLAY, if t.light { t.sunken } else { Color::rgba(0.0, 0.0, 0.0, 0.35) });
        let n = self.target(Target::Viewer(r));
        let zoomed = self.zoom > 1.01;
        ui.hit_with(r, Action::Media(n), if zoomed { Cursor::Grab } else { Cursor::Default });
        ui.clip(r, super::kit::radius::OVERLAY);
        match &self.view {
            View::None => {}
            View::Loading(_) => {
                ui.spinner(r.center(), 10.0, 2.6, t.muted);
            }
            View::Failed { path, message } => {
                let icon = match bro_media::kind_of(path) {
                    MediaKind::Pdf => "\u{F0226}",
                    MediaKind::Video => "\u{F0567}",
                    MediaKind::Audio => "\u{F075A}",
                    _ => "\u{F02E9}",
                };
                ui.icon(Rect::new(r.x, r.y + r.h * 0.3, r.w, 40.0), icon, 30.0, t.faint);
                let p = ui.para(message, Style::new(size::SMALL, weight::MEDIUM, t.muted), r.w - 40.0, Some(4));
                ui.draw(&p, Point::new(r.x + (r.w - p.w) / 2.0, r.y + r.h * 0.3 + 48.0));
            }
            View::Image { tex: Some(id), frames, frame, .. } => {
                let img = &frames[*frame].0;
                let d = self.fit(r, img.w, img.h);
                if has_alpha(img) {
                    checker(ui, d.intersect(&r));
                }
                ui.image(*id, d, [0.0, 0.0, 1.0, 1.0], 0.0, 1.0);
            }
            View::Image { .. } => ui.spinner(r.center(), 10.0, 2.6, t.muted),
            View::Media { player, tex, size, .. } => {
                let info = player.info().clone();
                match tex {
                    Some(id) if size.0 > 0 => {
                        let d = self.fit(r, size.0, size.1);
                        ui.image(*id, d, [0.0, 0.0, 1.0, 1.0], 0.0, 1.0);
                    }
                    _ if !info.has_video => {
                        // a song: a quiet record that turns while it plays
                        let c = Point::new(r.center().x, r.center().y - 14.0);
                        ui.circle(c, 46.0, t.text.with_alpha(0.06));
                        ui.ring(c, 46.0, 1.0, t.text.with_alpha(0.08));
                        ui.icon(Rect::new(c.x - 24.0, c.y - 24.0, 48.0, 48.0), "\u{F075A}", 34.0, t.muted);
                        if player.is_playing() {
                            ui.pulse(c, 52.0, t.accent.with_alpha(0.25));
                        }
                    }
                    _ => ui.spinner(r.center(), 10.0, 2.6, t.muted),
                }
                if let Some(e) = player.error() {
                    let p = ui.para(&e, Style::new(size::SMALL, weight::MEDIUM, t.danger), r.w - 32.0, Some(3));
                    ui.draw(&p, Point::new(r.x + 16.0, r.y + 14.0));
                }
            }
        }
        ui.unclip();
        self.sharpen(ui.scale);
        // controls for video and songs
        if let View::Media { player, .. } = &self.view {
            let playing = player.is_playing();
            let pos = player.position();
            let dur = player.info().duration;
            let ended = player.ended();
            let video = player.info().has_video;
            let show = !video || ui.hovered(r) || !playing || self.drag.is_some();
            let k = ui.ease("media-controls", if show { 1.0 } else { 0.0 }, 160.0);
            if k > 0.01 {
                let bar = Rect::new(r.x + 10.0, r.bottom() - 46.0, r.w - 20.0, 36.0);
                ui.round_over(bar, super::kit::radius::CONTROL, Color::rgba(0.0, 0.0, 0.0, 0.55 * k));
                let play = Rect::new(bar.x + 4.0, bar.y + 4.0, 28.0, 28.0);
                let pn = self.target(Target::Play);
                let icon = if playing && !ended { "\u{F03E4}" } else { "\u{F040A}" };
                ui.icon(play, icon, size::HEADING, Color::WHITE.fade(k));
                ui.hit(play, Action::Media(pn));
                let time = format!("{} / {}", clock(pos), dur.map(clock).unwrap_or_else(|| "—".into()));
                let ts = Style::new(size::CAPTION, weight::MEDIUM, Color::WHITE.fade(0.85 * k));
                let tw = ui.measure(&time, ts);
                // volume (when there's room), mute, loop
                let vol_w = if bar.w > 320.0 { 56.0 } else { 0.0 };
                let vol = Rect::new(bar.right() - 8.0 - vol_w, bar.y + 12.0, vol_w, 12.0);
                let mute = Rect::new(vol.x - 2.0 - 26.0 - if vol_w > 0.0 { 2.0 } else { 0.0 }, bar.y + 4.0, 26.0, 28.0);
                let lp = Rect::new(mute.x - 28.0, bar.y + 4.0, 26.0, 28.0);
                ui.label(Point::new(lp.x - tw - 8.0, bar.y + 11.0), &time, ts);
                let seek = Rect::new(play.right() + 8.0, bar.y + 12.0, (lp.x - tw - 18.0 - play.right() - 8.0).max(10.0), 12.0);
                let frac = dur.map(|d| pos.as_secs_f32() / d.as_secs_f32().max(0.001)).unwrap_or(0.0).clamp(0.0, 1.0);
                slider(ui, seek, frac, t.accent, k);
                let sn = self.target(Target::Seek(seek));
                ui.hit(Rect::new(seek.x - 4.0, seek.y - 6.0, seek.w + 8.0, 24.0), Action::Media(sn));
                let ln = self.target(Target::Loop);
                ui.icon(lp, "\u{F0456}", size::BODY, Color::WHITE.fade(if self.looping { k } else { 0.4 * k }));
                ui.hit(lp, Action::Media(ln));
                let mn = self.target(Target::Mute);
                let speaker = if self.muted || self.volume <= 0.01 { "\u{F075F}" } else if self.volume < 0.5 { "\u{F057F}" } else { "\u{F057E}" };
                ui.icon(mute, speaker, size::BODY, Color::WHITE.fade(k));
                ui.hit(mute, Action::Media(mn));
                if vol_w > 0.0 {
                    slider(ui, vol, if self.muted { 0.0 } else { self.volume }, Color::WHITE.with_alpha(0.9), k);
                    let vn = self.target(Target::Volume(vol));
                    ui.hit(Rect::new(vol.x - 4.0, vol.y - 6.0, vol.w + 8.0, 24.0), Action::Media(vn));
                }
            }
        }
        // zoom badge
        if zoomed {
            let label = format!("{:.0}%", self.zoom * self.fit_scale() * self.scale * self.decoded_share() * 100.0);
            let st = Style::new(size::CAPTION, weight::SEMIBOLD, Color::WHITE);
            let w = ui.measure(&label, st) + 16.0;
            let b = Rect::new(r.x + 10.0, r.y + 10.0, w, 20.0);
            ui.round_over(b, super::kit::radius::CONTROL, Color::rgba(0.0, 0.0, 0.0, 0.5));
            ui.label_in(b, &label, st, Align::Center);
        }
        // previous / next on hover
        if ui.hovered(r) && count > 1 {
            for (glyph, target, x) in [("\u{F0141}", Target::Prev, r.x + 8.0), ("\u{F0142}", Target::Next, r.right() - 36.0)] {
                let b = Rect::new(x, r.y + r.h / 2.0 - 14.0, 28.0, 28.0);
                let h = ui.hover_amount(("media-step", x as i32), b);
                ui.circle_over(b.center(), 14.0, Color::rgba(0.0, 0.0, 0.0, 0.4 + 0.2 * h));
                ui.icon(b, glyph, size::TITLE, Color::WHITE);
                let n = self.target(target);
                ui.hit(b, Action::Media(n));
            }
        }
    }

    /// Zoomed in past the decoded pixels of a bigger picture (a photo larger than [`VIEW_SIDE`], an SVG): decode
    /// it again at the size shown, up to [`SHARP_SIDE`]. The copy on screen stays until the sharper one is in.
    fn sharpen(&mut self, scale: f32) {
        let View::Image { path, frames, frame, original, asked, format, .. } = &self.view else { return };
        let Some((img, _)) = frames.get(*frame) else { return };
        if frames.len() != 1 || self.zoom <= 1.01 {
            return;
        }
        let d = self.fit(self.viewer, img.w, img.h);
        let need = (d.w.max(d.h) * scale).ceil() as u32;
        let have = img.w.max(img.h);
        // vector art draws sharp at any size; a photo has only its own pixels
        let long = if format == "SVG" { SHARP_SIDE } else { original.0.max(original.1).min(SHARP_SIDE) };
        let side = (need + need / 4).min(long);
        if need <= have + have / 8 || have >= long || side <= *asked {
            return;
        }
        let path = path.clone();
        if let View::Image { asked, .. } = &mut self.view {
            *asked = side;
        }
        self.loader.ask(Job::Image(path, side, self.req));
    }

    /// Decoded pixels per pixel of the picture's own size (below 1 for a photo decoded smaller, above for
    /// vector art drawn bigger).
    fn decoded_share(&self) -> f32 {
        match &self.view {
            View::Image { frames, frame, original, .. } => frames.get(*frame).map_or(1.0, |(f, _)| f.w.max(f.h) as f32 / original.0.max(original.1).max(1) as f32),
            _ => 1.0,
        }
    }

    /// How far in zooming goes: [`MAX_ZOOM`], or twice the picture's own pixels for a big one.
    fn max_zoom(&self) -> f32 {
        MAX_ZOOM.max(self.actual_zoom() * 2.0).min(256.0)
    }

    /// The picture's size: (w, h) in its pixels.
    fn picture_size(&self) -> Option<(u32, u32)> {
        match &self.view {
            View::Image { frames, frame, .. } => frames.get(*frame).map(|(f, _)| (f.w, f.h)),
            View::Media { size, .. } if size.0 > 0 => Some(*size),
            _ => None,
        }
    }

    /// Logical pixels per picture pixel when fitted.
    fn fit_scale(&self) -> f32 {
        let Some((w, h)) = self.picture_size() else { return 1.0 };
        let r = self.viewer;
        let scale = (r.w / w.max(1) as f32).min(r.h / h.max(1) as f32);
        if self.expanded { scale } else { scale.min(2.0) }
    }

    /// The zoom at which one of the picture's own pixels (not the decoded copy's) is one screen pixel.
    fn actual_zoom(&self) -> f32 {
        1.0 / (self.fit_scale() * self.scale * self.decoded_share()).max(0.0001)
    }

    /// Where a `w`×`h` picture goes in `r` at the current zoom and pan.
    fn fit(&self, r: Rect, w: u32, h: u32) -> Rect {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        let s = (r.w / w).min(r.h / h);
        let s = if self.expanded {
            s
        } else {
            // small pictures aren't blown up past 2x their pixels
            s.min(2.0)
        } * self.zoom;
        let (dw, dh) = (w * s, h * s);
        let c = r.center();
        Rect::new(c.x - dw / 2.0 + self.pan.x, c.y - dh / 2.0 + self.pan.y, dw, dh)
    }

    fn draw_info(&mut self, ui: &mut Ui, r: Rect, it: &Item) -> f32 {
        let t = ui.t;
        let name = it.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let nb = ui.line(&name, Style::new(size::BODY, weight::SEMIBOLD, t.text), Some(r.w));
        ui.draw(&nb, Point::new(r.x, r.y));
        // kind · mime · pixels or length · size, as the file preview says it
        let mut meta = vec![];
        let mime = mime(&it.path).map(str::to_string);
        match &self.view {
            View::Image { original, format, frames, .. } if self.view.path() == Some(it.path.as_path()) => {
                meta.push(format.clone());
                meta.extend(mime);
                meta.push(format!("{}×{}", original.0, original.1));
                if frames.len() > 1 {
                    meta.push(format!("{} frames", frames.len()));
                }
            }
            View::Media { player, .. } => {
                let i = player.info();
                if let Some(c) = &i.codec {
                    meta.push(c.clone());
                }
                meta.extend(mime);
                if let Some((w, h)) = i.size {
                    meta.push(format!("{w}×{h}"));
                }
                if let Some(d) = i.duration {
                    meta.push(clock(d));
                }
            }
            _ => {
                meta.push(
                    match it.kind {
                        MediaKind::Pdf => "PDF",
                        MediaKind::Video => "Video",
                        MediaKind::Audio => "Audio",
                        _ => "Picture",
                    }
                    .into(),
                );
                meta.extend(mime);
            }
        }
        if let Ok(m) = std::fs::metadata(&it.path) {
            meta.push(bytes(m.len()));
        }
        let mb = ui.line(&meta.join(" · "), Style::new(size::SMALL, weight::REGULAR, t.muted), Some(r.w));
        ui.draw(&mb, Point::new(r.x, r.y + 20.0));
        // where it is: relative to the session's folder when inside it
        let place = match &it.owner {
            Some(o) => match it.path.strip_prefix(&o.cwd) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => crate::util::short_path(&it.path, 60),
            },
            None => crate::util::short_path(&it.path, 60),
        };
        let pb = ui.line(&place, Style::mono(size::CAPTION, t.faint), Some(r.w));
        ui.draw(&pb, Point::new(r.x, r.y + 38.0));
        let when = ago(it.at);
        let from = match &it.owner {
            Some(o) => format!("{} {} · {when}", it.source.verb(), o.who),
            None => format!("{} · {when}", if it.source == Source::Pasted { "pasted" } else { "dropped here" }),
        };
        let fb = ui.line(&from, Style::new(size::SMALL, weight::REGULAR, t.faint), Some(r.w));
        ui.draw(&fb, Point::new(r.x, r.y + 56.0));
        let mut y = r.y + 78.0;
        if let Some(c) = &it.caption {
            let cb = ui.para(c, Style { italic: true, ..Style::new(size::SMALL, weight::REGULAR, t.muted) }, r.w, Some(3));
            ui.draw(&cb, Point::new(r.x, y));
            y += cb.h + 6.0;
        }
        y + 4.0
    }

    fn draw_gallery(&mut self, ui: &mut Ui, r: Rect, shown: &[Item]) {
        let t = ui.t;
        let gap = 6.0;
        let cols = ((r.w + gap) / (84.0 + gap)).floor().max(2.0);
        let side = (r.w - gap * (cols - 1.0)) / cols;
        let rows = (shown.len() as f32 / cols).ceil();
        self.gallery_view = r;
        self.gallery_content = rows * (side + gap);
        self.gallery_scroll = self.gallery_scroll.clamp(0.0, (self.gallery_content - r.h).max(0.0));
        let gn = self.target(Target::Gallery);
        ui.hit_with(r, Action::Media(gn), Cursor::Default);
        ui.clip(r, 0.0);
        let mut tip: Option<(Rect, String)> = None;
        for (i, it) in shown.iter().enumerate() {
            let (cx, cy) = ((i as f32 % cols), (i as f32 / cols).floor());
            let b = Rect::new(r.x + cx * (side + gap), r.y + cy * (side + gap) - self.gallery_scroll, side, side);
            if b.bottom() < r.y || b.y > r.bottom() {
                continue;
            }
            let selected = self.sel.as_ref() == Some(&it.path);
            let h = ui.hover_amount(("media-thumb", &it.path), b);
            ui.round(b, super::kit::radius::CONTROL, t.text.with_alpha(0.05));
            match self.thumbs.get(&it.path) {
                None => {
                    if matches!(it.kind, MediaKind::Image | MediaKind::Video) {
                        self.thumbs.insert(it.path.clone(), (Thumb::Loading, None));
                        self.loader.ask(Job::Thumb(it.path.clone()));
                    } else {
                        self.thumbs.insert(it.path.clone(), (Thumb::Failed, None));
                    }
                }
                Some((Thumb::Ready(id, w, hh), _)) => {
                    // cover the square
                    let (w, hh) = (*w as f32, *hh as f32);
                    let uv = if w > hh {
                        let k = hh / w;
                        [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
                    } else {
                        let k = w / hh;
                        [0.0, (1.0 - k) / 2.0, 1.0, (1.0 + k) / 2.0]
                    };
                    ui.image(*id, b, uv, super::kit::radius::CONTROL, 1.0);
                }
                Some((Thumb::Loading, _)) => ui.spinner(b.center(), 6.0, 2.0, t.faint),
                Some((Thumb::Failed, _)) => {
                    let icon = match it.kind {
                        MediaKind::Pdf => "\u{F0226}",
                        MediaKind::Audio => "\u{F075A}",
                        MediaKind::Video => "\u{F0567}",
                        _ => "\u{F02E9}",
                    };
                    ui.icon(b, icon, 22.0, t.faint);
                }
            }
            if it.kind == MediaKind::Video {
                let p = Rect::new(b.x + 6.0, b.bottom() - 24.0, 18.0, 18.0);
                ui.circle_over(p.center(), 9.0, Color::rgba(0.0, 0.0, 0.0, 0.55));
                ui.icon(p, "\u{F040A}", size::SMALL, Color::WHITE);
            }
            // where it came from, quietly in the corner
            let badge = Rect::new(b.right() - 22.0, b.y + 4.0, 18.0, 18.0);
            ui.circle_over(badge.center(), 9.0, Color::rgba(0.0, 0.0, 0.0, 0.45 + 0.2 * h));
            ui.icon(badge, it.source.icon(), size::CAPTION, Color::WHITE.with_alpha(0.9));
            if selected {
                ui.outline_over(b, super::kit::radius::CONTROL, 2.0, t.accent);
            } else if h > 0.0 {
                ui.round_over(b, super::kit::radius::CONTROL, Color::WHITE.with_alpha(0.08 * h));
            }
            if h > 0.6 {
                let name = it.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let whose = it.owner.as_ref().map(|o| format!("{} {}", it.source.verb(), o.who)).unwrap_or_else(|| if it.source == Source::Pasted { "pasted".into() } else { "dropped here".into() });
                let mut text = format!("{name}\n{whose} · {}", ago(it.at));
                if let Some(c) = &it.caption {
                    text.push_str(&format!("\n“{c}”"));
                }
                tip = Some((b, text));
            }
            let n = self.target(Target::Thumb(it.path.clone()));
            ui.hit(b, Action::Media(n));
        }
        ui.unclip();
        widgets::scrollbar(ui, r, self.gallery_content, r.h, self.gallery_scroll, if ui.hovered(r) { 1.0 } else { 0.0 });
        if let Some((b, text)) = tip {
            widgets::tooltip(ui, b, &text);
        }
    }

    // ------------------------------------------------------------------ input

    /// Target `n` was pressed (`count` 2 for a double-click).
    pub fn click(&mut self, n: usize, p: Point, count: u32, focus: Option<&Owner>) -> Ask {
        self.focus = true;
        let Some(target) = self.targets.get(n).cloned() else { return Ask::None };
        let sel = self.sel.clone();
        match target {
            Target::Close => return Ask::Close,
            Target::Collapse => {
                if self.expanded {
                    self.toggle_expanded();
                }
            }
            Target::Scope(whole) => self.whole_project = whole,
            Target::Thumb(path) => {
                self.select(&path);
                if count > 0 && count % 2 == 0 {
                    self.toggle_expanded();
                }
            }
            Target::Viewer(_) => {
                if count > 0 && count % 2 == 0 {
                    // Double-click opens or closes the workspace viewer; the wheel still zooms.
                    if let Some(playing) = self.viewer_was_playing.take()
                        && let View::Media { player, .. } = &mut self.view
                    {
                        if playing { player.play() } else { player.pause() }
                    }
                    self.toggle_expanded();
                } else if let View::Media { player, .. } = &mut self.view
                    && player.info().has_video
                {
                    self.viewer_was_playing = Some(player.is_playing());
                    toggle(player);
                } else if self.zoom > 1.01 {
                    self.drag = Some(DragKind::Pan(p, self.pan));
                }
            }
            Target::Play => {
                if let View::Media { player, .. } = &mut self.view {
                    toggle(player);
                }
            }
            Target::Seek(bar) => {
                self.drag = Some(DragKind::Seek(bar));
                self.seek_to(bar, p);
            }
            Target::Volume(bar) => {
                self.drag = Some(DragKind::Volume(bar));
                self.volume_to(bar, p);
            }
            Target::Mute => self.toggle_mute(),
            Target::Loop => self.toggle_loop(),
            Target::Copy => return self.copy(),
            Target::Mention => return self.mention(),
            Target::Open => {
                if let Some(path) = sel {
                    return Ask::Open(path);
                }
            }
            Target::Reveal => {
                if let Some(path) = sel {
                    return Ask::Reveal(path);
                }
            }
            Target::Remove => self.remove_selected(focus),
            Target::Prev => self.step(-1, focus),
            Target::Next => self.step(1, focus),
            Target::Gallery => {}
        }
        Ask::None
    }

    fn copy(&self) -> Ask {
        match &self.sel {
            Some(p) => {
                let text = p.to_string_lossy().to_string();
                super::window::set_clipboard(&text);
                Ask::Toast(format!("copied {text}"))
            }
            None => Ask::None,
        }
    }

    fn mention(&self) -> Ask {
        match &self.sel {
            Some(p) => {
                let pane = self.items.iter().find(|it| &it.path == p).and_then(|it| it.owner.as_ref()).and_then(|o| o.pane);
                Ask::Mention(p.clone(), pane)
            }
            None => Ask::None,
        }
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if !self.muted && self.volume <= 0.01 {
            self.volume = 0.6;
        }
        if let View::Media { player, .. } = &mut self.view {
            player.set_muted(self.muted);
            player.set_volume(self.volume);
        }
    }

    fn toggle_loop(&mut self) {
        self.looping = !self.looping;
        if let View::Media { player, .. } = &mut self.view {
            player.set_looping(self.looping);
        }
    }

    fn remove_selected(&mut self, focus: Option<&Owner>) {
        let Some(path) = self.sel.clone() else { return };
        let shown: Vec<PathBuf> = self.shown(focus).iter().map(|it| it.path.clone()).collect();
        let at = shown.iter().position(|p| *p == path).unwrap_or(0);
        self.items.retain(|it| it.path != path);
        // hidden, not deleted: the file stays where it is, and re-reading the transcript doesn't bring it back
        self.hidden.insert(key(&path), SystemTime::now());
        if let Some((Thumb::Ready(id, ..), _)) = self.thumbs.remove(&path) {
            self.free.push(id);
        }
        self.drop_view();
        self.sel = None;
        let shown: Vec<PathBuf> = self.shown(focus).iter().map(|it| it.path.clone()).collect();
        if let Some(next) = shown.get(at.min(shown.len().saturating_sub(1))).cloned() {
            self.select(&next);
        } else {
            self.expanded = false;
        }
    }

    /// The previous (-1) or next item among those shown.
    pub fn step(&mut self, by: i32, focus: Option<&Owner>) {
        let shown: Vec<PathBuf> = self.shown(focus).iter().map(|it| it.path.clone()).collect();
        if shown.is_empty() {
            return;
        }
        let at = self.sel.as_ref().and_then(|s| shown.iter().position(|p| p == s)).unwrap_or(0) as i32;
        let next = (at + by).rem_euclid(shown.len() as i32) as usize;
        let path = shown[next].clone();
        self.select(&path);
        // keep the gallery's selection in view
        let cols = ((self.gallery_view.w + 6.0) / 90.0).floor().max(2.0);
        let side = (self.gallery_view.w - 6.0 * (cols - 1.0)) / cols;
        let top = (next as f32 / cols).floor() * (side + 6.0);
        if top < self.gallery_scroll {
            self.gallery_scroll = top;
        } else if top + side > self.gallery_scroll + self.gallery_view.h {
            self.gallery_scroll = top + side - self.gallery_view.h;
        }
    }

    fn seek_to(&mut self, bar: Rect, p: Point) {
        if let View::Media { player, .. } = &mut self.view
            && let Some(d) = player.info().duration
        {
            let f = ((p.x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
            player.seek(d.mul_f32(f));
        }
    }

    fn volume_to(&mut self, bar: Rect, p: Point) {
        self.volume = ((p.x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
        self.muted = self.volume <= 0.01;
        if let View::Media { player, .. } = &mut self.view {
            player.set_volume(self.volume);
            player.set_muted(self.muted);
        }
    }

    fn zoom_at(&mut self, r: Rect, p: Point, to: f32) {
        let to = to.clamp(1.0, self.max_zoom());
        // keep the point under the pointer where it is
        let c = r.center();
        let k = to / self.zoom;
        self.pan = Point::new((self.pan.x - (p.x - c.x)) * k + (p.x - c.x), (self.pan.y - (p.y - c.y)) * k + (p.y - c.y));
        self.zoom = to;
        if self.zoom <= 1.001 {
            self.pan = Point::default();
        }
    }

    /// A drag continues (seek scrub, volume, image pan). True while the panel owns the drag.
    pub fn dragged(&mut self, p: Point) -> bool {
        match self.drag {
            Some(DragKind::Seek(bar)) => {
                self.seek_to(bar, p);
                true
            }
            Some(DragKind::Volume(bar)) => {
                self.volume_to(bar, p);
                true
            }
            Some(DragKind::Pan(start, pan0)) => {
                self.pan = Point::new(pan0.x + p.x - start.x, pan0.y + p.y - start.y);
                true
            }
            None => false,
        }
    }

    pub fn released(&mut self) -> bool {
        self.drag.take().is_some()
    }

    /// The wheel over target `n`: zooms a picture (around the pointer), scrolls the gallery, sets the volume.
    pub fn wheel(&mut self, n: usize, p: Point, dy: f32) {
        match self.targets.get(n).cloned() {
            Some(Target::Viewer(r)) if matches!(self.view, View::Image { .. }) => {
                let to = self.zoom * if dy > 0.0 { 1.18 } else { 1.0 / 1.18 };
                self.zoom_at(r, p, to);
            }
            Some(Target::Volume(_) | Target::Mute) => {
                self.volume = (self.volume + dy.signum() * 0.05).clamp(0.0, 1.0);
                self.muted = self.volume <= 0.01;
                if let View::Media { player, .. } = &mut self.view {
                    player.set_volume(self.volume);
                    player.set_muted(self.muted);
                }
            }
            Some(Target::Thumb(_) | Target::Gallery) => {
                self.gallery_scroll = (self.gallery_scroll - dy * 40.0).max(0.0);
            }
            _ => {}
        }
    }

    /// Keys while the panel has focus; None when the key isn't the panel's.
    pub fn key(&mut self, k: &crossterm::event::KeyEvent, focus: Option<&Owner>) -> Option<Ask> {
        use crossterm::event::KeyCode;
        if !k.modifiers.difference(crossterm::event::KeyModifiers::SHIFT).is_empty() {
            return None;
        }
        let zoomed = self.zoom > 1.01;
        let pan = |m: &mut Media, dx: f32, dy: f32| m.pan = Point::new(m.pan.x + dx, m.pan.y + dy);
        Some(match k.code {
            // arrows pan a zoomed picture, else step through the gallery
            KeyCode::Left if zoomed => {
                pan(self, 60.0, 0.0);
                Ask::None
            }
            KeyCode::Right if zoomed => {
                pan(self, -60.0, 0.0);
                Ask::None
            }
            KeyCode::Up if zoomed => {
                pan(self, 0.0, 60.0);
                Ask::None
            }
            KeyCode::Down if zoomed => {
                pan(self, 0.0, -60.0);
                Ask::None
            }
            KeyCode::Left | KeyCode::Up => {
                self.step(-1, focus);
                Ask::None
            }
            KeyCode::Right | KeyCode::Down => {
                self.step(1, focus);
                Ask::None
            }
            KeyCode::Char(' ') => {
                if let View::Media { player, .. } = &mut self.view {
                    toggle(player);
                }
                Ask::None
            }
            KeyCode::Char('m') => {
                self.toggle_mute();
                Ask::None
            }
            KeyCode::Char('l') => {
                self.toggle_loop();
                Ask::None
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let r = self.viewer;
                self.zoom_at(r, r.center(), self.zoom * 1.25);
                Ask::None
            }
            KeyCode::Char('-') => {
                let r = self.viewer;
                self.zoom_at(r, r.center(), self.zoom / 1.25);
                Ask::None
            }
            KeyCode::Char('0') => {
                self.zoom = 1.0;
                self.pan = Point::default();
                Ask::None
            }
            KeyCode::Char('1') => {
                let r = self.viewer;
                let actual = self.actual_zoom();
                self.zoom_at(r, r.center(), actual);
                Ask::None
            }
            KeyCode::Char('c') => self.copy(),
            KeyCode::Char('o') => self.sel.clone().map(Ask::Open).unwrap_or(Ask::None),
            KeyCode::Char('@') | KeyCode::Char('i') => self.mention(),
            KeyCode::Delete | KeyCode::Backspace => {
                self.remove_selected(focus);
                Ask::None
            }
            KeyCode::Esc => {
                if self.expanded {
                    self.toggle_expanded();
                } else {
                    self.focus = false;
                }
                Ask::None
            }
            _ => return None,
        })
    }

    /// Read new media from the sessions' transcripts: files they sent you, wrote, moved or looked at; and a
    /// picture an agent opened (`bro open`) while you were looking elsewhere (the terminal UI only toasts then;
    /// watching, the preview hands it over). Also notices a shown file rewritten on disk.
    pub fn collect(&mut self, app: &crate::app::App) -> bool {
        use bro_core::sessions::live::TouchKind;
        let mut added = false;
        let ids: Vec<PaneId> = app.panes.keys().copied().collect();
        for id in ids {
            let Some(t) = app.panes.get(&id).and_then(|p| p.as_term_ref()) else { continue };
            let Some(snap) = t.insights() else { continue };
            let Some(facts) = snap.facts.as_ref() else { continue };
            if self.revs.get(&id) == Some(&facts.revision) {
                continue;
            }
            self.revs.insert(id, facts.revision);
            let who = owner(app, id);
            // oldest first so the newest ends up on top; each at its own time, so reading the same facts again
            // changes nothing and a file sent again moves up
            for f in facts.sent.iter().rev() {
                added |= self.add_seen(f.path.clone(), Source::Sent, who.clone(), f.caption.clone(), fact_time(f.at_ms));
            }
            for f in facts.touched.iter().rev() {
                let source = match f.kind {
                    TouchKind::Delete => continue,
                    TouchKind::Read => Source::Viewed,
                    // a move's path is where the file went
                    TouchKind::Edit | TouchKind::Write | TouchKind::Create | TouchKind::Move => Source::Made,
                };
                added |= self.add_seen(f.path.clone(), source, who.clone(), None, fact_time(f.at_ms));
            }
        }
        if let Some(o) = &app.last_opened {
            let seen = (o.pane, o.path.clone());
            if self.opened.as_ref() != Some(&seen) {
                self.opened = Some(seen);
                let who = owner(app, o.pane);
                let path = match &who {
                    Some(w) if o.path.is_relative() => w.cwd.join(&o.path),
                    _ => o.path.clone(),
                };
                if is_media(&path) {
                    added |= self.add(path, Source::Opened, who, None);
                }
            }
        }
        self.check_changes();
        added
    }

    /// The selected file and the newest few, rewritten on disk since they were shown: show the new pixels.
    /// At most every 1.5 s (a handful of `stat`s).
    fn check_changes(&mut self) {
        if self.checked.elapsed() < Duration::from_millis(1500) {
            return;
        }
        self.checked = Instant::now();
        let sel = self.sel.clone();
        let changed: Vec<PathBuf> = self
            .items
            .iter_mut()
            .enumerate()
            .filter(|(i, it)| *i < 12 || sel.as_ref() == Some(&it.path))
            .filter_map(|(_, it)| {
                let now = mtime(&it.path)?;
                (Some(now) != it.mtime).then(|| {
                    it.mtime = Some(now);
                    it.path.clone()
                })
            })
            .collect();
        for p in changed {
            self.refresh(&p);
        }
    }
}

fn toggle(p: &mut Player) {
    if p.ended() {
        p.seek(Duration::ZERO);
        p.play();
    } else {
        p.toggle();
    }
}

/// A thin track with a filled part and a knob.
fn slider(ui: &mut Ui, r: Rect, frac: f32, fill: Color, k: f32) {
    let frac = frac.clamp(0.0, 1.0);
    ui.round_over(Rect::new(r.x, r.y + 4.0, r.w, 4.0), 2.0, Color::WHITE.fade(0.25 * k));
    ui.round_over(Rect::new(r.x, r.y + 4.0, r.w * frac, 4.0), 2.0, fill.fade(k));
    ui.circle_over(Point::new(r.x + r.w * frac, r.y + 6.0), 5.0, Color::WHITE.fade(k));
}

fn has_alpha(img: &Rgba) -> bool {
    img.px.chunks_exact(4).step_by(97).any(|px| px[3] < 250)
}

/// A soft checkerboard behind transparent pictures.
fn checker(ui: &mut Ui, r: Rect) {
    let s = 10.0;
    let (a, b) = (Color::rgba(1.0, 1.0, 1.0, 0.06), Color::rgba(1.0, 1.0, 1.0, 0.02));
    ui.fill(r, b);
    let cols = (r.w / s).ceil() as usize;
    let rows = (r.h / s).ceil() as usize;
    if cols * rows > 6000 {
        return;
    }
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let c = Rect::new(r.x + x as f32 * s, r.y + y as f32 * s, s, s).intersect(&r);
                ui.fill(c, a);
            }
        }
    }
}

/// "just now", "3m ago".
fn ago(at: SystemTime) -> String {
    match at.elapsed().map(|d| d.as_secs()).unwrap_or(0) {
        0..=4 => "just now".into(),
        s => format!("{} ago", crate::util::short_dur(s)),
    }
}

/// "1:05", "12:40", "1:02:03".
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(sid: &str, project: &str) -> Option<Owner> {
        Some(Owner { sid: sid.into(), project: project.into(), who: sid.into(), ..Default::default() })
    }

    #[test]
    fn double_clicking_recent_media_selects_and_expands_every_kind() {
        let dir = tempfile::tempdir().unwrap();
        for ext in ["png", "gif", "mp4", "mp3"] {
            let path = dir.path().join(format!("recent.{ext}"));
            std::fs::write(&path, b"fixture").unwrap();
            let mut m = Media::new(Arc::new(|| {}));
            assert!(m.add(path.clone(), Source::Opened, None, None));
            let thumb = m.target(Target::Thumb(path.clone()));
            m.click(thumb, Point::default(), 1, None);
            assert!(!m.expanded, "single-click only selects {ext}");
            let req = m.req;
            m.click(thumb, Point::default(), 2, None);
            assert!(m.expanded, "double-click expands {ext}");
            assert_eq!(m.sel.as_ref(), Some(&path));
            assert_eq!(m.req, req, "expanding must keep the existing decoder or player");
            let viewer = m.target(Target::Viewer(Rect::new(0.0, 0.0, 1440.0, 900.0)));
            m.click(viewer, Point::default(), 3, None);
            assert!(m.expanded, "the first click of another double-click keeps the viewer open");
            m.click(viewer, Point::default(), 4, None);
            assert!(!m.expanded, "another double-click closes {ext}");
            assert_eq!(m.req, req);
            assert_eq!(m.sel.as_ref(), Some(&path));
        }
    }

    #[test]
    fn preview_double_click_and_escape_keep_the_selected_media_loaded() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut m = Media::new(Arc::new(|| {}));
        let path = PathBuf::from("preview.png");
        m.select(&path);
        let req = m.req;
        let viewer = m.target(Target::Viewer(Rect::new(20.0, 30.0, 360.0, 280.0)));
        m.zoom = 3.0;
        m.pan = Point::new(20.0, 10.0);
        m.click(viewer, Point::default(), 2, None);
        assert!(m.expanded);
        assert_eq!(m.zoom, 1.0, "the whole image fits the expanded viewer");
        assert_eq!((m.pan.x, m.pan.y), (0.0, 0.0));
        m.zoom = 2.0;
        assert!(matches!(m.key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None), Some(Ask::None)));
        assert!(!m.expanded);
        assert!(m.focus, "Escape returns to the media panel");
        assert_eq!(m.zoom, 1.0);
        assert_eq!(m.sel.as_ref(), Some(&path));
        assert_eq!(m.view.path(), Some(path.as_path()));
        assert_eq!(m.req, req, "Escape does not reload the media");
        m.key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None);
        assert!(!m.focus, "another Escape leaves the panel as before");
    }

    #[test]
    fn expanded_media_fits_small_images_without_stretching_or_cropping() {
        let mut m = Media::new(Arc::new(|| {}));
        let window = Rect::new(0.0, 0.0, 1440.0, 900.0);
        assert_eq!(m.fit(window, 320, 200).w, 640.0, "the panel keeps its existing upscale limit");
        m.expanded = true;
        let fitted = m.fit(window, 320, 200);
        assert_eq!((fitted.x, fitted.y, fitted.w, fitted.h), (0.0, 0.0, 1440.0, 900.0));
        let video = m.fit(window, 160, 90);
        assert_eq!((video.x, video.y, video.w, video.h), (0.0, 45.0, 1440.0, 810.0));
    }

    #[test]
    fn clocks_and_sizes_read_well() {
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(412 * 1024), "412 KB");
        assert_eq!(bytes(3 * 1024 * 1024 + 1024 * 300), "3.3 MB");
    }

    #[test]
    fn newest_first_once_each_and_stronger_sources_win() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.mp4");
        let c = dir.path().join("notes.txt");
        for p in [&a, &b, &c] {
            std::fs::write(p, b"x").unwrap();
        }
        let mut m = Media::new(Arc::new(|| {}));
        let t = |s: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + s));
        assert!(m.add_seen(a.clone(), Source::Viewed, None, None, t(100)));
        assert!(m.add_seen(b.clone(), Source::Made, None, None, t(200)));
        assert!(!m.add(c.clone(), Source::Made, None, None), "not media");
        assert!(!m.add(dir.path().join("gone.png"), Source::Made, None, None), "missing files aren't listed");
        assert_eq!(m.items[0].path, b);
        assert!(!m.add_seen(a.clone(), Source::Sent, None, Some("the chart".into()), t(300)), "sent later: moved up, not added");
        assert_eq!(m.items[0].path, a);
        assert_eq!(m.items[0].source, Source::Sent);
        assert_eq!(m.items[0].caption.as_deref(), Some("the chart"));
        assert!(!m.add_seen(b.clone(), Source::Viewed, None, None, t(200)), "the same fact read again doesn't move it");
        assert!(!m.add_seen(b.clone(), Source::Viewed, None, None, None), "nor does one without a time");
        assert_eq!(m.items[0].path, a);
        assert_eq!(m.items[1].source, Source::Made, "a weaker reason doesn't replace a stronger one");
        assert!(!m.add(b.clone(), Source::Opened, None, None), "opened now: on top");
        assert_eq!(m.items[0].path, b);
        assert_eq!(m.items.len(), 2);
    }

    #[test]
    fn removing_hides_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("chart.png");
        std::fs::write(&a, b"x").unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        let sent = SystemTime::now() - Duration::from_secs(60);
        assert!(m.add_seen(a.clone(), Source::Sent, None, None, Some(sent)));
        m.sel = Some(a.clone());
        m.remove_selected(None);
        assert!(m.items.is_empty());
        assert!(a.is_file(), "the file stays");
        // the transcript read again (same fact, or one without a time) doesn't bring it back
        assert!(!m.add_seen(a.clone(), Source::Sent, None, None, Some(sent)));
        assert!(!m.add_seen(a.clone(), Source::Made, None, None, None));
        assert!(m.items.is_empty());
        // opened again, or sent again later: it's back
        assert!(m.add(a.clone(), Source::Opened, None, None));
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn one_entry_per_file_however_it_is_spelled() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("shot.png");
        std::fs::write(&a, b"x").unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        assert!(m.add(a.clone(), Source::Dropped, None, None));
        let slashed = PathBuf::from(a.to_string_lossy().replace('\\', "/"));
        assert!(!m.add(slashed, Source::Opened, None, None));
        if cfg!(windows) {
            let shouty = PathBuf::from(a.to_string_lossy().to_uppercase());
            assert!(!m.add(shouty.clone(), Source::Opened, None, None));
            let verbatim = PathBuf::from(format!(r"\\?\{}", a.display()));
            assert!(!m.add(verbatim, Source::Opened, None, None));
        }
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn a_picture_an_agent_opens_while_you_look_elsewhere_joins_the_gallery() {
        crate::testkit::isolate();
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(rx);
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = crate::app::App::new(svc, tx, crate::app::Opts { demo: false, fixed_demo: true, load_recents: false });
        let dir = tempfile::tempdir().unwrap();
        let (pic, notes) = (dir.path().join("diagram.png"), dir.path().join("notes.md"));
        std::fs::write(&pic, b"x").unwrap();
        std::fs::write(&notes, b"x").unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        // `bro open` in a session you weren't watching: the terminal UI only toasts; the gallery has it too
        app.last_opened = Some(crate::app::files::Opened { pane: 7, path: pic.clone(), line: None, column: None });
        assert!(m.collect(&app));
        assert_eq!(m.items[0].path, pic);
        assert_eq!(m.items[0].source, Source::Opened);
        assert!(!m.collect(&app), "seen once");
        // not a picture: nothing
        app.last_opened = Some(crate::app::files::Opened { pane: 7, path: notes, line: Some(3), column: None });
        assert!(!m.collect(&app));
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn a_file_rewritten_on_disk_loads_again() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("plot.png");
        std::fs::write(&a, b"x").unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        m.add(a.clone(), Source::Made, None, None);
        m.select(&a);
        let first = m.req;
        // nothing changed: nothing reloads
        m.checked = Instant::now() - Duration::from_secs(5);
        m.check_changes();
        assert_eq!(m.req, first);
        // rewritten: the shown file is asked for again
        let f = std::fs::File::options().write(true).open(&a).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
        m.checked = Instant::now() - Duration::from_secs(5);
        m.check_changes();
        assert!(m.req > first, "the new pixels are loaded");
        assert_eq!(m.sel.as_deref(), Some(a.as_path()));
    }

    #[test]
    fn the_info_line_names_the_type() {
        assert_eq!(mime(Path::new("a/b.PNG")), Some("image/png"));
        assert_eq!(mime(Path::new("clip.mp4")), Some("video/mp4"));
        assert_eq!(mime(Path::new("song.flac")), Some("audio/flac"));
        assert_eq!(mime(Path::new("doc.pdf")), Some("application/pdf"));
        assert_eq!(mime(Path::new("scan.tiff")), None);
        assert_eq!(fact_time(Some(1_000)), Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)));
        assert_eq!(fact_time(None), None);
    }

    #[test]
    fn the_scope_follows_the_session_or_its_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        for (name, o) in [("a.png", owner("s1", "p1")), ("b.png", owner("s2", "p1")), ("c.png", owner("s3", "p2")), ("d.png", None)] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            m.add(p, Source::Sent, o, None);
        }
        let names = |m: &Media, f: Option<&Owner>| m.shown(f).iter().map(|it| it.path.file_name().unwrap().to_string_lossy().to_string()).collect::<Vec<_>>();
        let s1 = owner("s1", "p1").unwrap();
        assert_eq!(names(&m, Some(&s1)), ["d.png", "a.png"], "the session's, and what belongs to none");
        m.whole_project = true;
        assert_eq!(names(&m, Some(&s1)), ["d.png", "b.png", "a.png"], "the project's");
        assert_eq!(names(&m, None).len(), 4, "no session in focus: everything");
    }

    #[test]
    fn older_items_slot_in_below_newer_ones() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Media::new(Arc::new(|| {}));
        let (a, b) = (dir.path().join("new.png"), dir.path().join("old.png"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        m.add(a.clone(), Source::Pasted, None, None);
        m.add_at(b.clone(), Source::Pasted, None, None, SystemTime::now() - Duration::from_secs(3600));
        assert_eq!(m.items[0].path, a);
        assert_eq!(m.items[1].path, b);
    }
}
