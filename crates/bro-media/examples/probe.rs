//! Print what bro-media makes of a file and save its thumbnail as a PNG.
//!
//!     cargo run -p bro-media --example probe -- <path> [max_side] [--out thumb.png]
//!
//! Videos and audio also play muted for two seconds, to show how fast frames arrive.

use bro_media::{MediaKind, Player};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn main() {
    let mut path = None;
    let mut max_side = 256;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().map(PathBuf::from),
            n if n.parse::<u32>().is_ok() && path.is_some() => max_side = n.parse().unwrap_or(256),
            p => path = Some(PathBuf::from(p)),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: probe <path> [max_side] [--out thumb.png]");
        std::process::exit(2);
    };
    let kind = bro_media::kind_of(&path);
    println!("kind        {kind:?}");
    println!("executable  {}", bro_media::is_executable(&path));
    println!("ffmpeg      {}", if bro_media::ffmpeg_available() { "installed" } else { "not found" });
    match kind {
        MediaKind::Image => image(&path, max_side),
        MediaKind::Video | MediaKind::Audio => play(&path, max_side),
        _ => {}
    }
    let started = Instant::now();
    match bro_media::thumbnail(&path, max_side) {
        Ok(thumb) => {
            let out = out.unwrap_or_else(|| PathBuf::from(format!("{}.thumb.png", path.file_stem().unwrap_or_default().to_string_lossy())));
            match image::save_buffer(&out, &thumb.px, thumb.w, thumb.h, image::ExtendedColorType::Rgba8) {
                Ok(()) => println!("thumbnail   {}x{} in {:?} -> {}", thumb.w, thumb.h, started.elapsed(), out.display()),
                Err(e) => println!("thumbnail   {}x{}, can't save {}: {e}", thumb.w, thumb.h, out.display()),
            }
        }
        Err(e) => println!("thumbnail   error: {e}"),
    }
}

fn image(path: &Path, max_side: u32) {
    let started = Instant::now();
    match bro_media::load_image(path, max_side) {
        Ok(img) => {
            let (first, _) = &img.frames[0];
            let total: Duration = img.frames.iter().map(|(_, d)| *d).sum();
            println!("image       {} {}x{} -> {}x{} in {:?}", img.format, img.original.0, img.original.1, first.w, first.h, started.elapsed());
            if img.is_animated() {
                println!("animation   {} frames, {:?} per loop", img.frames.len(), total);
            }
        }
        Err(e) => println!("image       error: {e}"),
    }
}

fn play(path: &Path, max_side: u32) {
    let started = Instant::now();
    let mut player = match Player::open(path, max_side) {
        Ok(player) => player,
        Err(e) => return println!("player      error: {e}"),
    };
    println!("opened      in {:?}", started.elapsed());
    println!("info        {:?}", player.info());
    let poster = Instant::now();
    while poster.elapsed() < Duration::from_secs(5) {
        if let Some(frame) = player.take_frame() {
            println!("poster      {}x{} after {:?}", frame.w, frame.h, poster.elapsed());
            break;
        }
        if player.info().has_video {
            std::thread::sleep(Duration::from_millis(2));
        } else {
            break;
        }
    }
    player.set_muted(true);
    player.play();
    let playing = Instant::now();
    let mut frames = 0;
    while playing.elapsed() < Duration::from_secs(2) && !player.ended() {
        if player.take_frame().is_some() {
            frames += 1;
        }
        let wake = player.next_wake().unwrap_or_else(|| Instant::now() + Duration::from_millis(20));
        std::thread::sleep(wake.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
    }
    println!("played      {frames} frames in {:?}, position {:?}", playing.elapsed(), player.position());
    if let Some(e) = player.error() {
        println!("error       {e}");
    }
    let dropping = Instant::now();
    drop(player);
    println!("dropped     in {:?}", dropping.elapsed());
}
