//! Optional, latest-only workspace geometry. This lane never writes request events.

use super::*;

const MAX_SURFACES: usize = 96;
const MAX_EXCLUSIONS: usize = 32;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_STAGE_ID: usize = 160;
const MAX_VIEWPORT: f64 = 100_000.0;
const MAX_SCALE: f64 = 16.0;
const MAX_PHYSICAL: f64 = 10_000_000.0;
// Leave room for authenticated body buffering/response work inside the 1s deadline.
const HEARTBEAT: Duration = Duration::from_millis(900);
pub(super) const MAX_WATCHERS: usize = 8;

/// Client-local logical pixels, y down; physical origins use physical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageWindow {
    pub handle: String,
    pub pid: u32,
    pub epoch: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageViewport {
    pub width: f64,
    pub height: f64,
    pub scale_factor: f64,
    pub physical_origin: StagePoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageKind {
    Solid,
    OneWay,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageSurface {
    pub id: String,
    pub rect: StageRect,
    pub kind: StageKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageFrame {
    pub window: StageWindow,
    pub viewport: StageViewport,
    pub surfaces: Vec<StageSurface>,
    pub exclusions: Vec<StageRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_surface_id: Option<String>,
    pub suspended: bool,
}

#[derive(Clone, Default)]
pub(super) struct Snapshot {
    pub revision: u64,
    frame: Option<StageFrame>,
}

impl Snapshot {
    fn available(&self) -> bool {
        self.frame.as_ref().is_some_and(|frame| !frame.suspended)
    }

    pub(super) fn metadata(&self) -> Value {
        json!({"schema":1,"revision":self.revision,"available":self.available()})
    }

    fn response(&self, instance_id: &str) -> Value {
        json!({"schema":1,"instanceId":instance_id,"revision":self.revision,
            "available":self.available(),"frame":self.frame})
    }
}

pub(super) fn capabilities() -> Value {
    let mut capabilities: Vec<_> = CAPABILITIES.iter().copied().collect();
    capabilities.push("avatar.stage.v1");
    capabilities.push("avatar.roam.v1");
    json!(capabilities)
}

pub(super) fn replace(stage: &watch::Sender<Snapshot>, frame: Option<StageFrame>) {
    stage.send_if_modified(|current| {
        if current.frame == frame {
            return false;
        }
        current.revision = current.revision.saturating_add(1);
        current.frame = frame;
        true
    });
}

pub(super) fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_STAGE_ID
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

pub(super) fn matches_window(frame: &StageFrame, window: &Value) -> bool {
    window["handle"].as_str() == Some(frame.window.handle.as_str())
        && window["pid"].as_u64() == Some(frame.window.pid as u64)
        && window
            .get("epoch")
            .is_none_or(|epoch| epoch.as_str() == Some(frame.window.epoch.as_str()))
}

pub(super) fn valid_frame(frame: &StageFrame) -> bool {
    let window = &frame.window;
    let viewport = &frame.viewport;
    let positive = |value: f64, maximum: f64| value.is_finite() && value > 0.0 && value <= maximum;
    if window.handle.is_empty()
        || window.handle.len() > 32
        || !window.handle.bytes().all(|byte| byte.is_ascii_digit())
        || window
            .handle
            .parse::<u64>()
            .ok()
            .is_none_or(|handle| handle == 0)
        || window.pid == 0
        || !safe_id(&window.epoch)
        || !positive(viewport.width, MAX_VIEWPORT)
        || !positive(viewport.height, MAX_VIEWPORT)
        || !positive(viewport.scale_factor, MAX_SCALE)
        || !viewport.physical_origin.x.is_finite()
        || !viewport.physical_origin.y.is_finite()
        || viewport.physical_origin.x.abs() > MAX_PHYSICAL
        || viewport.physical_origin.y.abs() > MAX_PHYSICAL
        || frame.surfaces.len() > MAX_SURFACES
        || frame.exclusions.len() > MAX_EXCLUSIONS
    {
        return false;
    }
    let mut ids = std::collections::HashSet::with_capacity(frame.surfaces.len());
    if frame.surfaces.iter().any(|surface| {
        !safe_id(&surface.id)
            || !ids.insert(surface.id.as_str())
            || !valid_rect(&surface.rect, viewport)
    }) || frame
        .exclusions
        .iter()
        .any(|rect| !valid_rect(rect, viewport))
        || frame
            .preferred_surface_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id.as_str()))
    {
        return false;
    }
    // Counts and string lengths are already bounded, so serialization cannot
    // allocate an unbounded buffer (and NaN/Infinity were rejected above).
    serde_json::to_vec(frame).is_ok_and(|encoded| encoded.len() <= MAX_FRAME_BYTES)
}

fn valid_rect(rect: &StageRect, viewport: &StageViewport) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.w.is_finite()
        && rect.h.is_finite()
        && rect.x >= 0.0
        && rect.y >= 0.0
        && rect.w > 0.0
        && rect.h > 0.0
        && rect.x + rect.w <= viewport.width
        && rect.y + rect.h <= viewport.height
}

pub(super) struct Rate {
    start: Instant,
    count: usize,
}

impl Rate {
    pub(super) fn new() -> Self {
        Self {
            start: Instant::now(),
            count: 0,
        }
    }

    pub(super) fn admit(&mut self) -> bool {
        if self.start.elapsed() >= Duration::from_secs(1) {
            self.start = Instant::now();
            self.count = 0;
        }
        if self.count >= 128 {
            return false;
        }
        self.count += 1;
        true
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StageQuery {
    #[serde(default)]
    after: Option<u64>,
}

pub(super) fn stopped() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "stopped",
        "The Orbit host is stopping.",
    )
}

pub(super) async fn get(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<StageQuery>,
) -> Result<Json<Value>, ApiError> {
    // Subscribe before checking the cursor so no publication can be lost between
    // the initial read and installing the waiter. No Data lock crosses an await.
    let mut updates = shared.stage.subscribe();
    let mut shutdown = shared.shutdown.subscribe();
    if !shared.running.load(Ordering::Acquire) || *shutdown.borrow() {
        return Err(stopped());
    }
    let revision = updates.borrow_and_update().revision;
    if query.after == Some(revision) {
        tokio::select! {
            _ = updates.changed() => {},
            _ = tokio::time::sleep(HEARTBEAT) => {},
            _ = shutdown.wait_for(|stopped| *stopped) => return Err(stopped()),
        }
    }
    if !shared.running.load(Ordering::Acquire) || *shutdown.borrow() {
        return Err(stopped());
    }
    // A future cursor (e.g. host restart) returns the current snapshot immediately.
    Ok(Json(updates.borrow().response(&shared.instance_id)))
}
