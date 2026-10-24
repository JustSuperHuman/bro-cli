//! Claude Code's messages between sessions (`ListAgents` / `SendMessage`), kept to one project.
//!
//! Claude Code lets a session find and message every live session that shares its config dir, whatever each one
//! works on: they all register in `<config dir>/sessions/<pid>.json` (`cwd`, `name`, `messagingSocketPath`), and
//! nothing there knows about projects. So bro hands every Claude launch two hooks that run bro itself, in exec
//! form with no shell (`bro hook peers <scope> <cwd>`):
//!
//! * `PreToolUse` on `SendMessage`: a send to a session on this machine that's out of reach is denied, with a
//!   reason Claude passes on to you;
//! * `PostToolUse` on `ListAgents`: those sessions leave the listing Claude reads (`updatedToolOutput`).
//!
//! A listing row's `[ref]` is the first six hex digits of SHA-256(`session:<inbox>`), so each row maps back to its
//! registry record and that record's `cwd`. In reach under [`Scope::Project`]: sessions in the same repository
//! (its worktrees too, see [`crate::projects::repository_for`]) and bro's orchestrator, which spans projects by
//! design. Under [`Scope::Off`]: no other session, and the `peers` knob also refuses messages coming in. Subagents,
//! teammates and `main` aren't in the registry and pass untouched. The orchestrator's own launch gets no hooks.
//!
//! The scope is the `peers` knob in `[claude_env]` (see [`crate::claude_env`]). A failing hook lets the call go
//! ahead, so a missing bro never breaks Claude's messaging to its own subagents.
use crate::claude_env::ClaudeEnvSettings;
use crate::projects::{ProjectKey, repository_for};
use crate::util::{is_within, same_path, short_hash};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The `[claude_env]` knob that picks the [`Scope`].
pub const KNOB: &str = "peers";

/// Which other Claude sessions on this machine a session may find and message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Sessions in the same repository, and bro's orchestrator.
    Project,
    /// None.
    Off,
}

impl Scope {
    pub fn id(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Off => "off",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        [Scope::Project, Scope::Off].into_iter().find(|s| s.id() == id)
    }

    /// What `env` picks. None: Claude's default, every session on the machine.
    pub fn of(env: &ClaudeEnvSettings) -> Option<Self> {
        let k = crate::claude_env::knob(KNOB)?;
        Self::parse(k.options[env.choice(k).0].id)
    }
}

/// Adds the two hooks for a session opened in `cwd` to a launch's `--settings`, next to whatever hooks are there.
pub fn add_hooks(settings: &mut Map<String, Value>, scope: Scope, bro_bin: &Path, cwd: &Path) {
    let hook = json!({
        "type": "command",
        "command": bro_bin.to_string_lossy(),
        "args": ["hook", "peers", scope.id(), cwd.to_string_lossy()],
        // a run takes a tenth of a second; the first after an update can wait on a virus scan of the new bro
        "timeout": 30,
    });
    let Some(hooks) = settings.entry("hooks").or_insert_with(|| json!({})).as_object_mut() else { return };
    for (event, tool) in [("PreToolUse", "SendMessage"), ("PostToolUse", "ListAgents")] {
        if let Some(list) = hooks.entry(event).or_insert_with(|| json!([])).as_array_mut() {
            list.push(json!({ "matcher": tool, "hooks": [hook.clone()] }));
        }
    }
}

/// `bro hook peers <scope> [<cwd>]`: the answer to the hook event on stdin (`input`, Claude Code's JSON) for a
/// session opened in `cwd` (else the event's own `cwd`). None: nothing to say, the call goes ahead as it is.
pub fn run_hook(args: &[String], input: &str) -> Option<String> {
    let scope = Scope::parse(args.first()?)?;
    let event: Value = serde_json::from_str(input).ok()?;
    let cwd = args.get(1).map(PathBuf::from).or_else(|| event["cwd"].as_str().map(PathBuf::from))?;
    let reach = Reach::new(scope, &cwd, &crate::orchestrator::workspace(), event["session_id"].as_str(), registry(&sessions_dir()));
    let out = match (event["hook_event_name"].as_str()?, event["tool_name"].as_str()?) {
        ("PreToolUse", "SendMessage") => {
            let why = reach.refusal(event["tool_input"]["to"].as_str()?)?;
            json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": why } })
        }
        ("PostToolUse", "ListAgents") => {
            let listing = reach.listing(&event["tool_response"])?;
            json!({ "hookSpecificOutput": { "hookEventName": "PostToolUse", "updatedToolOutput": listing } })
        }
        _ => return None,
    };
    Some(out.to_string())
}

