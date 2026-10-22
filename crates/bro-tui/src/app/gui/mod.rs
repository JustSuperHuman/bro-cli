//! The desktop app, `bro gui`: the same workspace as the terminal UI — every session, view, overlay and
//! background service of [`App`] — drawn on the GPU (bro-gfx on wgpu), with each terminal on an
//! alacritty_terminal screen, and a media sidebar that shows the photos and videos agents make instead of
//! handing them to another app.
//!
//! The workspace model is shared with the terminal UI: keys arrive as the same crossterm events (so every
//! binding works), panes and overlays keep their state machines, and this module adds the window, the layout
//! in pixels, the drawing and the mouse.

pub(crate) mod actions;
mod backdrop;
mod chrome;
mod hints;
mod input;
pub(crate) mod kit;
mod layout;
mod media;
mod mouse;
mod overlays;
mod prefs;
mod script;
mod sidebar;
pub(crate) mod surface;
mod term;
mod toasts;
mod tui;
mod views;
mod welcome;
mod window;

use super::{App, Opts};
use crate::pane::{Event, PaneId};
use crate::services::Services;
use bro_gfx::{Point, Rect, Renderer, Scene, Text};
use kit::{Anims, Hit, Tokens, Ui};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

/// Events from bro's own threads, carried into winit's loop.
pub enum UserEvent {
    App(Event),
    /// something finished in the background (a decoded picture, a video frame): draw
    Wake,
}

/// How `bro gui` was asked for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flag {
    /// the copy started without a console (see [`launch`])
    pub child: bool,
}

/// Take `gui` (as the first word) or `--gui` off the command line; `--gui-child` marks the copy [`launch`] starts.
pub fn take_flag(args: &mut Vec<String>) -> Option<Flag> {
    let mut found = None;
    if args.first().map(String::as_str) == Some("gui") {
        args.remove(0);
        found = Some(Flag::default());
    }
    if let Some(i) = args.iter().position(|a| a == "--gui") {
        args.remove(i);
        found = Some(found.unwrap_or_default());
    }
    if let Some(i) = args.iter().position(|a| a == "--gui-child") {
        args.remove(i);
        found = Some(Flag { child: true });
    }
    found
}

/// `bro gui`: started with a console (from a shell, or bro.exe opened from Explorer or Start), hand off to a
/// copy without a console window so the shell gets its prompt back (like `code .`) and closing the console
/// leaves the app open; otherwise run here.
pub fn launch(flag: Flag, demo: bool, new: bool, dir: Option<PathBuf>) -> anyhow::Result<()> {
    if !flag.child {
        let mut args = vec!["gui".to_string()];
        if let Some(d) = &dir {
            let d = std::fs::canonicalize(d).map(crate::util::strip_verbatim).unwrap_or_else(|_| d.clone());
            args.push(d.to_string_lossy().to_string());
        } else if let Ok(cwd) = std::env::current_dir() {
            args.push(cwd.to_string_lossy().to_string());
        }
        if demo {
            args.push("--demo".into());
        }
        if new {
            args.push("--new".into());
        }
        args.push("--gui-child".into());
        if window::detach_from_console(&args) {
            return Ok(());
        }
    }
    run(demo, new, dir)
}

