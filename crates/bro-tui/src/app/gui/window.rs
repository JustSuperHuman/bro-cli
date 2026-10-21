//! The window itself: creating it where it was last time, bro's icon, a title bar that matches the theme
//! (Windows 11 lets apps colour it), pointer shapes, the clipboard, and telling the single-instance hand-off
//! which window to raise.

use super::kit::Cursor;
use super::prefs::Prefs;
use bro_gfx::Color;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/bro.png");

pub fn create(el: &ActiveEventLoop, prefs: &Prefs, _bg: Color) -> Result<Window, String> {
    let (w, h) = prefs.window.unwrap_or((1440.0, 900.0));
    let mut attrs = Window::default_attributes()
        .with_title("bro")
        .with_inner_size(winit::dpi::LogicalSize::new(w.max(640.0), h.max(420.0)))
        .with_min_inner_size(winit::dpi::LogicalSize::new(560.0, 360.0))
        .with_visible(false)
        .with_window_icon(icon());
    if let Some((x, y)) = prefs.position {
        attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
    }
    let window = el.create_window(attrs).map_err(|e| e.to_string())?;
    if prefs.maximized {
        window.set_maximized(true);
    }
    // a saved position on a monitor that's gone: start on the primary one instead
    if prefs.position.is_some() && window.current_monitor().is_none() {
        window.set_outer_position(winit::dpi::PhysicalPosition::new(120, 80));
    }
    Ok(window)
}

fn icon() -> Option<winit::window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    winit::window::Icon::from_rgba(rgba, info.width, info.height).ok()
}

/// Paint the native title bar in the theme's background (Windows 11), dark or light to match.
pub fn style_title_bar(window: &Window, bg: Color, light: bool) {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        let [r, g, b, _] = bg.to_rgba8();
        let colorref: u32 = r as u32 | (g as u32) << 8 | (b as u32) << 16;
        let dark: i32 = (!light) as i32;
        // DWMWA_USE_IMMERSIVE_DARK_MODE = 20, DWMWA_CAPTION_COLOR = 35, DWMWA_BORDER_COLOR = 34
        // SAFETY: attributes on our own window with correctly sized values; failures (older Windows) are ignored.
        unsafe {
            DwmSetWindowAttribute(hwnd, 20, &dark as *const i32 as *const _, 4);
            DwmSetWindowAttribute(hwnd, 35, &colorref as *const u32 as *const _, 4);
            DwmSetWindowAttribute(hwnd, 34, &colorref as *const u32 as *const _, 4);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, bg);
        window.set_theme(Some(if light { winit::window::Theme::Light } else { winit::window::Theme::Dark }));
    }
}

/// Tell the single-instance hand-off which window to bring forward.
pub fn remember_handle(window: &Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    #[allow(clippy::single_match)]
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => {
            crate::instance::APP_WINDOW.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
            #[cfg(windows)]
            keep_alt(h.hwnd.get());
        }
        _ => {}
    }
}

/// Tapping alt on its own puts a Windows window into menu mode: the next key opens the system menu or is eaten.
/// bro uses a held alt for its key tips and alt chords for everything else, so that one menu activation is
/// turned off (alt+space still opens the window menu).
#[cfg(windows)]
fn keep_alt(hwnd: isize) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SC_KEYMENU, WM_SYSCOMMAND};
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        // lparam 0: the menu was asked for by alt alone (alt+space passes the space)
        if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) as u32 == SC_KEYMENU && lparam == 0 {
            return 0;
        }
        // SAFETY: passing the message on to the next handler in the chain, unchanged.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
    // SAFETY: the window is ours and outlives the subclass (it's removed with the window).
    unsafe {
        SetWindowSubclass(hwnd as HWND, Some(proc), 1, 0);
    }
}

pub fn cursor_icon(c: Cursor) -> CursorIcon {
    match c {
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Text => CursorIcon::Text,
        Cursor::ResizeH => CursorIcon::ColResize,
        Cursor::ResizeV => CursorIcon::RowResize,
        Cursor::Grab => CursorIcon::Grab,
    }
}

/// Put text on the system clipboard.
pub fn set_clipboard(text: &str) {
    // one clipboard for the app's life: on X11 and Wayland what we copied lives only as long as it does
    static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);
    let Ok(mut slot) = CLIPBOARD.lock() else { return };
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    if let Some(c) = slot.as_mut() {
        let _ = c.set_text(text.to_string());
    }
}

/// Leave the shell's console: when this process shares one (started from a terminal), start `args` as a copy
/// without a visible console and return true (this one exits); when the console is our own (bro.exe opened
/// from Explorer), hide it and return false. Child processes keep a console either way, so none of them flash
/// one of their own.
#[cfg(windows)]
pub fn detach_from_console(args: &[String]) -> bool {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    let mut list = [0u32; 8];
    // SAFETY: a fixed buffer of the length passed.
    let n = unsafe { GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32) };
    if n == 0 {
        return false;
    }
    if n == 1 {
        // SAFETY: hiding our own console window, if there is one.
        unsafe {
            let h = GetConsoleWindow();
            if !h.is_null() {
                ShowWindow(h, SW_HIDE);
            }
        }
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Started from a terminal (`bro` in a shell): run the window as its own process, in its own process group so
/// the shell's ctrl+c doesn't reach it, and give the shell its prompt back (as `code .` does). Returns true when
/// it handed off.
#[cfg(not(windows))]
pub fn detach_from_console(args: &[String]) -> bool {
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    if !std::io::stdin().is_terminal() && !std::io::stdout().is_terminal() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}
