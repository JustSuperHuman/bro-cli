//! Geometry from a successfully presented GUI frame. No text, pixels, serialization or transport.
//!
//! Drawing records only visible chrome; publication binds it to the current native client, not saved
//! window placement. The service's latest-value worker owns coalescing and its trailing deadline.

use super::layout::PaneBox;
use crate::app::App;
use crate::pane::PaneId;
use bro_bridge::orbit_voice::{
    StageFrame, StageKind, StagePoint, StageRect, StageSurface, StageViewport, StageWindow,
};
use bro_gfx::Rect;
use std::collections::HashMap;
use winit::window::Window;

const MAX_SURFACES: usize = 96;
const MAX_EXCLUSIONS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Pane(PaneId, &'static str),
    Chrome(String),
}

struct Surface {
    key: Key,
    rect: StageRect,
    kind: StageKind,
    priority: u8,
}

/// One draw attempt, discarded wholesale on atlas retries or presentation failure.
pub(super) struct Drawn {
    viewport: Rect,
    scale_factor: f64,
    surfaces: Vec<Surface>,
    exclusions: Vec<StageRect>,
    preferred: Option<Key>,
    suspended: bool,
}

impl Drawn {
    pub fn new(viewport: Rect, scale_factor: f64) -> Self {
        Self {
            viewport,
            scale_factor,
            surfaces: Vec::with_capacity(MAX_SURFACES),
            exclusions: vec![],
            preferred: None,
            suspended: false,
        }
    }

    pub fn suspend(&mut self) {
        self.suspended = true;
    }

