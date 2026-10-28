//! HTML in the file preview, as the page: the system's web view (WebView2 on Windows, WKWebView on macOS) laid
//! over the preview card's body, so the file shows with its styles, scripts and pictures instead of as source.
//! `m` still flips to the source; Linux keeps the source (a WebKitGTK view would need a GTK loop beside winit's).
//!
//! One view, made the first time a page shows and kept for the next one (hidden, on a blank page) until it has
//! sat unused for [`IDLE`]. It shows once its page's document is ready (or after [`SHOW_ANYWAY`]), so a page
//! never flashes white before it paints. The keyboard stays with bro, whose scroll keys scroll the page; a click
//! in the page hands it the keys until a click elsewhere on the window, and Esc there still closes the preview.
//! Links to the web open in your browser, links to other local pages in the view. A page loads from its file as
//! a browser opens one (`file://`), so its scripts can't read other files.

// (Linux makes no view: the page's messages and script go unused there)
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use super::UserEvent;
use bro_bridge::file_preview::ResolveRoots;
use bro_gfx::Rect;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

/// Unused this long, the view goes (and the browser processes behind it).
const IDLE: Duration = Duration::from_secs(90);
/// A page that hasn't said its document is ready by now shows anyway.
const SHOW_ANYWAY: Duration = Duration::from_millis(1500);

/// What a page said, carried into the event loop by the view's handlers.
#[derive(Debug, PartialEq)]
pub enum Msg {
    /// the document at this URL is ready (or finished loading)
    Ready(String),
    /// Esc in the page
    Close,
    /// a link to the web: for your browser
    Open(String),
    /// a local page asked for a window of its own: it opens in the view
    Load(String),
}

/// Where the page goes this frame.
pub struct Want<'a> {
    pub path: &'a Path,
    /// the preview's session folders (what the page may read from, see [`read_root`])
    pub roots: &'a ResolveRoots,
    /// the card's body, logical pixels
    pub rect: Rect,
}

pub struct Pages {
    proxy: EventLoopProxy<UserEvent>,
    view: Option<imp::View>,
    /// the view couldn't be made (no WebView2 runtime): HTML shows as source from now on
    failed: bool,
    /// hidden since, while a view is kept
    hidden_at: Option<Instant>,
}

impl Pages {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Pages {
        Pages { proxy, view: None, failed: false, hidden_at: None }
    }

    /// This build can show pages at all.
    pub fn available() -> bool {
        cfg!(any(windows, target_os = "macos"))
    }

    /// Put the page where `want` says, or hide it. Ok(whether it's on screen); Err once, when the view can't be
    /// made.
    pub fn sync(&mut self, window: &Window, want: Option<Want<'_>>, scale: f32, light: bool) -> Result<bool, String> {
        let Some(want) = want else {
            if let Some(v) = &mut self.view {
                v.blank();
                self.hidden_at.get_or_insert_with(Instant::now);
            }
            return Ok(false);
        };
        if self.failed {
            return Ok(false);
        }
        let bounds = physical(want.rect, scale);
        let v = match &mut self.view {
            Some(v) => v,
            None => match imp::View::new(window, self.proxy.clone(), bounds) {
                Ok(v) => self.view.insert(v),
                Err(e) => {
                    self.failed = true;
                    return Err(e);
                }
            },
        };
        self.hidden_at = None;
        v.theme(light);
        if v.file() != Some(want.path) {
            v.open(want.path, want.roots);
        }
        v.place(bounds);
        let show = v.waiting_until().is_none_or(|at| Instant::now() >= at);
        v.show(show);
        Ok(show)
    }

    /// A page said its document is ready: true when it's the one asked for (it can show now).
    pub fn ready(&mut self, url: &str) -> bool {
        match &mut self.view {
            Some(v) if v.waiting_until().is_some() && v.file().is_some_and(|f| same_file(url, f)) => {
                v.set_ready();
                true
            }
            _ => false,
        }
    }

    /// A local page asked for a window: it shows in the view instead.
    pub fn load(&self, url: &str) {
        if let Some(v) = &self.view {
            v.load_url(url);
        }
    }

