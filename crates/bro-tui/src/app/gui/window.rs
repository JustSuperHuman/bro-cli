//! The window itself: creating it where it was last time, bro's icon, a title bar that matches the theme
//! (Windows 11 lets apps colour it), the material behind a see-through window (Acrylic, Mica), pointer shapes,
//! the clipboard, and telling the single-instance hand-off which window to raise.

use super::kit::Cursor;
use super::prefs::Prefs;
use bro_core::config::Material;
use bro_gfx::Color;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/bro.png");

/// The window, where it was last time; `see_through` when its material lets the desktop show. That's settled
/// when the window is made: on Windows it has no redirection bitmap then, so all it shows is the DirectComposition
/// swap chain, with the system's backdrop wherever that is see-through (elsewhere the window asks for alpha).
pub fn create(el: &ActiveEventLoop, prefs: &Prefs, see_through: bool) -> Result<Window, String> {
    let (w, h) = prefs.window.unwrap_or((1440.0, 900.0));
    let mut attrs = Window::default_attributes()
        .with_title("bro")
        .with_inner_size(winit::dpi::LogicalSize::new(w.max(640.0), h.max(420.0)))
        .with_min_inner_size(winit::dpi::LogicalSize::new(560.0, 360.0))
        .with_visible(false)
        .with_transparent(see_through)
        .with_window_icon(icon());
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs = attrs.with_no_redirection_bitmap(see_through);
    }
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

/// Dress the window for the theme and its material: the native title bar in the theme's background (Windows 11),
/// dark or light to match, and what the system draws behind a see-through window (Acrylic, a blur, or
/// nothing for clear glass). False when the system can't do the material asked for (Acrylic before
/// Windows 11): the window is drawn solid then.
pub fn style(window: &Window, bg: Color, light: bool, material: Material) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Dwm::{
            DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_COLOR_DEFAULT,
            DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmEnableBlurBehindWindow, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        };
        use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
        use windows_sys::Win32::UI::Controls::MARGINS;
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return false };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return false };
        let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        let see_through = material.see_through();
        let backdrop = match material {
            Material::Acrylic => DWMSBT_TRANSIENTWINDOW,
            Material::Solid | Material::Glass => DWMSBT_NONE,
        };
        let [r, g, b, _] = bg.with_alpha(1.0).to_rgba8();
        let theme_colour: u32 = r as u32 | (g as u32) << 8 | (b as u32) << 16;
        let dark: i32 = (!light) as i32;
        let set = |attr: i32, v: &u32| {
            // SAFETY: an attribute of our own window with a correctly sized value.
            unsafe { DwmSetWindowAttribute(hwnd, attr as u32, v as *const u32 as *const _, 4) >= 0 }
        };
        // dark or light chrome, and the dark or light flavour of Acrylic
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, &(dark as u32));
        // Windows 11 22H2 and later draw a backdrop; earlier ones turn the attribute down
        let drawn = set(DWMWA_SYSTEMBACKDROP_TYPE, &(backdrop as u32));
        if backdrop != DWMSBT_NONE && !drawn {
            keep_acrylic_active(hwnd, false);
            set(DWMWA_SYSTEMBACKDROP_TYPE, &(DWMSBT_NONE as u32));
            set(DWMWA_CAPTION_COLOR, &theme_colour);
            set(DWMWA_BORDER_COLOR, &theme_colour);
            return false;
        }
        keep_acrylic_active(hwnd, material == Material::Acrylic);
        // SAFETY: our own window; the region is ours to free once DWM has copied it.
        unsafe {
            // the frame reaches across the whole window, so the backdrop is drawn behind all of it
            let m = if backdrop != DWMSBT_NONE { -1 } else { 0 };
            DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m });
            // clear glass: the window's alpha counts with nothing behind it but the desktop (an empty blur region:
            // no blur of its own)
            let region = CreateRectRgn(0, 0, -1, -1);
            let bb = DWM_BLURBEHIND { dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION, fEnable: (see_through && backdrop == DWMSBT_NONE) as _, hRgnBlur: region, fTransitionOnMaximized: 0 };
            DwmEnableBlurBehindWindow(hwnd, &bb);
            DeleteObject(region as _);
        }
        // the title bar in the theme's colour, or the material running up through it
        let caption = if backdrop != DWMSBT_NONE { DWMWA_COLOR_DEFAULT } else { theme_colour };
        set(DWMWA_CAPTION_COLOR, &caption);
        set(DWMWA_BORDER_COLOR, &caption);
        true
    }
    #[cfg(not(windows))]
    {
        let _ = bg;
        window.set_theme(Some(if light { winit::window::Theme::Light } else { winit::window::Theme::Dark }));
        window.set_transparent(material.see_through());
        // macOS and KDE blur what's behind; elsewhere it's clear
        window.set_blur(matches!(material, Material::Acrylic));
        true
    }
}