/// SHA-256(`session:<inbox>`) in hex, for the session listening at `inbox`: `ListAgents` shows its first six digits
/// as the session's `[ref]`, and `SendMessage` takes any longer prefix too.
fn digest(inbox: &str) -> String {
    short_hash(&format!("session:{inbox}"), 64)
}

/// Claude Code's registry of the login this session runs in: `$CLAUDE_CONFIG_DIR/sessions`, else `~/.claude`'s.
fn sessions_dir() -> PathBuf {
    crate::util::env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(crate::paths::claude_local_dir).join("sessions")
}

/// A session's record in the registry (`<pid>.json`), as far as bro reads it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    name: String,
    cwd: PathBuf,
    session_id: Option<String>,
    /// Absent for a session that can't take messages (bare mode).
    messaging_socket_path: Option<String>,
}

/// A session that can take messages: live, or a record its session left behind.
#[derive(Debug, Clone)]
struct Peer {
    name: String,
    cwd: PathBuf,
    session_id: Option<String>,
    inbox: String,
    /// SHA-256(`session:<inbox>`) in hex: a ref is any prefix of it (six digits in a listing).
    digest: String,
}

impl Peer {
    /// The ref `ListAgents` shows for it.
    fn reference(&self) -> &str {
        &self.digest[..6]
    }
}

fn registry(dir: &Path) -> Vec<Peer> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Record>(&std::fs::read(e.path()).ok()?).ok())
        .filter_map(|r| {
            let inbox = r.messaging_socket_path.filter(|s| !s.is_empty())?;
            Some(Peer { name: r.name, cwd: r.cwd, session_id: r.session_id, digest: digest(&inbox), inbox })
        })
        .collect()
}

/// A `SendMessage` recipient as Claude writes it: `name`, `name [ref]`, or a session's inbox address.
struct Target {
    name: String,
    reference: Option<String>,
}

impl Target {
    fn parse(to: &str) -> Self {
        let to = to.trim();
        if let Some(open) = to.rfind('[')
            && let Some(inner) = to[open + 1..].strip_suffix(']')
            && inner.len() >= 4
            && inner.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Target { name: bare_name(&to[..open]), reference: Some(inner.to_ascii_lowercase()) };
        }
        Target { name: bare_name(to), reference: None }
    }

    /// `p` is (or may be) the session this names. A ref settles it; a bare name matches regardless of case, as
    /// a send that slipped past on case would still be delivered.
    fn names(&self, p: &Peer) -> bool {
        if let Some(r) = &self.reference {
            return p.digest.starts_with(r.as_str());
        }
        let address = self.name.strip_prefix("uds:").unwrap_or(&self.name);
        (!p.name.is_empty() && p.name.eq_ignore_ascii_case(&self.name)) || p.inbox.eq_ignore_ascii_case(address)
    }
}

/// `@name`, `"a name"` or `name` as the name alone.
fn bare_name(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('@').unwrap_or(s);
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s).trim().to_string()
}

/// Who a session may reach.
struct Reach {
    scope: Scope,
    /// The repository the session was opened in.
    home: ProjectKey,
    /// bro's orchestrator works here.
    orchestrator: PathBuf,
    /// The session's own id: its record is never out of reach.
    me: Option<String>,
    peers: Vec<Peer>,
}

impl Reach {
    fn new(scope: Scope, cwd: &Path, orchestrator: &Path, me: Option<&str>, peers: Vec<Peer>) -> Self {
        let me = me.filter(|s| !s.is_empty()).map(str::to_string);
        Reach { scope, home: repository_for(cwd), orchestrator: orchestrator.to_path_buf(), me, peers }
    }

    fn reaches(&self, p: &Peer) -> bool {
        if self.me.is_some() && p.session_id == self.me {
            return true;
        }
        match self.scope {
            Scope::Off => false,
            Scope::Project => {
                same_path(&p.cwd, &self.orchestrator) || is_within(&self.orchestrator, &p.cwd) || repository_for(&p.cwd).key == self.home.key
            }
        }
    }