/// Start the desktop app (mirrors `run_tui`'s startup).
pub fn run(demo: bool, new: bool, dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = match dir {
        Some(d) => crate::util::strip_verbatim(std::fs::canonicalize(&d).map_err(|_| anyhow::anyhow!("{} isn't a folder", d.display()))?),
        None => std::env::current_dir()?,
    };
    // already running? hand the folder over and get out of the way
    if !demo && !new && crate::instance::hand_off(&dir).unwrap_or(false) {
        return Ok(());
    }
    crate::services::guard::install_panic_hook();
    crate::util::probe_local_offset();
    let (settings, problem) = if demo { (crate::services::fallback_settings(), None) } else { crate::services::load_settings() };
    let (tx, rx) = std::sync::mpsc::channel();
    // the font setting is ours to apply (the terminal UI asks its terminal to switch)
    crate::fonts::GUI.store(true, std::sync::atomic::Ordering::Relaxed);
    // copies go to the system clipboard directly (no terminal to send OSC 52 to)
    let _ = crate::clip::NATIVE.set(window::set_clipboard);
    // every terminal pane gets an alacritty screen next to its vt100 model
    let env = term::shared_env();
    crate::panes::mirror::install(move || Arc::new(term::mirror::Mirror::new(env.clone())));
    // `--new` beside a bro that's already running: your real data, but the orchestrator, proxy, bridge, voice and
    // hand-offs stay with that bro (a second orchestrator would resume the same session twice)
    let guest = !demo && new && crate::instance::running();
    let svc = if demo {
        Services::demo(settings, tx.clone())
    } else if guest {
        Services::start_guest(settings, tx.clone())
    } else {
        Services::start(settings, tx.clone())
    };
    let _instance = if demo || guest { None } else { crate::instance::serve(tx.clone()) };
    let mut app = App::new(svc, tx.clone(), Opts { demo, fixed_demo: false, load_recents: !demo });
    app.guest = guest;
    if !demo {
        app.start_in(dir);
        if guest {
            app.toast(crate::alerts::Kind::Info, "another bro is running: it keeps the orchestrator, the proxy and the phone bridge");
        } else {
            app.restore_orchestrator();
        }
    }
    if let Some(p) = problem.filter(|p| !crate::services::guard::is_unimplemented(p)) {
        app.toast(crate::alerts::Kind::Error, format!("settings: {p} — using defaults"));
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    {
        let proxy = proxy.clone();
        std::thread::Builder::new().name("bro-gui-events".into()).spawn(move || {
            while let Ok(ev) = rx.recv() {
                if proxy.send_event(UserEvent::App(ev)).is_err() {
                    break;
                }
            }
        })?;
    }
    let mut gui = Gui::new(app, proxy);
    event_loop.run_app(&mut gui)?;
    gui.shutdown();
    Ok(())
}

/// GPU state, created once the window exists.
struct Gfx {
    renderer: Renderer,
    text: Text,
    scene: Scene,
}

/// Per-pane state the desktop app keeps beside the workspace's.
#[derive(Default)]
struct PaneState {
    /// where the screen was drawn last frame
    drawn: term::draw::Drawn,
    /// links on screen, recomputed when the screen changes
    links: Vec<term::draw::LinkSpan>,
    links_seq: u64,
    links_offset: usize,
    /// file paths on screen were still being checked: look again once they are
    links_pending: bool,
    /// fractional scroll from a touchpad, in lines
    scroll_acc: f32,
}

/// A drag in progress.
#[derive(Clone, Debug)]
enum Drag {
    Sidebar { start_x: f32, start_w: f32 },
    Media { start_x: f32, start_w: f32 },
    Divider { path: Vec<bool>, dir: crate::layout::Dir, area: ratatui::layout::Rect },
    TileEdge { pane: PaneId, ax: usize, far: bool, offset: i32 },
    /// selecting text in a terminal
    Select { pane: PaneId },
    /// the program in this pane gets the drag (mouse reporting)
    Program { pane: PaneId },
}

pub struct Gui {
    app: App,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    scale: f32,
    tokens: Tokens,
    theme_key: String,
    prefs: prefs::Prefs,
    mods: ModifiersState,
    pointer: Option<Point>,
    pressing: bool,
    hits: Vec<Hit>,
    top_layer: usize,
    anim: Anims,
    layout: layout::Layout,
    panes: HashMap<PaneId, PaneState>,
    drag: Option<Drag>,
    /// (when, where, how many) of the last left press, for double and triple clicks
    last_press: Option<(Instant, Point, u32)>,
    /// a link pressed: it opens on release at the same cell
    link_press: Option<(PaneId, Arc<str>, (usize, usize))>,
    started: Instant,
    /// the workspace's last Tick (its timers: pane polls, toasts, resumes)
    last_tick: Instant,
    /// the minute last drawn (the sidebar's clock)
    minute: u64,
    /// when the links on screen were last looked for again without the screen changing (file paths that
    /// appeared or went away since, as the terminal UI finds them on every redraw)
    links_refreshed: Instant,
    /// something changed since the last frame
    dirty: bool,
    /// draw again soon (animations, spinners)
    ticking: bool,
    focused_window: bool,
    tui: tui::TuiCanvas,
    /// where the open overlay was painted by the terminal UI's drawing (its clicks are translated to its cells)
    overlay_grid: Option<tui::Grid>,
    /// the open overlay was drawn natively (its clicks go to its module)
    overlay_native: bool,
    /// where views and card backs were painted
    painted: chrome::Painted,
    sidebar: sidebar::SideState,
    /// alt was pressed with nothing else (key hints)
    alt_alone: bool,
    first_frame_done: bool,
    frames: u64,
    shot_taken: bool,
    title: String,
    script: script::Script,
    /// the mono font id in use
    font_id: Option<String>,
    font_px: f32,
    media: media::Media,
}

impl Gui {
    fn new(app: App, proxy: EventLoopProxy<UserEvent>) -> Gui {
        let tokens = Tokens::from_theme(&app.theme);
        let wake_proxy = proxy.clone();
        let mut media = media::Media::new(Arc::new(move || {
            let _ = wake_proxy.send_event(UserEvent::Wake);
        }));
        // your real pastes stay out of the demo
        if !app.svc.is_demo() {
            media.scan_pastes();
        }
        // ffmpeg plays what Windows can't: look where Quick Install puts it too
        std::thread::spawn(|| bro_media::set_search_dirs(bro_core::quick_install::search_dirs()));
        let prefs = prefs::Prefs::load(app.persist);
        Gui {
            theme_key: theme_key(&app.theme),
            tokens,
            app,
            window: None,
            gfx: None,
            scale: 1.0,
            prefs,
            mods: ModifiersState::empty(),
            pointer: None,
            pressing: false,
            hits: vec![],
            top_layer: 0,
            anim: Anims::default(),
            layout: layout::Layout::default(),
            panes: HashMap::new(),
            drag: None,
            last_press: None,
            link_press: None,
            started: Instant::now(),
            last_tick: Instant::now(),
            minute: 0,
            links_refreshed: Instant::now(),
            dirty: true,
            ticking: false,
            focused_window: true,
            tui: tui::TuiCanvas::default(),
            overlay_grid: None,
            overlay_native: false,
            painted: chrome::Painted::default(),
            sidebar: sidebar::SideState::default(),
            alt_alone: false,
            first_frame_done: false,
            frames: 0,
            shot_taken: false,
            title: String::new(),
            script: script::Script::from_env(),
            font_id: None,
            font_px: 0.0,
            media,
        }
    }

    fn shutdown(&mut self) {
        self.prefs.save(self.app.persist);
        self.app.shutdown();
    }

    /// Draw once this batch of events is handled: `about_to_wait` asks for the frame after the workspace's
    /// housekeeping, so it shows what `after_events` did (as the terminal UI draws after it). Asking winit here
    /// would paint in the middle of the batch: Windows delivers the paint before the loop goes idle.
    fn redraw(&mut self) {
        self.dirty = true;
    }

    /// Hand an event to the workspace (with the desktop app's own handling for a few).
    fn app_event(&mut self, ev: Event) {
        // what the file preview loaded: pictures and videos show in the media panel instead of another app; a
        // file it can't show stays on its card (with Open), never opening by itself; programs never open
        if let Event::FileView(loaded) = &ev
            && let crate::app::Overlay::File(v) = &self.app.overlay
            && v.request_id() == loaded.req
        {
            // the preview's own tables don't know every picture, video or PDF (tiff, heic, avi…): those it calls
            // binary go to the panel too
            let media_path = match &loaded.result {
                Ok(crate::file_view::Payload::Media(m)) if media::is_media(&m.path) => Some(m.path.clone()),
                Err((_, Some(p))) if media::is_media(p) => Some(p.clone()),
                _ => None,
            };
            if let Some(path) = media_path {
                let owner = v.pane.and_then(|id| media::owner(&self.app, id));
                self.app.overlay = crate::app::Overlay::None;
                self.media.add(path.clone(), media::Source::Opened, owner, None);
                self.show_media(&path);
                return;
            }
            match &loaded.result {
                Ok(crate::file_view::Payload::Media(m)) if media::is_media(&m.path) => {
                    let owner = v.pane.and_then(|id| media::owner(&self.app, id));
                    let path = m.path.clone();
                    self.app.overlay = crate::app::Overlay::None;
                    self.media.add(path.clone(), media::Source::Opened, owner, None);
                    self.show_media(&path);
                    return;
                }
                Ok(crate::file_view::Payload::Media(_)) | Err((_, Some(_))) => {
                    if let Event::FileView(loaded) = ev
                        && let crate::app::Overlay::File(v) = &mut self.app.overlay
                    {
                        let _ = v.apply(*loaded);
                    }
                    self.dirty = true;
                    return;
                }
                _ => {}
            }
        }
        match &ev {
            // alt+v pasted a picture: it's in the gallery too
            Event::Clipboard(id, Some(paths), _) => {
                let owner = media::owner(&self.app, *id);
                for p in paths {
                    self.media.add(PathBuf::from(p.trim_matches('"')), media::Source::Pasted, owner.clone(), None);
                }
            }
            // a link to a program is never run from a click
            Event::OpenLink(uri) if uri.starts_with("file:") => {
                if let Some((path, _, _)) = crate::panes::links::parse_file_uri(uri)
                    && bro_media::is_executable(&path)
                {
                    self.app.toast(crate::alerts::Kind::Error, format!("bro doesn't run programs from a link · {}", path.display()));
                    self.dirty = true;
                    return;
                }
            }
            _ => {}
        }
        // another `bro` was started in a folder: open it here and ask for the user's attention (the terminal UI
        // rings the terminal's bell, which a window doesn't have)
        if let Event::OpenProject(dir) = ev {
            if let Some(w) = &self.window {
                w.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
            self.app.add_project(dir);
            self.app.leave_sidebar();
            self.dirty = true;
            return;
        }
        self.app.perf.begin();
        let mut perf = std::mem::take(&mut self.app.perf);
        perf.event(ev, |ev| self.app.handle(ev));
        self.app.perf = perf;
        self.dirty = true;
    }

    /// The session in focus, for the media panel's scope.
    fn focused_owner(&self) -> Option<media::Owner> {
        self.app.focused().and_then(|id| media::owner(&self.app, id))
    }

    /// Open the media panel on `path` (the keyboard stays where it is).
    fn show_media(&mut self, path: &std::path::Path) {
        if !self.prefs.media_open {
            self.prefs.media_open = true;
            self.prefs.save(self.app.persist);
        }
        self.media.select(path);
        self.dirty = true;
    }

    /// Do what the media panel asked.
    fn media_ask(&mut self, ask: media::Ask) {
        match ask {
            media::Ask::None => {}
            media::Ask::Toast(t) => self.app.toast(crate::alerts::Kind::Info, t),
            media::Ask::Close => {
                self.prefs.media_open = false;
                self.media.focus = false;
                self.prefs.save(self.app.persist);
            }
            media::Ask::Open(path) => {
                if bro_media::is_executable(&path) {
                    self.app.toast(crate::alerts::Kind::Error, format!("bro doesn't run programs from here · {}", path.display()));
                } else if let Some(uri) = crate::panes::links::file_uri(&path, None, None) {
                    self.app_event(Event::OpenLink(uri));
                }
            }
            media::Ask::Reveal(path) => reveal(&path),
            media::Ask::Mention(path, pane) => {
                let target = match pane {
                    Some(p) if self.app.panes.contains_key(&p) => p,
                    Some(_) => {
                        self.app.toast(crate::alerts::Kind::Info, "that session has closed");
                        self.dirty = true;
                        return;
                    }
                    None => match self.app.focused() {
                        Some(f) => f,
                        None => return,
                    },
                };
                let cwd = self.app.panes.get(&target).and_then(|p| p.as_term_ref()).map(|t| t.meta.cwd.clone());
                let shown = match cwd.as_deref().and_then(|c| path.strip_prefix(c).ok()) {
                    Some(rel) => rel.to_string_lossy().to_string(),
                    None => path.to_string_lossy().to_string(),
                };
                let mention = if shown.contains(char::is_whitespace) { format!("@\"{shown}\" ") } else { format!("@{shown} ") };
                self.app.focus_pane(target);
                self.app.leave_sidebar();
                self.media.focus = false;
                self.app.with_pane(target, |p, cx| p.paste(&mention, cx));
            }
        }
        self.dirty = true;
    }

    /// A key the window doesn't keep for itself: the media panel's while it has the keyboard, else the
    /// workspace's.
    fn key(&mut self, k: crossterm::event::KeyEvent) {
        // any key ends a text selection and a link press, as in the terminal UI (ctrl+c copies first: `gui_key`)
        self.link_press = None;
        mouse::clear_selections(self, None);
        // a paste chord while the summary screensaver shows: turn it back first, so the paste still lands (a
        // terminal's paste arrives as a paste, which does both)
        use crossterm::event::{KeyCode, KeyModifiers};
        let paste = (matches!(k.code, KeyCode::Char('v' | 'V')) && (k.modifiers == KeyModifiers::CONTROL || k.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT)))
            || (k.code == KeyCode::Insert && k.modifiers == KeyModifiers::SHIFT);
        if paste {
            self.app.touched();
        }
        if self.prefs.media_open && self.media.focus && !self.app.overlay.is_open() {
            let focus = self.focused_owner();
            if let Some(ask) = self.media.key(&k, focus.as_ref()) {
                self.media_ask(ask);
                return;
            }
        }
        self.app_event(Event::Input(crossterm::event::Event::Key(k)));
    }

    /// The workspace's housekeeping after a batch of events, then anything the screens asked for.
    fn after_events(&mut self) {
        let t = Instant::now();
        self.app.after_events();
        self.app.perf.housekeeping(t.elapsed());
        // screens' requests: OSC 52 clipboard copies, bells
        let ids: Vec<PaneId> = self.app.panes.keys().copied().collect();
        for id in ids {
            let Some(m) = self.app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.mirror()).and_then(|m| term::mirror::Mirror::of(m.as_ref())) else { continue };
            let req = m.take_requests();
            if let Some(text) = req.clipboard {
                window::set_clipboard(&text);
            }
            if req.bell && !self.focused_window
                && let Some(w) = &self.window
            {
                w.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
        }
        if self.media.collect(&self.app) {
            self.dirty = true;
        }
        for r in crate::panes::mirror::take_requests() {
            match r {
                crate::panes::mirror::Request::MediaPanel => {
                    self.prefs.media_open = !self.prefs.media_open;
                    self.media.focus = false;
                    self.prefs.save(self.app.persist);
                    self.dirty = true;
                }
            }
        }
        if self.app.quit {
            self.dirty = false;
        }
    }

    fn theme_changed(&mut self) -> bool {
        let key = theme_key(&self.app.theme);
        if key == self.theme_key {
            return false;
        }
        self.theme_key = key;
        self.tokens = Tokens::from_theme(&self.app.theme);
        term::update_env(&self.tokens, self.font_cell());
        if let Some(w) = &self.window {
            window::style_title_bar(w, self.tokens.bg, self.tokens.light);
        }
        true
    }

    fn font_cell(&self) -> (u16, u16) {
        self.gfx.as_ref().and_then(|g| g.text.mono_metrics()).map(|m| (m.cell_w as u16, m.cell_h as u16)).unwrap_or((9, 19))
    }

    /// Load the terminal font the settings ask for, at the size the prefs ask for.
    fn sync_font(&mut self) {
        let Some(gfx) = &mut self.gfx else { return };
        let want = self.app.svc.settings().font.unwrap_or_else(|| "jetbrains-mono".to_string());
        if self.font_id.as_deref() != Some(want.as_str()) {
            if let Some(f) = crate::fonts::get(&want) {
                gfx.text.set_mono_font(f.data);
            }
            self.font_id = Some(want);
            self.font_px = 0.0;
        }
        let px = (self.prefs.font_size * self.scale).round();
        if (px - self.font_px).abs() > 0.1 {
            self.font_px = px;
            let m = gfx.text.set_mono(px, self.prefs.line_height);
            term::update_env(&self.tokens, (m.cell_w as u16, m.cell_h as u16));
        }
    }

    // ------------------------------------------------------------------ frame

    fn frame(&mut self) {
        let started = Instant::now();
        self.frame_inner();
        let total = started.elapsed();
        self.app.perf.drawn(total, total);
        self.app.perf.finish();
    }

    fn frame_inner(&mut self) {
        let Some(window) = self.window.clone() else { return };
        if self.gfx.is_none() {
            return;
        }
        // minimised: no swapchain image comes, and each try would block the loop for wgpu's 1 s acquire timeout.
        // Coming back resizes the window, which rebuilds the swapchain and draws.
        if window.is_minimized() == Some(true) {
            return;
        }
        self.theme_changed();
        self.sync_font();
        let size = window.inner_size();
        let (w, h) = (size.width as f32 / self.scale, size.height as f32 / self.scale);
        let now = Instant::now();
        let time = self.started.elapsed().as_secs_f64();
        let theme = self.app.theme.clone();
        let mut gfx = self.gfx.take().expect("gfx");
        self.media.prepare(&mut gfx.renderer);
        if gfx.text.poll_fonts() {
            self.font_px = 0.0;
            self.sync_font_with(&mut gfx);
        }
        let mut attempts = 0;
        let (hits, overlay_grid, painted) = loop {
            attempts += 1;
            gfx.scene.clear();
            gfx.scene.background = self.tokens.bg;
            gfx.text.begin_frame();
            self.anim.moving = false;
            let mut hits = Vec::with_capacity(self.hits.len().max(64));
            let mut ui = Ui {
                scene: &mut gfx.scene,
                text: &mut gfx.text,
                t: &self.tokens,
                theme: &theme,
                scale: self.scale,
                pointer: self.pointer,
                pressing: self.pressing,
                hits: &mut hits,
                top_layer: self.top_layer,
                anim: &mut self.anim,
                now,
                time,
                ticking: false,
                screen: Rect::new(0.0, 0.0, w, h),
            };
            let window_rect = Rect::new(0.0, 0.0, w, h);
            let (grid, painted) = draw_all(&mut self.app, &mut ui, window_rect, &mut self.layout, &mut self.panes, &mut self.tui, &mut self.sidebar, &self.prefs, self.focused_window, self.drag.as_ref(), self.alt_alone, &mut self.media);
            self.ticking = ui.ticking || ui.anim.moving;
            if !ui.text.overflow || attempts > 2 {
                break (hits, grid, painted);
            }
            ui.text.grow_atlases();
        };
        self.hits = hits;
        self.overlay_native = matches!(overlay_grid, Some(overlays::Drawn::Native));
        self.overlay_grid = match overlay_grid {
            Some(overlays::Drawn::Fallback(g)) => Some(g),
            _ => None,
        };
        self.painted = painted;
        // a selection ends when its session leaves the screen (else ctrl+c would copy what you can't see)
        for (id, p) in &self.app.panes {
            if self.layout.pane(*id).is_none()
                && let Some(m) = p.as_term_ref().and_then(|t| t.mirror()).and_then(|m| term::mirror::Mirror::of(m.as_ref()))
                && let Some(mut s) = m.try_lock()
                && s.term.selection.is_some()
            {
                s.term.selection = None;
            }
        }
        self.frames += 1;
        // `BRO_GUI_SCREENSHOT=out.png` saves a frame once things settle (docs, tests); `BRO_GUI_EXIT=1` then quits
        if let Ok(out) = std::env::var("BRO_GUI_SCREENSHOT")
            && !self.shot_taken
            && self.started.elapsed() > Duration::from_millis(std::env::var("BRO_GUI_SHOT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500))
        {
            self.shot_taken = true;
            if let Some((cw, ch, px)) = gfx.renderer.capture(&gfx.scene, &mut gfx.text) {
                save_png(&out, cw, ch, &px);
            }
            if std::env::var_os("BRO_GUI_EXIT").is_some() {
                self.app.quit = true;
            }
        }
        // the skin's whole-screen effects
        let fx = self.app.theme.skin.fx;
        let rgb = |c: bro_gfx::Color| [c.r, c.g, c.b];
        gfx.renderer.effects = bro_gfx::Effects {
            phosphor: fx.phosphor.then(|| (rgb(self.tokens.bg), rgb(self.tokens.text))),
            vignette: if fx.vignette { 0.4 } else { 0.0 },
            scanlines: if fx.scanlines { 0.12 } else { 0.0 },
            scanline_px: (2.0 * self.scale).round(),
        };
        match gfx.renderer.render(&gfx.scene, &mut gfx.text) {
            Ok(()) => {}
            Err(bro_gfx::FrameError::Reconfigured) => self.dirty = true,
            Err(bro_gfx::FrameError::Skipped) => {}
        }
        self.gfx = Some(gfx);
        self.update_hover();
        if !self.first_frame_done {
            self.first_frame_done = true;
            window.set_visible(true);
        }
        // input methods open their window at the focused terminal's cursor
        if let Some(id) = self.app.focused()
            && let Some(st) = self.panes.get(&id)
            && let Some((row, col)) = st.drawn.cursor
        {
            let d = &st.drawn;
            let (x, y) = (d.origin.x + col as f32 * d.cell_w, d.origin.y + row as f32 * d.cell_h);
            window.set_ime_cursor_area(winit::dpi::LogicalPosition::new(x as f64, y as f64), winit::dpi::LogicalSize::new(d.cell_w as f64, d.cell_h as f64));
        }
        // the window's title follows the session you're in
        let title = match self.app.focused().and_then(|id| self.app.panes.get(&id)) {
            Some(p) => format!("{} — bro", p.title()),
            None => "bro".to_string(),
        };
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }
        self.anim.sweep(now);
    }

    fn sync_font_with(&mut self, gfx: &mut Gfx) {
        let px = (self.prefs.font_size * self.scale).round();
        self.font_px = px;
        let m = gfx.text.set_mono(px, self.prefs.line_height);
        term::update_env(&self.tokens, (m.cell_w as u16, m.cell_h as u16));
    }

    /// Which layer the pointer is over, and the pointer shape for what's under it.
    fn update_hover(&mut self) {
        let Some(p) = self.pointer else {
            self.top_layer = 0;
            return;
        };
        let top = self.hits.iter().rev().find(|h| h.rect.contains(p));
        self.top_layer = top.map(|h| h.layer).unwrap_or(0);
        let cursor = match &self.drag {
            Some(Drag::Sidebar { .. } | Drag::Media { .. }) => kit::Cursor::ResizeH,
            Some(Drag::Divider { dir: crate::layout::Dir::Right, .. }) => kit::Cursor::ResizeH,
            Some(Drag::Divider { .. }) => kit::Cursor::ResizeV,
            Some(Drag::TileEdge { ax: 0, .. }) => kit::Cursor::ResizeH,
            Some(Drag::TileEdge { .. }) => kit::Cursor::ResizeV,
            Some(Drag::Select { .. }) => kit::Cursor::Text,
            _ => mouse::cursor_at(self, p).unwrap_or(top.map(|h| h.cursor).unwrap_or_default()),
        };
        if let Some(w) = &self.window {
            w.set_cursor(window::cursor_icon(cursor));
        }
    }

    /// When to wake next with nothing happening.
    fn next_wake(&self) -> Duration {
        let mut d = self.app.next_deadline();
        if self.ticking || self.shot_pending() {
            d = d.min(Duration::from_millis(16));
        }
        if let Some(s) = self.script.wake() {
            d = d.min(s.max(Duration::from_millis(1)));
        }
        if self.prefs.media_open
            && let Some(at) = self.media.next_wake()
        {
            d = d.min(at.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)));
        }
        // the sidebar's clock turns over on the minute
        let into = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs() % 60).unwrap_or(0);
        d = d.min(Duration::from_secs(60 - into) + Duration::from_millis(30));
        d
    }

    /// The workspace's clock: pane polls, toasts running out, queued resumes (each keeps its own pace).
    fn tick(&mut self) {
        self.last_tick = Instant::now();
        // the terminal UI looks for links on every redraw, so a path that became a file (or stopped being one)
        // links (or stops) while the screen stays still: look again now and then (the file cache keeps its pace)
        if self.links_refreshed.elapsed() >= Duration::from_secs(2) {
            self.links_refreshed = Instant::now();
            for st in self.panes.values_mut() {
                st.links_seq = u64::MAX;
            }
        }
        self.app_event(Event::Tick);
    }

    /// Save what's on screen as a PNG.
    fn screenshot(&mut self, path: &str) {
        if let Some(gfx) = &mut self.gfx
            && let Some((w, h, px)) = gfx.renderer.capture(&gfx.scene, &mut gfx.text)
        {
            save_png(path, w, h, &px);
        }
    }

    fn shot_pending(&self) -> bool {
        !self.shot_taken && std::env::var_os("BRO_GUI_SCREENSHOT").is_some()
    }
}

