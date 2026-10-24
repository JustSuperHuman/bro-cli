//! PeonPing (<https://peonping.com>): voice lines from games when an agent starts, finishes, fails or needs you,
//! from sound packs in the CESP format (`openpeon.json`, <https://github.com/PeonPing/openpeon>). bro plays them
//! itself instead of running peon-ping's scripts, so a pack sounds the same everywhere: MP3, OGG and FLAC packs
//! play on Windows without ffmpeg, the volume is honoured for WAV there too, and nothing needs bash or PowerShell.
//!
//! * **Where.** peon-ping's own folder when it's installed (`$CLAUDE_PEON_DIR`, else `~/.claude/hooks/peon-ping`):
//!   bro then shares its `config.json`, its packs and its `.state.json`, so `peon packs use …` and bro agree.
//!   Without it, bro keeps the same layout in `~/.bro/peon`.
//! * **Hooks.** With `[peon] on` in `~/.bro/v2.toml`, every session bro starts runs `bro hook peon <harness>` on
//!   its events ([`wire`]): Claude Code through the launch's `--settings`, Codex through `-c hooks.…` (already
//!   trusted, so it never asks), Pi and omp through an extension. Nothing is written into their own configs; a
//!   login whose config already runs peon-ping keeps it, so nothing plays twice.
//! * **The hook** ([`run_hook`]) maps the event to a CESP category the way peon-ping does (debounced stops, the
//!   "annoyed" easter egg on rapid prompts, per-session / per-path / per-harness packs, no sound twice in a row),
//!   then `bro hook peon play` plays it in a process of its own so the agent never waits.
//! * **Packs** ([`pack`], [`registry`]): what's installed, and the registry at peonping.github.io to browse,
//!   preview and install from (each file checked against its SHA-256).
use crate::util::{atomic_write, env_path, is_within, read_json};
use crate::{Harness, paths};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub mod pack;
pub mod registry;
pub mod wire;

#[cfg(test)]
mod tests;

/// `[peon]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PeonSettings {
    /// New sessions bro starts play PeonPing sounds. Off by default: you turn it on in Sounds.
    pub on: bool,
    /// Which harnesses get the hooks.
    pub claude: bool,
    pub codex: bool,
    pub pi: bool,
    pub omp: bool,
}

impl Default for PeonSettings {
    fn default() -> Self {
        Self { on: false, claude: true, codex: true, pi: true, omp: true }
    }
}

impl PeonSettings {
    /// Sessions of `h` get the hooks.
    pub fn wires(&self, h: Harness) -> bool {
        self.on && self.harness(h)
    }
    /// `h`'s own switch (whether or not sounds are on).
    pub fn harness(&self, h: Harness) -> bool {
        match h {
            Harness::Claude => self.claude,
            Harness::Codex => self.codex,
            Harness::Pi => self.pi,
            Harness::Omp => self.omp,
        }
    }
    pub fn set_harness(&mut self, h: Harness, on: bool) {
        match h {
            Harness::Claude => self.claude = on,
            Harness::Codex => self.codex = on,
            Harness::Pi => self.pi = on,
            Harness::Omp => self.omp = on,
        }
    }
}

/// The CESP categories, in the order Sounds lists them, with what each means.
pub const CATEGORIES: [(&str, &str); 8] = [
    ("session.start", "Session starts"),
    ("task.acknowledge", "Task acknowledged"),
    ("task.complete", "Task done"),
    ("input.required", "Needs your input"),
    ("task.error", "Something failed"),
    ("resource.limit", "Context compacting"),
    ("user.spam", "Rapid prompts"),
    ("session.end", "Session ends"),
];

/// The pack a fresh config starts on (peon-ping's own default).
pub const DEFAULT_PACK: &str = "peon";

/// peon-ping's own folder: `~/.claude/hooks/peon-ping`.
pub fn peon_ping_dir() -> PathBuf {
    paths::claude_local_dir().join("hooks").join("peon-ping")
}

/// peon-ping itself is installed there (its hook script, for Claude Code): its CLI shares bro's config.
pub fn peon_ping_installed() -> bool {
    let d = env_path("CLAUDE_PEON_DIR").unwrap_or_else(peon_ping_dir);
    d.join("peon.sh").is_file() || d.join("peon.ps1").is_file()
}

/// Where the config, the packs and the state live: `$CLAUDE_PEON_DIR`, else peon-ping's folder when it has a
/// config or packs, else `~/.bro/peon`.
pub fn home() -> PathBuf {
    if let Some(d) = env_path("CLAUDE_PEON_DIR") {
        return d;
    }
    let peon = peon_ping_dir();
    if peon.join("config.json").is_file() || peon.join("packs").is_dir() {
        return peon;
    }
    paths::bro_dir().join("peon")
}