    fn add(&mut self, key: Key, rect: Rect, clip: Rect, kind: StageKind, priority: u8) {
        let Some(rect) = clipped(rect, clip.intersect(&self.viewport)) else {
            return;
        };
        // Bound collection even with hundreds of sessions. Pane headers take precedence over sidebar chrome.
        if self.surfaces.iter().any(|s| s.key == key) {
            return;
        }
        let surface = Surface {
            key,
            rect,
            kind,
            priority,
        };
        if self.surfaces.len() < MAX_SURFACES {
            self.surfaces.push(surface);
        } else if let Some((at, old)) = self
            .surfaces
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| s.priority)
            && old.priority > priority
        {
            self.surfaces[at] = surface;
        }
    }

    /// The card rectangle/header passed here are the ones chrome actually painted, including flips.
    pub fn pane(&mut self, pane: &PaneBox, shown: Rect, header: Rect, active: bool, turning: bool) {
        if turning {
            // Navigation need not chase a card narrowing to its edge. Its settled face publishes next frame.
            self.suspend();
        }
        let key = Key::Pane(pane.id, "header");
        self.add(
            key.clone(),
            header,
            shown,
            StageKind::OneWay,
            if active { 0 } else { 1 },
        );
        if active && self.surfaces.iter().any(|s| s.key == key) {
            self.preferred = Some(key);
        }
        if !pane.folded {
            // Thin visible frame rails, never a solid rectangle across the terminal/conversation interior.
            self.add(
                Key::Pane(pane.id, "left"),
                Rect::new(
                    shown.x,
                    header.bottom(),
                    1.0,
                    shown.bottom() - header.bottom(),
                ),
                shown,
                StageKind::Solid,
                3,
            );
            self.add(
                Key::Pane(pane.id, "right"),
                Rect::new(
                    shown.right() - 1.0,
                    header.bottom(),
                    1.0,
                    shown.bottom() - header.bottom(),
                ),
                shown,
                StageKind::Solid,
                3,
            );
            self.add(
                Key::Pane(pane.id, "bottom"),
                Rect::new(shown.x, shown.bottom() - 1.0, shown.w, 1.0),
                shown,
                StageKind::OneWay,
                3,
            );
        }
    }

    pub fn composer(&mut self, id: PaneId, rect: Rect, clip: Rect) {
        self.add(Key::Pane(id, "composer"), rect, clip, StageKind::Solid, 2);
    }

    pub fn choice(&mut self, id: PaneId, rect: Rect, clip: Rect) {
        self.add(Key::Pane(id, "choice"), rect, clip, StageKind::Solid, 2);
    }

    pub fn sidebar_row(&mut self, id: PaneId, rect: Rect, clip: Rect) {
        // Rows have a perch only when a real visible chip has usable width/height, not one clipped to a sliver.
        if let Some(visible) = clipped(rect, clip)
            && visible.w >= 48.0
            && visible.h >= 12.0
        {
            self.add(Key::Pane(id, "sidebar"), rect, clip, StageKind::OneWay, 5);
        }
    }

    pub fn chrome(&mut self, id: &str, rect: Rect, clip: Rect, kind: StageKind) {
        self.add(Key::Chrome(id.to_owned()), rect, clip, kind, 4);
    }

    pub fn tab(&mut self, key: &str, rect: Rect, clip: Rect) {
        // Project keys may contain paths/unicode: export an opaque stable ASCII identifier, never their text.
        self.chrome(
            &format!("tab-{:016x}", fingerprint(key)),
            rect,
            clip,
            StageKind::OneWay,
        );
    }

    pub fn workspace(&mut self, body: Rect) {
        self.add(
            Key::Chrome("workspace-floor".into()),
            Rect::new(body.x, self.viewport.bottom() - 1.0, body.w, 1.0),
            self.viewport,
            StageKind::OneWay,
            0,
        );
    }

    fn finish(
        self,
        native: Native,
        mut pane_id: impl FnMut(PaneId) -> Option<String>,
    ) -> Option<StageFrame> {
        // Reject a resize/DPI transition that happened during drawing. Origin is read again after presentation.
        if !self.scale_factor.is_finite()
            || (self.scale_factor - native.viewport.scale_factor).abs() > 0.000001
            || (f64::from(self.viewport.w) - native.viewport.width).abs() > 0.01
            || (f64::from(self.viewport.h) - native.viewport.height).abs() > 0.01
        {
            return None;
        }
        let mut preferred = None;
        let mut surfaces = Vec::with_capacity(self.surfaces.len());
        for surface in self.surfaces {
            // bro-gfx lays out in f32, while the native contract is exact f64 physical-size / DPI.
            // Clip once more in contract coordinates so a rounded-up logical edge is never out of bounds.
            let Some(rect) = client_rect(surface.rect, &native.viewport) else {
                continue;
            };
            let id = match &surface.key {
                Key::Pane(pane, part) => format!("pane-{}-{part}", pane_id(*pane)?),
                Key::Chrome(id) => id.clone(),
            };
            if self.preferred.as_ref() == Some(&surface.key) {
                preferred = Some(id.clone());
            }
            surfaces.push(StageSurface {
                id,
                rect,
                kind: surface.kind,
            });
        }
        // Canonical order makes text/spinner/focus paint order irrelevant to semantic comparison.
        surfaces.sort_by(|a, b| a.id.cmp(&b.id));
        let mut exclusions = self.exclusions;
        exclusions.truncate(MAX_EXCLUSIONS);
        if self.suspended {
            // A suspended stage deliberately has no invisible colliders or stale preferred perch.
            surfaces.clear();
            exclusions.clear();
            preferred = None;
        }
        Some(StageFrame {
            window: native.window,
            viewport: native.viewport,
            surfaces,
            exclusions,
            preferred_surface_id: preferred,
            suspended: self.suspended,
        })
    }
}

fn clipped(rect: Rect, clip: Rect) -> Option<StageRect> {
    if ![
        rect.x, rect.y, rect.w, rect.h, clip.x, clip.y, clip.w, clip.h,
    ]
    .into_iter()
    .all(f32::is_finite)
        || rect.w <= 0.0
        || rect.h <= 0.0
        || clip.w <= 0.0
        || clip.h <= 0.0
    {
        return None;
    }
    let r = rect.intersect(&clip);
    (r.w > 0.0 && r.h > 0.0).then(|| StageRect {
        x: f64::from(r.x),
        y: f64::from(r.y),
        w: f64::from(r.w),
        h: f64::from(r.h),
    })
}

