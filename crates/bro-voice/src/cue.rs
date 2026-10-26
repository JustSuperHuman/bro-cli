//! bro's own cues, made here rather than read from a file: the chime bro's Claude mod plays when Claude asks you
//! a question, and the ticks of the question's countdown. `bro hook sound question|ticks` plays them, in a process
//! of its own, on [`crate::sound`]'s player, so they sound the same on every OS.

use crate::sound::{Clip, play_clip};
use anyhow::{Context, bail};
use std::f32::consts::PI;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RATE: u32 = 24_000;
/// How loud a cue plays (0..1).
const VOLUME: f32 = 0.8;
/// The longest countdown that ticks, in seconds.
const MAX_TICKS: u32 = 60;
/// How far ahead `ticks` may be started; any earlier is a mistake, not a wait.
const MAX_WAIT: Duration = Duration::from_secs(120);

/// Two rising bell notes in half a second: Claude has a question for you.
pub fn question() -> Clip {
    let mut out = silence(0.62);
    note(&mut out, 0.0, 659.25, 0.40, 0.30); // E5
    note(&mut out, 0.12, 987.77, 0.50, 0.32); // B5
    mono(out)
}

/// The last `secs` seconds of a countdown: a tick a second, firmer for the last five, and a higher note on the
/// last; the first tick at the start, the last a second before the end.
pub fn ticks(secs: u32) -> Clip {
    let secs = secs.clamp(1, MAX_TICKS);
    let mut out = silence(secs as f32 - 1.0 + 0.35);
    for k in 0..secs {
        match secs - k {
            1 => note(&mut out, k as f32, 1318.5, 0.30, 0.35),
            2..=5 => note(&mut out, k as f32, 880.0, 0.09, 0.30),
            _ => note(&mut out, k as f32, 880.0, 0.05, 0.16),
        }
    }
    mono(out)
}

/// `bro hook sound <cue> …`: plays it and returns once it's played.
pub fn play(args: &[String]) -> anyhow::Result<()> {
    let (wait, clip) = plan(args, now_ms())?;
    std::thread::sleep(wait);
    play_clip(&clip, VOLUME, None)
}

/// What `play` plays, and how long it waits first. `question`; or `ticks <closes at, epoch ms> <seconds>`, timed
/// to the question's close: started late, the ticks already due are dropped and the rest keep their time.
fn plan(args: &[String], now: u64) -> anyhow::Result<(Duration, Clip)> {
    match args.first().map(String::as_str) {
        Some("question") => Ok((Duration::ZERO, question())),
        Some("ticks") => {
            let closes_at: u64 = args.get(1).and_then(|v| v.parse().ok()).context("ticks: when the question closes (epoch ms)")?;
            let secs = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(15u32).clamp(1, MAX_TICKS);
            let start = closes_at.saturating_sub(u64::from(secs) * 1000);
            let wait = Duration::from_millis(start.saturating_sub(now));
            if wait > MAX_WAIT {
                bail!("ticks: the countdown starts in {}s", wait.as_secs());
            }
            let mut clip = ticks(secs);
            let late = now.saturating_sub(start) as usize * RATE as usize / 1000;
            clip.samples.drain(..late.min(clip.samples.len()));
            Ok((wait, clip))
        }
        other => bail!("no such sound: {other:?} (question, ticks)"),
    }
}

fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * RATE as f32) as usize]
}

/// A bell-ish note into `out` from `at` seconds: a sine and a little of its octave, a 4 ms attack and an
/// exponential fall that is all but silent by `len`.
fn note(out: &mut [f32], at: f32, hz: f32, len: f32, gain: f32) {
    let start = (at * RATE as f32) as usize;
    for i in 0..(len * RATE as f32) as usize {
        let Some(s) = out.get_mut(start + i) else { break };
        let t = i as f32 / RATE as f32;
        let envelope = (t / 0.004).min(1.0) * (-5.0 * t / len).exp();
        let w = 2.0 * PI * hz * t;
        *s += gain * envelope * (w.sin() + 0.25 * (2.0 * w).sin());
    }
}

fn mono(samples: Vec<f32>) -> Clip {
    Clip { rate: RATE, channels: 1, samples }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    /// The loudest sample within `ms` of `at_ms`.
    fn peak(c: &Clip, at_ms: u64, ms: u64) -> f32 {
        let from = (at_ms * u64::from(RATE) / 1000) as usize;
        let to = ((at_ms + ms) * u64::from(RATE) / 1000) as usize;
        c.samples.get(from..to.min(c.samples.len())).unwrap_or(&[]).iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    #[test]
    fn the_question_chime_is_half_a_second_and_never_clips() {
        let c = question();
        assert!((c.duration().as_secs_f32() - 0.62).abs() < 0.01);
        assert!(peak(&c, 0, 620) > 0.3 && peak(&c, 0, 620) < 1.0);
        assert!(peak(&c, 600, 20) < 0.02, "it fades out rather than stopping dead");
    }

    #[test]
    fn ticks_land_on_each_second_and_the_last_five_are_firmer() {
        let c = ticks(15);
        assert!((c.duration().as_secs_f32() - 14.35).abs() < 0.01);
        for s in 0..15 {
            assert!(peak(&c, s * 1000, 30) > 0.05, "a tick at {s}s");
            assert!(peak(&c, s * 1000 + 500, 100) < 0.01, "quiet between ticks at {s}s");
        }
        assert!(peak(&c, 10_000, 30) > peak(&c, 9_000, 30), "firmer from five seconds left");
        assert!(peak(&c, 14_000, 300) < 1.0);
    }

    #[test]
    fn ticks_wait_for_their_start_and_drop_what_is_already_past() {
        let closes = 1_000_000;
        let (wait, c) = plan(&args(&["ticks", "1000000", "15"]), closes - 17_000).unwrap();
        assert_eq!(wait, Duration::from_secs(2));
        assert_eq!(c, ticks(15));
        let (wait, c) = plan(&args(&["ticks", "1000000", "15"]), closes - 10_000).unwrap();
        assert_eq!(wait, Duration::ZERO);
        // five seconds in: what's left starts with the tick at ten seconds left
        assert!((c.duration().as_secs_f32() - 9.35).abs() < 0.01);
        assert!(peak(&c, 0, 30) > 0.05);
        let (_, c) = plan(&args(&["ticks", "1000000", "15"]), closes + 5_000).unwrap();
        assert!(c.samples.is_empty(), "closed already: nothing to tick");
    }

    #[test]
    fn a_bad_cue_is_refused_rather_than_guessed() {
        assert!(plan(&args(&["ticks"]), 0).is_err());
        assert!(plan(&args(&["ticks", "soon"]), 0).is_err());
        assert!(plan(&args(&["ticks", "900000", "15"]), 0).is_err(), "far in the future: a mistake, not a wait");
        assert!(plan(&args(&["bell"]), 0).is_err());
        assert!(plan(&args(&[]), 0).is_err());
        assert!(plan(&args(&["question"]), 0).is_ok());
    }
}