    /// Run `js` in the page on screen.
    pub fn run(&self, js: &str) {
        if let Some(v) = self.view.as_ref().filter(|v| v.shown()) {
            v.eval(js);
        }
    }

    /// A click on the window outside the page: the keys come back to bro.
    pub fn focus_window(&self) {
        if let Some(v) = self.view.as_ref().filter(|v| v.shown()) {
            v.focus_parent();
        }
    }

    /// Let the view go once it has sat unused for [`IDLE`].
    pub fn idle(&mut self) {
        if self.hidden_at.is_some_and(|t| t.elapsed() >= IDLE) {
            self.reset();
        }
    }

    /// Let the view go now (its window is going).
    pub fn reset(&mut self) {
        self.view = None;
        self.hidden_at = None;
    }

    /// When the loop has to come back for the view: a page due to show anyway, or an idle one due to go.
    pub fn next_wake(&self) -> Option<Instant> {
        self.view.as_ref().and_then(imp::View::waiting_until).or(self.hidden_at.map(|t| t + IDLE))
    }
}

/// `rect` (logical) in whole physical pixels: x, y, width, height.
fn physical(rect: Rect, scale: f32) -> (i32, i32, u32, u32) {
    let (x, y) = ((rect.x * scale).round(), (rect.y * scale).round());
    let (r, b) = ((rect.right() * scale).round(), (rect.bottom() * scale).round());
    (x as i32, y as i32, (r - x).max(1.0) as u32, (b - y).max(1.0) as u32)
}

/// `url` is the page for `path` (the view reports it percent-encoded, perhaps with a fragment).
fn same_file(url: &str, path: &Path) -> bool {
    let Some(shown) = url::Url::parse(url).ok().and_then(|u| u.to_file_path().ok()) else { return false };
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if cfg!(any(windows, target_os = "macos")) { norm(&shown).eq_ignore_ascii_case(&norm(path)) } else { norm(&shown) == norm(path) }
}

/// The folder a page may read from, for WKWebView (WebView2 reads like a browser): the session's project or
/// folder when the page is inside one (so `../styles.css` still loads), else its own.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn read_root(path: &Path, roots: &ResolveRoots) -> PathBuf {
    let own = path.parent().map(Path::to_path_buf).unwrap_or_default();
    [roots.project.as_deref(), Some(roots.cwd.as_path())]
        .into_iter()
        .flatten()
        .filter_map(|r| r.canonicalize().ok().map(crate::util::strip_verbatim))
        .find(|r| path.starts_with(r))
        .unwrap_or(own)
}

fn is_web(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s))
}

/// What the page's script posted (see [`SCRIPT`]), from the page at `url`.
fn message(body: &str, url: &str) -> Option<Msg> {
    let (what, arg) = body.split_once(' ').unwrap_or((body, ""));
    match what {
        "ready" => Some(Msg::Ready(url.to_string())),
        "close" => Some(Msg::Close),
        "open" if is_web(arg) => Some(Msg::Open(arg.to_string())),
        _ => None,
    }
}

/// What a page asking for a new window gets: the web goes to your browser, a local page into the view.
fn new_window(url: &str) -> Option<Msg> {
    if is_web(url) {
        Some(Msg::Open(url.to_string()))
    } else if url.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
        Some(Msg::Load(url.to_string()))
    } else {
        None
    }
}

