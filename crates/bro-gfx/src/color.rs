//! Colours: sRGB-encoded RGBA in 0..1, blended the way browsers and design tools blend (in sRGB space).

/// An sRGB colour with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }
    /// From 8-bit channels.
    pub fn rgb8(r: u8, g: u8, b: u8) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }
    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { a: a as f32 / 255.0, ..Color::rgb8(r, g, b) }
    }
    /// `0xRRGGBB`.
    pub fn hex(v: u32) -> Color {
        Color::rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
    /// "#rrggbb", "#rrggbbaa" or "#rgb".
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(s.get(i..i + 1)?, 16).ok().map(|v| v * 17);
                Some(Color::rgb8(n(0)?, n(1)?, n(2)?))
            }
            6 => Some(Color::rgb8(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Color::rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }
    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
    /// Multiply the alpha (fades).
    pub fn fade(self, f: f32) -> Color {
        Color { a: self.a * f.clamp(0.0, 1.0), ..self }
    }
    /// `self` → `o` by `t` (0..1), alpha included.
    pub fn mix(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let l = |a: f32, b: f32| a + (b - a) * t;
        Color { r: l(self.r, o.r), g: l(self.g, o.g), b: l(self.b, o.b), a: l(self.a, o.a) }
    }
    /// `self` painted over an opaque `under`: the opaque result.
    pub fn over(self, under: Color) -> Color {
        let a = self.a.clamp(0.0, 1.0);
        Color { r: self.r * a + under.r * (1.0 - a), g: self.g * a + under.g * (1.0 - a), b: self.b * a + under.b * (1.0 - a), a: 1.0 }
    }
    /// Relative luminance (WCAG), 0 black … 1 white.
    pub fn luminance(self) -> f32 {
        let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }
    pub fn is_light(self) -> bool {
        self.luminance() > 0.35
    }
    /// WCAG contrast ratio between two opaque colours.
    pub fn contrast(self, o: Color) -> f32 {
        let (a, b) = (self.luminance(), o.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
    pub fn to_rgba8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }
    /// Hue/saturation/lightness (all 0..1) for gentle adjustments.
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r, self.g, self.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f32::EPSILON {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s, l)
    }
    pub fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Color {
        if s <= 0.0 {
            return Color { r: l, g: l, b: l, a };
        }
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let hue = |mut t: f32| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        Color { r: hue(h + 1.0 / 3.0), g: hue(h), b: hue(h - 1.0 / 3.0), a }
    }
    /// Lighter (`amount` > 0) or darker (< 0) in HSL lightness.
    pub fn lighten(self, amount: f32) -> Color {
        let (h, s, l) = self.to_hsl();
        Color::from_hsl(h, s, (l + amount).clamp(0.0, 1.0), self.a)
    }
    pub fn saturate(self, amount: f32) -> Color {
        let (h, s, l) = self.to_hsl();
        Color::from_hsl(h, (s + amount).clamp(0.0, 1.0), l, self.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_mix() {
        assert_eq!(Color::parse("#ff0000"), Some(Color::rgb8(255, 0, 0)));
        assert_eq!(Color::parse("0f0"), Some(Color::rgb8(0, 255, 0)));
        assert_eq!(Color::parse("#00000080").map(|c| (c.a * 255.0).round()), Some(128.0));
        assert_eq!(Color::parse("nope"), None);
        let grey = Color::BLACK.mix(Color::WHITE, 0.5);
        assert!((grey.r - 0.5).abs() < 1e-6);
        assert!(Color::WHITE.is_light() && !Color::BLACK.is_light());
        assert!(Color::WHITE.contrast(Color::BLACK) > 20.0);
        let c = Color::hex(0x3b82f6);
        let (h, s, l) = c.to_hsl();
        let back = Color::from_hsl(h, s, l, 1.0);
        assert!((back.r - c.r).abs() < 1e-4 && (back.g - c.g).abs() < 1e-4 && (back.b - c.b).abs() < 1e-4);
    }
}
