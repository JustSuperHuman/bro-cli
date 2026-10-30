//! Sound files in focus mode's conversation (a voice line the agent made, a song it sent you): a card under the
//! reply or the step that names one, to play it right there. JustImagine's player in bro's type: a round play
//! button, the sound's own waveform (what's played in colour, the rest quiet), where it's at and how long it runs.
//! A click on the waveform plays from there. One sound plays at a time.
//!
//! Playing goes through bro-media's player, opened (and let go) off this thread; a file's waveform is read once,
//! off this thread too ([`bro_core::audio::waveform`]). The window draws by the clock only while a card has
//! something moving: a sound playing, a file opening, a waveform being read.

use super::super::actions::Action;
use super::super::kit::{Style, Ui, radius, size, weight, widgets};
use super::Typo;
use crate::layout::PaneId;
use bro_core::audio::Waveform;
use bro_core::sessions::conversation::{Item, StepKind, StepState};
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use bro_media::Player;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// A card's height, and the room above and between cards (logical px).
pub const CARD_H: f32 = 64.0;
pub const GAP: f32 = 10.0;
/// At most this many cards under one item, and this wide.
const MOST: usize = 4;
const WIDEST: f32 = 520.0;
/// Peaks read per file (the card draws as many bars as fit, from these).
const PEAKS: usize = 240;
/// Waveforms read at once.
const READERS: usize = 2;

/// JustImagine's voice colour: deep teal on light themes, bright on dark.
fn voice(light: bool) -> Color {
    if light { Color::hex(0x0f7f78) } else { Color::hex(0x4cc7bd) }
}

/// The sound files `it` names: absolute paths in what the agent said, the files its steps wrote or sent you. At
/// most [`MOST`], each once.
pub fn sounds_of(it: &Item) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    let mut add = |raw: &str| {
        let raw = raw.trim().trim_start_matches("file:///");
        let p = PathBuf::from(raw);
        if out.len() < MOST && p.is_absolute() && bro_core::audio::is_audio(&p) && !out.contains(&p) {
            out.push(p);
        }
    };
    match it {
        Item::Said { text, .. } => bro_bridge::file_links::find_file_links(text).iter().for_each(|m| add(&m.path)),
        Item::Did { steps } => steps
            .iter()
            .filter(|s| matches!(s.kind, StepKind::Send | StepKind::Create | StepKind::Edit | StepKind::Other) && s.state != StepState::Failed)
            .filter_map(|s| s.detail.as_deref())
            .flat_map(|d| d.lines())
            .for_each(&mut add),
        _ => {}
    }
    out
}

/// The room `n` cards take under an item.
pub fn height(n: usize) -> f32 {
    if n == 0 { 0.0 } else { GAP + n as f32 * CARD_H + (n - 1) as f32 * 8.0 }
}

// ------------------------------------------------------------------------------------------------ the deck

/// The sounds: their waveforms as read, the one playing (or paused part way), the one opening.
#[derive(Default)]
struct Deck {
    /// None while being read
    waves: HashMap<PathBuf, Option<Result<Arc<Waveform>, String>>>,
    queue: Vec<PathBuf>,
    reading: usize,
    read: Option<(Sender<(PathBuf, Result<Waveform, String>)>, Receiver<(PathBuf, Result<Waveform, String>)>)>,
    now: Option<(PathBuf, Player)>,
    /// opening, and where to start (0..1 of it)
    opening: Option<(PathBuf, f32, Receiver<Result<Player, String>>)>,
    /// a file that wouldn't play, and why
    failed: Option<(PathBuf, String)>,
}

thread_local! {
    static DECK: RefCell<Deck> = RefCell::new(Deck::default());
}

impl Deck {
    /// Pick up what the readers and the opener finished; start the next reads.
    fn settle(&mut self) {
        if let Some((_, rx)) = &self.read {
            while let Ok((path, r)) = rx.try_recv() {
                self.reading = self.reading.saturating_sub(1);
                self.waves.insert(path, Some(r.map(Arc::new)));
            }
        }
        while self.reading < READERS && !self.queue.is_empty() {
            let path = self.queue.remove(0);
            let tx = self.read.get_or_insert_with(channel).0.clone();
            self.reading += 1;
            let p = path.clone();
            let spawned = std::thread::Builder::new().name("bro-sound-wave".into()).spawn(move || {
                let r = bro_core::audio::waveform(&p, PEAKS).map_err(|e| format!("{e:#}"));
                let _ = tx.send((p, r));
            });
            if spawned.is_err() {
                self.reading -= 1;
                self.waves.insert(path, Some(Err("couldn't start reading it".into())));
            }
        }
        if let Some((path, at, rx)) = &self.opening
            && let Ok(r) = rx.try_recv()
        {
            let (path, at) = (path.clone(), *at);
            self.opening = None;
            match r {
                Ok(mut player) => {
                    if at > 0.0
                        && let Some(d) = player.info().duration
                    {
                        player.seek(d.mul_f32(at));
                    }
                    player.play();
                    self.now = Some((path, player));
                }
                Err(e) => self.failed = Some((path, e)),
            }
        }
    }

