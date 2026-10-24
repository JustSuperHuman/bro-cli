//! A title bar of bro's own while the window is see-through (Windows). The system draws its caption in the bare
//! material, with none of the theme over it, so it stood apart from the window under it; with this one the whole
//! window is one sheet: the title and the minimise, maximise and close buttons sit on the same glass as the rest.
//! The system still does the work: dragging, snapping, double-click to maximise, the window menu, the resize
//! edges, and Snap Layouts over the maximise button (it's reported to Windows as one).
//!
//! The band is the top [`HEIGHT`] px of the client area: `WM_NCCALCSIZE` gives the old caption's room to the
//! client, `WM_NCHITTEST` hands the band and its buttons back to the system as caption and caption buttons.
//! Pointer and presses over the buttons then arrive as non-client messages, kept in [`HOVER`] and [`PRESS`] for
//! the drawing.

use super::kit::{Style, Ui, size, weight};
use bro_gfx::text::Align;
use bro_gfx::{Color, Rect};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// The band's height and each button's width, logical px (Windows 11's caption).
pub const HEIGHT: f32 = 32.0;
pub const BUTTON_W: f32 = 46.0;

/// The window wears this title bar instead of the system's.
static ON: AtomicBool = AtomicBool::new(false);
/// The button under the pointer: 0 none, 1 minimise, 2 maximise, 3 close.
static HOVER: AtomicU8 = AtomicU8::new(0);
/// The button held down (same numbering).
static PRESS: AtomicU8 = AtomicU8::new(0);

