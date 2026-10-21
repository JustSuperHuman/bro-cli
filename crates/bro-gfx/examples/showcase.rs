//! Opens a window, draws a sample of everything bro-gfx can do, saves a screenshot and exits.
//!
//! `cargo run -p bro-gfx --example showcase -- [out.png] [scale]`

use bro_gfx::text::{Face, Span, TextOpts};
use bro_gfx::{Color, Corners, Point, Quad, Rect, Renderer, Scene, Text};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const MONO: &[u8] = include_bytes!("../../bro-tui/assets/fonts/JetBrainsMono-Variable.ttf");
const ICONS: &[u8] = include_bytes!("../../bro-tui/assets/fonts/SymbolsNerdFontMono-Regular.ttf");

struct App {
    out: String,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    text: Option<Text>,
    frames: u32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("bro-gfx showcase").with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let w = Arc::new(el.create_window(attrs).unwrap());
        let size = w.inner_size();
        let r = Renderer::new(w.clone(), size.width, size.height, true).unwrap();
        println!("adapter: {} ({:?})", r.adapter_info.name, r.adapter_info.backend);
        let t0 = std::time::Instant::now();
        let text = Text::new(MONO, ICONS, true);
        println!("text system ready in {:?}", t0.elapsed());
        self.renderer = Some(r);
        self.text = Some(text);
        self.window = Some(w.clone());
        w.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let window = self.window.clone().unwrap();
                let scale = window.scale_factor() as f32;
                let r = self.renderer.as_mut().unwrap();
                let text = self.text.as_mut().unwrap();
                // wait for the system fonts (emoji, CJK) so the shot shows fallback too
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !text.poll_fonts() && std::time::Instant::now() < deadline && self.frames == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                let (w, h) = r.size();
                let mut scene = Scene::new();
                loop {
                    scene.clear();
                    text.begin_frame();
                    draw(&mut scene, text, w as f32, h as f32, scale);
                    if !text.overflow {
                        break;
                    }
                    text.grow_atlases();
                }
                let _ = r.render(&scene, text);
                self.frames += 1;
                if self.frames == 3 {
                    if let Some((cw, ch, px)) = r.capture(&scene, text) {
                        save_png(&self.out, cw, ch, &px);
                        println!("saved {} ({cw}x{ch})", self.out);
                    }
                    el.exit();
                } else {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn save_png(path: &str, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn draw(s: &mut Scene, t: &mut Text, w: f32, h: f32, k: f32) {
    let bg = Color::hex(0x0b0d12);
    let panel = Color::hex(0x12151c);
    let raised = Color::hex(0x181c25);
    let hair = Color::rgba(1.0, 1.0, 1.0, 0.07);
    let fg = Color::hex(0xe6e8ee);
    let muted = Color::hex(0x8a90a0);
    let accent = Color::hex(0x7c9cff);
    let good = Color::hex(0x3ecf8e);
    let danger = Color::hex(0xff6b6b);
    let claude = Color::hex(0xd97757);
    s.background = bg;
    let px = |v: f32| (v * k).round();

    // sidebar
    let side = Rect::new(px(10.0), px(10.0), px(280.0), h - px(20.0));
    s.quad(Quad::new(side, panel).radius(px(14.0)).border(1.0, hair));
    let title = t.line("bro", Face::Ui, px(15.0), 650, fg, None);
    t.draw(s, &title, Point::new(side.x + px(18.0), side.y + px(16.0)), None, 1.0);
    let sub = t.line("agentic workspace", Face::Ui, px(12.0), 450, muted, None);
    t.draw(s, &sub, Point::new(side.x + px(50.0), side.y + px(18.0)), None, 1.0);

    let mut y = side.y + px(56.0);
    for (i, (name, live, sel)) in [("bro-cli-v2", 3, true), ("justgains", 1, false), ("terminal", 0, false)].iter().enumerate() {
        let row = Rect::new(side.x + px(8.0), y, side.w - px(16.0), px(40.0));
        if *sel {
            s.quad(Quad::new(row, raised).radius(px(9.0)));
        }
        let dot_c = [accent, good, muted][i];
        s.quad(Quad::new(Rect::new(row.x + px(12.0), row.y + px(15.0), px(10.0), px(10.0)), dot_c).radius(px(5.0)));
        let l = t.line(name, Face::Ui, px(13.5), if *sel { 600 } else { 500 }, fg, Some(px(150.0)));
        t.draw(s, &l, Point::new(row.x + px(32.0), row.y + px(11.0)), None, 1.0);
        if *live > 0 {
            let badge = t.line(&format!("{live} live"), Face::Ui, px(11.0), 600, accent, None);
            let bw = badge.width() + px(14.0);
            let br = Rect::new(row.right() - bw - px(10.0), row.y + px(11.0), bw, px(18.0));
            s.quad(Quad::new(br, accent.with_alpha(0.14)).radius(px(9.0)));
            t.draw(s, &badge, Point::new(br.x + px(7.0), br.y + px(2.0)), None, 1.0);
        }
        y += px(46.0);
    }

    // usage meters
    let mut y2 = side.bottom() - px(110.0);
    s.rect(Rect::new(side.x + px(16.0), y2 - px(14.0), side.w - px(32.0), 1.0), hair);
    for (label, pct, c) in [("claude", 0.46, claude), ("codex", 0.93, Color::hex(0x2ec49a))] {
        let l = t.line(label, Face::Ui, px(12.5), 500, fg, None);
        t.draw(s, &l, Point::new(side.x + px(18.0), y2), None, 1.0);
        let track = Rect::new(side.x + px(90.0), y2 + px(6.0), px(120.0), px(6.0));
        s.quad(Quad::new(track, hair).radius(px(3.0)));
        s.quad(Quad::new(Rect { w: track.w * pct, ..track }, c).radius(px(3.0)));
        let p = t.line(&format!("{:.0}%", pct * 100.0), Face::Ui, px(12.0), 600, muted, None);
        t.draw(s, &p, Point::new(side.right() - px(18.0) - p.width(), y2), None, 1.0);
        y2 += px(30.0);
    }

    // a session card with a terminal
    let card = Rect::new(side.right() + px(14.0), px(10.0), w - side.right() - px(24.0), h - px(20.0));
    s.shadow(card.translate(0.0, px(8.0)), Corners::all(px(14.0)), px(30.0), Color::rgba(0.0, 0.0, 0.0, 0.45));
    s.quad(Quad::new(card, panel).radius(px(14.0)).border(1.0, hair));
    let head = Rect::new(card.x, card.y, card.w, px(44.0));
    s.rect(Rect::new(card.x, head.bottom(), card.w, 1.0), hair);
    let glyph = t.line("\u{f0e7}", Face::Icons, px(14.0), 400, claude, None);
    t.draw(s, &glyph, Point::new(head.x + px(16.0), head.y + px(13.0)), None, 1.0);
    let spans = [
        Span::new("Fix the flaky workout timer", fg).weight(600),
        Span::new("   claude · work · opus-5", muted).weight(450),
    ];
    let mut o = TextOpts::new(px(13.5));
    o.max_width = Some(card.w - px(220.0));
    o.ellipsis = true;
    let hl = t.layout(&spans, &o);
    t.draw(s, &hl, Point::new(head.x + px(40.0), head.y + px(12.0)), None, 1.0);
    // status pill
    let st = t.line("working", Face::Ui, px(11.5), 600, accent, None);
    let pw = st.width() + px(30.0);
    let pill = Rect::new(head.right() - pw - px(14.0), head.y + px(12.0), pw, px(21.0));
    s.quad(Quad::new(pill, accent.with_alpha(0.13)).radius(px(10.5)));
    s.quad(Quad::new(Rect::new(pill.x + px(10.0), pill.y + px(7.5), px(6.0), px(6.0)), accent).radius(px(3.0)));
    t.draw(s, &st, Point::new(pill.x + px(21.0), pill.y + px(3.0)), None, 1.0);

    // terminal grid
    let m = t.set_mono(px(14.0), 1.2);
    let body = Rect::new(card.x + px(16.0), head.bottom() + px(12.0), card.w - px(32.0), card.h - px(70.0));
    s.push_clip(body, 0.0);
    let lines: Vec<(String, Color, bool)> = vec![
        ("╭──────────────────────────────────────────────╮".into(), muted, false),
        ("│ ✻ Welcome to Claude Code!                    │".into(), claude, true),
        ("│                                              │".into(), muted, false),
        ("│   /help for help, /status for your setup     │".into(), muted, false),
        ("╰──────────────────────────────────────────────╯".into(), muted, false),
        ("".into(), fg, false),
        ("> fix the flaky timer test in workouts/timer.rs".into(), fg, true),
        ("".into(), fg, false),
        ("⠹ Reading workouts/timer.rs… (esc to interrupt)".into(), accent, false),
        ("  fn tick(&mut self) -> Duration { self.elapsed += 1; } // => != <= ==".into(), Color::hex(0xc3e88d), false),
        ("  ▁▂▃▄▅▆▇█ ░▒▓ ┌┬┐ ├┼┤ └┴┘ ═║╔╗╚╝ ⣿⣶⣤⣀ 🚀 ✅ 漢字 ".into(), fg, false),
        ("  ● needs you   ◐ working   ○ idle   ✓ done".into(), good, false),
        ("PS F:\\bro-cli-v2> cargo test -p bro-gfx".into(), fg, false),
    ];
    for (row, (line, c, bold)) in lines.iter().enumerate() {
        let mut col = 0usize;
        for ch in line.chars() {
            let wide = unicode_width(ch);
            let x = body.x + col as f32 * m.cell_w;
            let yy = body.y + row as f32 * m.cell_h;
            if ch != ' ' {
                t.draw_cell(s, &ch.to_string(), *bold, false, x, yy, wide, *c);
            }
            col += wide as usize;
        }
    }
    // cursor
    let cy = body.y + 12.0 * m.cell_h;
    let cx = body.x + 39.0 * m.cell_w;
    s.quad(Quad::new(Rect::new(cx, cy, m.cell_w, m.cell_h), fg.with_alpha(0.85)));
    s.pop_clip();

    // a floating dialog with a shadow, in its own layer
    s.push_layer();
    let dlg = Rect::new(card.x + card.w * 0.5 - px(210.0), card.y + card.h - px(250.0), px(420.0), px(190.0));
    s.shadow(dlg.translate(0.0, px(16.0)), Corners::all(px(16.0)), px(48.0), Color::rgba(0.0, 0.0, 0.0, 0.55));
    s.quad(Quad::new(dlg, raised).radius(px(16.0)).border(1.0, Color::rgba(1.0, 1.0, 1.0, 0.09)));
    let q = t.line("Close Fix the flaky workout timer?", Face::Ui, px(15.0), 650, fg, Some(dlg.w - px(48.0)));
    t.draw(s, &q, Point::new(dlg.x + px(24.0), dlg.y + px(24.0)), None, 1.0);
    let mut o = TextOpts::new(px(13.0));
    o.max_width = Some(dlg.w - px(48.0));
    o.wrap = true;
    let d = t.layout(&[Span::new("The agent is still busy. It will be stopped; the conversation stays resumable.", muted)], &o);
    t.draw(s, &d, Point::new(dlg.x + px(24.0), dlg.y + px(56.0)), None, 1.0);
    let b1 = Rect::new(dlg.right() - px(24.0) - px(110.0), dlg.bottom() - px(56.0), px(110.0), px(34.0));
    let b0 = Rect::new(b1.x - px(10.0) - px(90.0), b1.y, px(90.0), px(34.0));
    s.quad(Quad::new(b0, Color::rgba(1.0, 1.0, 1.0, 0.06)).radius(px(9.0)));
    s.quad(Quad::new(b1, danger).radius(px(9.0)));
    let c0 = t.line("Cancel", Face::Ui, px(13.0), 600, fg, None);
    let c1 = t.line("Close session", Face::Ui, px(13.0), 600, Color::WHITE, None);
    t.draw(s, &c0, Point::new(b0.x + (b0.w - c0.width()) / 2.0, b0.y + px(8.0)), None, 1.0);
    t.draw(s, &c1, Point::new(b1.x + (b1.w - c1.width()) / 2.0, b1.y + px(8.0)), None, 1.0);
}

/// Crude width for the showcase (the app uses alacritty's cell flags).
fn unicode_width(c: char) -> u8 {
    let cp = c as u32;
    if (0x1100..=0x115F).contains(&cp) || (0x2E80..=0xA4CF).contains(&cp) || (0xAC00..=0xD7A3).contains(&cp) || (0xF900..=0xFAFF).contains(&cp) || (0x1F300..=0x1FAFF).contains(&cp) || cp == 0x2705 {
        2
    } else {
        1
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "showcase.png".into());
    let el = EventLoop::new().unwrap();
    let mut app = App { out, window: None, renderer: None, text: None, frames: 0 };
    el.run_app(&mut app).unwrap();
}