fn client_rect(rect: StageRect, viewport: &StageViewport) -> Option<StageRect> {
    let x = rect.x.max(0.0);
    let y = rect.y.max(0.0);
    let w = (rect.x + rect.w).min(viewport.width) - x;
    let h = (rect.y + rect.h).min(viewport.height) - y;
    (w > 0.0 && h > 0.0).then_some(StageRect { x, y, w, h })
}

fn fingerprint(key: &str) -> u64 {
    // Fixed FNV-1a, independent of randomized hash state and redraw order.
    key.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

/// Metadata sampled from the actual native client, never Prefs::position (which is outer placement).
struct Native {
    window: StageWindow,
    viewport: StageViewport,
}

impl Native {
    fn from_parts(
        handle: String,
        epoch: String,
        size: (u32, u32),
        scale: f64,
        origin: (i32, i32),
    ) -> Option<Self> {
        if size.0 == 0 || size.1 == 0 || !scale.is_finite() || scale <= 0.0 || handle.is_empty() {
            return None;
        }
        Some(Self {
            window: StageWindow {
                handle,
                pid: std::process::id(),
                epoch,
            },
            viewport: StageViewport {
                width: f64::from(size.0) / scale,
                height: f64::from(size.1) / scale,
                scale_factor: scale,
                physical_origin: StagePoint {
                    x: f64::from(origin.0),
                    y: f64::from(origin.1),
                },
            },
        })
    }

    fn read(window: &Window, epoch: &str) -> Option<Self> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if window.is_minimized() == Some(true) || window.is_visible() == Some(false) {
            return None;
        }
        // The current external host advertises native attachment on Windows only.
        let handle = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get().to_string(),
            _ => return None,
        };
        let origin = window.inner_position().ok()?;
        let size = window.inner_size();
        Self::from_parts(
            handle,
            epoch.to_owned(),
            (size.width, size.height),
            window.scale_factor(),
            (origin.x, origin.y),
        )
    }
}

/// GUI-local bounded semantic cache. The service worker owns serialization, I/O and the trailing flush.
pub(super) struct Publisher {
    epoch: String,
    ids: HashMap<PaneId, String>,
    last: Option<StageFrame>,
    initialized: bool,
}

impl Default for Publisher {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().to_string(),
            ids: HashMap::new(),
            last: None,
            initialized: false,
        }
    }
}

impl Publisher {
    pub fn invalidate(&mut self, svc: &crate::services::Services) {
        if self.replace(None) {
            svc.queue_orbit_stage(None);
        }
    }

    pub fn new_window(&mut self, svc: &crate::services::Services) {
        self.invalidate(svc);
        self.epoch = uuid::Uuid::new_v4().to_string();
    }

    /// Pure movement reuses only the last presented local geometry. A resize/DPI/identity change cannot.
    pub fn moved(&mut self, app: &App, window: &Window, occluded: bool) -> bool {
        let frame = if occluded {
            None
        } else {
            self.last.as_ref().and_then(|frame| {
                Native::read(window, &self.epoch).and_then(|native| rebase(frame, native))
            })
        };
        self.publish(app, frame)
    }

    pub fn needs_frame(&self) -> bool {
        self.last.is_none()
    }

    fn replace(&mut self, frame: Option<StageFrame>) -> bool {
        if self.initialized && self.last == frame {
            return false;
        }
        self.initialized = true;
        self.last = frame;
        true
    }

    pub fn presented(&mut self, app: &App, window: &Window, drawn: Drawn, occluded: bool) -> bool {
        self.ids.retain(|id, _| app.panes.contains_key(id));
        let frame = if occluded {
            None
        } else {
            Native::read(window, &self.epoch).and_then(|native| {
                drawn.finish(native, |id| {
                    let pane = app.panes.get(&id)?;
                    if let Some(sid) = pane
                        .as_term_ref()
                        .and_then(|t| uuid::Uuid::parse_str(&t.meta.sid).ok())
                    {
                        return Some(sid.to_string());
                    }
                    Some(
                        self.ids
                            .entry(id)
                            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
                            .clone(),
                    )
                })
            })
        };
        self.publish(app, frame)
    }

