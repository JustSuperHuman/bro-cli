//! A Create Slop project's storyboard: the film as timed scenes, each with what is heard (the script), what is
//! seen (the direction), its keyframes and its takes.
//!
//! It is one file in the project, `slop/storyboard.json`, written by two hands. The agent writes it through bro's
//! storyboard tools (`storyboard_plan`, `storyboard_scene`; see bro-bridge's `orchestrator/storyboard.rs`) or by
//! editing the file. You write it from the Storyboard view beside the session: change a line, pick a keyframe,
//! approve a scene, ask for a change. What you changed is kept in [`Board::edits`] until the agent reads the
//! board, so it learns what moved without comparing files.
//!
//! The shape follows SLOPCANNON's timed plan (scenes that run on from each other in whole milliseconds, a named
//! cast and one self-contained direction per scene), so [`Board::timed_plan`] hands it exactly what was agreed.
//! Its limits are checked here as warnings ([`Board::problems`]); the service still has the last word.

use crate::util::{atomic_write, now_secs};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The folder a project keeps its Create Slop work in, and the storyboard's name there.
pub const DIR: &str = "slop";
pub const FILE: &str = "storyboard.json";

/// SLOPCANNON's limits on a timed plan (from its capabilities manifest).
pub const SCENE_MIN_MS: u64 = 5_000;
pub const SCENE_MAX_MS: u64 = 15_000;
pub const BEAT_MAX_CHARS: usize = 2_800;
pub const SCENE_CAST_MAX: usize = 9;
pub const CAST_NAME_MAX: usize = 40;

/// How long a scene runs when nobody said.
const DEFAULT_SCENE_MS: u64 = 8_000;
const SCENES_MAX: usize = 512;
/// The longest a text field gets (a direction is capped tighter by SLOPCANNON; a treatment can run on).
const TEXT_MAX: usize = 12_000;
const SHOTS_MAX: usize = 24;

/// One process writes at a time (the window and the bridge share it).
static WRITING: Mutex<()> = Mutex::new(());

/// A picture or a clip made for a scene.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shot {
    /// where it is: inside the project as `slop/keyframes/s1-a.png`, else a full path
    pub path: String,
    /// what the agent said about it ("wide, golden hour", "take 2: the hand is fixed")
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// A scene of the film.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scene {
    /// stays the same while the scene is reworded, retimed or moved ("s1")
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    /// its job in the story ("Cold open")
    pub title: String,
    /// what is heard: the lyric, the lines, the voice-over
    pub script: String,
    /// what is seen: the staging, the camera, the performance. SLOPCANNON films this text as the scene's beat.
    pub direction: String,
    /// who is in it, by name
    pub cast: Vec<String>,
    /// pictures drawn for it, and which one it is filmed from
    pub keyframes: Vec<Shot>,
    pub keyframe: usize,
    /// clips filmed for it, and which one goes in the cut
    pub takes: Vec<Shot>,
    pub take: usize,
    /// you signed it off
    pub approved: bool,
    /// what you asked to be changed
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// what filming it was quoted at, in the service's credits
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
}

/// How far a scene has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// words only
    Written,
    /// it has a keyframe
    Keyframed,
    /// it has a take
    Filmed,
}

impl Scene {
    pub fn ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    pub fn stage(&self) -> Stage {
        if !self.takes.is_empty() {
            Stage::Filmed
        } else if !self.keyframes.is_empty() {
            Stage::Keyframed
        } else {
            Stage::Written
        }
    }

    /// The keyframe it is filmed from.
    pub fn chosen_keyframe(&self) -> Option<&Shot> {
        self.keyframes.get(self.keyframe.min(self.keyframes.len().saturating_sub(1)))
    }

    /// The take that goes in the cut.
    pub fn chosen_take(&self) -> Option<&Shot> {
        self.takes.get(self.take.min(self.takes.len().saturating_sub(1)))
    }
}

/// Something you changed in the Storyboard view that the agent hasn't read yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// the scene's id; empty for the film itself
    pub scene: String,
    /// what changed: `title`, `concept`, `script`, `direction`, `note`, `approved`, `keyframe`, `take`
    pub what: String,
    pub at: i64,
}

/// What you can change from the Storyboard view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// the film's title
    Film,
    /// the film's treatment
    Concept,
    /// a scene's title
    Title,
    Script,
    Direction,
    /// what you want changed in a scene
    Note,
}

impl Field {
    pub fn key(self) -> &'static str {
        match self {
            Field::Film | Field::Title => "title",
            Field::Concept => "concept",
            Field::Script => "script",
            Field::Direction => "direction",
            Field::Note => "note",
        }
    }
}

/// The storyboard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Board {
    pub version: u32,
    /// goes up with every save
    pub rev: u64,
    pub title: String,
    /// the treatment: the visual world, who is who, the arc
    pub concept: String,
    /// the frame, "16:9" or "9:16"
    pub aspect: String,
    /// how long the piece runs (a song's length); 0 when the scenes decide
    pub duration_ms: u64,
    /// the song or master audio it is cut to
    pub song: String,
    /// everyone in the film, by the names scenes use
    pub cast: Vec<String>,
    pub scenes: Vec<Scene>,
    /// what you changed since the agent last read the board
    pub edits: Vec<Edit>,
    pub updated_at: i64,
}

/// Where `project` keeps its storyboard.
pub fn path(project: &Path) -> PathBuf {
    project.join(DIR).join(FILE)
}

