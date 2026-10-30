//! MCP Apps in focus mode: a tool result whose server gave it a page of its own (a `ui://` resource, see
//! [`bro_core::sessions::conversation::McpApp`]) shows as that page, in a card in the conversation, the way
//! claude.ai and ChatGPT show them. Nothing on the server's side changes: it's the MCP Apps protocol.
//!
//! The page is the server's. bro reads it through the session's bro mod, over Claude Code's own connection to the
//! server (so connectors and their logins just work), and keeps it on disk: a page stays readable after its
//! session ends, and shows at once the next time. It runs in the system's web view, in a sandboxed frame of bro's
//! own host page ([`HOST`]), which speaks MCP Apps to it (and the Apps SDK's `window.openai`, for pages made for
//! ChatGPT): it gets the call's input and result and bro's theme, says how tall it is, and can open links, call
//! its own server's tools (through the mod again) and send the session a message.
//!
//! One web view per card near the screen, each laid over its card and clipped to the conversation as it scrolls.
//! The wheel scrolls the conversation unless the page can scroll that way itself; a click in the page hands it
//! the keys until a click elsewhere on the window, and Esc gives them back.

// (Linux makes no view: the cards show what they'd hold, without it)
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use super::UserEvent;
use crate::layout::PaneId;
use crate::services::Services;
use bro_core::mcp_apps::{self, Frame, HOST, script_json};
use bro_core::sessions::conversation::McpApp;
use bro_gfx::{Color, Rect};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

/// A card: its session and the call's id.
pub type Key = (PaneId, String);

/// Away from the screen this long, a card's view goes (and the page in it).
const IDLE: Duration = Duration::from_secs(60);
/// At most this many views at once: the ones furthest from use go first.
const VIEWS_MAX: usize = 8;
/// What a card's page is given before it says how tall it is.
pub const FIRST_HEIGHT: f32 = 320.0;
/// A page shows once it says how tall it is (so it never shows as an empty box), or after this long anyway.
const SHOW_ANYWAY: Duration = Duration::from_millis(1500);

/// A page as far as bro has it.
#[derive(Clone, Debug)]
pub enum Page {
    Reading,
    Ready(Arc<str>),
    /// why it can't be had (nothing said: no card, the step alone, as in the demo without the page)
    Failed(String),
}

/// Something for the window's loop, from a page, the mod or the disk.
#[derive(Debug)]
pub enum Msg {
    /// a page's HTML came, or why it couldn't
    Read { server: String, uri: String, page: Result<Arc<str>, String> },
    /// what a view's host page posted
    View { view: u64, body: String },
    /// the mod's answer to a page's request
    Answer { view: u64, n: String, result: Result<Value, String> },
}

/// What the window should do for a page.
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// draw again (a page's size or state changed)
    Redraw,
    /// a link for your browser
    Open(String),
    /// a message for the session, as if you'd sent it
    Send(PaneId, String),
    /// the wheel over a page that can't scroll: the conversation scrolls
    Wheel(PaneId, f32),
    /// Esc in a page: the keys come back to the window
    Keys,
}

/// A card on screen this frame: where its page goes.
pub struct Want {
    pub key: Key,
    /// the session's bro id (its mod's `BRO_SESSION_ID`)
    pub sid: String,
    pub app: Arc<McpApp>,
    /// the page, whole (logical px)
    pub rect: Rect,
    /// the part of the window it may show in
    pub clip: Rect,
    /// the card's fill, which the page sits on
    pub fill: Color,
    /// the tallest the page may grow
    pub max_h: f32,
}

pub struct Apps {
    proxy: EventLoopProxy<UserEvent>,
    pages: HashMap<(String, String), Page>,
    views: HashMap<Key, imp::View>,
    /// view id → its card
    ids: HashMap<u64, Key>,
    next_id: u64,
    /// each page's height as it last said
    heights: HashMap<Key, f32>,
    /// this frame's cards
    wants: Vec<Want>,
    /// the web view can't be made here: cards say so
    failed: Option<String>,
    /// views off screen since
    away: HashMap<Key, Instant>,
}

