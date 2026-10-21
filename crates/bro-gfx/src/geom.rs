//! Geometry in pixels: points, sizes, rectangles and edge insets.

/// A point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
    pub fn offset(self, dx: f32, dy: f32) -> Point {
        Point { x: self.x + dx, y: self.y + dy }
    }
    pub fn scale(self, s: f32) -> Point {
        Point { x: self.x * s, y: self.y * s }
    }
}

/// A size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Size {
        Size { w, h }
    }
}

/// Space around the inside of a rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const fn all(v: f32) -> Edges {
        Edges { top: v, right: v, bottom: v, left: v }
    }
    /// Vertical and horizontal.
    pub const fn xy(x: f32, y: f32) -> Edges {
        Edges { top: y, right: x, bottom: y, left: x }
    }
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Edges {
        Edges { top, right, bottom, left }
    }
}

/// An axis-aligned rectangle: top-left corner and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
    /// Big enough to never clip anything.
    pub const EVERYTHING: Rect = Rect { x: -1.0e7, y: -1.0e7, w: 2.0e7, h: 2.0e7 };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn from_points(a: Point, b: Point) -> Rect {
        let (x0, x1) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (y0, y1) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point { x: self.x + self.w * 0.5, y: self.y + self.h * 0.5 }
    }
    pub fn origin(&self) -> Point {
        Point { x: self.x, y: self.y }
    }
    pub fn size(&self) -> Size {
        Size { w: self.w, h: self.h }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
    /// Shrink by the same amount on every side.
    pub fn inset(&self, v: f32) -> Rect {
        self.pad(Edges::all(v))
    }
    pub fn pad(&self, e: Edges) -> Rect {
        Rect { x: self.x + e.left, y: self.y + e.top, w: (self.w - e.left - e.right).max(0.0), h: (self.h - e.top - e.bottom).max(0.0) }
    }
    /// Grow by the same amount on every side.
    pub fn outset(&self, v: f32) -> Rect {
        Rect { x: self.x - v, y: self.y - v, w: self.w + 2.0 * v, h: self.h + 2.0 * v }
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    /// Take `w` off the left: (taken, rest).
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (Rect { w, ..*self }, Rect { x: self.x + w, w: self.w - w, ..*self })
    }
    /// Take `w` off the right: (rest, taken).
    pub fn split_right(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (Rect { w: self.w - w, ..*self }, Rect { x: self.right() - w, w, ..*self })
    }
    /// Take `h` off the top: (taken, rest).
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (Rect { h, ..*self }, Rect { y: self.y + h, h: self.h - h, ..*self })
    }
    /// Take `h` off the bottom: (rest, taken).
    pub fn split_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (Rect { h: self.h - h, ..*self }, Rect { y: self.bottom() - h, h, ..*self })
    }
    /// A `w`×`h` rectangle centred in this one.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect { x: self.x + (self.w - w) * 0.5, y: self.y + (self.h - h) * 0.5, w, h }
    }
    pub fn scale(&self, s: f32) -> Rect {
        Rect { x: self.x * s, y: self.y * s, w: self.w * s, h: self.h * s }
    }
    /// Edges rounded to whole pixels (crisp hairlines and fills).
    pub fn snap(&self) -> Rect {
        let x = self.x.round();
        let y = self.y.round();
        Rect { x, y, w: (self.right().round() - x).max(0.0), h: (self.bottom().round() - y).max(0.0) }
    }
    pub fn to_array(&self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
    /// Linear interpolation between two rectangles.
    pub fn lerp(&self, o: &Rect, t: f32) -> Rect {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Rect { x: l(self.x, o.x), y: l(self.y, o.y), w: l(self.w, o.w), h: l(self.h, o.h) }
    }
}

/// Corner radii: top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl Corners {
    pub const ZERO: Corners = Corners { tl: 0.0, tr: 0.0, br: 0.0, bl: 0.0 };
    pub const fn all(r: f32) -> Corners {
        Corners { tl: r, tr: r, br: r, bl: r }
    }
    pub const fn top(r: f32) -> Corners {
        Corners { tl: r, tr: r, br: 0.0, bl: 0.0 }
    }
    pub const fn bottom(r: f32) -> Corners {
        Corners { tl: 0.0, tr: 0.0, br: r, bl: r }
    }
    pub const fn left(r: f32) -> Corners {
        Corners { tl: r, tr: 0.0, br: 0.0, bl: r }
    }
    pub const fn right(r: f32) -> Corners {
        Corners { tl: 0.0, tr: r, br: r, bl: 0.0 }
    }
    pub fn scale(&self, s: f32) -> Corners {
        Corners { tl: self.tl * s, tr: self.tr * s, br: self.br * s, bl: self.bl * s }
    }
    pub fn to_array(&self) -> [f32; 4] {
        [self.tl, self.tr, self.br, self.bl]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_intersections() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        let (l, rest) = r.split_left(30.0);
        assert_eq!(l, Rect::new(10.0, 20.0, 30.0, 50.0));
        assert_eq!(rest, Rect::new(40.0, 20.0, 70.0, 50.0));
        let (rest, b) = r.split_bottom(10.0);
        assert_eq!(b, Rect::new(10.0, 60.0, 100.0, 10.0));
        assert_eq!(rest.h, 40.0);
        assert!(r.intersect(&Rect::new(200.0, 0.0, 10.0, 10.0)).is_empty());
        assert_eq!(r.intersect(&Rect::new(0.0, 0.0, 20.0, 30.0)), Rect::new(10.0, 20.0, 10.0, 10.0));
        assert!(r.contains(Point::new(10.0, 20.0)));
        assert!(!r.contains(Point::new(110.0, 20.0)), "right edge is outside");
        assert_eq!(Rect::new(0.4, 0.6, 10.2, 10.0).snap(), Rect::new(0.0, 1.0, 11.0, 10.0));
    }
}