/// The project's storyboard; None when it has none (or the file can't be read as one).
pub fn load(project: &Path) -> Option<Board> {
    let text = std::fs::read_to_string(path(project)).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Write `board` as the project's storyboard, one revision on.
pub fn save(project: &Path, board: &mut Board) -> std::io::Result<()> {
    board.version = 1;
    board.rev += 1;
    board.updated_at = now_secs();
    let body = serde_json::to_string_pretty(board).unwrap_or_default() + "\n";
    let file = path(project);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    atomic_write(&file, body.as_bytes())
}

/// Change the project's storyboard: read what is on disk now (an empty board when there is none), run `change`,
/// write it back when `change` says something moved. Returns what `change` returned and the board as it stands.
pub fn update<R>(project: &Path, change: impl FnOnce(&mut Board) -> (bool, R)) -> std::io::Result<(R, Board)> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut board = load(project).unwrap_or_default();
    let (moved, out) = change(&mut board);
    if moved {
        save(project, &mut board)?;
    }
    Ok((out, board))
}

/// "0:07", "1:02.5": a moment in the film.
pub fn clock(ms: u64) -> String {
    let (m, s, tenth) = (ms / 60_000, (ms % 60_000) / 1000, (ms % 1000) / 100);
    if tenth == 0 { format!("{m}:{s:02}") } else { format!("{m}:{s:02}.{tenth}") }
}

/// "8s", "7.5s": how long something runs.
pub fn seconds(ms: u64) -> String {
    if ms % 1000 == 0 { format!("{}s", ms / 1000) } else { format!("{:.1}s", ms as f64 / 1000.0) }
}

/// Text as it is kept: line ends as `\n`, no other control characters, no trailing space, at most `max` characters.
fn clean(s: &str, max: usize) -> String {
    let s: String = s.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect();
    let s = s.trim();
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect() }
}

/// A name as it is kept: one line.
fn name(s: &str) -> String {
    clean(s, 200).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn names(v: &Value) -> Option<Vec<String>> {
    let list = v.as_array()?;
    let mut out: Vec<String> = vec![];
    for n in list.iter().filter_map(Value::as_str).map(name).filter(|n| !n.is_empty()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
            out.push(n);
        }
    }
    Some(out)
}

fn text_of<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str))
}

fn ms_of(v: &Value, key: &str) -> Option<u64> {
    let n = v.get(key)?;
    n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64)).or_else(|| n.as_str().and_then(|s| s.trim().parse::<f64>().ok()).filter(|f| *f >= 0.0).map(|f| f.round() as u64))
}

/// A file as the board keeps it: inside the project as a forward-slash path from its root, else as given.
pub fn kept_path(project: &Path, file: &str) -> String {
    let file = file.trim().trim_matches('"');
    let p = Path::new(file);
    let abs = if p.is_absolute() { p.to_path_buf() } else { project.join(p) };
    let (abs, root) = (crate::util::normalize_path(&abs), crate::util::normalize_path(project));
    match abs.strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => abs.to_string_lossy().into_owned(),
    }
}

/// Where a kept path is on disk.
pub fn full_path(project: &Path, kept: &str) -> PathBuf {
    let p = Path::new(kept);
    if p.is_absolute() { p.to_path_buf() } else { project.join(p) }
}