fn save_png(path: &str, w: u32, h: u32, rgba: &[u8]) {
    let Ok(file) = std::fs::File::create(path) else { return };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut wr) = enc.write_header() {
        let _ = wr.write_image_data(rgba);
    }
}

/// Show `path` in the file manager.
fn reveal(path: &std::path::Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Some(dir) = path.parent() {
                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
            }
        }
    });
}

/// Changes when the theme's colours change (a custom theme file edited in place keeps its name).
fn theme_key(t: &crate::theme::Theme) -> String {
    // every colour a theme file can set, so editing any of them restyles the window
    format!(
        "{}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{}",
        t.name, t.bg, t.fg, t.accent, t.shine, t.frame, t.muted, t.user, t.inline, t.danger, t.good, t.ansi, t.animated
    )
}

/// Draw one frame of everything.
#[allow(clippy::too_many_arguments)]
fn draw_all(
    app: &mut App,
    ui: &mut Ui,
    window: Rect,
    layout: &mut layout::Layout,
    panes: &mut HashMap<PaneId, PaneState>,
    tui: &mut tui::TuiCanvas,
    side: &mut sidebar::SideState,
    prefs: &prefs::Prefs,
    focused_window: bool,
    drag: Option<&Drag>,
    alt_alone: bool,
    media: &mut media::Media,
) -> (Option<overlays::Drawn>, chrome::Painted) {
    let overlay_open = app.overlay.is_open();
    let launcher = matches!(app.overlay, super::Overlay::Launcher(_));
    let sidebar_on = app.sidebar && window.w >= layout::SIDEBAR_MIN_WINDOW;
    let strip_on = app.strip_on() && window.h >= 320.0;
    if !strip_on {
        // no project tabs on screen: they can't have the keys (as the terminal UI's draw pass)
        app.strip_sel = None;
    }
    let media_w = if prefs.media_open { prefs.media_width } else { 0.0 };
    let (side_r, strip_r, media_r, body) = layout::frame(window, if sidebar_on { prefs.sidebar_width } else { 0.0 }, media_w, strip_on);
    // the terminal UI's notion of where the sidebar is (squeezed = asked for but no room)
    app.side_area = if side_r.w > 0.0 { ratatui::layout::Rect { x: 0, y: 0, width: 1, height: 1 } } else { ratatui::layout::Rect::default() };
    app.strip = if strip_r.h > 0.0 { ratatui::layout::Rect { x: 0, y: 0, width: 1, height: 1 } } else { ratatui::layout::Rect::default() };
    app.drawn_tab = app.showing_tab();
    let cell = ui.text.mono_metrics().map(|m| (m.cell_w / ui.scale, m.cell_h / ui.scale)).unwrap_or((8.4, 18.0));
    let (boxes, groups, zoomed, grown, all_minimized) = layout::panes(app, body, cell, overlay_open && !launcher);
    let vcols = (body.w / cell.0).floor().max(1.0);
    let vrows = (body.h / cell.1).floor().max(1.0);
    *layout = layout::Layout { window, sidebar: side_r, strip: strip_r, media: media_r, body, cell: (body.w / vcols, body.h / vrows), panes: boxes, groups, zoomed, grown, all_minimized };
    let _ = (drag, alt_alone);

    // the theme's backdrop (or the rain setting's) behind everything; a skin's gradient on the window
    if let Some((top, bottom)) = app.theme.skin.fx.gradient {
        let c = |c| kit::color(c, ui.t.bg, &ui.t.ansi);
        let p = ui.px(window);
        ui.scene.quad(bro_gfx::Quad::new(p, bro_gfx::Fill::Vertical(c(top), c(bottom))));
    }
    let (bd, _) = backdrop::choice(app);
    backdrop::draw(ui, window, bd, ui.t.bg);
    // everything else above the backdrop (its glyphs would otherwise draw over this layer's panels)
    ui.layer();

    // the content layer the sessions sit on (WinUI's frame): a step up from the window, its top-left corner
    // rounded where it meets the sidebar and the tab row
    {
        let x = side_r.right();
        let y = if strip_r.h > 0.0 { strip_r.bottom() } else { window.y };
        let lr = Rect::new(x, y, (window.right() - x).max(0.0), (window.bottom() - y).max(0.0));
        let corner = if side_r.w > 0.0 || strip_r.h > 0.0 { kit::radius::OVERLAY } else { 0.0 };
        let corners = bro_gfx::Corners { tl: corner, tr: 0.0, br: 0.0, bl: 0.0 };
        ui.round_corners(lr, corners, ui.t.layer);
        if side_r.w > 0.0 {
            ui.vline(lr.x, lr.y + corner, lr.h - corner, ui.t.stroke);
        }
        if strip_r.h <= 0.0 && side_r.w > 0.0 {
            let p = ui.px(lr);
            let w = ui.scale.round().max(1.0);
            ui.scene.quad(bro_gfx::Quad::new(bro_gfx::Rect::new(p.x, p.y, p.w + 2.0 * w, p.h + 2.0 * w), bro_gfx::Color::TRANSPARENT).corners(corners.scale(ui.scale)).border(w, ui.t.stroke));
        }
    }
    // sidebar
    if side_r.w > 0.0 {
        sidebar::draw(app, ui, side_r, side, focused_window, prefs.media_open, media.unseen);
    }
    // project tabs
    if strip_r.h > 0.0 {
        sidebar::draw_strip(app, ui, strip_r);
    }
    // sessions
    let mut painted = chrome::Painted::default();
    if app.tabs.is_empty() {
        welcome::draw(app, ui, Rect::new(body.x - layout::MARGIN, body.y - layout::MARGIN, body.w + 2.0 * layout::MARGIN, body.h + 2.0 * layout::MARGIN));
    } else {
        painted = chrome::draw_panes(app, ui, layout, panes, tui, focused_window);
        chrome::draw_edges(app, ui, layout);
    }
    // pictures and videos beside the sessions (video and animations only run while you can see them)
    media.set_active(media_r.w > 0.0 && focused_window);
    if media_r.w > 0.0 {
        let focus = app.focused().and_then(|id| media::owner(app, id));
        let key = app.keymap.primary(crate::keymap::Act::Media);
        media.draw(ui, media_r, focus.as_ref(), &key);
    }
    // toasts go under a dialog (as in the terminal UI) but over everything else, the launcher included: `draw`
    // paints them here when a dialog is open, `hints::draw` (via `toasts::draw_late`) otherwise
    toasts::draw(app, ui, window);
    let grid = overlays::draw(app, ui, tui, window, body, focused_window);
    hints::draw(app, ui, layout);
    (grid, painted)
}