pub fn packs_dir() -> PathBuf {
    home().join("packs")
}

fn config_path() -> PathBuf {
    home().join("config.json")
}

fn state_path() -> PathBuf {
    home().join(".state.json")
}

/// peon-ping's `config.json`, kept as it is on disk (every key bro doesn't know survives a save, numbers keep
/// their form), read and changed through the accessors.
#[derive(Debug, Clone, PartialEq)]
pub struct PeonConfig {
    raw: Map<String, Value>,
}

impl Default for PeonConfig {
    /// What a new config holds: on, half volume, the peon pack, every category.
    fn default() -> Self {
        let categories: Map<String, Value> = CATEGORIES.iter().filter(|(c, _)| *c != "session.end").map(|(c, _)| (c.to_string(), Value::Bool(true))).collect();
        let raw = json!({ "enabled": true, "volume": 0.5, "default_pack": DEFAULT_PACK, "categories": categories });
        PeonConfig { raw: raw.as_object().cloned().unwrap_or_default() }
    }
}

impl PeonConfig {
    /// The config in [`home`]; a fresh one when there's none (or it isn't JSON).
    pub fn load() -> PeonConfig {
        match read_json(&config_path()) {
            Some(Value::Object(raw)) => PeonConfig { raw },
            _ => PeonConfig::default(),
        }
    }

    /// Atomically, like peon-ping itself.
    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.raw).map_err(std::io::Error::other)?;
        atomic_write(&config_path(), text.as_bytes())
    }

    pub fn from_value(v: Value) -> PeonConfig {
        PeonConfig { raw: v.as_object().cloned().unwrap_or_default() }
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.raw.get(key).and_then(Value::as_f64)
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.raw.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    fn list(&self, key: &str) -> &[Value] {
        self.raw.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn enabled(&self) -> bool {
        self.raw.get("enabled").and_then(Value::as_bool).unwrap_or(true)
    }
    pub fn set_enabled(&mut self, on: bool) {
        self.raw.insert("enabled".into(), Value::Bool(on));
    }

    /// 0..1 (half when unset).
    pub fn volume(&self) -> f32 {
        self.num("volume").unwrap_or(0.5).clamp(0.0, 1.0) as f32
    }
    pub fn set_volume(&mut self, v: f32) {
        // in hundredths, as f64 so the file reads 0.46 rather than 0.46000000834465027
        let v = (f64::from(v.clamp(0.0, 1.0)) * 100.0).round() / 100.0;
        self.raw.insert("volume".into(), json!(v));
    }

    /// The pack sounds come from when nothing more specific picks one (`default_pack`, else the legacy
    /// `active_pack`).
    pub fn default_pack(&self) -> Option<&str> {
        self.str("default_pack").or_else(|| self.str("active_pack"))
    }
    /// Use `name` from now on: the default, and the only one in a rotation (peon-ping's `packs use`).
    pub fn use_pack(&mut self, name: &str) {
        self.raw.insert("default_pack".into(), Value::String(name.into()));
        if self.raw.contains_key("active_pack") {
            self.raw.insert("active_pack".into(), Value::String(name.into()));
        }
        if !self.list("pack_rotation").is_empty() {
            self.raw.insert("pack_rotation".into(), json!([name]));
        }
    }

    /// A category plays unless it's switched off.
    pub fn category_on(&self, cat: &str) -> bool {
        self.raw.get("categories").and_then(|c| c.get(cat)).and_then(Value::as_bool).unwrap_or(true)
    }
    pub fn set_category(&mut self, cat: &str, on: bool) {
        let cats = self.raw.entry("categories").or_insert_with(|| json!({}));
        if !cats.is_object() {
            *cats = json!({});
        }
        if let Some(m) = cats.as_object_mut() {
            m.insert(cat.into(), Value::Bool(on));
        }
    }

    /// `random` (default), `round-robin`, `session_override` / `agentskill`.
    fn rotation_mode(&self) -> &str {
        self.str("pack_rotation_mode").unwrap_or("random")
    }
    fn rotation(&self) -> Vec<&str> {
        self.list("pack_rotation").iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).collect()
    }
    /// `[{pattern, pack}]`, first match wins.
    fn path_rules(&self) -> Vec<(&str, &str)> {
        self.list("path_rules").iter().filter_map(|r| Some((r.get("pattern")?.as_str()?, r.get("pack")?.as_str()?))).collect()
    }
    /// `[{ide, pack}]`: a pack per harness ("claude", "codex", …).
    pub fn ide_pack(&self, ide: &str) -> Option<&str> {
        self.list("ide_rules").iter().find(|r| r.get("ide").and_then(Value::as_str).is_some_and(|i| i.eq_ignore_ascii_case(ide))).and_then(|r| r.get("pack")?.as_str())
    }
    fn exclude_dirs(&self) -> Vec<&str> {
        self.list("exclude_dirs").iter().filter_map(Value::as_str).collect()
    }
}