/// Keep Acrylic's frame rendering active when keyboard focus moves to another window. Only the
/// nonclient appearance changes; Windows and winit still receive the real activation and focus events.
#[cfg(windows)]
fn keep_acrylic_active(hwnd: windows_sys::Win32::Foundation::HWND, enabled: bool) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SendMessageW, WM_NCACTIVATE};

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        let appearance = if msg == WM_NCACTIVATE { 1 } else { wparam };
        // SAFETY: our own window's subclass chain; only the nonclient appearance is overridden.
        unsafe { DefSubclassProc(hwnd, msg, appearance, lparam) }
    }

    // SAFETY: called on our window's event-loop thread. The subclass has no allocated state and is
    // removed with the window; switching materials removes it explicitly and restores real appearance.
    unsafe {
        if enabled {
            SetWindowSubclass(hwnd, Some(proc), 3, 0);
        } else {
            RemoveWindowSubclass(hwnd, Some(proc), 3);
        }
        SendMessageW(hwnd, WM_NCACTIVATE, (GetForegroundWindow() == hwnd) as WPARAM, 0);
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
            {
                keep_alt(h.hwnd.get());
                super::caption::install(h.hwnd.get());
            }
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

/// How the window's text is smoothed: per colour channel the way Windows has ClearType set (on, and which way
/// round the screen's colours run), greyscale elsewhere and when the GPU can't blend it (`subpixel` false).
/// BRO_TEXT_AA (grayscale, rgb or bgr) overrides the system's choice.
pub fn text_antialias(subpixel: bool) -> bro_gfx::Antialias {
    use bro_gfx::Antialias;
    let pick = match std::env::var("BRO_TEXT_AA").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Ok("grayscale" | "greyscale" | "gray" | "grey" | "off") => Antialias::Grayscale,
        Ok("rgb" | "subpixel" | "cleartype") => Antialias::Rgb,
        Ok("bgr") => Antialias::Bgr,
        _ => system_antialias(),
    };
    if subpixel { pick } else { Antialias::Grayscale }
}

#[cfg(windows)]
fn system_antialias() -> bro_gfx::Antialias {
    use bro_gfx::Antialias;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FE_FONTSMOOTHINGCLEARTYPE, FE_FONTSMOOTHINGORIENTATIONBGR, SPI_GETFONTSMOOTHING, SPI_GETFONTSMOOTHINGORIENTATION, SPI_GETFONTSMOOTHINGTYPE, SystemParametersInfoW};
    let get = |what| {
        let mut v: u32 = 0;
        // SAFETY: each of these settings is one 32-bit value written into `v`.
        let ok = unsafe { SystemParametersInfoW(what, 0, (&mut v as *mut u32).cast(), 0) } != 0;
        ok.then_some(v)
    };
    if get(SPI_GETFONTSMOOTHING).is_none_or(|on| on == 0) || get(SPI_GETFONTSMOOTHINGTYPE) != Some(FE_FONTSMOOTHINGCLEARTYPE) {
        return Antialias::Grayscale;
    }
    if get(SPI_GETFONTSMOOTHINGORIENTATION) == Some(FE_FONTSMOOTHINGORIENTATIONBGR) { Antialias::Bgr } else { Antialias::Rgb }
}

/// macOS stopped smoothing per channel, and Linux leaves it to per-screen settings bro can't see.
#[cfg(not(windows))]
fn system_antialias() -> bro_gfx::Antialias {
    bro_gfx::Antialias::Grayscale
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

/// Leave the console: whenever this process has one, start `args` as a copy with a hidden console of its own and
/// return true (this one exits). That covers a shell's console (started from a terminal) and our own (bro.exe
/// opened from Explorer, Start or Run): on Windows 11 that console is a Windows Terminal tab, and hiding its
/// window leaves the tab open, so closing the tab would close the app. Child processes keep a console either
/// way, so none of them flash one of their own.
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
    let Ok(exe) = std::env::current_exe() else { return false };
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        std::process::Command::new(&exe)
            .args(args)
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    };
    let flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    // out of the terminal's job too, when it allows that: a terminal that kills its job on close takes no app
    // with it (a job that forbids breaking away fails the spawn, so try again inside it)
    if spawn(flags | CREATE_BREAKAWAY_FROM_JOB) || spawn(flags) {
        return true;
    }
    if n == 1 {
        // couldn't hand off: at least hide our own console window
        // SAFETY: hiding our own console window, if there is one.
        unsafe {
            let h = GetConsoleWindow();
            if !h.is_null() {
                ShowWindow(h, SW_HIDE);
            }
        }
    }
    false
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