impl Board {
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty() && self.title.is_empty() && self.concept.is_empty()
    }

    /// When the last scene ends.
    pub fn end_ms(&self) -> u64 {
        self.scenes.iter().map(|s| s.end_ms).max().unwrap_or(0)
    }

    /// How long the film runs: what was said, else where its scenes end.
    pub fn runs_ms(&self) -> u64 {
        if self.duration_ms > 0 { self.duration_ms } else { self.end_ms() }
    }

    /// The scene `key` names: its id, or its number in the film ("3", "#3", "scene 3").
    pub fn find(&self, key: &str) -> Option<usize> {
        let key = key.trim();
        if let Some(i) = self.scenes.iter().position(|s| s.id == key) {
            return Some(i);
        }
        let digits = key.trim_start_matches(|c: char| !c.is_ascii_digit());
        let n: usize = digits.parse().ok()?;
        (digits.len() == key.len() || key.to_ascii_lowercase().starts_with("scene") || key.starts_with('#')).then_some(n).filter(|n| (1..=self.scenes.len()).contains(n)).map(|n| n - 1)
    }

    /// An id no scene has yet.
    fn fresh_id(&self) -> String {
        (self.scenes.len() + 1..).map(|n| format!("s{n}")).find(|id| !self.scenes.iter().any(|s| &s.id == id)).unwrap_or_default()
    }

    /// How many scenes have got as far as `stage`.
    pub fn count(&self, stage: Stage) -> usize {
        self.scenes.iter().filter(|s| s.stage() >= stage).count()
    }

    pub fn approved(&self) -> usize {
        self.scenes.iter().filter(|s| s.approved).count()
    }

    /// What filming was quoted at, over the scenes that carry a quote.
    pub fn credits(&self) -> Option<f64> {
        let quoted: Vec<f64> = self.scenes.iter().filter_map(|s| s.credits).collect();
        (!quoted.is_empty()).then(|| quoted.iter().sum())
    }

    // -------------------------------------------------------------- the agent's hand

    /// Set the film and its scenes from a `storyboard_plan` call: `title`, `concept`, `aspect`, `durationMs`,
    /// `song`, `cast`, and `scenes` (each `id`, `startMs` and `endMs` or `seconds`, `title`, `script`,
    /// `direction`, `cast`). The scene list replaces the one there unless `append` is true; a scene that keeps
    /// its id keeps its keyframes and takes, and its sign-off while its words stay the same.
    pub fn apply_plan(&mut self, v: &Value, project: &Path) -> Result<(), String> {
        if let Some(t) = text_of(v, &["title"]) {
            self.title = name(t);
        }
        if let Some(t) = text_of(v, &["concept", "treatment"]) {
            self.concept = clean(t, TEXT_MAX);
        }
        if let Some(t) = text_of(v, &["aspect", "aspectRatio"]) {
            self.aspect = name(t);
        }
        if let Some(ms) = ms_of(v, "durationMs") {
            self.duration_ms = ms;
        }
        if let Some(t) = text_of(v, &["song"]) {
            self.song = if t.trim().is_empty() { String::new() } else { kept_path(project, t) };
        }
        if let Some(cast) = v.get("cast").and_then(names) {
            self.cast = cast;
        }
        let Some(list) = v.get("scenes") else { return Ok(()) };
        let list = list.as_array().ok_or("scenes is a list of scenes")?;
        if list.len() > SCENES_MAX {
            return Err(format!("a storyboard holds at most {SCENES_MAX} scenes"));
        }
        let append = v.get("append").and_then(Value::as_bool).unwrap_or(false);
        let before = std::mem::take(&mut self.scenes);
        if append {
            self.scenes = before.clone();
        }
        let mut at = self.end_ms();
        for (n, s) in list.iter().enumerate() {
            if !s.is_object() {
                return Err(format!("scene {} isn't an object", n + 1));
            }
            let id = text_of(s, &["id"]).map(name).filter(|i| !i.is_empty());
            let known = id.as_ref().and_then(|id| before.iter().find(|b| &b.id == id));
            let mut scene = known.cloned().unwrap_or_default();
            scene.id = match id {
                Some(id) if !self.scenes.iter().any(|x| x.id == id) || append => id,
                _ => self.fresh_id(),
            };
            let was = (scene.script.clone(), scene.direction.clone());
            self.write_scene(&mut scene, s, at)?;
            // a scene reworded since you signed it off is yours to look at again
            if (scene.script.as_str(), scene.direction.as_str()) != (was.0.as_str(), was.1.as_str()) {
                scene.approved = false;
            }
            at = scene.end_ms;
            match self.scenes.iter().position(|x| x.id == scene.id) {
                Some(i) => self.scenes[i] = scene,
                None => self.scenes.push(scene),
            }
        }
        // an edit about a scene that is gone means nothing now
        let scenes = &self.scenes;
        self.edits.retain(|e| e.scene.is_empty() || scenes.iter().any(|s| s.id == e.scene));
        Ok(())
    }

    /// The words and timing of one scene from a call's fields; `at`: where it starts when it doesn't say.
    fn write_scene(&self, scene: &mut Scene, s: &Value, at: u64) -> Result<(), String> {
        if let Some(t) = text_of(s, &["title"]) {
            scene.title = name(t);
        }
        if let Some(t) = text_of(s, &["script", "lyric", "lines"]) {
            scene.script = clean(t, TEXT_MAX);
        }
        if let Some(t) = text_of(s, &["direction", "beat"]) {
            scene.direction = clean(t, TEXT_MAX);
        }
        if let Some(cast) = s.get("cast").and_then(names) {
            scene.cast = cast;
        }
        let fresh = scene.end_ms == 0 && scene.start_ms == 0;
        let (start, end, secs) = (ms_of(s, "startMs"), ms_of(s, "endMs"), s.get("seconds").and_then(Value::as_f64).filter(|f| *f > 0.0).map(|f| (f * 1000.0).round() as u64));
        if let Some(start) = start {
            let ran = scene.ms();
            scene.start_ms = start;
            if end.is_none() && secs.is_none() {
                scene.end_ms = start + if fresh { DEFAULT_SCENE_MS } else { ran };
            }
        } else if fresh {
            scene.start_ms = at;
            scene.end_ms = at + DEFAULT_SCENE_MS;
        }
        if let Some(end) = end {
            scene.end_ms = end;
        } else if let Some(secs) = secs {
            scene.end_ms = scene.start_ms + secs;
        }
        if scene.end_ms <= scene.start_ms {
            return Err(format!("scene {} ends ({}) before it starts ({})", scene.id, clock(scene.end_ms), clock(scene.start_ms)));
        }
        if let Some(c) = s.get("credits") {
            scene.credits = c.as_f64().filter(|c| *c >= 0.0);
        }
        Ok(())
    }

    /// Change one scene from a `storyboard_scene` call: `scene` names it (an id or its number); then any of the
    /// plan's scene fields, `keyframe` / `take` (a file to add; an object with `path` and `note` also works),
    /// `selectKeyframe` / `selectTake` (1 is the first), `approved`, `resolveNote` (your request is done),
    /// `credits`, `remove`, or `insert` (a new scene after the one named, or first with `scene: 0`).
    /// Returns the scene's place in the film (where it was, for a removed one).
    pub fn apply_scene(&mut self, v: &Value, project: &Path) -> Result<usize, String> {
        let key = match v.get("scene") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err("scene is required: a scene's id or its number in the film".into()),
        };
        if v.get("insert").and_then(Value::as_bool).unwrap_or(false) {
            let after = if key.trim() == "0" { None } else { Some(self.find(&key).ok_or_else(|| self.no_scene(&key))?) };
            let at = after.map_or(0, |i| self.scenes[i].end_ms);
            let mut scene = Scene { id: self.fresh_id(), ..Scene::default() };
            self.write_scene(&mut scene, v, at)?;
            let i = after.map_or(0, |i| i + 1);
            self.scenes.insert(i, scene);
            return Ok(i);
        }
        let i = self.find(&key).ok_or_else(|| self.no_scene(&key))?;
        if v.get("remove").and_then(Value::as_bool).unwrap_or(false) {
            let gone = self.scenes.remove(i);
            self.edits.retain(|e| e.scene != gone.id);
            return Ok(i);
        }
        let mut scene = self.scenes[i].clone();
        let was = (scene.script.clone(), scene.direction.clone());
        let at = scene.start_ms;
        self.write_scene(&mut scene, v, at)?;
        if (scene.script.as_str(), scene.direction.as_str()) != (was.0.as_str(), was.1.as_str()) {
            scene.approved = false;
        }
        let shot = |v: &Value| -> Option<Shot> {
            let (path, note) = match v {
                Value::String(p) => (p.as_str(), ""),
                Value::Object(_) => (v.get("path").and_then(Value::as_str)?, v.get("note").and_then(Value::as_str).unwrap_or("")),
                _ => return None,
            };
            (!path.trim().is_empty()).then(|| Shot { path: kept_path(project, path), note: clean(note, 300) })
        };
        // a new picture or clip becomes the chosen one: it is the newest answer to what was asked
        if let Some(k) = v.get("keyframe").and_then(shot) {
            add_shot(&mut scene.keyframes, &mut scene.keyframe, k);
        }
        if let Some(t) = v.get("take").and_then(shot) {
            add_shot(&mut scene.takes, &mut scene.take, t);
        }
        let pick = |v: &Value, key: &str, len: usize| -> Result<Option<usize>, String> {
            match v.get(key).and_then(Value::as_u64) {
                None => Ok(None),
                Some(n) if (1..=len as u64).contains(&n) => Ok(Some(n as usize - 1)),
                Some(n) => Err(format!("{key}: {n} is out of range (the scene has {len})")),
            }
        };
        if let Some(n) = pick(v, "selectKeyframe", scene.keyframes.len())? {
            scene.keyframe = n;
        }
        if let Some(n) = pick(v, "selectTake", scene.takes.len())? {
            scene.take = n;
        }
        if let Some(on) = v.get("approved").and_then(Value::as_bool) {
            scene.approved = on;
        }
        if v.get("resolveNote").and_then(Value::as_bool).unwrap_or(false) {
            scene.note.clear();
            let id = scene.id.clone();
            self.edits.retain(|e| !(e.scene == id && e.what == "note"));
        }
        self.scenes[i] = scene;
        Ok(i)
    }

    fn no_scene(&self, key: &str) -> String {
        match self.scenes.len() {
            0 => "the storyboard has no scenes yet: write them with storyboard_plan".into(),
            n => format!("no scene \"{key}\": the film has scenes 1 to {n} ({})", self.scenes.iter().map(|s| s.id.as_str()).take(12).collect::<Vec<_>>().join(", ")),
        }
    }

    /// What stands between the board and a film: scenes that don't run on from each other, a scene with nothing
    /// to see, and (`slopcannon`) what SLOPCANNON's timed plan refuses.
    pub fn problems(&self, slopcannon: bool) -> Vec<String> {
        let mut out = vec![];
        if self.scenes.is_empty() {
            out.push("there are no scenes yet".to_string());
            return out;
        }
        if self.scenes[0].start_ms != 0 {
            out.push(format!("scene 1 starts at {}: the film starts at 0:00", clock(self.scenes[0].start_ms)));
        }
        for (i, s) in self.scenes.iter().enumerate() {
            let n = i + 1;
            if let Some(prev) = i.checked_sub(1).map(|p| &self.scenes[p])
                && s.start_ms != prev.end_ms
            {
                let how = if s.start_ms > prev.end_ms { "a gap" } else { "an overlap" };
                out.push(format!("scene {n} starts at {} but scene {} ends at {}: {how}", clock(s.start_ms), n - 1, clock(prev.end_ms)));
            }
            if s.direction.trim().is_empty() {
                out.push(format!("scene {n} has no direction: nothing says what is seen"));
            }
            if !slopcannon {
                continue;
            }
            if !(SCENE_MIN_MS..=SCENE_MAX_MS).contains(&s.ms()) {
                out.push(format!("scene {n} runs {}: SLOPCANNON films scenes of 5 to 15 seconds", seconds(s.ms())));
            }
            let len = s.direction.chars().count();
            if len > BEAT_MAX_CHARS {
                out.push(format!("scene {n}'s direction is {len} characters: SLOPCANNON takes {BEAT_MAX_CHARS}"));
            }
            if s.cast.len() > SCENE_CAST_MAX {
                out.push(format!("scene {n} casts {}: SLOPCANNON takes {SCENE_CAST_MAX} a scene", s.cast.len()));
            }
            for c in &s.cast {
                if c.chars().count() > CAST_NAME_MAX {
                    out.push(format!("scene {n}: the name \"{c}\" is longer than {CAST_NAME_MAX} characters"));
                } else if !self.cast.is_empty() && !self.cast.iter().any(|f| f.eq_ignore_ascii_case(c)) {
                    out.push(format!("scene {n} casts {c}, who isn't in the film's cast"));
                }
            }
        }
        if self.duration_ms > 0 && self.end_ms() != self.duration_ms {
            out.push(format!("the scenes end at {} and the film runs {}", clock(self.end_ms()), clock(self.duration_ms)));
        }
        out
    }

    /// The board as SLOPCANNON's timed plan (schema version 1): each scene's times, cast and direction.
    pub fn timed_plan(&self) -> Value {
        json!({
            "schemaVersion": 1,
            "durationMs": self.runs_ms(),
            "scenes": self.scenes.iter().map(|s| json!({ "startMs": s.start_ms, "endMs": s.end_ms, "cast": s.cast, "beat": s.direction })).collect::<Vec<_>>(),
        })
    }

    /// The board as the agent reads it: the film, every scene with its number, and what you changed since it
    /// last read ([`Board::take_edits`] clears those once it has).
    pub fn for_agent(&self, project: &Path) -> Value {
        let scenes: Vec<Value> = self
            .scenes
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut v = json!({
                    "n": i + 1, "id": s.id, "startMs": s.start_ms, "endMs": s.end_ms, "title": s.title,
                    "script": s.script, "direction": s.direction, "cast": s.cast, "approved": s.approved,
                    "keyframes": s.keyframes.iter().map(|k| k.path.as_str()).collect::<Vec<_>>(),
                    "takes": s.takes.iter().map(|k| k.path.as_str()).collect::<Vec<_>>(),
                });
                if let Some(k) = s.chosen_keyframe() {
                    v["keyframe"] = json!(k.path);
                }
                if let Some(t) = s.chosen_take() {
                    v["take"] = json!(t.path);
                }
                if !s.note.is_empty() {
                    v["userAsks"] = json!(s.note);
                }
                if let Some(c) = s.credits {
                    v["credits"] = json!(c);
                }
                v
            })
            .collect();
        json!({
            "file": path(project).to_string_lossy(), "rev": self.rev, "title": self.title, "concept": self.concept,
            "aspect": self.aspect, "durationMs": self.runs_ms(), "song": self.song, "cast": self.cast,
            "scenes": scenes, "changedByUser": self.edits_said(),
        })
    }

    // -------------------------------------------------------------- your hand

    /// Note that you changed `what` in the scene with this id (the film itself: an empty id).
    fn edited(&mut self, scene: &str, what: &str) {
        self.edits.retain(|e| !(e.scene == scene && e.what == what));
        self.edits.push(Edit { scene: scene.to_string(), what: what.to_string(), at: now_secs() });
    }

    /// You set a text: the film's title or treatment (`scene`: None), or a scene's. True when it changed.
    pub fn set_text(&mut self, scene: Option<usize>, field: Field, text: &str) -> bool {
        let text = if matches!(field, Field::Film | Field::Title) { name(text) } else { clean(text, TEXT_MAX) };
        let (slot, id): (&mut String, String) = match (scene, field) {
            (None, Field::Film) => (&mut self.title, String::new()),
            (None, Field::Concept) => (&mut self.concept, String::new()),
            (Some(i), f) if i < self.scenes.len() => {
                let s = &mut self.scenes[i];
                let id = s.id.clone();
                let slot = match f {
                    Field::Title => &mut s.title,
                    Field::Script => &mut s.script,
                    Field::Direction => &mut s.direction,
                    Field::Note => &mut s.note,
                    Field::Film | Field::Concept => return false,
                };
                (slot, id)
            }
            _ => return false,
        };
        if *slot == text {
            return false;
        }
        *slot = text;
        // asking for a change takes the sign-off back; so does rewording it yourself
        if let Some(s) = scene.and_then(|i| self.scenes.get_mut(i))
            && field == Field::Note
            && !s.note.is_empty()
        {
            s.approved = false;
        }
        self.edited(&id, field.key());
        true
    }

    /// You signed a scene off, or took that back.
    pub fn approve(&mut self, scene: usize, on: bool) -> bool {
        let Some(s) = self.scenes.get_mut(scene).filter(|s| s.approved != on) else { return false };
        s.approved = on;
        let id = s.id.clone();
        self.edited(&id, "approved");
        true
    }

    /// You chose which keyframe a scene is filmed from.
    pub fn pick_keyframe(&mut self, scene: usize, n: usize) -> bool {
        let Some(s) = self.scenes.get_mut(scene).filter(|s| n < s.keyframes.len() && s.keyframe != n) else { return false };
        s.keyframe = n;
        let id = s.id.clone();
        self.edited(&id, "keyframe");
        true
    }

    /// You chose which take goes in the cut.
    pub fn pick_take(&mut self, scene: usize, n: usize) -> bool {
        let Some(s) = self.scenes.get_mut(scene).filter(|s| n < s.takes.len() && s.take != n) else { return false };
        s.take = n;
        let id = s.id.clone();
        self.edited(&id, "take");
        true
    }

    /// What you changed, a line each, as the agent is told it.
    pub fn edits_said(&self) -> Vec<String> {
        self.edits
            .iter()
            .filter_map(|e| {
                if e.scene.is_empty() {
                    return Some(match e.what.as_str() {
                        "title" => format!("the film's title is now \"{}\"", self.title),
                        _ => "the treatment was rewritten".to_string(),
                    });
                }
                let i = self.scenes.iter().position(|s| s.id == e.scene)?;
                let s = &self.scenes[i];
                let who = format!("scene {} ({})", i + 1, s.id);
                Some(match e.what.as_str() {
                    "title" => format!("{who}: retitled \"{}\"", s.title),
                    "script" => format!("{who}: the script was rewritten"),
                    "direction" => format!("{who}: the direction was rewritten"),
                    "note" if s.note.is_empty() => format!("{who}: the change that was asked for is withdrawn"),
                    "note" => format!("{who}: asks for a change: {}", s.note),
                    "approved" if s.approved => format!("{who}: approved"),
                    "approved" => format!("{who}: no longer approved"),
                    "keyframe" => format!("{who}: keyframe {} is the one to film from", s.keyframe + 1),
                    "take" => format!("{who}: take {} goes in the cut", s.take + 1),
                    other => format!("{who}: {other} changed"),
                })
            })
            .collect()
    }

    /// The agent has read the board: what you changed is no longer news. True when there was something.
    pub fn take_edits(&mut self) -> bool {
        let had = !self.edits.is_empty();
        self.edits.clear();
        had
    }

    /// What to tell the agent after you changed things, as a message typed into its session. `tools`: it has
    /// bro's storyboard tools (else it reads the file).
    pub fn nudge(&self, tools: bool) -> Option<String> {
        let said = self.edits_said();
        if said.is_empty() {
            return None;
        }
        let what = match said.len() {
            // a request that ends its own sentence doesn't get a second full stop
            1 => said[0].trim_end_matches(['.', ' ']).to_string(),
            n => format!("{n} changes ({})", said.iter().take(3).map(|s| s.split(": ").next().unwrap_or(s).to_string()).collect::<Vec<_>>().join(", ") + if n > 3 { ", …" } else { "" }),
        };
        let how = if tools { "Read it with storyboard_get" } else { "Read slop/storyboard.json (my changes are listed under \"edits\")" };
        Some(format!("I changed the storyboard: {what}. {how} and carry on from what it says now."))
    }
}