/// `.state.json`: the last sound per category, stop and prompt times, per-session packs. Shared with peon-ping's
/// scripts, so unknown keys stay.
struct State {
    raw: Map<String, Value>,
    dirty: bool,
}

impl State {
    fn load() -> State {
        let raw = match read_json(&state_path()) {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        State { raw, dirty: false }
    }
    fn save(&self) {
        if self.dirty
            && let Ok(text) = serde_json::to_string(&self.raw)
        {
            let _ = atomic_write(&state_path(), text.as_bytes());
        }
    }
    fn num(&self, key: &str) -> f64 {
        self.raw.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }
    fn set(&mut self, key: &str, v: Value) {
        self.raw.insert(key.into(), v);
        self.dirty = true;
    }
}

/// `.state.lock` beside the state while one hook reads and writes it; removed on drop. A lock older than a few
/// seconds is a hook that died holding it, and is taken over. Without it within a second, the hook goes ahead
/// (a lost debounce is better than a silent agent).
struct StateLock(Option<PathBuf>);

impl StateLock {
    fn take() -> StateLock {
        use std::time::{Duration, Instant};
        let path = home().join(".state.lock");
        let _ = std::fs::create_dir_all(home());
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return StateLock(Some(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = path.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(5));
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                }
                Err(_) => return StateLock(None),
            }
            if Instant::now() > until {
                return StateLock(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// One hook event, whichever harness sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Claude Code's name for it: `SessionStart`, `Stop`, `PermissionRequest`…
    pub name: String,
    pub session: String,
    pub cwd: Option<PathBuf>,
    pub notification_type: Option<String>,
    /// `SessionStart`'s source: `startup`, `resume`, `clear`, `compact`
    pub source: Option<String>,
}

/// The event in a hook's stdin. Claude Code, Codex and bro's Pi / omp extension send Claude's shape; Cursor-style
/// camelCase names and keys are understood too.
pub fn parse_event(harness: Harness, input: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(input.trim_start_matches('\u{feff}')).ok()?;
    let get = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string));
    let raw = get(&["hook_event_name", "hookEventName"])?;
    let name = match raw.as_str() {
        "sessionStart" | "session_start" => "SessionStart",
        "sessionEnd" | "session_end" => "SessionEnd",
        "beforeSubmitPrompt" | "userPromptSubmit" | "user_prompt_submit" => "UserPromptSubmit",
        "stop" | "subagentStop" => "Stop",
        "subagentStart" => "SubagentStart",
        "preCompact" | "pre_compact" => "PreCompact",
        "notification" => "Notification",
        "permissionRequest" => "PermissionRequest",
        "postToolUseFailure" => "PostToolUseFailure",
        other => other,
    }
    .to_string();
    let id = get(&["session_id", "sessionId", "conversation_id"]).unwrap_or_else(|| "default".into());
    // Claude's ids are its own; the others carry the harness, like peon-ping's adapters
    let session = match harness {
        Harness::Claude => id,
        h if id.starts_with(&format!("{}-", h.label())) => id,
        h => format!("{}-{id}", h.label()),
    };
    Some(Event {
        name,
        session,
        cwd: get(&["cwd"]).map(PathBuf::from),
        notification_type: get(&["notification_type", "notificationType"]),
        source: get(&["source"]),
    })
}

/// What a hook decided to play.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub path: PathBuf,
    /// 0..1
    pub volume: f32,
    pub category: &'static str,
    pub pack: String,
}

