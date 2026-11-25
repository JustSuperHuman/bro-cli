//! A Bro-owned window using exactly the phone's paired browser protocol and viewer.

pub fn same_viewer(expected: &str, candidate: &str) -> bool {
    let (Ok(expected), Ok(candidate)) = (url::Url::parse(expected), url::Url::parse(candidate)) else { return false; };
    expected.origin() == candidate.origin() && expected.path() == candidate.path() && expected.fragment() == candidate.fragment()
}

#[cfg(test)]
mod tests {
    #[test]
    fn exact_handoff_links_remain_in_the_paired_native_viewer() {
        let base = "http://127.0.0.1:10001/browser?theme=dark#token=paired";
        assert!(super::same_viewer(base, "http://127.0.0.1:10001/browser?tabId=42&handoffId=one#token=paired"));
        for wrong in ["http://127.0.0.1:10002/browser#token=paired", "http://127.0.0.1:10001/other#token=paired", "http://127.0.0.1:10001/browser#token=wrong"] {
            assert!(!super::same_viewer(base, wrong));
        }
    }
}

/// What a handoff card says: its heading and the line under it.
fn copy(handoff: &bro_bridge::BrowserHandoff) -> (&'static str, String) {
    let page = if handoff.title.trim().is_empty() { "Browser tab" } else { handoff.title.trim() };
    match handoff.phase.as_str() {
        "requested" if handoff.returning => ("Handing the browser back", format!("{page} · The agent picks up from here.")),
        "requested" => ("Browser needs your input", if handoff.reason.trim().is_empty() { page.to_owned() } else { format!("{page} · {}", handoff.reason.trim()) }),
        _ => ("You have the browser", format!("{page} · Press Done in the browser when you're finished, and the agent carries on.")),
    }
}

const CARD_PAD: f32 = 14.0;

/// The handoff card focus mode pins over a session's reply box, sized for `w`.
pub fn focus_card_height(ui: &mut super::Ui, small: f32, body: f32, handoff: &bro_bridge::BrowserHandoff, w: f32) -> f32 {
    use super::kit::{Style, weight};
    let (_, line) = copy(handoff);
    let text = ui.para(&line, Style::new(body, weight::REGULAR, ui.t.muted), (w - 2.0 * CARD_PAD - 18.0).max(40.0), Some(3));
    CARD_PAD + (small * 1.5).round() + 6.0 + text.h + 12.0 + 32.0 + CARD_PAD
}

/// The handoff card in focus mode: what the agent asked for, and the way into the page or back out of it.
pub fn draw_focus_card(ui: &mut super::Ui, small: f32, body: f32, handoff: &bro_bridge::BrowserHandoff, r: bro_gfx::Rect) {
    use super::actions::Action;
    use super::kit::{Style, radius, weight, widgets};
    use bro_gfx::{Point, Rect};
    let t = ui.t;
    let waiting = handoff.phase == "requested" && !handoff.returning;
    let tone = if waiting { t.warn } else { t.good };
    ui.panel(r, radius::OVERLAY, tone.with_alpha(if t.light { 0.04 } else { 0.06 }), t.hairline.mix(tone, 0.4));
    ui.block(r);
    let inner = r.inset(CARD_PAD);
    let (title, line) = copy(handoff);
    let head_h = (small * 1.5).round();
    ui.round(Rect::new(inner.x, inner.y + (head_h - 8.0) / 2.0, 8.0, 8.0), 4.0, tone);
    let head = ui.line(title, Style::new(small, weight::SEMIBOLD, t.text), Some((inner.w - 18.0).max(0.0)));
    ui.draw(&head, Point::new(inner.x + 18.0, inner.y + (head_h - head.h) / 2.0));
    let text = ui.para(&line, Style::new(body, weight::REGULAR, t.muted), (inner.w - 18.0).max(40.0), Some(3));
    ui.draw(&text, Point::new(inner.x + 18.0, inner.y + head_h + 6.0));
    let y = r.bottom() - CARD_PAD - 32.0;
    let open = if handoff.viewer_available { "Open browser" } else { "View request" };
    let back = if handoff.returning { "Returning…" } else { "Return control" };
    let open_w = widgets::button_width(ui, open).max(120.0);
    let back_w = widgets::button_width(ui, back).max(120.0);
    let x = inner.x + 18.0;
    for (bx, w, label, kind, finish) in [(x, open_w, open, widgets::Kind::Primary, false), (x + open_w + 8.0, back_w, back, widgets::Kind::Secondary, true)] {
        let button = Rect::new(bx, y, w.min((inner.right() - bx).max(0.0)), 32.0);
        if button.w < 60.0 { continue; }
        if waiting {
            widgets::button(ui, button, label, kind, Action::BrowserHandoff { tab_id: handoff.tab_id, id: handoff.id.clone(), finish });
        } else {
            widgets::control(ui, button, t.control);
            ui.label_in(button, label, Style::new(size_of(body), weight::MEDIUM, t.muted), bro_gfx::text::Align::Center);
        }
    }
}

fn size_of(body: f32) -> f32 { (body * 0.86).round().max(12.0) }