    /// Why a send to `to` mustn't go: it names sessions on this machine and none of them is in reach. None: it may
    /// go (one is in reach, or `to` isn't a session here: a subagent, a teammate, `main`, a session elsewhere).
    fn refusal(&self, to: &str) -> Option<String> {
        let target = Target::parse(to);
        let named: Vec<&Peer> = self.peers.iter().filter(|p| target.names(p)).collect();
        if named.iter().any(|p| self.reaches(p)) {
            return None;
        }
        let p = named.first()?;
        let name = if p.name.is_empty() { to.trim() } else { p.name.as_str() };
        let ask = "If your user wants it delivered, tell them what you would have sent.";
        Some(match self.scope {
            Scope::Project => format!(
                "Not sent: bro keeps Claude sessions to their own project. {name} works in {}; this session works in {}. Don't relay it through another session. {ask}",
                p.cwd.display(),
                self.home.root.display()
            ),
            Scope::Off => format!("Not sent: messages between Claude sessions are off in bro, so {name} can't be reached. {ask}"),
        })
    }

    /// The `ListAgents` result (`response`: `listing`, the text Claude reads, plus its `sections` of rows) without
    /// the sessions out of reach. None: every row stays.
    fn listing(&self, response: &Value) -> Option<Value> {
        let text = response.get("listing")?.as_str()?;
        let by_ref: HashMap<&str, &Peer> = self.peers.iter().map(|p| (p.reference(), p)).collect();
        let mut hidden: HashSet<String> = HashSet::new();
        let mut out = response.clone();
        if let Some(sections) = out.get_mut("sections").and_then(Value::as_array_mut) {
            for section in sections.iter_mut() {
                let Some(rows) = section.get_mut("rows").and_then(Value::as_array_mut) else { continue };
                let before = rows.len();
                rows.retain(|row| {
                    let reference = row["ref"].as_str().unwrap_or_default();
                    let out_of_reach = match by_ref.get(reference) {
                        Some(p) => !self.reaches(p),
                        // under Off, sessions on other machines and in the cloud go too
                        None => self.scope == Scope::Off && matches!(row["type"].as_str(), Some("cloud" | "remote_control")),
                    };
                    if out_of_reach {
                        hidden.insert(reference.to_string());
                    }
                    !out_of_reach
                });
                let gone = before - rows.len();
                if gone > 0
                    && let Some(total) = section.get("total").and_then(Value::as_u64)
                {
                    section["total"] = json!(total.saturating_sub(gone as u64));
                }
            }
        }
        // rows the sections didn't carry still show in the text
        for (reference, p) in &by_ref {
            if !hidden.contains(*reference) && text.contains(&format!("[{reference}]")) && !self.reaches(p) {
                hidden.insert(reference.to_string());
            }
        }
        if hidden.is_empty() {
            return None;
        }
        out["listing"] = Value::String(self.filtered_text(text, &hidden));
        Some(out)
    }

    /// `text` without the rows of `hidden` refs (and their indented continuation lines), each section's count
    /// brought down to match, an emptied section dropped, and a note saying what bro left out.
    fn filtered_text(&self, text: &str, hidden: &HashSet<String>) -> String {
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut lines: Vec<Option<String>> = vec![];
        // per section heading ("Peer sessions (16):"): its line and how many rows went
        let mut heading: Option<usize> = None;
        let mut dropped: HashMap<usize, u64> = HashMap::new();
        let mut skipping: Option<usize> = None;
        let mut gone = 0;
        for line in text.lines() {
            if let Some(depth) = skipping {
                if !line.trim().is_empty() && indent(line) > depth {
                    continue;
                }
                skipping = None;
            }
            if indent(line) == 0 && count_of(line).is_some() {
                heading = Some(lines.len());
            }
            // a row is indented and leads with `name [ref]`: the session's own line ("This session is …") isn't
            let row_ref = (indent(line) > 0).then(|| first_ref(line)).flatten();
            if row_ref.is_some_and(|r| hidden.contains(r)) {
                skipping = Some(indent(line));
                gone += 1;
                if let Some(h) = heading {
                    *dropped.entry(h).or_default() += 1;
                }
                continue;
            }
            lines.push(Some(line.to_string()));
        }
        for (h, n) in dropped {
            let Some(line) = lines[h].as_deref() else { continue };
            let Some((count, at)) = count_of(line) else { continue };
            let left = count.saturating_sub(n);
            lines[h] = if left == 0 { None } else { Some(format!("{}({left}):", &line[..at])) };
        }
        let mut out: Vec<String> = lines.into_iter().flatten().collect();
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        let sessions = if gone == 1 { "1 session".to_string() } else { format!("{gone} sessions") };
        let note = match self.scope {
            Scope::Project => format!(
                "({sessions} working in other projects left out by bro: sessions here only reach sessions in {}.)",
                self.home.name
            ),
            Scope::Off => format!("({sessions} left out by bro: messages between Claude sessions are off.)"),
        };
        if gone > 0 {
            out.push(String::new());
            out.push(note);
        }
        out.join("\n")
    }
}