    /// `path`'s waveform: asked for the first time it's wanted.
    fn wave(&mut self, path: &Path) -> Option<Result<Arc<Waveform>, String>> {
        match self.waves.get(path) {
            Some(w) => w.clone(),
            None => {
                self.waves.insert(path.to_path_buf(), None);
                self.queue.push(path.to_path_buf());
                None
            }
        }
    }

    /// Let go of the sound playing (its threads wind down on one of their own: dropping waits for them).
    fn stop(&mut self) {
        if let Some((_, player)) = self.now.take() {
            let _ = std::thread::Builder::new().name("bro-sound-drop".into()).spawn(move || drop(player));
        }
    }

    /// Play `path` from `at` (0..1 of it): the one playing now pauses or plays on, another starts in its place.
    fn play(&mut self, path: &Path, at: Option<f32>) {
        self.failed = None;
        if let Some((p, player)) = &mut self.now
            && p == path
        {
            match at {
                Some(at) => {
                    if let Some(d) = player.info().duration {
                        player.seek(d.mul_f32(at.clamp(0.0, 1.0)));
                    }
                    player.play();
                }
                None if player.ended() => {
                    player.seek(Duration::ZERO);
                    player.play();
                }
                None => player.toggle(),
            }
            return;
        }
        self.stop();
        let (tx, rx) = channel();
        let p = path.to_path_buf();
        let _ = std::thread::Builder::new().name("bro-sound-open".into()).spawn(move || {
            let _ = tx.send(Player::open(&p, 64));
        });
        self.opening = Some((path.to_path_buf(), at.unwrap_or(0.0).clamp(0.0, 1.0), rx));
    }
}

/// Play or pause `path` (another sound stops).
pub fn toggle(path: &Path) {
    DECK.with(|d| d.borrow_mut().play(path, None));
}

/// Play `path` from `at` (0..1 of it).
pub fn seek(path: &Path, at: f32) {
    DECK.with(|d| d.borrow_mut().play(path, Some(at)));
}

// ------------------------------------------------------------------------------------------------ the card

/// Draw the cards for `sounds` under an item, from `at`, `w` wide. Their play buttons are `play + n` and their
/// waveforms `wave + n` in `targets` (n its index there, with the waveform's rect for where a click lands).
#[allow(clippy::too_many_arguments)]
pub fn cards(ui: &mut Ui, c: &Typo, id: PaneId, sounds: &[PathBuf], at: Point, w: f32, play: usize, wave: usize, targets: &mut Vec<(PathBuf, Rect)>) {
    let w = w.min(WIDEST);
    DECK.with(|d| {
        let mut d = d.borrow_mut();
        d.settle();
        let mut y = at.y + GAP;
        for path in sounds {
            let n = targets.len();
            let r = Rect::new(at.x, y, w, CARD_H);
            let wr = card(ui, c, &mut d, id, path, r, play + n, wave + n);
            targets.push((path.clone(), wr));
            y += CARD_H + 8.0;
        }
        if d.reading > 0 || !d.queue.is_empty() || d.opening.is_some() || d.now.as_ref().is_some_and(|(_, p)| p.is_playing() && !p.ended()) {
            ui.ticking = true;
        }
    });
}