pub fn draw_handoffs(app: &mut super::App, ui: &mut super::Ui, area: bro_gfx::Rect, pending: &[bro_bridge::BrowserHandoff]) {
    use super::actions::Action;
    use super::kit::{Style, size, weight, widgets};
    use bro_gfx::{Point, Rect};
    let handoff = &pending[app.browser_handoffs.selected % pending.len()];
    let t = ui.t;
    let card = area.inset(8.0);
    ui.shadow(card, 8.0, 18.0, 4.0, t.shadow.fade(0.4));
    ui.panel(card, 8.0, t.raised, t.border);
    let r = card.inset(14.0);
    let narrow = area.w < 760.0;
    let available = if narrow { r.w } else { (r.w - 300.0).max(120.0) };
    let waiting = handoff.phase == "requested";
    // a dot for the state: amber while the agent waits, green once you have the page
    ui.round(Rect::new(r.x, r.y + 7.0, 8.0, 8.0), 4.0, if waiting { t.warn } else { t.good });
    let (title, message) = copy(handoff);
    let badge = if pending.len() > 1 { 96.0 } else { 0.0 };
    ui.label_in(Rect::new(r.x + 18.0, r.y, available - 18.0 - badge, 22.0), title, Style::new(size::BODY, weight::SEMIBOLD, t.text), bro_gfx::text::Align::Left);
    if pending.len() > 1 {
        let label = format!("{} of {} · Next", app.browser_handoffs.selected % pending.len() + 1, pending.len());
        widgets::button(ui, Rect::new(r.x + available - 92.0, r.y - 2.0, 92.0, 26.0), &label, widgets::Kind::Ghost, Action::BrowserHandoffNext);
    }
    let text = ui.para(&message, Style::new(size::BODY, weight::REGULAR, t.muted), available - 18.0, Some(2));
    ui.draw(&text, Point::new(r.x + 18.0, r.y + 26.0));
    let x = if narrow { r.x } else { r.right() - 282.0 };
    let y = if narrow { r.bottom() - 34.0 } else { r.y + (r.h - 34.0) / 2.0 };
    let ready = waiting && !handoff.returning;
    for (offset, label, finish, kind, width) in [
        (0.0, if handoff.viewer_available { "Open browser" } else { "View request" }, false, widgets::Kind::Primary, 126.0),
        (136.0, if handoff.returning { "Returning…" } else { "Return control" }, true, widgets::Kind::Secondary, 146.0),
    ] {
        let button = Rect::new(x + offset, y, width, 34.0);
        if ready {
            widgets::button(ui, button, label, kind, Action::BrowserHandoff { tab_id: handoff.tab_id, id: handoff.id.clone(), finish });
        } else {
            widgets::control(ui, button, t.control);
            ui.label_in(button, label, Style::new(size::BODY, weight::MEDIUM, t.muted), bro_gfx::text::Align::Center);
        }
    }
}

/// The paired viewer, filling the main window below its title row (project tabs, window buttons).
#[cfg(any(windows, target_os = "macos"))]
pub struct Browser {
    web: wry::WebView,
    _context: wry::WebContext,
    url: String,
    bounds: (i32, i32, u32, u32),
}
#[cfg(any(windows, target_os = "macos"))]
impl Browser {
    /// `closed` is raised (and `wake` called) when the viewer's Back button asks to leave.
    pub fn new(window: &winit::window::Window, url: &str, closed: std::sync::Arc<std::sync::atomic::AtomicBool>, wake: impl Fn() + 'static) -> Result<Self, String> {
        let target = url::Url::parse(url).map_err(|e| e.to_string())?;
        let origin = target.origin().ascii_serialization();
        // Separate from untrusted MCP app cards; the viewer's pairing token stays here.
        let mut context = wry::WebContext::new(Some(bro_core::paths::bro_dir().join("webview-browser")));
        let bounds = Self::area(window);
        let web = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_url(url).with_devtools(false)
            .with_bounds(Self::rect(bounds))
            .with_navigation_handler(move |uri| url::Url::parse(&uri).is_ok_and(|u| u.origin().ascii_serialization() == origin && u.path() == "/browser"))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_ipc_handler(move |req| {
                if req.body() == "close" {
                    closed.store(true, std::sync::atomic::Ordering::SeqCst);
                    wake();
                }
            })
            .with_hotkeys_zoom(false)
            .build_as_child(window).map_err(|e| e.to_string())?;
        Ok(Self { web, _context: context, url: url.to_owned(), bounds })
    }
    /// Everything under the title row, physical px.
    fn area(window: &winit::window::Window) -> (i32, i32, u32, u32) {
        let size = window.inner_size();
        let top = ((super::layout::STRIP * window.scale_factor() as f32).round() as u32).min(size.height);
        (0, top as i32, size.width, size.height - top)
    }
    fn rect(b: (i32, i32, u32, u32)) -> wry::Rect {
        wry::Rect { position: wry::dpi::PhysicalPosition::new(b.0, b.1).into(), size: wry::dpi::PhysicalSize::new(b.2.max(1), b.3.max(1)).into() }
    }
    /// Follow the window's size.
    pub fn fit(&mut self, window: &winit::window::Window) {
        let bounds = Self::area(window);
        if bounds != self.bounds {
            self.bounds = bounds;
            let _ = self.web.set_bounds(Self::rect(bounds));
        }
    }
    pub fn open(&mut self, url: &str) {
        if self.url != url {
            let _ = self.web.load_url(url);
            self.url = url.to_owned();
        }
        let _ = self.web.focus();
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub struct Browser;
#[cfg(not(any(windows, target_os = "macos")))]
impl Browser {
    pub fn new(_: &winit::window::Window, _: &str, _: std::sync::Arc<std::sync::atomic::AtomicBool>, _: impl Fn() + 'static) -> Result<Self, String> { Err("Use the Browser page in your browser on this platform.".into()) }
    pub fn fit(&mut self, _: &winit::window::Window) {}
    pub fn open(&mut self, _: &str) {}
}