/// A section heading's count and where its `(` starts: "Peer sessions (16):" is `(16, 14)`.
fn count_of(line: &str) -> Option<(u64, usize)> {
    let body = line.trim_end().strip_suffix("):")?;
    let open = body.rfind('(')?;
    Some((body[open + 1..].parse().ok()?, open))
}

/// The first `[hex]` on a line, without its brackets.
fn first_ref(line: &str) -> Option<&str> {
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let close = after.find(']')?;
        let inner = &after[..close];
        if inner.len() >= 4 && inner.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(inner);
        }
        rest = &after[close + 1..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    /// Repositories under a root: `app` (with a worktree inside it, the way Claude Code makes them), `other`, and
    /// the orchestrator's workspace, with sessions registered in each.
    struct World {
        app: PathBuf,
        other: PathBuf,
        orchestrator: PathBuf,
        peers: Vec<Peer>,
    }

    fn inbox(name: &str) -> String {
        format!(r"\\.\pipe\LOCAL\cc-msg-{name}")
    }

    fn world(root: &Path) -> World {
        let app = root.join("app");
        let admin = app.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..").unwrap();
        let wt = app.join(".claude").join("worktrees").join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", admin.display())).unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        let orchestrator = root.join("orchestrator");
        std::fs::create_dir_all(&orchestrator).unwrap();
        let records = [
            ("app-11", app.clone(), "me"),
            ("app-22", app.join("src"), "s22"),
            ("wt-33", wt, "s33"),
            ("other-44", other.clone(), "s44"),
            ("orchestrator-55", orchestrator.clone(), "s55"),
            ("dup", app.clone(), "s66"),
            ("dup", other.clone(), "s77"),
        ];
        let dir = root.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, (name, cwd, id)) in records.iter().enumerate() {
            let rec = json!({ "pid": i, "sessionId": id, "cwd": cwd, "name": name, "messagingSocketPath": inbox(id), "status": "idle" });
            std::fs::write(dir.join(format!("{i}.json")), rec.to_string()).unwrap();
        }
        // bare mode: no inbox, can't be messaged; and the auth keys beside the records
        std::fs::write(dir.join("90.json"), json!({ "pid": 90, "sessionId": "s90", "cwd": other, "name": "bare-90" }).to_string()).unwrap();
        std::fs::write(dir.join("0.0123abcd.key"), "secret").unwrap();
        let peers = registry(&dir);
        World { app, other, orchestrator, peers }
    }

    fn reach(w: &World, scope: Scope) -> Reach {
        Reach::new(scope, &w.app, &w.orchestrator, Some("me"), w.peers.clone())
    }

    fn reference(w: &World, id: &str) -> String {
        w.peers.iter().find(|p| p.session_id.as_deref() == Some(id)).unwrap().reference().to_string()
    }

    #[test]
    fn refs_are_claude_codes() {
        // from a live listing: `bro-cli-v2-67 [4380bc]`, `claude-cd [ddaf2d]`
        assert_eq!(&digest(r"\\.\pipe\LOCAL\cc-msg-90a5d8d438cf6cc18f393141689034da")[..6], "4380bc");
        assert_eq!(&digest(r"\\.\pipe\LOCAL\cc-msg-998c24479be56e02c7113fe3f8f0695d")[..6], "ddaf2d");
        assert_eq!(digest("x").len(), 64);
    }

    #[test]
    fn the_registry_holds_sessions_that_take_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let w = world(tmp.path());
        assert_eq!(w.peers.len(), 7, "{:?}", w.peers);
        assert!(w.peers.iter().all(|p| p.name != "bare-90"));
        assert!(registry(&tmp.path().join("nowhere")).is_empty());
    }

    #[test]
    fn sends_reach_the_repository_its_worktrees_and_the_orchestrator() {
        let tmp = tempfile::tempdir().unwrap();
        let w = world(tmp.path());
        let r = reach(&w, Scope::Project);
        for to in ["app-22", "wt-33", "orchestrator-55", "app-11", "dup", "researcher", "main"] {
            assert_eq!(r.refusal(to), None, "{to}");
        }
        let why = r.refusal("other-44").unwrap();
        assert!(why.starts_with("Not sent") && why.contains(&w.other.display().to_string()), "{why}");
        let other = reference(&w, "s44");
        let full = digest(&inbox("s44"));
        for to in ["OTHER-44", " @other-44 ", "\"other-44\"", &format!("other-44 [{other}]"), &format!("[{other}]"), &format!("other-44 [{}]", &full[..12]), &format!("uds:{}", inbox("s44"))] {
            assert!(r.refusal(to).is_some(), "{to}");
        }
        // two sessions answer to "dup": the ref settles which
        assert!(r.refusal(&format!("dup [{}]", reference(&w, "s77"))).is_some());
        assert_eq!(r.refusal(&format!("dup [{}]", reference(&w, "s66"))), None);
    }

    #[test]
    fn off_reaches_no_other_session() {
        let tmp = tempfile::tempdir().unwrap();
        let w = world(tmp.path());
        let r = reach(&w, Scope::Off);
        for to in ["app-22", "wt-33", "orchestrator-55", "other-44", "dup"] {
            assert!(r.refusal(to).is_some_and(|why| why.contains("off in bro")), "{to}");
        }
        for to in ["app-11", "researcher", "main"] {
            assert_eq!(r.refusal(to), None, "{to}");
        }
    }

    fn listed(w: &World) -> Value {
        let rows = [("app-22", "s22"), ("other-44", "s44"), ("orchestrator-55", "s55")];
        let mut text = format!(
            "This session is app-11 [{}] \u{2014} the name other sessions use to message it.\n\nPeer sessions (4):\n",
            reference(w, "me")
        );
        let mut json_rows = vec![];
        for (name, id) in rows {
            let r = reference(w, id);
            text.push_str(&format!("  {name} [{r}]  \u{b7}  interactive  \u{b7}  idle  \u{b7}  started 2h ago\n"));
            if name == "other-44" {
                text.push_str("    can't receive cross-session messages (off in that session)\n");
            }
            json_rows.push(json!({ "name": name, "ref": r, "type": "interactive", "status": "idle" }));
        }
        text.push_str("  laptop [abc123]  \u{b7}  remote_control  \u{b7}  idle\n");
        json_rows.push(json!({ "name": "laptop", "ref": "abc123", "type": "remote_control", "status": "idle" }));
        json!({ "listing": text, "sections": [{ "kind": "sessions", "total": 4, "rows": json_rows }], "notes": [] })
    }

    #[test]
    fn listings_leave_out_sessions_out_of_reach() {
        let tmp = tempfile::tempdir().unwrap();
        let w = world(tmp.path());
        let out = reach(&w, Scope::Project).listing(&listed(&w)).unwrap();
        let text = out["listing"].as_str().unwrap();
        assert!(text.starts_with("This session is app-11"), "{text}");
        assert!(text.contains("Peer sessions (3):") && text.contains("app-22") && text.contains("orchestrator-55") && text.contains("laptop"), "{text}");
        assert!(!text.contains("other-44") && !text.contains("can't receive"), "{text}");
        assert!(text.ends_with("(1 session working in other projects left out by bro: sessions here only reach sessions in app.)"), "{text}");
        assert_eq!(out["sections"][0]["total"], 3);
        let names: Vec<&str> = out["sections"][0]["rows"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["app-22", "orchestrator-55", "laptop"]);
        assert_eq!(out["notes"], json!([]));

        let out = reach(&w, Scope::Off).listing(&listed(&w)).unwrap();
        let text = out["listing"].as_str().unwrap();
        assert!(text.starts_with("This session is app-11") && !text.contains("Peer sessions") && !text.contains("laptop"), "{text}");
        assert!(text.ends_with("(4 sessions left out by bro: messages between Claude sessions are off.)"), "{text}");
        assert_eq!(out["sections"][0]["total"], 0);

        // nothing out of reach: the listing stays as Claude Code wrote it
        let mut only_app = listed(&w);
        only_app["sections"][0]["rows"].as_array_mut().unwrap().truncate(1);
        only_app["listing"] = json!(format!("Peer sessions (1):\n  app-22 [{}]  \u{b7}  idle", reference(&w, "s22")));
        assert_eq!(reach(&w, Scope::Project).listing(&only_app), None);
        assert_eq!(reach(&w, Scope::Project).listing(&json!({ "nope": 1 })), None);
    }

    #[test]
    fn the_hook_answers_claude_code() {
        let sb = sandbox();
        let claude = sb.home().join(".claude");
        let w = world(&claude);
        std::fs::create_dir_all(sb.home().join("orchestrator")).unwrap();
        let app = w.app.to_string_lossy().into_owned();
        let args = |scope: &str| vec![scope.to_string(), app.clone()];
        let send = |to: &str| json!({ "session_id": "me", "cwd": app, "hook_event_name": "PreToolUse", "tool_name": "SendMessage", "tool_input": { "to": to, "message": "hi" } }).to_string();
        let out: Value = serde_json::from_str(&run_hook(&args("project"), &send("other-44")).unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(out["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().starts_with("Not sent"));
        assert_eq!(run_hook(&args("project"), &send("app-22")), None);
        // the session's cwd from the event when bro didn't pass one
        assert!(run_hook(&["project".to_string()], &send("other-44")).is_some());

        let list = json!({ "session_id": "me", "hook_event_name": "PostToolUse", "tool_name": "ListAgents", "tool_input": {}, "tool_response": listed(&w) }).to_string();
        let out: Value = serde_json::from_str(&run_hook(&args("project"), &list).unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert!(!out["hookSpecificOutput"]["updatedToolOutput"]["listing"].as_str().unwrap().contains("other-44"));

        // a pooled login's registry lives in its own config dir
        let pooled = sb.home().join("pool");
        std::fs::create_dir_all(pooled.join("sessions")).unwrap();
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &pooled) };
        assert_eq!(run_hook(&args("project"), &send("other-44")), None, "not a session of this login");
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };

        assert_eq!(run_hook(&args("everyone"), &send("other-44")), None);
        assert_eq!(run_hook(&args("project"), "not json"), None);
        let bash = json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" } }).to_string();
        assert_eq!(run_hook(&args("project"), &bash), None);
    }

    #[test]
    fn hooks_join_the_launch_settings() {
        let mut settings = Map::new();
        settings.insert("hooks".into(), json!({ "PreToolUse": [{ "matcher": "Bash", "hooks": [] }] }));
        add_hooks(&mut settings, Scope::Project, Path::new("/opt/bro/bro"), Path::new("/work/app"));
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[1]["matcher"], "SendMessage");
        let hook = &pre[1]["hooks"][0];
        assert_eq!(hook["command"], "/opt/bro/bro");
        assert_eq!(hook["args"], json!(["hook", "peers", "project", "/work/app"]));
        assert_eq!(settings["hooks"]["PostToolUse"][0]["matcher"], "ListAgents");
        assert_eq!(settings["hooks"]["PostToolUse"][0]["hooks"][0], *hook);
    }

    #[test]
    fn the_knob_picks_the_scope() {
        let mut env = ClaudeEnvSettings::default();
        assert_eq!(Scope::of(&env), Some(Scope::Project), "kept to the project unless you say otherwise");
        let k = crate::claude_env::knob(KNOB).unwrap();
        let at = |id: &str| k.options.iter().position(|o| o.id == id).unwrap();
        env.set_choice(k, at("off"));
        assert_eq!(Scope::of(&env), Some(Scope::Off));
        assert_eq!(env.settings()["crossSessionInbound"], "refuse");
        env.set_choice(k, at("default"));
        assert_eq!(Scope::of(&env), None);
        assert!(env.settings().get("crossSessionInbound").is_none());
    }
}