/// One card in `r`; returns its waveform's rect.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut Ui, c: &Typo, d: &mut Deck, id: PaneId, path: &Path, r: Rect, play_target: usize, wave_target: usize) -> Rect {
    let t = ui.t;
    let tone = voice(t.light);
    // where it's at: playing, paused part way, opening, or not started
    let wave = d.wave(path);
    let mine = d.now.as_ref().filter(|(p, _)| p == path).map(|(_, p)| p);
    let ended = mine.is_some_and(|p| p.ended());
    let playing = mine.is_some_and(|p| p.is_playing()) && !ended;
    let opening = d.opening.as_ref().is_some_and(|(p, ..)| p == path);
    let failed = d.failed.as_ref().filter(|(p, _)| p == path).map(|(_, e)| e.clone());
    let duration = mine.and_then(|p| p.info().duration).or_else(|| wave.as_ref().and_then(|w| w.as_ref().ok()).map(|w| Duration::from_millis(w.duration_ms)));
    let pos = if ended { Duration::ZERO } else { mine.map(|p| p.position()).unwrap_or_default() };
    let frac = duration.map_or(0.0, |dd| (pos.as_secs_f32() / dd.as_secs_f32().max(0.001)).clamp(0.0, 1.0));
    // the card: the conversation's well, lit teal from the left while it plays
    let lit = ui.ease(("focus-sound-lit", id, path), if playing || opening { 1.0 } else { 0.0 }, 220.0);
    ui.round(r, radius::OVERLAY, c.well);
    let pr = ui.px(r);
    ui.scene.quad(Quad::new(pr, Fill::Horizontal(tone.with_alpha(0.07 + 0.09 * lit), tone.with_alpha(0.0))).radius(radius::OVERLAY * ui.scale));
    // the play button: a solid disc (it reads over any material), teal while it plays
    let center = Point::new(r.x + 16.0 + 18.0, r.y + r.h / 2.0);
    let br = Rect::new(center.x - 18.0, center.y - 18.0, 36.0, 36.0);
    let hot = ui.hover_amount(("focus-sound-play", id, path), br);
    let rest = if t.light { t.text } else { Color::WHITE.mix(t.text, 0.12) };
    let disc = rest.mix(tone, lit);
    let rad = 18.0 * (1.0 + 0.06 * hot);
    ui.circle(center, rad, disc);
    let ink = t.ink_on(disc);
    if opening {
        ui.spinner(center, 6.5, 2.0, ink);
    } else {
        let glyph = if playing { "\u{F03E4}" } else { "\u{F040A}" };
        // the play triangle sits a touch right of centre, as it looks centred
        let nudge = if playing { 0.0 } else { 1.0 };
        ui.icon(Rect::new(br.x + nudge, br.y, br.w, br.h), glyph, 19.0, ink);
    }
    ui.hit(br, Action::Focus(id, play_target));
    if hot > 0.5 {
        widgets::tooltip(ui, br, if playing { "Pause" } else { "Play" });
    }
    // its name, and where it's at
    let left = br.right() + 14.0;
    let right = r.right() - 16.0;
    let time = match duration {
        _ if failed.is_some() => "won't play".to_string(),
        Some(dd) if mine.is_some() && !ended => format!("{} / {}", length(pos, dd), length(dd, dd)),
        Some(dd) => length(dd, dd),
        None => String::new(),
    };
    let ts = Style::mono(size::CAPTION, c.muted);
    let tb = ui.line(&time, ts, None);
    ui.draw(&tb, Point::new(right - tb.w, r.y + 11.0));
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let nb = ui.line(&name, Style::new(size::SMALL, weight::MEDIUM, c.text), Some((right - left - tb.w - 12.0).max(20.0)));
    ui.draw(&nb, Point::new(left, r.y + 10.0));
    // the waveform: as many bars as fit, the played part in teal (to the playhead, mid-bar)
    let wr = Rect::new(left, r.y + 32.0, (right - left).max(10.0), 22.0);
    let quiet = c.faint.mix(c.text, 0.12);
    let peaks: Vec<f32> = match &wave {
        Some(Ok(w)) => bars(&w.peaks, ((wr.w + 2.0) / 5.0).floor().max(1.0) as usize),
        // being read, or unreadable (it may still play): a flat line of dots
        _ => vec![0.0; ((wr.w + 2.0) / 5.0).floor().max(1.0) as usize],
    };
    let shown = ui.ease(("focus-sound-wave", path), if matches!(wave, Some(Ok(_))) { 1.0 } else { 0.0 }, 260.0);
    let draw_bars = |ui: &mut Ui, ink: Color| {
        for (i, p) in peaks.iter().enumerate() {
            let h = (2.5 + (wr.h - 2.5) * p * shown).max(2.5);
            let x = wr.x + i as f32 * 5.0;
            ui.round(Rect::new(x, wr.y + (wr.h - h) / 2.0, 3.0, h), 1.5, ink);
        }
    };
    draw_bars(ui, quiet);
    if frac > 0.0 {
        ui.clip(Rect::new(wr.x - 1.0, wr.y - 2.0, wr.w * frac + 1.0, wr.h + 4.0), 0.0);
        draw_bars(ui, tone);
        ui.unclip();
    }
    // under the pointer: where a click would play from
    let over = ui.hovered(Rect::new(wr.x, r.y + 26.0, wr.w, r.bottom() - r.y - 26.0));
    if over && let Some(p) = ui.pointer {
        let x = p.x.clamp(wr.x, wr.right());
        ui.top_fill(Rect::new(x - 0.75, wr.y - 3.0, 1.5, wr.h + 6.0), 0.75, c.text.with_alpha(0.55));
        if let Some(dd) = duration {
            let at = dd.mul_f32(((x - wr.x) / wr.w.max(1.0)).clamp(0.0, 1.0));
            widgets::tooltip(ui, Rect::new(x - 1.0, wr.y - 4.0, 2.0, wr.h + 8.0), &length(at, dd));
        }
    }
    let hit = Rect::new(wr.x, r.y + 26.0, wr.w, r.bottom() - r.y - 26.0);
    ui.hit(hit, Action::Focus(id, wave_target));
    if let Some(e) = failed
        && ui.hovered(r)
    {
        widgets::tooltip(ui, Rect::new(right - tb.w, r.y + 8.0, tb.w, 18.0), &e);
    }
    hit
}