impl Apps {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Apps {
        Apps { proxy, pages: HashMap::new(), views: HashMap::new(), ids: HashMap::new(), next_id: 1, heights: HashMap::new(), wants: vec![], failed: None, away: HashMap::new() }
    }

    /// This build can show pages at all.
    pub fn available() -> bool {
        cfg!(any(windows, target_os = "macos"))
    }

    /// The web view couldn't be made: why.
    pub fn unavailable(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// How tall the page of `key` said it is.
    pub fn height(&self, key: &Key) -> Option<f32> {
        self.heights.get(key).copied()
    }

    /// The page `app` shows in, read (from the disk, else through session `sid`'s mod) the first time it's asked.
    pub fn page(&mut self, svc: &Services, sid: &str, app: &McpApp) -> Page {
        let k = (app.server.clone(), app.resource.clone());
        if let Some(p) = self.pages.get(&k) {
            return p.clone();
        }
        self.pages.insert(k.clone(), Page::Reading);
        let (svc, proxy, sid) = (svc.clone(), self.proxy.clone(), sid.to_string());
        std::thread::spawn(move || read(&svc, &proxy, &sid, k.0, k.1));
        Page::Reading
    }

    /// Read the page of `app` again (it failed): through the mod this time.
    pub fn retry(&mut self, svc: &Services, sid: &str, app: &McpApp) {
        self.pages.remove(&(app.server.clone(), app.resource.clone()));
        self.page(svc, sid, app);
    }

    /// A card on screen this frame (its page goes there once the frame is drawn, see [`Apps::sync`]).
    pub fn want(&mut self, w: Want) {
        self.wants.push(w);
    }

    /// Put each page where this frame's cards are, or hide it; `hidden` hides them all (something covers the
    /// window). Views away from the screen a while go.
    pub fn sync(&mut self, window: &Window, scale: f32, light: bool, hidden: bool, version: &str) {
        let wants = std::mem::take(&mut self.wants);
        let now = Instant::now();
        let mut shown: Vec<Key> = vec![];
        if !hidden && self.failed.is_none() {
            for w in &wants {
                let Some(Page::Ready(html)) = self.pages.get(&(w.app.server.clone(), w.app.resource.clone())).cloned() else { continue };
                let visible = w.rect.intersect(&w.clip);
                if visible.w < 1.0 || visible.h < 1.0 {
                    continue;
                }
                if !self.views.contains_key(&w.key) {
                    if self.views.len() >= VIEWS_MAX {
                        self.evict(&shown);
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    match imp::View::new(window, self.proxy.clone(), id, physical(visible, scale), w.fill) {
                        Ok(v) => {
                            self.views.insert(w.key.clone(), v);
                            self.ids.insert(id, w.key.clone());
                        }
                        Err(e) => {
                            self.failed = Some(e);
                            break;
                        }
                    }
                }
                let Some(v) = self.views.get_mut(&w.key) else { continue };
                if v.app.as_ref().is_none_or(|a| !Arc::ptr_eq(a, &w.app)) {
                    v.app = Some(w.app.clone());
                    v.sid = w.sid.clone();
                }
                if v.payload.is_none() {
                    v.payload = Some(mcp_apps::payload(&w.app, &html, &frame(w, light, version)));
                    v.deliver();
                }
                v.fill(w.fill);
                v.context(mcp_apps::context(&frame(w, light, version)));
                v.place(physical(w.rect, scale), physical(visible, scale));
                v.show(v.sized || v.made.elapsed() >= SHOW_ANYWAY);
                shown.push(w.key.clone());
                self.away.remove(&w.key);
            }
        }
        for (k, v) in &mut self.views {
            if !shown.contains(k) {
                if v.shown() {
                    v.show(false);
                }
                self.away.entry(k.clone()).or_insert(now);
            }
        }
        // views left off screen a while go
        let gone: Vec<Key> = self.away.iter().filter(|(_, t)| now.duration_since(**t) >= IDLE).map(|(k, _)| k.clone()).collect();
        for k in gone {
            self.drop_view(&k);
        }
    }

    /// When the loop has to come back: a page due to show anyway, an idle view due to go.
    pub fn next_wake(&self) -> Option<Instant> {
        let showing = self.views.values().filter(|v| !v.sized && !v.shown()).map(|v| v.made + SHOW_ANYWAY).min();
        self.away.values().map(|t| *t + IDLE).chain(showing).min()
    }

    /// The page of `key` has said how tall it is (it's drawn).
    pub fn sized(&self, key: &Key) -> bool {
        self.views.get(key).is_some_and(|v| v.sized)
    }

    /// Room for one more view: the one away longest goes (never one on screen this frame).
    fn evict(&mut self, keep: &[Key]) {
        let oldest = self.away.iter().filter(|(k, _)| !keep.contains(k)).min_by_key(|(_, t)| **t).map(|(k, _)| k.clone());
        let any = || self.views.keys().find(|k| !keep.contains(k)).cloned();
        if let Some(k) = oldest.or_else(any) {
            self.drop_view(&k);
        }
    }

    fn drop_view(&mut self, k: &Key) {
        self.away.remove(k);
        if let Some(v) = self.views.remove(k) {
            self.ids.remove(&v.id);
        }
    }

    /// Let every view go (the window is going, or being swapped).
    pub fn reset(&mut self) {
        self.views.clear();
        self.ids.clear();
        self.away.clear();
    }

    /// A click on the window outside the pages: the keys are bro's again.
    pub fn focus_window(&self) {
        if let Some(v) = self.views.values().find(|v| v.shown()) {
            v.focus_parent();
        }
    }

    /// Something from a page, the mod or the disk: what the window should do.
    pub fn msg(&mut self, m: Msg, svc: &Services) -> Vec<Effect> {
        match m {
            Msg::Read { server, uri, page } => {
                let p = match page {
                    Ok(html) => Page::Ready(html),
                    Err(e) => Page::Failed(e),
                };
                self.pages.insert((server, uri), p);
                vec![Effect::Redraw]
            }
            Msg::Answer { view, n, result } => {
                if let Some(v) = self.ids.get(&view).and_then(|k| self.views.get(k)) {
                    let (ok, value) = match result {
                        Ok(v) => (true, v),
                        Err(e) => (false, Value::String(e)),
                    };
                    v.eval(&format!("window.__broApp && window.__broApp.answer({}, {ok}, {})", json!(n), script_json(&value)));
                }
                vec![]
            }
            Msg::View { view, body } => {
                let Some(key) = self.ids.get(&view).cloned() else { return vec![] };
                let Ok(m) = serde_json::from_str::<Value>(&body) else { return vec![] };
                let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                match m.get("t").and_then(Value::as_str) {
                    Some("host") => {
                        if let Some(v) = self.views.get_mut(&key) {
                            v.host_ready = true;
                            v.deliver();
                        }
                        vec![]
                    }
                    Some("size") => {
                        let Some(h) = m.get("h").and_then(Value::as_f64).filter(|h| h.is_finite() && *h > 0.0) else { return vec![] };
                        let h = (h as f32).clamp(48.0, 4000.0).ceil();
                        let first = self.views.get_mut(&key).is_some_and(|v| !std::mem::replace(&mut v.sized, true));
                        if !first && self.heights.get(&key).is_some_and(|old| (old - h).abs() < 1.0) {
                            return vec![];
                        }
                        self.heights.insert(key, h);
                        vec![Effect::Redraw]
                    }
                    Some("wheel") => m.get("dy").and_then(Value::as_f64).map(|dy| vec![Effect::Wheel(key.0, dy as f32)]).unwrap_or_default(),
                    Some("esc") => vec![Effect::Keys],
                    Some("open") => {
                        let url = s("url");
                        let web = ["http://", "https://", "mailto:"].iter().any(|p| url.to_ascii_lowercase().starts_with(p));
                        if web { vec![Effect::Open(url)] } else { vec![] }
                    }
                    Some("message") => {
                        let text = message_text(m.get("params").unwrap_or(&Value::Null));
                        if text.trim().is_empty() { vec![] } else { vec![Effect::Send(key.0, text)] }
                    }
                    Some("ask") => {
                        let n = s("n");
                        let Some(job) = self.views.get(&key).and_then(|v| v.app.clone()).and_then(|app| mcp_apps::job(&app, &s("method"), m.get("params").unwrap_or(&Value::Null))) else {
                            if let Some(v) = self.views.get(&key) {
                                v.eval(&format!("window.__broApp && window.__broApp.answer({}, false, {})", json!(n), json!("bro can't do that for a page")));
                            }
                            return vec![];
                        };
                        let sid = self.views.get(&key).map(|v| v.sid.clone()).unwrap_or_default();
                        let proxy = self.proxy.clone();
                        svc.mod_ask(&sid, job, Box::new(move |result| {
                            let _ = proxy.send_event(UserEvent::Apps(Msg::Answer { view, n, result }));
                        }));
                        vec![]
                    }
                    _ => vec![],
                }
            }
        }
    }
}

/// `rect` (logical) in whole physical pixels: x, y, width, height.
fn physical(rect: Rect, scale: f32) -> (i32, i32, u32, u32) {
    let (x, y) = ((rect.x * scale).round(), (rect.y * scale).round());
    let (r, b) = ((rect.right() * scale).round(), (rect.bottom() * scale).round());
    (x as i32, y as i32, (r - x).max(1.0) as u32, (b - y).max(1.0) as u32)
}

/// Where a card's page is and what it's shown in, as its host context says.
fn frame<'a>(w: &'a Want, light: bool, version: &'a str) -> Frame<'a> {
    Frame { call: &w.key.1, light, fill: [w.fill.r, w.fill.g, w.fill.b], width: w.rect.w, max_h: w.max_h, mobile: false, version }
}

/// The words of a page's `ui/message` (its text blocks).
fn message_text(params: &Value) -> String {
    let blocks = match params.get("content") {
        Some(Value::Array(a)) => a.clone(),
        Some(b @ Value::Object(_)) => vec![b.clone()],
        Some(Value::String(s)) => return s.clone(),
        _ => vec![],
    };
    blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n\n")
}

// ------------------------------------------------------------------------------------------------- reading pages

/// Read a page (off the window's thread): from the disk when it's been read before, else through the session's
/// mod. A page from the disk is read again through the mod as well, for next time.
fn read(svc: &Services, proxy: &EventLoopProxy<UserEvent>, sid: &str, server: String, uri: String) {
    let path = mcp_apps::page_path(&bro_core::paths::bro_dir(), &server, &uri);
    let send = |page: Result<Arc<str>, String>, server: String, uri: String| {
        let _ = proxy.send_event(UserEvent::Apps(Msg::Read { server, uri, page }));
    };
    let kept = std::fs::read_to_string(&path).ok().filter(|s| !s.trim().is_empty());
    let have = kept.is_some();
    if let Some(html) = kept {
        send(Ok(Arc::from(html)), server.clone(), uri.clone());
    }
    if svc.is_demo() {
        if !have {
            send(Err(String::new()), server, uri);
        }
        return;
    }
    if have && !svc.mod_listening(sid) {
        return;
    }
    let proxy = proxy.clone();
    svc.mod_ask(sid, json!({ "kind": "read", "server": server, "uri": uri }), Box::new(move |result| {
        let page = result.and_then(|v| mcp_apps::page_text(&v));
        if let Ok(html) = &page {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = bro_core::util::atomic_write(&path, html.as_bytes());
        }
        // a page already shown from the disk stays as it is until next time
        if !have {
            let _ = proxy.send_event(UserEvent::Apps(Msg::Read { server, uri, page: page.map(Arc::from) }));
        }
    }));
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use super::{HOST, Msg, UserEvent};
    use bro_core::sessions::conversation::McpApp;
    use bro_gfx::Color;
    use serde_json::Value;
    use std::sync::Arc;
    use std::time::Instant;
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    /// A rectangle in physical px: x, y, width, height.
    type Px = (i32, i32, u32, u32);

    pub struct View {
        pub id: u64,
        web: wry::WebView,
        _context: wry::WebContext,
        /// the host page is up: the page can go in
        pub host_ready: bool,
        /// the page and what it's given, as a script's argument, once known
        pub payload: Option<String>,
        delivered: bool,
        /// the context the page was last given
        context: Option<Value>,
        /// what the page is (its server, for what it asks), and whose session
        pub app: Option<Arc<McpApp>>,
        pub sid: String,
        shown: bool,
        /// whole page and the part shown
        at: Option<(Px, Px)>,
        /// the page has said how tall it is, and when the view was made
        pub sized: bool,
        pub made: Instant,
        /// what shows where the page has no background of its own (the card's fill)
        fill: Color,
    }

    /// A solid colour as the web view takes it (WebView2 has only solid or clear).
    fn rgba(c: Color) -> (u8, u8, u8, u8) {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (b(c.r), b(c.g), b(c.b), 255)
    }

    fn rect((x, y, w, h): (i32, i32, u32, u32)) -> wry::Rect {
        wry::Rect { position: wry::dpi::PhysicalPosition::new(x, y).into(), size: wry::dpi::PhysicalSize::new(w, h).into() }
    }

    impl View {
        pub fn new(window: &Window, proxy: EventLoopProxy<UserEvent>, id: u64, bounds: (i32, i32, u32, u32), fill: Color) -> Result<View, String> {
            let mut context = wry::WebContext::new(Some(bro_core::paths::bro_dir().join("webview")));
            let (ipc, popup) = (proxy.clone(), proxy);
            let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
                .with_visible(false)
                .with_focused(false)
                .with_bounds(rect(bounds))
                .with_background_color(rgba(fill))
                .with_devtools(true)
                .with_html(HOST)
                .with_ipc_handler(move |req| {
                    let _ = ipc.send_event(UserEvent::Apps(Msg::View { view: id, body: req.body().clone() }));
                })
                // a page's own links and windows go to your browser
                .with_new_window_req_handler(move |url, _| {
                    let _ = popup.send_event(UserEvent::Apps(Msg::View { view: id, body: serde_json::json!({ "t": "open", "url": url }).to_string() }));
                    wry::NewWindowResponse::Deny
                })
                .with_navigation_handler(|url| url.starts_with("about:") || url.starts_with("data:"));
            #[cfg(windows)]
            let builder = {
                use wry::WebViewBuilderExtWindows;
                builder.with_scroll_bar_style(wry::ScrollBarStyle::FluentOverlay)
            };
            let web = builder.build_as_child(window).map_err(|e| e.to_string())?;
            Ok(View { id, web, _context: context, host_ready: false, payload: None, delivered: false, context: None, app: None, sid: String::new(), shown: false, at: None, sized: false, made: Instant::now(), fill })
        }

        /// The page goes in once the host page is up and the page is known.
        pub fn deliver(&mut self) {
            #[cfg(not(windows))]
            self.clip();
            if self.delivered || !self.host_ready {
                return;
            }
            if let Some(p) = &self.payload {
                self.delivered = true;
                self.eval(&format!("window.__broApp.load({p})"));
            }
        }

        /// The page's context as it is now (the theme, its room): told to the page when it changes.
        pub fn context(&mut self, c: Value) {
            if self.context.as_ref() == Some(&c) {
                return;
            }
            let first = self.context.is_none();
            self.context = Some(c.clone());
            if !first && self.delivered {
                self.eval(&format!("window.__broApp.context({})", super::script_json(&c)));
            }
        }

        pub fn shown(&self) -> bool {
            self.shown
        }

        /// The card's fill changed (the theme did): the page sits on the new one.
        pub fn fill(&mut self, fill: Color) {
            if fill != self.fill {
                self.fill = fill;
                let _ = self.web.set_background_color(rgba(fill));
            }
        }

        pub fn eval(&self, js: &str) {
            let _ = self.web.evaluate_script(js);
        }

        pub fn focus_parent(&self) {
            let _ = self.web.focus_parent();
        }

        pub fn show(&mut self, show: bool) {
            if show != self.shown {
                self.shown = show;
                let _ = self.web.set_visible(show);
            }
        }

        /// The page whole at `full`, showing only `visible` (the conversation scrolls it under its edges): the
        /// view's window is the visible part, the page inside it sits where the whole card is.
        pub fn place(&mut self, full: (i32, i32, u32, u32), visible: (i32, i32, u32, u32)) {
            if self.at == Some((full, visible)) {
                return;
            }
            self.at = Some((full, visible));
            #[cfg(windows)]
            {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
                use windows::Win32::Foundation::RECT;
                use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
                use wry::WebViewExtWindows;
                let (dx, dy) = (full.0 - visible.0, full.1 - visible.1);
                let controller: ICoreWebView2Controller = self.web.controller();
                unsafe {
                    let _ = SetWindowPos(self.web.hwnd(), None, visible.0, visible.1, visible.2 as i32, visible.3 as i32, SWP_NOACTIVATE | SWP_NOZORDER);
                    let _ = controller.SetBounds(RECT { left: dx, top: dy, right: dx + full.2 as i32, bottom: dy + full.3 as i32 });
                }
            }
            #[cfg(not(windows))]
            {
                let _ = self.web.set_bounds(rect(visible));
                self.clip();
            }
        }

        /// macOS: a web view can't be clipped the way Windows clips its window, so the view is only the part shown
        /// and the host page moves the page inside it to where the whole card is (once the host page is up).
        #[cfg(not(windows))]
        fn clip(&self) {
            if let (true, Some((full, visible))) = (self.host_ready, self.at) {
                self.eval(&format!("window.__broApp.clip({}, {}, {}, {})", full.0 - visible.0, full.1 - visible.1, full.2, full.3));
            }
        }
    }
}

/// No web view here: none is ever made (see [`Apps::available`]).
#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::UserEvent;
    use bro_core::sessions::conversation::McpApp;
    use bro_gfx::Color;
    use serde_json::Value;
    use std::sync::Arc;
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    pub struct View {
        pub id: u64,
        pub host_ready: bool,
        pub payload: Option<String>,
        pub app: Option<Arc<McpApp>>,
        pub sid: String,
        pub sized: bool,
        pub made: std::time::Instant,
    }

    impl View {
        pub fn new(_: &Window, _: EventLoopProxy<UserEvent>, _: u64, _: (i32, i32, u32, u32), _: Color) -> Result<View, String> {
            Err("this system has no web view bro can use".into())
        }
        pub fn deliver(&mut self) {}
        pub fn fill(&mut self, _: Color) {}
        pub fn context(&mut self, _: Value) {}
        pub fn shown(&self) -> bool {
            false
        }
        pub fn eval(&self, _: &str) {}
        pub fn focus_parent(&self) {}
        pub fn show(&mut self, _: bool) {}
        pub fn place(&mut self, _: (i32, i32, u32, u32), _: (i32, i32, u32, u32)) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_its_words() {
        assert_eq!(message_text(&json!({"role": "user", "content": [{"type": "text", "text": "Swap the curls"}, {"type": "image"}, {"type": "text", "text": "for rows"}]})), "Swap the curls\n\nfor rows");
        assert_eq!(message_text(&json!({"content": {"type": "text", "text": "one"}})), "one");
        assert_eq!(message_text(&json!({})), "");
    }

}