/// In every page, before its own scripts: say when the document is ready, hand Esc and web links back to bro,
/// and scroll for bro's scroll keys (the page's scroller, or failing that its biggest scrolling panel).
const SCRIPT: &str = r#"(function () {
  if (window.top !== window || window.__bro) return;
  var post = function (m) { try { window.ipc.postMessage(m); } catch (e) {} };
  var scroller = function () {
    var el = document.scrollingElement || document.documentElement;
    if (el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1 || !document.body) return el;
    var best = null, area = 0, all = document.body.getElementsByTagName('*');
    for (var i = 0; i < all.length; i++) {
      var n = all[i];
      if (n.scrollHeight <= n.clientHeight + 1) continue;
      var o = getComputedStyle(n).overflowY, a = n.clientWidth * n.clientHeight;
      if ((o === 'auto' || o === 'scroll') && a > area) { best = n; area = a; }
    }
    return best || el;
  };
  window.__bro = {
    scroll: function (lines, pages, cols, edge) {
      var el = scroller();
      if (edge) { el.scrollTo({ top: edge < 0 ? 0 : el.scrollHeight, behavior: 'smooth' }); return; }
      var page = el === document.scrollingElement ? window.innerHeight : el.clientHeight;
      el.scrollBy({ left: cols * 40, top: lines * 40 + pages * page * 0.875, behavior: 'smooth' });
    }
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', function () { post('ready'); }, { once: true });
  else post('ready');
  window.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && !e.defaultPrevented && !e.ctrlKey && !e.altKey && !e.metaKey) post('close');
  });
  document.addEventListener('click', function (e) {
    if (e.defaultPrevented || e.button !== 0 || !e.target || !e.target.closest) return;
    var a = e.target.closest('a[href]');
    if (a && /^(https?|mailto):/i.test(a.href)) { e.preventDefault(); post('open ' + a.href); }
  });
})();"#;