/// `peaks` fitted to `n` bars: each the loudest of its stretch.
fn bars(peaks: &[f32], n: usize) -> Vec<f32> {
    if peaks.is_empty() || n == 0 {
        return vec![0.0; n];
    }
    (0..n)
        .map(|i| {
            let (a, b) = (i * peaks.len() / n, ((i + 1) * peaks.len() / n).max(i * peaks.len() / n + 1).min(peaks.len()));
            peaks[a..b].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

/// `d` into a sound `of` long: seconds for a short one ("2.6s", "45s"), else a clock ("1:42").
fn length(d: Duration, of: Duration) -> String {
    match of.as_secs_f32() {
        s if s < 10.0 => format!("{:.1}s", d.as_secs_f32()),
        s if s < 60.0 => format!("{}s", d.as_secs()),
        _ => clock(d),
    }
}

/// "0:07", "1:42", "1:02:05".
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::sessions::conversation::Step;

    #[test]
    fn the_sounds_an_item_names_are_its_absolute_sound_files_once_each() {
        let (a, b) = if cfg!(windows) { (r"C:\out\nora-01.mp3", r"C:\out\take.wav") } else { ("/out/nora-01.mp3", "/out/take.wav") };
        let said = Item::Said { text: format!("Here's the line: `{a}`, and again {a}. Also {b}, notes.txt and ./relative.mp3."), at_ms: None };
        assert_eq!(sounds_of(&said), vec![PathBuf::from(a), PathBuf::from(b)]);
        let step = |kind, detail: &str, state| Step { id: String::new(), kind, label: String::new(), detail: Some(detail.into()), state, added: 0, removed: 0, line: None, at_ms: None, app: None };
        let did = Item::Did { steps: vec![step(StepKind::Send, a, StepState::Done), step(StepKind::Run, b, StepState::Done), step(StepKind::Create, b, StepState::Failed)] };
        assert_eq!(sounds_of(&did), vec![PathBuf::from(a)], "sent, not a command's text or a failed write");
        assert_eq!(height(0), 0.0);
        assert_eq!(height(2), GAP + 2.0 * CARD_H + 8.0);
    }

    #[test]
    fn peaks_fit_any_number_of_bars_keeping_the_loud_parts() {
        let p = [0.1, 0.9, 0.2, 0.3, 1.0, 0.0];
        assert_eq!(bars(&p, 3), vec![0.9, 0.3, 1.0]);
        assert_eq!(bars(&p, 6), p.to_vec());
        assert_eq!(bars(&p, 12).len(), 12);
        assert_eq!(bars(&[], 4), vec![0.0; 4]);
        assert_eq!(clock(Duration::from_secs(7)), "0:07");
        let (short, mid, long) = (Duration::from_millis(2600), Duration::from_secs(45), Duration::from_secs(102));
        assert_eq!((length(short, short), length(Duration::from_millis(1200), short)), ("2.6s".into(), "1.2s".into()));
        assert_eq!((length(mid, mid), length(long, long), length(Duration::from_secs(7), long)), ("45s".into(), "1:42".into(), "0:07".into()));
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }
}
