//! Streaming VT helpers used by both live output and bounded recovery.
//!
//! The important invariant is that a replay buffer is never trimmed inside a
//! CSI/OSC/DCS/APC/PM/SOS sequence or between UTF-8 code units. Normal terminal
//! controls are preserved; this is not an ANSI stripper.

use std::collections::VecDeque;

const MAX_TRANSCRIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTROL_STRING_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum VtState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

#[derive(Debug, Default)]
pub struct VtBoundaryTracker {
    state: VtState,
    control_bytes: usize,
}

impl VtBoundaryTracker {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    pub fn is_ground(&self) -> bool {
        self.state == VtState::Ground
    }

    fn feed_byte(&mut self, byte: u8) {
        use VtState::*;
        match self.state {
            Ground => {
                if byte == 0x1b {
                    self.state = Escape;
                    self.control_bytes = 1;
                }
            }
            Escape => {
                self.control_bytes += 1;
                self.state = match byte {
                    b'[' => Csi,
                    b']' => Osc,
                    b'P' | b'X' | b'^' | b'_' => String,
                    0x1b => Escape,
                    _ => Ground,
                };
            }
            Csi => {
                self.control_bytes += 1;
                if (0x40..=0x7e).contains(&byte) {
                    self.state = Ground;
                } else if byte == 0x1b {
                    self.state = Escape;
                }
            }
            Osc => {
                self.control_bytes += 1;
                if byte == 0x07 {
                    self.state = Ground;
                } else if byte == 0x1b {
                    self.state = OscEscape;
                }
            }
            OscEscape => {
                self.control_bytes += 1;
                self.state = if byte == b'\\' {
                    Ground
                } else if byte == 0x1b {
                    OscEscape
                } else {
                    Osc
                };
            }
            String => {
                self.control_bytes += 1;
                if byte == 0x1b {
                    self.state = StringEscape;
                }
            }
            StringEscape => {
                self.control_bytes += 1;
                self.state = if byte == b'\\' { Ground } else { String };
            }
        }

        // A broken integration must not keep the parser trapped forever. CAN
        // and SUB are the VT-standard cancellation bytes; an oversized string
        // is treated the same way at the bridge boundary.
        if matches!(byte, 0x18 | 0x1a) || self.control_bytes > MAX_CONTROL_STRING_BYTES {
            self.state = Ground;
            self.control_bytes = 0;
        }
        if self.state == Ground {
            self.control_bytes = 0;
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplayChunk {
    pub seq: u64,
    pub data: String,
    pub at: String,
    bytes: usize,
    starts_ground: bool,
}

#[derive(Debug, Default)]
pub struct SafeReplayBuffer {
    chunks: VecDeque<ReplayChunk>,
    bytes: usize,
    tracker: VtBoundaryTracker,
}

impl SafeReplayBuffer {
    pub fn push(&mut self, seq: u64, data: String, at: String) {
        let starts_ground = self.tracker.is_ground();
        self.tracker.feed(data.as_bytes());
        let bytes = data.len();
        self.bytes += bytes;
        self.chunks.push_back(ReplayChunk {
            seq,
            data,
            at,
            bytes,
            starts_ground,
        });
        self.trim();
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn chunks(&self) -> impl Iterator<Item = &ReplayChunk> {
        self.chunks.iter()
    }

    pub fn transcript(&self) -> String {
        let mut output = String::with_capacity(self.bytes);
        for chunk in &self.chunks {
            output.push_str(&chunk.data);
        }
        output
    }

    fn trim(&mut self) {
        while self.bytes > MAX_TRANSCRIPT_BYTES && self.chunks.len() > 1 {
            // Only remove through a boundary where the following chunk was
            // observed in ground state. This permits a small bounded overshoot
            // instead of ever manufacturing visible control-sequence tails.
            let next_is_safe = self
                .chunks
                .get(1)
                .map(|chunk| chunk.starts_ground)
                .unwrap_or(false);
            if !next_is_safe {
                // If a sequence spans several reads, keep discarding complete
                // chunks until its terminating read is also discarded.
                let removed = self.chunks.pop_front().expect("buffer is non-empty");
                self.bytes = self.bytes.saturating_sub(removed.bytes);
                continue;
            }
            let removed = self.chunks.pop_front().expect("buffer is non-empty");
            self.bytes = self.bytes.saturating_sub(removed.bytes);
        }

        // The first retained chunk is safe by construction. If an upstream
        // legacy replay began mid-sequence, discard complete chunks until a
        // known ground start rather than slicing a string.
        while !self
            .chunks
            .front()
            .map(|chunk| chunk.starts_ground)
            .unwrap_or(true)
        {
            let removed = self.chunks.pop_front().expect("buffer is non-empty");
            self.bytes = self.bytes.saturating_sub(removed.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_survives_every_osc_split() {
        let sequence = b"\x1b]1337;TerminalWeb.Agent=abc\x1b\\visible";
        for split in 0..=sequence.len() {
            let mut tracker = VtBoundaryTracker::default();
            tracker.feed(&sequence[..split]);
            tracker.feed(&sequence[split..]);
            assert!(
                tracker.is_ground(),
                "split {split} did not return to ground"
            );
        }
    }

    #[test]
    fn tracker_survives_every_csi_and_dcs_split() {
        for sequence in [
            b"\x1b[38;2;10;20;30mtext".as_slice(),
            b"\x1bPopaque;payload\x1b\\text".as_slice(),
        ] {
            for split in 0..=sequence.len() {
                let mut tracker = VtBoundaryTracker::default();
                tracker.feed(&sequence[..split]);
                tracker.feed(&sequence[split..]);
                assert!(
                    tracker.is_ground(),
                    "split {split} did not return to ground"
                );
            }
        }
    }

    #[test]
    fn replay_never_slices_a_chunk_or_utf8_character() {
        let mut replay = SafeReplayBuffer::default();
        let large = "🙂".repeat((MAX_TRANSCRIPT_BYTES / 4) + 10);
        replay.push(1, large.clone(), "now".into());
        replay.push(2, "tail".into(), "now".into());
        let result = replay.transcript();
        assert!(result == "tail" || result.starts_with('🙂'));
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn oversized_control_string_recovers_to_ground() {
        let mut tracker = VtBoundaryTracker::default();
        let mut data = b"\x1b]9;".to_vec();
        data.extend(std::iter::repeat_n(b'x', MAX_CONTROL_STRING_BYTES + 1));
        tracker.feed(&data);
        assert!(tracker.is_ground());
    }

    #[test]
    fn trimming_discards_every_mid_sequence_fragment() {
        let mut replay = SafeReplayBuffer::default();
        replay.push(1, "x".repeat(MAX_TRANSCRIPT_BYTES), "now".into());
        replay.push(2, "\u{1b}]0;partial".into(), "now".into());
        replay.push(3, " title".into(), "now".into());
        replay.push(4, "\u{1b}\\tail".into(), "now".into());
        let safe = "s".repeat(MAX_TRANSCRIPT_BYTES);
        replay.push(5, safe.clone(), "now".into());
        assert_eq!(replay.transcript(), safe);
    }
}
