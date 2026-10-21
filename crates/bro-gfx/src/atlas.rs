//! Glyph atlases: a single-channel one for ordinary text and an RGBA one for colour emoji. Space is handed
//! out with etagere's shelf allocator; pixels wait in `uploads` until the renderer copies them to the GPU.
//! When an atlas fills up it is reset at a bigger size and the frame is drawn again.

use etagere::{BucketedAtlasAllocator, size2};

/// Pixels on the GPU texture per glyph pixel, plus a one-pixel gutter so neighbours never bleed.
const PAD: i32 = 1;
const MAX_SIZE: u32 = 8192;

/// Which atlas a glyph lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Mask,
    Color,
}

impl Kind {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Kind::Mask => 1,
            Kind::Color => 4,
        }
    }
}

/// Pixels to copy into the texture at (x, y).
pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub struct Atlas {
    pub kind: Kind,
    pub size: u32,
    alloc: BucketedAtlasAllocator,
    pub uploads: Vec<Upload>,
    /// Bumps every reset: the renderer recreates its texture when it changes.
    pub generation: u64,
}

impl Atlas {
    pub fn new(kind: Kind, size: u32) -> Atlas {
        Atlas { kind, size, alloc: BucketedAtlasAllocator::new(size2(size as i32, size as i32)), uploads: vec![], generation: 0 }
    }

    /// Room for a `w`×`h` image: its top-left corner, or None when the atlas is full.
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w == 0 || h == 0 {
            return Some((0, 0));
        }
        let a = self.alloc.allocate(size2(w as i32 + PAD, h as i32 + PAD))?;
        Some((a.rectangle.min.x as u32, a.rectangle.min.y as u32))
    }

    /// Queue `data` (rows of `w` pixels) for the texture at (x, y).
    pub fn put(&mut self, x: u32, y: u32, w: u32, h: u32, data: Vec<u8>) {
        debug_assert_eq!(data.len(), (w * h) as usize * self.kind.bytes_per_pixel());
        if w > 0 && h > 0 {
            self.uploads.push(Upload { x, y, w, h, data });
        }
    }

    /// Start over, twice as big when `grow` (up to the limit). Every cached placement is now invalid.
    pub fn reset(&mut self, grow: bool) {
        if grow {
            self.size = (self.size * 2).min(MAX_SIZE);
        }
        self.alloc = BucketedAtlasAllocator::new(size2(self.size as i32, self.size as i32));
        self.uploads.clear();
        self.generation += 1;
    }

    pub fn uv(&self, x: u32, y: u32, w: u32, h: u32) -> [f32; 4] {
        let s = self.size as f32;
        [x as f32 / s, y as f32 / s, (x + w) as f32 / s, (y + h) as f32 / s]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_up_and_resets_bigger() {
        let mut a = Atlas::new(Kind::Mask, 64);
        let mut n = 0;
        while a.alloc(15, 15).is_some() {
            n += 1;
            assert!(n < 100);
        }
        assert!(n >= 9, "a 64px atlas holds at least nine 16px cells, got {n}");
        a.reset(true);
        assert_eq!(a.size, 128);
        assert_eq!(a.generation, 1);
        assert!(a.alloc(15, 15).is_some());
        assert_eq!(a.uv(0, 0, 64, 32), [0.0, 0.0, 0.5, 0.25]);
    }
}
