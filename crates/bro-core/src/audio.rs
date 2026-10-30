//! Sound files the agents make (a voice line, a song): what counts as one, and its waveform for the players that
//! show it inline in a conversation (the desktop app's focus mode; the phone's through the bridge). Decoded in
//! pure Rust (symphonia), so it reads the same on every OS without ffmpeg: WAV, MP3, Ogg Vorbis, FLAC and AAC /
//! M4A. Opus has no decoder here: the players still play it, with a flat waveform.

use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// The extensions a sound file goes by (what the bridge serves as audio).
pub const EXTENSIONS: [&str; 8] = ["mp3", "wav", "m4a", "aac", "flac", "ogg", "oga", "opus"];

/// Longer than this is read only this far: the rest of the waveform stays flat (an hour of podcast would keep a
/// worker busy for seconds).
const MAX_SECS: u64 = 30 * 60;

/// A sound file at a glance: `peaks` (0..1, the loudest is 1) across its length, and how long it plays.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    pub peaks: Vec<f32>,
    pub duration_ms: u64,
}

/// `path` names a sound file (by its extension).
pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Read `path` through and sum it up in `bars` peaks, each the loudest moment in its stretch. Blocking (a minute
/// of MP3 is tens of milliseconds): call it off the UI thread.
pub fn waveform(path: &Path, bars: usize) -> anyhow::Result<Waveform> {
    let bars = bars.clamp(1, 1000);
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("{}: not a sound file", path.display()))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| anyhow!("{}: no sound in it", path.display()))?;
    let id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0) as u64;
    let listed = track.codec_params.n_frames;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).with_context(|| format!("{}: no decoder for it", path.display()))?;
    // the loudest sample in each 10 ms, kept small whatever the length; bars are made from these at the end
    let mut blocks: Vec<f32> = vec![];
    let (mut block, mut in_block, mut frames) = (0f32, 0u64, 0u64);
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => return Err(anyhow!("{}: {e}", path.display())),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // a damaged frame: skip it, as players do
            Err(SymErr::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow!("{}: {e}", path.display())),
        };
        let spec = *decoded.spec();
        rate = spec.rate as u64;
        let channels = spec.channels.count().max(1);
        let sb = match &mut buf {
            Some(b) if b.capacity() >= decoded.capacity() * channels => b,
            _ => buf.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        sb.copy_interleaved_ref(decoded);
        let per_block = (rate / 100).max(1);
        for frame in sb.samples().chunks(channels) {
            block = frame.iter().fold(block, |m, s| m.max(s.abs()));
            in_block += 1;
            if in_block == per_block {
                blocks.push(block);
                (block, in_block) = (0.0, 0);
            }
        }
        frames += (sb.samples().len() / channels) as u64;
        if rate > 0 && frames > rate * MAX_SECS {
            break;
        }
    }
    if in_block > 0 {
        blocks.push(block);
    }
    if rate == 0 || frames == 0 {
        bail!("{}: no sound in it", path.display());
    }
    // read only so far: the rest is there, flat
    let total = listed.filter(|n| *n > frames).unwrap_or(frames);
    let span = (total as f64 / (rate / 100).max(1) as f64).ceil().max(1.0) as usize;
    let mut peaks = vec![0f32; bars];
    for (i, b) in blocks.iter().enumerate() {
        let bar = (i * bars / span.max(blocks.len())).min(bars - 1);
        peaks[bar] = peaks[bar].max(*b);
    }
    let loudest = peaks.iter().copied().fold(0f32, f32::max);
    if loudest > 0.0 {
        peaks.iter_mut().for_each(|p| *p /= loudest);
    }
    Ok(Waveform { peaks, duration_ms: total * 1000 / rate })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit mono WAV: `quiet` seconds of near silence, then `loud` of a full-scale tone.
    fn wav(path: &Path, rate: u32, quiet: f32, loud: f32) {
        let n_quiet = (rate as f32 * quiet) as usize;
        let n = n_quiet + (rate as f32 * loud) as usize;
        let samples: Vec<i16> = (0..n)
            .map(|i| {
                let a = if i < n_quiet { 300.0 } else { 32000.0 };
                (a * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin()) as i16
            })
            .collect();
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = vec![];
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_waveform_follows_the_sound_and_says_how_long_it_plays() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("line.wav");
        wav(&p, 22_050, 1.0, 1.0);
        let w = waveform(&p, 20).unwrap();
        assert_eq!(w.peaks.len(), 20);
        assert!((w.duration_ms as i64 - 2000).abs() <= 20, "{} ms", w.duration_ms);
        // quiet first half, loud second, the loudest at 1
        assert!(w.peaks[..9].iter().all(|p| *p < 0.05), "{:?}", w.peaks);
        assert!(w.peaks[11..].iter().all(|p| *p > 0.9), "{:?}", w.peaks);
        assert_eq!(w.peaks.iter().copied().fold(0f32, f32::max), 1.0);
    }

    #[test]
    fn what_counts_as_a_sound_file_and_what_isnt_one() {
        for p in ["a.mp3", "b.WAV", "c.m4a", "d.flac", "e.ogg", "f.opus", "g.aac"] {
            assert!(is_audio(Path::new(p)), "{p}");
        }
        for p in ["a.mp4", "b.png", "c", "d.mp3.txt"] {
            assert!(!is_audio(Path::new(p)), "{p}");
        }
        let dir = tempfile::tempdir().unwrap();
        let not = dir.path().join("not.mp3");
        std::fs::write(&not, b"hello").unwrap();
        assert!(waveform(&not, 10).is_err());
        assert!(waveform(&dir.path().join("missing.wav"), 10).is_err());
    }
}