/// The category `ev` plays, if any, updating the state it keeps (debounces, prompt times).
fn category(ev: &Event, cfg: &PeonConfig, st: &mut State, now: f64) -> Option<&'static str> {
    let sid = ev.session.as_str();
    match ev.name.as_str() {
        "SessionStart" => {
            // a compaction restarts the session; PreCompact already spoke
            if ev.source.as_deref() == Some("compact") {
                return None;
            }
            // several sessions opening at once greet once
            let cooldown = cfg.num("session_start_cooldown_seconds").unwrap_or(30.0);
            let last = st.num("last_session_start_time");
            st.set("last_session_start_time", json!(now as i64));
            (cooldown <= 0.0 || now - last >= cooldown).then_some("session.start")
        }
        "Stop" => {
            // Stop can come in bursts (subagents, retries): one sound per 5 s
            let last = st.num("last_stop_time");
            st.set("last_stop_time", json!(now as i64));
            if now - last < 5.0 {
                return None;
            }
            // a quick answer needs no fanfare
            let silent = cfg.num("silent_window_seconds").unwrap_or(0.0);
            let started = st.raw.get("prompt_start_times").and_then(|m| m.get(sid)).and_then(Value::as_f64);
            if silent > 0.0 && started.is_some_and(|s| now - s < silent) {
                return None;
            }
            Some("task.complete")
        }
        "Notification" => match ev.notification_type.as_deref() {
            // PermissionRequest plays for the prompt, and Stop already did for idle
            Some("permission_prompt" | "idle_prompt") => None,
            Some("elicitation_dialog") => Some("input.required"),
            _ => Some("task.complete"),
        },
        "PermissionRequest" => Some("input.required"),
        "UserPromptSubmit" => {
            let threshold = cfg.num("annoyed_threshold").unwrap_or(3.0).max(1.0) as usize;
            let window = cfg.num("annoyed_window_seconds").unwrap_or(10.0);
            let mut all = st.raw.get("prompt_timestamps").and_then(Value::as_object).cloned().unwrap_or_default();
            let mut recent: Vec<f64> = all.get(sid).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).filter(|t| now - t < window).collect()).unwrap_or_default();
            recent.push(now);
            let spam = recent.len() >= threshold;
            all.insert(sid.into(), json!(recent.iter().map(|t| *t as i64).collect::<Vec<_>>()));
            st.set("prompt_timestamps", Value::Object(trim_sessions(all)));
            let mut starts = st.raw.get("prompt_start_times").and_then(Value::as_object).cloned().unwrap_or_default();
            starts.insert(sid.into(), json!(now as i64));
            st.set("prompt_start_times", Value::Object(trim_sessions(starts)));
            spam.then_some("user.spam")
        }
        "PostToolUseFailure" => Some("task.error"),
        "SubagentStart" => Some("task.acknowledge"),
        "PreCompact" => Some("resource.limit"),
        "SessionEnd" => Some("session.end"),
        _ => None,
    }
}

/// A per-session map keeps its newest 200 sessions (the oldest were inserted first).
fn trim_sessions(mut map: Map<String, Value>) -> Map<String, Value> {
    const KEEP: usize = 200;
    if map.len() > KEEP {
        let drop: Vec<String> = map.keys().take(map.len() - KEEP).cloned().collect();
        for k in drop {
            map.remove(&k);
        }
    }
    map
}

/// `*` and `?` glob, case-insensitive (PowerShell's `-like`, which peon-ping's rules use), `/` and `\` alike.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let (p, t): (Vec<char>, Vec<char>) = (norm(&expand_home(pattern)).chars().collect(), norm(text).chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn expand_home(p: &str) -> String {
    match p.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("{}{rest}", crate::util::home_dir().display()),
        _ => p.to_string(),
    }
}

/// `cwd` is in an excluded folder (a glob, or a folder and everything under it).
fn excluded(cfg: &PeonConfig, cwd: &Path) -> bool {
    let text = cwd.to_string_lossy();
    cfg.exclude_dirs().into_iter().any(|pat| glob(pat, &text) || (!pat.contains(['*', '?']) && is_within(Path::new(&expand_home(pat)), cwd)))
}

/// The pack for this event: the session's own (when the rotation mode allows one), a path rule, the harness's
/// rule, the rotation, the default; skipping any that isn't installed.
fn pick_pack(cfg: &PeonConfig, st: &mut State, ev: &Event, harness: Harness, installed: &dyn Fn(&str) -> bool) -> Option<String> {
    let ok = |p: &str| pack::valid_name(p) && installed(p);
    let mode = cfg.rotation_mode();
    if matches!(mode, "session_override" | "agentskill") {
        let sessions = st.raw.get("session_packs").and_then(Value::as_object);
        let of = |v: &Value| v.as_str().or_else(|| v.get("pack")?.as_str()).map(str::to_string);
        if let Some(p) = sessions.and_then(|s| s.get(&ev.session).or_else(|| s.get("default"))).and_then(of).filter(|p| ok(p)) {
            return Some(p);
        }
    }
    if let Some(cwd) = &ev.cwd {
        let text = cwd.to_string_lossy();
        if let Some((_, p)) = cfg.path_rules().into_iter().find(|(pat, p)| glob(pat, &text) && ok(p)) {
            return Some(p.to_string());
        }
    }
    if let Some(p) = cfg.ide_pack(harness.label()).filter(|p| ok(p)) {
        return Some(p.to_string());
    }
    let rotation: Vec<&str> = cfg.rotation().into_iter().filter(|p| ok(p)).collect();
    if !rotation.is_empty() && !matches!(mode, "session_override" | "agentskill") {
        let i = if mode == "round-robin" {
            let i = st.num("rotation_index") as usize;
            st.set("rotation_index", json!(i + 1));
            i % rotation.len()
        } else {
            rand::random_range(0..rotation.len())
        };
        return Some(rotation[i].to_string());
    }
    if let Some(p) = cfg.default_pack().filter(|p| ok(p)) {
        return Some(p.to_string());
    }
    if ok(DEFAULT_PACK) {
        return Some(DEFAULT_PACK.into());
    }
    pack::installed_names().into_iter().next()
}