/// The script that scrolls the page for one of the preview's scroll keys (the same keys as the source's).
pub fn scroll_script(k: &KeyEvent) -> Option<String> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    // lines, pages, columns, edge (-1 the top, 1 the bottom)
    let (lines, pages, cols, edge): (i32, f32, i32, i32) = match k.code {
        KeyCode::Down | KeyCode::Char('j') => (1, 0.0, 0, 0),
        KeyCode::Up | KeyCode::Char('k') => (-1, 0.0, 0, 0),
        KeyCode::Char('d') if ctrl => (0, 0.5, 0, 0),
        KeyCode::Char('u') if ctrl => (0, -0.5, 0, 0),
        KeyCode::PageDown | KeyCode::Char(' ') => (0, 1.0, 0, 0),
        KeyCode::Char('f') if ctrl => (0, 1.0, 0, 0),
        KeyCode::PageUp | KeyCode::Char('b') => (0, -1.0, 0, 0),
        KeyCode::Home | KeyCode::Char('g') => (0, 0.0, 0, -1),
        KeyCode::End | KeyCode::Char('G') => (0, 0.0, 0, 1),
        KeyCode::Left | KeyCode::Char('h') => (0, 0.0, -1, 0),
        KeyCode::Right | KeyCode::Char('l') => (0, 0.0, 1, 0),
        _ => return None,
    };
    Some(format!("window.__bro && window.__bro.scroll({lines}, {pages}, {cols}, {edge})"))
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use super::{Msg, ResolveRoots, SCRIPT, SHOW_ANYWAY, UserEvent, message, new_window};
    use std::path::{Path, PathBuf};
    use std::time::Instant;
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    pub struct View {
        web: wry::WebView,
        _context: wry::WebContext,
        /// the file asked for, and when
        file: Option<PathBuf>,
        asked: Instant,
        ready: bool,
        shown: bool,
        bounds: (i32, i32, u32, u32),
        light: Option<bool>,
    }

    fn rect((x, y, w, h): (i32, i32, u32, u32)) -> wry::Rect {
        wry::Rect { position: wry::dpi::PhysicalPosition::new(x, y).into(), size: wry::dpi::PhysicalSize::new(w, h).into() }
    }

    impl View {
        pub fn new(window: &Window, proxy: EventLoopProxy<UserEvent>, bounds: (i32, i32, u32, u32)) -> Result<View, String> {
            // its cache and storage live with bro's, not beside the program
            let mut context = wry::WebContext::new(Some(bro_core::paths::bro_dir().join("webview")));
            let send = move |m: Msg| {
                let _ = proxy.send_event(UserEvent::Page(m));
            };
            let (ipc, loaded, popup) = (send.clone(), send.clone(), send);
            let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
                .with_visible(false)
                .with_focused(false)
                .with_bounds(rect(bounds))
                .with_devtools(true)
                .with_initialization_script(SCRIPT)
                .with_ipc_handler(move |req| {
                    if let Some(m) = message(req.body(), &req.uri().to_string()) {
                        ipc(m);
                    }
                })
                .with_on_page_load_handler(move |event, url| {
                    if matches!(event, wry::PageLoadEvent::Finished) {
                        loaded(Msg::Ready(url));
                    }
                })
                .with_new_window_req_handler(move |url, _| {
                    if let Some(m) = new_window(&url) {
                        popup(m);
                    }
                    wry::NewWindowResponse::Deny
                });
            // Edge's thin scrollbars that show while you scroll, not the classic grey bar beside every page
            #[cfg(windows)]
            let builder = {
                use wry::WebViewBuilderExtWindows;
                builder.with_scroll_bar_style(wry::ScrollBarStyle::FluentOverlay)
            };
            let web = builder.build_as_child(window).map_err(|e| e.to_string())?;
            Ok(View { web, _context: context, file: None, asked: Instant::now(), ready: false, shown: false, bounds, light: None })
        }

        pub fn file(&self) -> Option<&Path> {
            self.file.as_deref()
        }

        /// While a page loads: when it shows even if it hasn't said it's ready.
        pub fn waiting_until(&self) -> Option<Instant> {
            (self.file.is_some() && !self.ready).then(|| self.asked + SHOW_ANYWAY)
        }

        pub fn set_ready(&mut self) {
            self.ready = true;
        }

        pub fn shown(&self) -> bool {
            self.shown
        }

        /// Pages that follow `prefers-color-scheme` match bro's theme.
        pub fn theme(&mut self, light: bool) {
            if self.light == Some(light) {
                return;
            }
            self.light = Some(light);
            #[cfg(windows)]
            {
                use wry::WebViewExtWindows;
                let _ = self.web.set_theme(if light { wry::Theme::Light } else { wry::Theme::Dark });
            }
        }

        pub fn open(&mut self, path: &Path, roots: &ResolveRoots) {
            self.file = Some(path.to_path_buf());
            self.asked = Instant::now();
            self.ready = false;
            load_file(&self.web, path, roots);
        }

        pub fn load_url(&self, url: &str) {
            let _ = self.web.load_url(url);
        }

        pub fn eval(&self, js: &str) {
            let _ = self.web.evaluate_script(js);
        }

        pub fn focus_parent(&self) {
            let _ = self.web.focus_parent();
        }

        pub fn place(&mut self, bounds: (i32, i32, u32, u32)) {
            if bounds != self.bounds {
                self.bounds = bounds;
                let _ = self.web.set_bounds(rect(bounds));
            }
        }

        pub fn show(&mut self, show: bool) {
            if show != self.shown {
                self.shown = show;
                let _ = self.web.set_visible(show);
            }
        }

        /// Hidden on a blank page, the keys back with the window if the page had them.
        pub fn blank(&mut self) {
            if self.shown {
                self.focus_parent();
            }
            self.show(false);
            if self.file.take().is_some() {
                let _ = self.web.load_url("about:blank");
            }
            self.ready = false;
        }
    }

    #[cfg(windows)]
    fn load_file(web: &wry::WebView, path: &Path, _: &ResolveRoots) {
        if let Ok(url) = url::Url::from_file_path(path) {
            let _ = web.load_url(url.as_str());
        }
    }

    /// WKWebView reads a local page, and what it links to, only from a folder it's given.
    #[cfg(target_os = "macos")]
    fn load_file(web: &wry::WebView, path: &Path, roots: &ResolveRoots) {
        use objc2_foundation::{NSString, NSURL};
        use wry::WebViewExtMacOS;
        let root = super::read_root(path, roots);
        let file = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let dir = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&root.to_string_lossy()), true);
        let wk = web.webview();
        unsafe {
            let _ = wk.loadFileURL_allowingReadAccessToURL(&file, &dir);
        }
    }
}

