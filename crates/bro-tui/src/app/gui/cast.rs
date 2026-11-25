//! Portraits for the launcher's Create Slop tab: each character's picture, fetched once, shrunk and kept in
//! `~/.bro/studio/portraits/`, so the cast shows at once the next time the tab opens. A few worker threads fetch
//! and decode; the window puts what they hand back on the GPU and tells the tab which pictures it can draw
//! ([`Studio::pictures`]). Only the cards the tab drew are asked for ([`Studio::wanted`]): a long roster costs
//! nothing until it is scrolled to.

use crate::launcher::studio::{Picture, Studio};
use bro_gfx::{ImageId, Renderer};
use bro_media::Rgba;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// The longest side kept: sharp for a 180 px card on a 3x screen.
const SIDE: u32 = 560;
/// Portraits fetched at once.
const WORKERS: usize = 4;
/// New pictures put on the GPU in one frame (each is a megabyte or so).
const UPLOADS: usize = 6;
/// Pictures held; past this, ones the tab isn't showing are let go.
const HELD: usize = 160;
const FETCH_LIMIT: Duration = Duration::from_secs(45);

struct Job {
    key: String,
    url: String,
}

struct Done {
    key: String,
    url: String,
    px: Option<Rgba>,
}

struct Entry {
    url: String,
    /// decoded, waiting for the GPU or on it (kept: a new renderer needs it again)
    px: Option<Rgba>,
    tex: Option<ImageId>,
    /// the answer came back (with or without a picture)
    settled: bool,
}

pub struct Portraits {
    entries: HashMap<String, Entry>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Portraits {
    /// `wake` runs after each answer (on a worker's thread). `fetch`: pictures not kept yet come from the network
    /// (off in the demo, which then shows the tiles without them).
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, fetch: bool) -> Portraits {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..WORKERS {
            let (rx, tx, wake) = (rx.clone(), tx.clone(), wake.clone());
            let _ = std::thread::Builder::new().name(format!("bro-cast-portraits-{n}")).spawn(move || {
                loop {
                    // hold the queue only to take a job, so the others fetch meanwhile
                    let job = match rx.lock().recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    let px = crate::services::guard("cast::portrait", || portrait(&job.url, fetch)).ok().flatten();
                    let _ = tx.send(Done { key: job.key, url: job.url, px });
                    wake();
                }
            });
        }
        Portraits { entries: HashMap::new(), jobs, done }
    }

    /// Before drawing: take the workers' answers, ask for what the tab's last draw wanted, put new pictures on
    /// the GPU and tell the tab what it can draw.
    pub fn prepare(&mut self, r: &mut Renderer, studio: &mut Studio) {
        while let Ok(done) = self.done.try_recv() {
            if let Some(e) = self.entries.get_mut(&done.key).filter(|e| e.url == done.url) {
                e.px = done.px;
                e.settled = true;
            }
        }
        let wanted = std::mem::take(&mut studio.wanted);
        for (key, url) in &wanted {
            // a new version of a character has a new address: its picture is fetched again
            if self.entries.get(key).is_some_and(|e| e.url == *url) {
                continue;
            }
            if let Some(tex) = self.entries.remove(key).and_then(|e| e.tex) {
                r.remove_image(tex);
            }
            self.entries.insert(key.clone(), Entry { url: url.clone(), px: None, tex: None, settled: false });
            let _ = self.jobs.send(Job { key: key.clone(), url: url.clone() });
        }
        if self.entries.len() > HELD {
            let drop: Vec<String> = self.entries.iter().filter(|(k, e)| e.settled && !wanted.iter().any(|(w, _)| w == *k)).map(|(k, _)| k.clone()).take(self.entries.len() - HELD).collect();
            for key in drop {
                if let Some(tex) = self.entries.remove(&key).and_then(|e| e.tex) {
                    r.remove_image(tex);
                }
            }
        }
        let mut uploads = 0;
        studio.pictures.clear();
        for (key, e) in &mut self.entries {
            let Some(px) = &e.px else { continue };
            if e.tex.is_none() && uploads < UPLOADS {
                e.tex = Some(r.create_image(px.w, px.h, &px.px));
                uploads += 1;
            }
            if let Some(tex) = e.tex {
                studio.pictures.insert(key.clone(), Picture { image: tex.0, w: px.w, h: px.h });
            }
        }
    }

    /// Pictures decoded and still waiting their turn for the GPU: the window draws again soon.
    pub fn pending(&self) -> bool {
        self.entries.values().any(|e| e.px.is_some() && e.tex.is_none())
    }

    /// The window has a new renderer and every texture went with the old one: they go up again.
    pub fn gpu_lost(&mut self) {
        for e in self.entries.values_mut() {
            e.tex = None;
        }
    }
}

/// The portrait at `url`, fitted in [`SIDE`]: the one kept from before, else fetched, shrunk and kept.
fn portrait(url: &str, fetch: bool) -> Option<Rgba> {
    let kept = bro_core::studio::portrait_path(url);
    if let Some(px) = decode(&kept) {
        return Some(px);
    }
    if !fetch {
        return None;
    }
    // whatever the site sends (PNG, JPEG, WebP): the decoder reads the format from the file itself
    let part = kept.with_extension(format!("{}.part", std::process::id()));
    std::fs::create_dir_all(kept.parent()?).ok()?;
    let fetched = bro_core::http::download(url, &part, FETCH_LIMIT).is_ok();
    let px = if fetched { decode(&part) } else { None };
    let _ = std::fs::remove_file(&part);
    let px = px?;
    if let Some(bytes) = png_bytes(&px) {
        let _ = crate::util::atomic_write(&kept, &bytes);
    }
    Some(px)
}

fn decode(path: &Path) -> Option<Rgba> {
    if !path.is_file() {
        return None;
    }
    let image = bro_media::load_image(path, SIDE).ok()?;
    image.frames.into_iter().next().map(|(frame, _)| frame).filter(|px| px.w > 0 && px.h > 0)
}

fn png_bytes(px: &Rgba) -> Option<Vec<u8>> {
    let mut out = vec![];
    {
        let mut enc = png::Encoder::new(&mut out, px.w, px.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&px.px).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_portrait_is_read_back_without_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.png");
        let px = Rgba { w: 3, h: 2, px: vec![200; 24] };
        std::fs::write(&file, png_bytes(&px).unwrap()).unwrap();
        assert_eq!(decode(&file), Some(px));
        assert_eq!(decode(&dir.path().join("missing.png")), None);
        // something that isn't a picture is no portrait
        std::fs::write(dir.path().join("junk.png"), b"not a picture").unwrap();
        assert_eq!(decode(&dir.path().join("junk.png")), None);
    }
}