/// Add `shot` to a scene's keyframes or takes and choose it; one already there (the same file) is chosen instead.
fn add_shot(list: &mut Vec<Shot>, chosen: &mut usize, shot: Shot) {
    if let Some(i) = list.iter().position(|s| s.path == shot.path) {
        if !shot.note.is_empty() {
            list[i].note = shot.note;
        }
        *chosen = i;
        return;
    }
    if list.len() >= SHOTS_MAX {
        // the oldest that isn't the chosen one makes room
        let drop = if *chosen == 0 { 1 } else { 0 };
        list.remove(drop);
        if *chosen > drop {
            *chosen -= 1;
        }
    }
    list.push(shot);
    *chosen = list.len() - 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Value {
        json!({
            "title": "Night  Drive", "concept": "One take through a city that won't sleep.\r\nNeon, rain, no dialogue.", "aspect": "16:9",
            "durationMs": 24000, "song": "slop/audio/night-drive.mp3", "cast": ["CLAUDIA", "BECKY", "claudia"],
            "scenes": [
                { "startMs": 0, "endMs": 8000, "title": "Cold open", "script": "We carry the morning home", "direction": "[Shot 1] CLAUDIA (toward camera; static) under a flickering sign.", "cast": ["CLAUDIA"] },
                { "seconds": 9.5, "title": "The drive", "script": "", "beat": "[Shot 1] The car pulls out. CLAUDIA and BECKY in the front seats.", "cast": ["CLAUDIA", "BECKY"] },
                { "id": "finale", "endMs": 24000, "title": "Dawn", "direction": "[Shot 1] The skyline at dawn.", "cast": [] }
            ]
        })
    }

    #[test]
    fn a_plan_becomes_scenes_that_run_on() {
        let project = Path::new("/films/night");
        let mut b = Board::default();
        b.apply_plan(&plan(), project).unwrap();
        assert_eq!((b.title.as_str(), b.aspect.as_str(), b.duration_ms), ("Night Drive", "16:9", 24000));
        assert_eq!(b.concept, "One take through a city that won't sleep.\nNeon, rain, no dialogue.");
        assert_eq!(b.cast, ["CLAUDIA", "BECKY"], "a name is listed once whatever its case");
        assert_eq!(b.song, "slop/audio/night-drive.mp3");
        let times: Vec<(u64, u64)> = b.scenes.iter().map(|s| (s.start_ms, s.end_ms)).collect();
        assert_eq!(times, [(0, 8000), (8000, 17500), (17500, 24000)], "a scene without a start runs on from the one before");
        assert_eq!(b.scenes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1", "s2", "finale"]);
        assert_eq!(b.scenes[1].direction, "[Shot 1] The car pulls out. CLAUDIA and BECKY in the front seats.", "SLOPCANNON's `beat` is the direction");
        assert!(b.problems(true).is_empty(), "{:?}", b.problems(true));
        assert_eq!((b.find("finale"), b.find("2"), b.find("scene 3"), b.find("#1"), b.find("s2"), b.find("9"), b.find("x")), (Some(2), Some(1), Some(2), Some(0), Some(1), None, None));
        // the timed plan is the board's own times, cast and directions
        let tp = b.timed_plan();
        assert_eq!((tp["schemaVersion"].as_u64(), tp["durationMs"].as_u64(), tp["scenes"].as_array().unwrap().len()), (Some(1), Some(24000), 3));
        assert_eq!(tp["scenes"][1], json!({ "startMs": 8000, "endMs": 17500, "cast": ["CLAUDIA", "BECKY"], "beat": b.scenes[1].direction }));
        assert_eq!((clock(0), clock(7_400), clock(62_500), clock(600_000), seconds(8000), seconds(9500)), ("0:00".into(), "0:07.4".into(), "1:02.5".into(), "10:00".into(), "8s".into(), "9.5s".into()));
    }

    #[test]
    fn a_scene_keeps_its_pictures_through_a_rewrite_and_loses_its_sign_off() {
        let project = Path::new("/films/night");
        let mut b = Board::default();
        b.apply_plan(&plan(), project).unwrap();
        let abs = project.join("slop/keyframes/s1-b.png");
        assert_eq!(b.apply_scene(&json!({ "scene": 1, "keyframe": "slop/keyframes/s1-a.png" }), project), Ok(0));
        b.apply_scene(&json!({ "scene": "s1", "keyframe": { "path": abs.to_string_lossy(), "note": "closer" } }), project).unwrap();
        b.apply_scene(&json!({ "scene": "s1", "take": "D:/renders/s1-take1.mp4", "credits": 64 }), project).unwrap();
        let s = &b.scenes[0];
        assert_eq!(s.keyframes.iter().map(|k| k.path.as_str()).collect::<Vec<_>>(), ["slop/keyframes/s1-a.png", "slop/keyframes/s1-b.png"], "a file in the project is kept from its root");
        assert_eq!((s.keyframe, s.keyframes[1].note.as_str(), s.stage(), s.credits), (1, "closer", Stage::Filmed, Some(64.0)), "the newest picture is the chosen one");
        assert!(Path::new(&s.takes[0].path).is_absolute() || s.takes[0].path.starts_with("D:"), "a file elsewhere keeps its full path");
        // the same file again is chosen, not added twice
        b.apply_scene(&json!({ "scene": "s1", "keyframe": "slop/keyframes/s1-a.png" }), project).unwrap();
        assert_eq!((b.scenes[0].keyframes.len(), b.scenes[0].keyframe), (2, 0));
        b.apply_scene(&json!({ "scene": "s1", "selectKeyframe": 2 }), project).unwrap();
        assert_eq!(b.scenes[0].keyframe, 1);
        assert!(b.apply_scene(&json!({ "scene": "s1", "selectKeyframe": 5 }), project).unwrap_err().contains("out of range"));
        assert!(b.approve(0, true) && b.scenes[0].approved);

        // the plan again with scene 1 reworded: same id, same pictures, the sign-off gone
        let mut again = plan();
        again["scenes"][0]["id"] = json!("s1");
        again["scenes"][0]["script"] = json!("We carry the evening home");
        again["scenes"].as_array_mut().unwrap().truncate(2);
        b.apply_plan(&again, project).unwrap();
        assert_eq!(b.scenes.len(), 2, "the list replaces the one there");
        assert_eq!((b.scenes[0].keyframes.len(), b.scenes[0].takes.len(), b.scenes[0].approved), (2, 1, false));
        assert_eq!(b.count(Stage::Keyframed), 1);
        assert_eq!(b.credits(), Some(64.0));
        // unchanged words keep the sign-off
        b.approve(0, true);
        b.apply_plan(&again, project).unwrap();
        assert!(b.scenes[0].approved);
        // insert after scene 1, then remove it
        assert_eq!(b.apply_scene(&json!({ "scene": 1, "insert": true, "seconds": 6, "title": "Interlude", "direction": "Rain on glass." }), project), Ok(1));
        assert_eq!((b.scenes[1].start_ms, b.scenes[1].end_ms, b.scenes[1].id.as_str()), (8000, 14000, "s3"));
        assert_eq!(b.apply_scene(&json!({ "scene": "s3", "remove": true }), project), Ok(1));
        assert_eq!(b.scenes.len(), 2);
        assert!(b.apply_scene(&json!({ "scene": "nope" }), project).unwrap_err().contains("scenes 1 to 2"));
        assert!(b.apply_scene(&json!({}), project).unwrap_err().contains("scene is required"));
        assert!(b.apply_scene(&json!({ "scene": 1, "startMs": 9000, "endMs": 4000 }), project).unwrap_err().contains("before it starts"));
    }

    #[test]
    fn what_slopcannon_refuses_is_said_before_it_is_bought() {
        let project = Path::new("/films/night");
        let mut b = Board::default();
        assert_eq!(b.problems(true), ["there are no scenes yet"]);
        b.apply_plan(&json!({ "cast": ["CLAUDIA"], "durationMs": 40000, "scenes": [
            { "startMs": 1000, "endMs": 4000, "direction": "x", "cast": ["CLAUDIA"] },
            { "startMs": 5000, "endMs": 25000, "direction": "", "cast": ["BECKY"] },
            { "startMs": 24000, "endMs": 30000, "direction": "y".repeat(3000) }
        ] }), project).unwrap();
        let p = b.problems(true).join("\n");
        for want in ["scene 1 starts at 0:01", "scene 1 runs 3s", "scene 2 starts at 0:05 but scene 1 ends at 0:04: a gap", "scene 2 has no direction", "scene 2 runs 20s", "scene 2 casts BECKY", "scene 3 starts at 0:24 but scene 2 ends at 0:25: an overlap", "3000 characters", "the scenes end at 0:30 and the film runs 0:40"] {
            assert!(p.contains(want), "missing {want:?} in:\n{p}");
        }
        // a film that isn't for SLOPCANNON is only held to running on
        let plain = b.problems(false).join("\n");
        assert!(plain.contains("a gap") && !plain.contains("SLOPCANNON"), "{plain}");
    }

    #[test]
    fn your_changes_wait_for_the_agent_to_read_them() {
        let _sb = crate::util::test_env::sandbox();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        assert_eq!(load(project), None);
        let ((), board) = update(project, |b| (true, b.apply_plan(&plan(), project).unwrap())).unwrap();
        assert_eq!((board.rev, board.version), (1, 1));
        assert!(path(project).is_file());
        // nothing moved: nothing written
        let (_, same) = update(project, |b| (b.set_text(Some(0), Field::Script, "We carry the morning home"), ())).unwrap();
        assert_eq!(same.rev, 1);
        let (_, b) = update(project, |b| {
            let a = b.set_text(Some(0), Field::Script, "We carry the evening home  ");
            let c = b.set_text(Some(1), Field::Note, "make it night");
            let d = b.approve(2, true);
            let e = b.set_text(None, Field::Film, "Night Drive II");
            b.set_text(Some(0), Field::Script, "We carry the evening home");
            (a && c && d && e, ())
        })
        .unwrap();
        assert_eq!(b.rev, 2);
        assert_eq!(b.scenes[0].script, "We carry the evening home");
        assert_eq!(
            b.edits_said(),
            ["scene 1 (s1): the script was rewritten", "scene 2 (s2): asks for a change: make it night", "scene 3 (finale): approved", "the film's title is now \"Night Drive II\""]
        );
        let nudge = b.nudge(true).unwrap();
        assert!(nudge.starts_with("I changed the storyboard: 4 changes (scene 1 (s1), scene 2 (s2), scene 3 (finale), …).") && nudge.contains("storyboard_get"), "{nudge}");
        assert!(b.nudge(false).unwrap().contains("slop/storyboard.json"));
        let agent = b.for_agent(project);
        assert_eq!((agent["scenes"][1]["userAsks"].as_str(), agent["changedByUser"].as_array().map(Vec::len), agent["scenes"][0]["n"].as_u64()), (Some("make it night"), Some(4), Some(1)));
        // read back from disk as it was written, then the agent's read clears the news
        let mut read = load(project).unwrap();
        assert_eq!(read, b);
        assert!(read.take_edits() && read.edits.is_empty() && read.nudge(true).is_none());
        // the agent did what was asked: the note goes, and with it nothing is left to tell
        let (_, b) = update(project, |b| (true, b.apply_scene(&json!({ "scene": "s2", "direction": "Night now.", "resolveNote": true }), project))).unwrap();
        assert!(b.scenes[1].note.is_empty() && !b.edits.iter().any(|e| e.what == "note"));
        // asking for a change takes a sign-off back, and one lone change is said in full
        let (_, b) = update(project, |b| {
            b.take_edits();
            (b.set_text(Some(2), Field::Note, "hold the last frame longer"), ())
        })
        .unwrap();
        assert!(!b.scenes[2].approved);
        assert_eq!(b.nudge(true).unwrap(), "I changed the storyboard: scene 3 (finale): asks for a change: hold the last frame longer. Read it with storyboard_get and carry on from what it says now.");
        assert!(!b.clone().pick_keyframe(0, 3), "a keyframe that isn't there can't be chosen");
        // a request written as a sentence keeps one full stop
        let mut c = b.clone();
        c.take_edits();
        c.set_text(Some(2), Field::Note, "Give the alligator a gold chain.");
        assert_eq!(c.nudge(true).unwrap(), "I changed the storyboard: scene 3 (finale): asks for a change: Give the alligator a gold chain. Read it with storyboard_get and carry on from what it says now.");
    }

    #[test]
    fn a_board_someone_else_wrote_is_read_leniently() {
        let _sb = crate::util::test_env::sandbox();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(DIR)).unwrap();
        // an agent that wrote the file by hand: a byte order mark, a few fields, others missing
        std::fs::write(path(dir.path()), "\u{feff}{ \"title\": \"Hand made\", \"scenes\": [ { \"id\": \"a\", \"startMs\": 0, \"endMs\": 6000, \"direction\": \"A door opens.\", \"keyframes\": [ { \"path\": \"slop/k/a.png\" } ] } ] }").unwrap();
        let b = load(dir.path()).unwrap();
        assert_eq!((b.title.as_str(), b.scenes.len(), b.scenes[0].stage(), b.rev), ("Hand made", 1, Stage::Keyframed, 0));
        assert_eq!(b.scenes[0].chosen_keyframe().map(|k| k.path.as_str()), Some("slop/k/a.png"));
        assert_eq!(full_path(dir.path(), "slop/k/a.png"), dir.path().join("slop/k/a.png"));
        std::fs::write(path(dir.path()), "not json").unwrap();
        assert_eq!(load(dir.path()), None);
    }
}