/// No web view here: none is ever made (see [`Pages::available`]).
#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::{ResolveRoots, UserEvent};
    use std::path::Path;
    use std::time::Instant;
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    pub enum View {}

    impl View {
        pub fn new(_: &Window, _: EventLoopProxy<UserEvent>, _: (i32, i32, u32, u32)) -> Result<View, String> {
            Err("this system has no web view bro can use".into())
        }
        pub fn file(&self) -> Option<&Path> {
            match *self {}
        }
        pub fn waiting_until(&self) -> Option<Instant> {
            match *self {}
        }
        pub fn set_ready(&mut self) {}
        pub fn shown(&self) -> bool {
            match *self {}
        }
        pub fn theme(&mut self, _: bool) {}
        pub fn open(&mut self, _: &Path, _: &ResolveRoots) {}
        pub fn load_url(&self, _: &str) {}
        pub fn eval(&self, _: &str) {}
        pub fn focus_parent(&self) {}
        pub fn place(&mut self, _: (i32, i32, u32, u32)) {}
        pub fn show(&mut self, _: bool) {}
        pub fn blank(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_says_ready_close_and_open() {
        let page = "file:///C:/work/report.html";
        assert_eq!(message("ready", page), Some(Msg::Ready(page.into())));
        assert_eq!(message("close", page), Some(Msg::Close));
        assert_eq!(message("open https://example.com/a b", page), Some(Msg::Open("https://example.com/a b".into())));
        assert_eq!(message("open mailto:me@example.com", page), Some(Msg::Open("mailto:me@example.com".into())));
        assert_eq!(message("open file:///C:/Windows/System32/calc.exe", page), None, "only web links go to the browser");
        assert_eq!(message("open javascript:alert(1)", page), None);
        assert_eq!(message("nonsense", page), None);
    }

    #[test]
    fn new_windows_go_to_the_browser_or_the_view() {
        assert_eq!(new_window("HTTPS://example.com"), Some(Msg::Open("HTTPS://example.com".into())));
        assert_eq!(new_window("file:///C:/work/other.html"), Some(Msg::Load("file:///C:/work/other.html".into())));
        assert_eq!(new_window("about:blank"), None);
        assert_eq!(new_window("javascript:void(0)"), None);
    }

    #[test]
    fn a_ready_page_is_matched_to_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my report.html");
        let url = url::Url::from_file_path(&path).unwrap();
        assert!(same_file(url.as_str(), &path), "{url}");
        assert!(same_file(&format!("{url}#totals"), &path), "a fragment is the same page");
        assert!(!same_file(url.as_str(), &dir.path().join("other.html")));
        assert!(!same_file("about:blank", &path));
    }

    #[test]
    fn a_page_reads_from_its_session_folder_when_it_is_inside_one() {
        let dir = tempfile::tempdir().unwrap();
        let project = crate::util::strip_verbatim(dir.path().canonicalize().unwrap());
        std::fs::create_dir_all(project.join("reports")).unwrap();
        let page = project.join("reports").join("week.html");
        let roots = ResolveRoots { cwd: project.join("reports"), project: Some(project.clone()) };
        assert_eq!(read_root(&page, &roots), project, "the project, so ../assets loads");
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("page.html");
        assert_eq!(read_root(&outside, &roots), elsewhere.path(), "outside the session: its own folder");
    }

    #[test]
    fn the_scroll_keys_scroll_the_page() {
        let key = |c: KeyCode, m: KeyModifiers| scroll_script(&KeyEvent::new(c, m));
        assert_eq!(key(KeyCode::Down, KeyModifiers::NONE).as_deref(), Some("window.__bro && window.__bro.scroll(1, 0, 0, 0)"));
        assert_eq!(key(KeyCode::PageUp, KeyModifiers::NONE).as_deref(), Some("window.__bro && window.__bro.scroll(0, -1, 0, 0)"));
        assert_eq!(key(KeyCode::Char('d'), KeyModifiers::CONTROL).as_deref(), Some("window.__bro && window.__bro.scroll(0, 0.5, 0, 0)"));
        assert_eq!(key(KeyCode::Char('G'), KeyModifiers::SHIFT).as_deref(), Some("window.__bro && window.__bro.scroll(0, 0, 0, 1)"));
        assert_eq!(key(KeyCode::Char('e'), KeyModifiers::NONE), None, "the preview's other keys stay its own");
        assert_eq!(key(KeyCode::Esc, KeyModifiers::NONE), None);
    }

    #[test]
    fn bounds_round_to_whole_pixels() {
        assert_eq!(physical(Rect::new(10.25, 20.0, 100.5, 50.0), 1.5), (15, 30, 151, 75));
        assert_eq!(physical(Rect::new(0.0, 0.0, 0.0, 0.0), 2.0), (0, 0, 1, 1), "never empty");
    }
}