impl ApplicationHandler<UserEvent> for Gui {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let w = match window::create(el, &self.prefs, self.tokens.bg) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("bro: couldn't open a window: {e}");
                el.exit();
                return;
            }
        };
        self.scale = w.scale_factor() as f32;
        window::style_title_bar(&w, self.tokens.bg, self.tokens.light);
        window::remember_handle(&w);
        let size = w.inner_size();
        match Renderer::new(w.clone(), size.width, size.height, true) {
            Ok(renderer) => {
                let mono = crate::fonts::get(self.app.svc.settings().font.as_deref().unwrap_or("jetbrains-mono")).or_else(|| crate::fonts::get("jetbrains-mono")).map(|f| f.data).unwrap_or(&[]);
                let mut text = Text::new(mono, crate::fonts::SYMBOLS.data, true);
                text.set_antialias(window::text_antialias(renderer.subpixel_text()));
                self.gfx = Some(Gfx { renderer, text, scene: Scene::new() });
                self.font_id = self.app.svc.settings().font.or(Some("jetbrains-mono".into()));
            }
            Err(e) => {
                eprintln!("bro: {e}");
                el.exit();
                return;
            }
        }
        w.set_ime_allowed(true);
        self.window = Some(w.clone());
        self.icons_init();
        self.redraw();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::App(ev) => self.app_event(ev),
            UserEvent::Wake => self.dirty = true,
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.tick();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() && self.first_frame_done {
            script::step(self);
        }
        // the terminal UI ticks when its wait runs out; here redraws wake the loop first, so tick on the clock
        if self.last_tick.elapsed() >= self.app.next_deadline() {
            self.tick();
        }
        // a new minute: the clock in the sidebar changes
        let minute = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs() / 60).unwrap_or(0);
        if minute != self.minute {
            self.minute = minute;
            self.dirty = true;
        }
        self.after_events();
        if self.app.quit {
            el.exit();
            return;
        }
        if (self.dirty || self.ticking || self.shot_pending())
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + self.next_wake()));
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                // like quit: confirm when sessions are running
                self.app.run_act(crate::keymap::Act::Quit);
                if self.app.quit {
                    el.exit();
                }
                self.redraw();
            }
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.renderer.resize(s.width, s.height);
                }
                self.prefs.note_size(self.window.as_deref());
                // draw right away so live resizing doesn't smear
                self.frame();
            }
            WindowEvent::Moved(_) => self.prefs.note_size(self.window.as_deref()),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.font_px = 0.0;
                self.redraw();
            }
            WindowEvent::Focused(f) => {
                self.focused_window = f;
                use crossterm::event::Event as CEvent;
                self.app_event(Event::Input(if f { CEvent::FocusGained } else { CEvent::FocusLost }));
                if !f {
                    self.alt_alone = false;
                    // every drag ends with the focus (a held tile edge too, which the terminal UI kept); a panel's
                    // edge keeps the width it was dragged to
                    if matches!(self.drag.take(), Some(Drag::Sidebar { .. } | Drag::Media { .. })) {
                        self.prefs.save(self.app.persist);
                    }
                    self.pressing = false;
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                let was_alt = self.mods.alt_key();
                self.mods = m.state();
                let alt_only = m.state() == ModifiersState::ALT;
                if alt_only && !was_alt {
                    self.alt_alone = true;
                    self.app_event(Event::Alt(true));
                } else if !m.state().alt_key() && was_alt {
                    self.alt_alone = false;
                    self.app_event(Event::Alt(false));
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                if is_synthetic {
                    return;
                }
                if input::modifier_key(&event).is_some() {
                    return;
                }
                self.alt_alone = false;
                if event.state == ElementState::Pressed && self.gui_key(&event) {
                    self.redraw();
                    return;
                }
                if let Some(k) = input::key(&event, self.mods) {
                    self.key(k);
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.app.touched();
                // typed text ends a selection, like a key or a paste
                mouse::clear_selections(self, None);
                mouse::type_text(self, &text);
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = Point::new(position.x as f32 / self.scale, position.y as f32 / self.scale);
                self.pointer = Some(p);
                // the mouse is input too: it restarts the summary screensaver's wait (and wakes it)
                self.app.touched();
                mouse::moved(self, p);
                self.update_hover();
                self.redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                self.redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(p) = self.pointer else { return };
                match state {
                    // a click that wakes the summary screensaver does only that, as in the terminal UI
                    ElementState::Pressed if self.app.touched() => {}
                    ElementState::Pressed => {
                        self.pressing = button == MouseButton::Left;
                        mouse::pressed(self, p, button);
                    }
                    ElementState::Released => {
                        self.pressing = false;
                        mouse::released(self, p, button);
                    }
                }
                self.redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(p) = self.pointer else { return };
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(d) => (d.x as f32 / (18.0 * self.scale), d.y as f32 / (18.0 * self.scale)),
                };
                if self.app.touched() {
                    self.redraw();
                    return;
                }
                mouse::wheel(self, p, dx, dy);
                self.redraw();
            }
            WindowEvent::DroppedFile(path) => {
                // a drop pastes its path: a touch, and the end of a selection, like a paste
                self.app.touched();
                mouse::clear_selections(self, None);
                mouse::dropped(self, path);
                self.redraw();
            }
            WindowEvent::RedrawRequested => {
                self.dirty = false;
                self.frame();
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.prefs.note_size(self.window.as_deref());
        // sessions and services stop after the loop (the bridge may take a few seconds): out of sight, not frozen
        if let Some(w) = &self.window {
            w.set_visible(false);
        }
    }
}

impl Gui {
    /// Keys the desktop app handles itself before the workspace sees them: font size, copy, fullscreen, and a
    /// keyboard's browser back / forward keys (the mouse's back / forward buttons' history, as the terminal UI has
    /// them on Windows).
    fn gui_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let ctrl = self.mods.control_key();
        let shift = self.mods.shift_key();
        let alt = self.mods.alt_key();
        // macOS: cmd+c copies the selection and cmd+v pastes, as every Mac app does (the workspace's own paste)
        #[cfg(target_os = "macos")]
        if self.mods.super_key() && !ctrl && !alt {
            match &ev.logical_key {
                Key::Character(c) if c.eq_ignore_ascii_case("c") => return mouse::copy_selection(self),
                Key::Character(c) if c.eq_ignore_ascii_case("v") => {
                    let k = crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('v'), crossterm::event::KeyModifiers::CONTROL);
                    self.app_event(Event::Input(crossterm::event::Event::Key(k)));
                    return true;
                }
                _ => {}
            }
        }
        match &ev.logical_key {
            Key::Named(n @ (NamedKey::BrowserBack | NamedKey::BrowserForward)) => {
                self.app_event(Event::Navigate(*n == NamedKey::BrowserBack));
                true
            }
            // terminal font size: ctrl+= / ctrl+- / ctrl+0 (ctrl+shift on terminals would reach programs)
            Key::Character(c) if ctrl && !alt && (c == "=" || c == "+") => {
                self.prefs.font_size = (self.prefs.font_size + 1.0).min(32.0);
                true
            }
            Key::Character(c) if ctrl && !alt && c == "-" => {
                self.prefs.font_size = (self.prefs.font_size - 1.0).max(8.0);
                true
            }
            Key::Character(c) if ctrl && !alt && c == "0" => {
                self.prefs.font_size = prefs::DEFAULT_FONT_SIZE;
                true
            }
            // copy the selection: ctrl+shift+c, or ctrl+c while something is selected
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("c") && (shift || mouse::has_selection(self)) => mouse::copy_selection(self),
            Key::Named(NamedKey::F11) => {
                if let Some(w) = &self.window {
                    let full = w.fullscreen().is_some();
                    w.set_fullscreen(if full { None } else { Some(winit::window::Fullscreen::Borderless(None)) });
                }
                true
            }
            _ => false,
        }
    }

    fn icons_init(&mut self) {
        crate::icons::retint(self.app.theme.is_light());
    }
}