/// bro draws the window's title bar now.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Wear bro's title bar (see-through window) or the system's, and have Windows lay the frame out again.
pub fn set(window: &winit::window::Window, on: bool) {
    if ON.swap(on, Ordering::Relaxed) == on {
        return;
    }
    HOVER.store(0, Ordering::Relaxed);
    PRESS.store(0, Ordering::Relaxed);
    #[cfg(windows)]
    win::frame_changed(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Watch `hwnd`'s non-client messages (the window's handle, once it exists).
#[cfg(windows)]
pub fn install(hwnd: isize) {
    win::install(hwnd);
}

/// Draw the title bar across `band`: the window's title, then minimise, maximise (or restore) and close. No fill:
/// it's the window's own glass.
pub fn draw(ui: &mut Ui, band: Rect, title: &str, focused: bool, maximized: bool) {
    let t = ui.t;
    let hover = HOVER.load(Ordering::Relaxed);
    let press = PRESS.load(Ordering::Relaxed);
    // the window's own glass stays bare; an inactive window's title steps back, as Windows' does
    let ink = if focused { t.text } else { t.faint };
    let buttons = 3.0 * BUTTON_W;
    let st = Style::new(size::SMALL, weight::REGULAR, ink);
    ui.label_in(Rect::new(band.x + 14.0, band.y, (band.w - buttons - 28.0).max(0.0), band.h), title, st, Align::Left);
    // minimise, maximise / restore, close: Windows 11's sizes, VS Code's codicons
    let glyphs = [(1u8, "\u{EABA}"), (2, if maximized { "\u{EABB}" } else { "\u{EAB9}" }), (3, "\u{EAB8}")];
    for (i, (id, glyph)) in glyphs.into_iter().enumerate() {
        let r = Rect::new(band.right() - buttons + i as f32 * BUTTON_W, band.y, BUTTON_W, band.h);
        let lit = hover == id;
        let held = press == id && lit;
        let close = id == 3;
        // close lights up in Windows' red, the others in a wash of the text colour
        let fill = match (close, lit, held) {
            (true, true, false) => Some(Color::hex(0xc42b1c)),
            (true, true, true) => Some(Color::hex(0xc42b1c).with_alpha(0.9)),
            (false, true, false) => Some(t.text.with_alpha(if t.light { 0.06 } else { 0.08 })),
            (false, true, true) => Some(t.text.with_alpha(if t.light { 0.04 } else { 0.05 })),
            _ => None,
        };
        if let Some(c) = fill {
            ui.fill(r, c);
        }
        let ink = if close && lit { Color::WHITE } else if held { ink.mix(t.faint, 0.4) } else { ink };
        ui.icon(r, glyph, 16.0, ink);
    }
}

#[cfg(windows)]
mod win {
    use super::{BUTTON_W, HEIGHT, HOVER, ON, PRESS};
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetClientRect, GetWindowLongW, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, NCCALCSIZE_PARAMS, PostMessageW,
        SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXFRAME, SM_CXPADDEDBORDER, SM_CYFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowPos, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND, WS_CAPTION,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub fn install(hwnd: isize) {
        // SAFETY: the window is ours and outlives the subclass (it's removed with the window).
        unsafe {
            SetWindowSubclass(hwnd as HWND, Some(proc), 2, 0);
        }
    }

    /// Have Windows ask for the frame again (`WM_NCCALCSIZE`) now that it's laid out differently.
    pub fn frame_changed(window: &winit::window::Window) {
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        // SAFETY: our own window; nothing moves or resizes, the frame is recalculated.
        unsafe {
            SetWindowPos(h.hwnd.get() as HWND, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE);
        }
    }

    /// The window's scale (its monitor's DPI over 96) and its resize frame's thickness, physical px.
    unsafe fn metrics(hwnd: HWND) -> (f32, i32, i32) {
        // SAFETY: plain queries about our own window.
        unsafe {
            let dpi = GetDpiForWindow(hwnd).max(96);
            let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            (dpi as f32 / 96.0, GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad, GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad)
        }
    }

    /// Which of our buttons a hit-test code names (1 minimise, 2 maximise, 3 close).
    fn button(hit: u32) -> u8 {
        match hit {
            HTMINBUTTON => 1,
            HTMAXBUTTON => 2,
            HTCLOSE => 3,
            _ => 0,
        }
    }

    /// The pointer moved onto (or off) a button: draw the window again.
    unsafe fn hover(hwnd: HWND, b: u8) {
        if HOVER.swap(b, Ordering::Relaxed) != b {
            // SAFETY: our own window.
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        // SAFETY (the whole body): the messages of our own window; pointers are the ones Windows hands us.
        unsafe {
            // full screen (no caption style) and the system's own title bar: nothing to do
            if !ON.load(Ordering::Relaxed) || (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION) != WS_CAPTION {
                return DefSubclassProc(hwnd, msg, wparam, lparam);
            }
            match msg {
                WM_NCCALCSIZE if wparam != 0 => {
                    let p = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                    let top = p.rgrc[0].top;
                    let r = DefSubclassProc(hwnd, msg, wparam, lparam);
                    // the caption's room goes to the client; a maximised window hangs its frame off the screen,
                    // so its client starts a frame's width down
                    p.rgrc[0].top = top + if IsZoomed(hwnd) != 0 { metrics(hwnd).2 } else { 0 };
                    r
                }
                WM_NCHITTEST => {
                    let hit = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if hit != HTCLIENT as LRESULT {
                        return hit;
                    }
                    let mut pt = POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
                    ScreenToClient(hwnd, &mut pt);
                    let (scale, fx, fy) = metrics(hwnd);
                    if pt.y >= (HEIGHT * scale).round() as i32 {
                        return HTCLIENT as LRESULT;
                    }
                    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetClientRect(hwnd, &mut rc);
                    // the top edge resizes, as the system's frame did there
                    if IsZoomed(hwnd) == 0 && pt.y < fy {
                        return if pt.x < 2 * fx {
                            HTTOPLEFT
                        } else if pt.x >= rc.right - 2 * fx {
                            HTTOPRIGHT
                        } else {
                            HTTOP
                        } as LRESULT;
                    }
                    let bw = (BUTTON_W * scale).round() as i32;
                    let from_right = rc.right - pt.x;
                    (if from_right <= bw {
                        HTCLOSE
                    } else if from_right <= 2 * bw {
                        HTMAXBUTTON
                    } else if from_right <= 3 * bw {
                        HTMINBUTTON
                    } else {
                        HTCAPTION
                    }) as LRESULT
                }
                WM_NCMOUSEMOVE => {
                    let b = button(wparam as u32);
                    hover(hwnd, b);
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                    TrackMouseEvent(&mut track);
                    // over our buttons there's nothing for the system to draw
                    if b != 0 { 0 } else { DefSubclassProc(hwnd, msg, wparam, lparam) }
                }
                WM_NCMOUSELEAVE => {
                    hover(hwnd, 0);
                    PRESS.store(0, Ordering::Relaxed);
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if button(wparam as u32) != 0 => {
                    PRESS.store(button(wparam as u32), Ordering::Relaxed);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    0
                }
                WM_NCLBUTTONUP if button(wparam as u32) != 0 => {
                    let b = button(wparam as u32);
                    let held = PRESS.swap(0, Ordering::Relaxed);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    if held == b {
                        let cmd = match b {
                            1 => SC_MINIMIZE,
                            2 if IsZoomed(hwnd) != 0 => SC_RESTORE,
                            2 => SC_MAXIMIZE,
                            _ => SC_CLOSE,
                        };
                        PostMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0);
                    }
                    0
                }
                _ => DefSubclassProc(hwnd, msg, wparam, lparam),
            }
        }
    }
}