    fn publish(&mut self, app: &App, frame: Option<StageFrame>) -> bool {
        if self.initialized && self.last == frame {
            return false;
        }
        self.replace(frame.clone());
        app.svc.queue_orbit_stage(frame);
        true
    }
}

fn rebase(frame: &StageFrame, native: Native) -> Option<StageFrame> {
    if frame.window != native.window
        || frame.viewport.width != native.viewport.width
        || frame.viewport.height != native.viewport.height
        || frame.viewport.scale_factor != native.viewport.scale_factor
    {
        return None;
    }
    let mut moved = frame.clone();
    moved.viewport.physical_origin = native.viewport.physical_origin;
    Some(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::gui::layout::Joined;

    fn viewport() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 600.0)
    }
    fn native(scale: f64, origin: (i32, i32)) -> Native {
        Native::from_parts(
            "123".into(),
            "epoch-1".into(),
            ((800.0 * scale) as u32, (600.0 * scale) as u32),
            scale,
            origin,
        )
        .unwrap()
    }
    fn pane(id: PaneId, x: f32) -> PaneBox {
        let card = Rect::new(x, 80.0, 320.0, 400.0);
        PaneBox {
            id,
            card,
            header: Rect { h: 34.0, ..card },
            content: Rect::new(x + 1.0, 114.0, 318.0, 365.0),
            cells: Default::default(),
            focused: id == 1,
            folded: false,
            grouped: false,
            number: None,
            joined: Joined::default(),
        }
    }
    fn frame(drawn: Drawn) -> StageFrame {
        drawn
            .finish(native(1.5, (-1920, -120)), |id| {
                Some(format!("00000000-0000-0000-0000-{id:012}"))
            })
            .unwrap()
    }

    #[test]
    fn clips_to_visible_bounds_and_rejects_nonfinite_or_empty_rectangles() {
        let r = clipped(
            Rect::new(-10.0, 90.0, 50.0, 30.0),
            Rect::new(0.0, 100.0, 800.0, 600.0),
        )
        .unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 100.0, 40.0, 20.0));
        assert!(clipped(Rect::new(f32::NAN, 0.0, 20.0, 20.0), viewport()).is_none());
        assert!(clipped(Rect::new(900.0, 0.0, 20.0, 20.0), viewport()).is_none());
        assert!(clipped(Rect::new(0.0, 0.0, -1.0, 20.0), viewport()).is_none());
    }

    #[test]
    fn split_headers_rails_and_active_perch_are_real_and_stable() {
        let mut draw = Drawn::new(viewport(), 1.5);
        for id in 1..=2 {
            let p = pane(id, if id == 1 { 20.0 } else { 400.0 });
            draw.pane(&p, p.card, p.header, id == 1, false);
        }
        let f = frame(draw);
        assert_eq!(f.surfaces.len(), 8);
        assert_eq!(
            f.preferred_surface_id.as_deref(),
            Some("pane-00000000-0000-0000-0000-000000000001-header")
        );
        assert!(!f.surfaces.iter().any(|s| s.id.ends_with("composer")));
        assert!(
            f.surfaces
                .iter()
                .filter(|s| matches!(s.kind, StageKind::Solid))
                .all(|s| s.rect.w == 1.0)
        );
    }

    #[test]
    fn zoom_and_removed_panes_do_not_keep_old_surfaces() {
        let mut before = Drawn::new(viewport(), 1.5);
        for id in 1..=2 {
            let p = pane(id, id as f32 * 100.0);
            before.pane(&p, p.card, p.header, id == 1, false);
        }
        let mut zoom = Drawn::new(viewport(), 1.5);
        let p = pane(2, 20.0);
        zoom.pane(&p, p.card, p.header, true, false);
        assert_eq!(frame(before).surfaces.len(), 8);
        let f = frame(zoom);
        assert_eq!(f.surfaces.len(), 4);
        assert!(f.surfaces.iter().all(|s| s.id.contains("000000000002")));
        assert!(frame(Drawn::new(viewport(), 1.5)).surfaces.is_empty());
    }

    #[test]
    fn flips_and_unknown_modal_or_native_child_suspend_without_colliders() {
        let p = pane(1, 20.0);
        for turning in [false, true] {
            let mut draw = Drawn::new(viewport(), 1.5);
            draw.pane(
                &p,
                Rect::new(100.0, 80.0, 100.0, 400.0),
                Rect::new(100.0, 80.0, 100.0, 34.0),
                true,
                turning,
            );
            if !turning {
                draw.suspend();
            }
            let f = frame(draw);
            assert!(f.suspended);
            assert!(f.surfaces.is_empty());
            assert!(f.preferred_surface_id.is_none());
        }
    }

    #[test]
    fn sidebar_rows_and_tabs_use_their_actual_scrolled_clips() {
        let mut draw = Drawn::new(viewport(), 1.5);
        let clip = Rect::new(0.0, 150.0, 240.0, 200.0);
        draw.sidebar_row(1, Rect::new(10.0, 130.0, 220.0, 32.0), clip);
        draw.sidebar_row(2, Rect::new(10.0, 110.0, 220.0, 32.0), clip);
        draw.tab(
            "C:/a unicode project/\u{03b2}",
            Rect::new(700.0, 6.0, 200.0, 34.0),
            Rect::new(240.0, 0.0, 520.0, 40.0),
        );
        let f = frame(draw);
        assert_eq!(f.surfaces.len(), 2);
        let row = f
            .surfaces
            .iter()
            .find(|s| s.id.ends_with("sidebar"))
            .unwrap();
        assert_eq!((row.rect.y, row.rect.h), (150.0, 12.0));
        let tab = f
            .surfaces
            .iter()
            .find(|s| s.id.starts_with("tab-"))
            .unwrap();
        assert!(tab.id.is_ascii());
        assert_eq!(tab.rect.w, 60.0);
    }

    #[test]
    fn only_real_focus_composer_and_choice_are_exported() {
        let mut draw = Drawn::new(viewport(), 1.5);
        draw.composer(1, Rect::new(50.0, 500.0, 500.0, 70.0), viewport());
        draw.choice(1, Rect::new(50.0, 450.0, 500.0, 38.0), viewport());
        let f = frame(draw);
        assert_eq!(f.surfaces.len(), 2);
        assert!(
            f.surfaces
                .iter()
                .all(|s| matches!(s.kind, StageKind::Solid))
        );
        assert_eq!(
            f.surfaces
                .iter()
                .find(|s| s.id.ends_with("composer"))
                .unwrap()
                .rect
                .y,
            500.0
        );
    }

    #[test]
    fn current_physical_origin_and_exact_mixed_dpi_client_extent_are_used() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let f = Drawn::new(viewport(), scale)
                .finish(native(scale, (-2560, -95)), |_| None)
                .unwrap();
            assert_eq!(
                (f.viewport.width * scale, f.viewport.height * scale),
                (800.0 * scale, 600.0 * scale)
            );
            assert_eq!(
                (f.viewport.physical_origin.x, f.viewport.physical_origin.y),
                (-2560.0, -95.0)
            );
        }
        let fractional =
            Native::from_parts("1".into(), "e".into(), (1001, 777), 1.5, (-100, 20)).unwrap();
        assert_eq!(fractional.viewport.width, 1001.0 / 1.5);
        assert!(Native::from_parts("1".into(), "e".into(), (0, 777), 1.5, (0, 0)).is_none());
        assert!(Native::from_parts("1".into(), "e".into(), (100, 777), f64::NAN, (0, 0)).is_none());
    }

    #[test]
    fn resize_or_dpi_mismatch_drops_frame_instead_of_reusing_stale_origin() {
        let wrong =
            Native::from_parts("1".into(), "e".into(), (1000, 600), 1.0, (-999, 33)).unwrap();
        assert!(
            Drawn::new(viewport(), 1.5)
                .finish(wrong, |_| None)
                .is_none()
        );
        let mut publisher = Publisher::default();
        assert!(publisher.replace(Some(frame(Drawn::new(viewport(), 1.5)))));
        assert!(
            publisher.replace(None),
            "hidden/minimized/failed frames clear old geometry"
        );
        assert!(!publisher.replace(None));
        let moved = Drawn::new(viewport(), 1.5)
            .finish(native(1.5, (-1800, 22)), |_| None)
            .unwrap();
        assert!(publisher.replace(Some(moved)));
        assert_eq!(publisher.last.unwrap().viewport.physical_origin.x, -1800.0);
    }

    #[test]
    fn movement_rebases_presented_surfaces_but_rejects_viewport_dpi_and_epoch_changes() {
        let mut draw = Drawn::new(viewport(), 1.5);
        let p = pane(1, 20.0);
        draw.pane(&p, p.card, p.header, true, false);
        let f = frame(draw);
        let moved = rebase(&f, native(1.5, (-2400, 40))).unwrap();
        assert_eq!(moved.surfaces, f.surfaces);
        assert_eq!(moved.preferred_surface_id, f.preferred_surface_id);
        assert_eq!(moved.viewport.physical_origin.x, -2400.0);
        assert!(rebase(&f, native(2.0, (-2400, 40))).is_none());
        let mut replacement = native(1.5, (-2400, 40));
        replacement.window.epoch = "epoch-2".into();
        assert!(rebase(&f, replacement).is_none());
        let mut resized = native(1.5, (-2400, 40));
        resized.viewport.width += 1.0;
        assert!(rebase(&f, resized).is_none());
    }

    #[test]
    fn fractional_dpi_edges_are_clipped_again_in_exact_contract_coordinates() {
        let snapshot =
            Native::from_parts("1".into(), "e".into(), (1000, 778), 1.5, (-100, 20)).unwrap();
        let viewport = Rect::new(
            0.0,
            0.0,
            snapshot.viewport.width as f32,
            snapshot.viewport.height as f32,
        );
        assert!(f64::from(viewport.w) > snapshot.viewport.width, "fixture must exercise an f32 edge rounded out of bounds");
        let mut draw = Drawn::new(viewport, 1.5);
        draw.chrome("full-client", viewport, viewport, StageKind::OneWay);
        let f = draw.finish(snapshot, |_| None).unwrap();
        assert_eq!(f.surfaces.len(), 1);
        for s in f.surfaces {
            assert!(s.rect.x + s.rect.w <= f.viewport.width);
            assert!(s.rect.y + s.rect.h <= f.viewport.height);
        }
        // Same logical size is not enough to reuse a frame drawn before a DPI transition.
        assert!(
            Drawn::new(Rect::new(0.0, 0.0, 800.0, 600.0), 1.0)
                .finish(native(2.0, (0, 0)), |_| None)
                .is_none()
        );
    }

    #[test]
    fn collection_and_semantic_cache_are_bounded_and_quiet_frames_compare_equal() {
        let mut draw = Drawn::new(viewport(), 1.5);
        for i in 0..1000 {
            draw.tab(
                &i.to_string(),
                Rect::new(10.0, 6.0, 100.0, 34.0),
                viewport(),
            );
        }
        let p = pane(1, 20.0);
        draw.pane(&p, p.card, p.header, true, false);
        let f = frame(draw);
        assert_eq!(f.surfaces.len(), MAX_SURFACES);
        assert!(f.preferred_surface_id.is_some());
        assert!(f.exclusions.len() <= MAX_EXCLUSIONS);
        let mut publisher = Publisher::default();
        assert!(publisher.replace(Some(f.clone())));
        assert!(!publisher.replace(Some(f)));
    }
}