/// The hook's whole decision for `input` (the event JSON) from `harness` at `now` (unix seconds): what to play,
/// if anything. Reads the config, the packs and the state, and saves the state.
pub fn decide(harness: Harness, input: &str, now: f64) -> Option<Play> {
    let ev = parse_event(harness, input)?;
    let cfg = PeonConfig::load();
    if !cfg.enabled() || ev.cwd.as_deref().is_some_and(|c| excluded(&cfg, c)) {
        return None;
    }
    // hooks for one turn run at once (a prompt and a stop, a subagent): one at a time through the state
    let _lock = StateLock::take();
    let mut st = State::load();
    let cat = category(&ev, &cfg, &mut st, now);
    let play = cat.filter(|c| cfg.category_on(c)).and_then(|cat| {
        let packs = packs_dir();
        let name = pick_pack(&cfg, &mut st, &ev, harness, &|p| packs.join(p).is_dir())?;
        let dir = packs.join(&name);
        let manifest = pack::load_manifest(&dir)?;
        let key = format!("last_{cat}");
        let last = st.raw.get(&key).and_then(Value::as_str).unwrap_or_default().to_string();
        let sound = manifest.pick(cat, &last)?;
        let path = pack::sound_path(&dir, &sound.file)?;
        st.set(&key, Value::String(pack::base_name(&sound.file).to_string()));
        Some(Play { path, volume: cfg.volume(), category: cat, pack: name })
    });
    st.save();
    play
}

/// `bro hook peon <harness>`: the sound for the event on stdin (`input`), or None.
pub fn run_hook(args: &[String], input: &str) -> Option<Play> {
    let harness = args.first().and_then(|h| Harness::ALL.into_iter().find(|x| x.label() == h)).unwrap_or(Harness::Claude);
    decide(harness, input, crate::util::now_ms() as f64 / 1000.0)
}

/// `bro hook peon play <file> <volume>`'s arguments.
pub fn play_args(path: &Path, volume: f32) -> Vec<String> {
    vec!["hook".into(), "peon".into(), "play".into(), path.to_string_lossy().into_owned(), format!("{volume:.2}")]
}

/// Plays `play` from a process of its own (`exe`: this bro), so the hook returns at once and the agent never
/// waits on the sound. It outlives the hook and the agent's process group.
pub fn spawn_player(exe: &Path, play: &Play) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.args(play_args(&play.path, play.volume)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // out of the agent's job when the job lets it go, so closing the agent doesn't cut the line short
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

/// The arguments of `bro hook peon play <file> <volume>`: the file and the volume (0..1).
pub fn parse_play(args: &[String]) -> Option<(PathBuf, f32)> {
    let file = PathBuf::from(args.first()?);
    let vol = args.get(1).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5).clamp(0.0, 1.0);
    Some((file, vol))
}

/// A sound from the default pack for `category` (else any it has), for Sounds' test button.
pub fn sample(category: Option<&str>) -> Option<Play> {
    let cfg = PeonConfig::load();
    let packs = packs_dir();
    let name = cfg.default_pack().filter(|p| pack::valid_name(p) && packs.join(p).is_dir()).map(str::to_string).or_else(|| pack::installed_names().into_iter().next())?;
    let dir = packs.join(&name);
    let manifest = pack::load_manifest(&dir)?;
    let cat = category.and_then(|c| CATEGORIES.iter().find(|(k, _)| *k == c)).map(|(k, _)| *k).unwrap_or("session.start");
    let sound = manifest.pick(cat, "").or_else(|| manifest.categories.iter().find_map(|(_, s)| s.first().cloned()))?;
    Some(Play { path: pack::sound_path(&dir, &sound.file)?, volume: cfg.volume(), category: cat, pack: name })
}
