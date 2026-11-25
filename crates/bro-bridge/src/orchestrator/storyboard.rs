//! The storyboard tools: how an agent writes a Create Slop project's film (`slop/storyboard.json`, modelled in
//! `bro_core::storyboard`) and reads what the user changed in bro's window. Served on `/mcp` with the rest, and
//! alone on `/mcp/studio` for Create Slop sessions.

use super::tools::{self, tool};
use crate::BridgeCommand;
use crate::state::AppState;
use bro_core::storyboard::{self, Board, clock};
use serde_json::{Value, json};
use std::path::PathBuf;

const NO_BOARD: &str = "This project has no storyboard yet. Write one with storyboard_plan.";

/// The five storyboard tools in MCP form.
pub(crate) fn definitions() -> Vec<Value> {
    let project = json!({ "type": "string", "description": "The project's folder. Omit it to use this session's folder." });
    let shot = |what: &str| json!({ "type": ["string", "object"], "description": what, "properties": { "path": { "type": "string" }, "note": { "type": "string" } } });
    vec![
        tool(
            "storyboard_get",
            "Read the project's storyboard: the film (title, treatment, frame, length, cast) and every scene with its number, id, times, script (what is heard), direction (what is seen), keyframes and takes, whether the user approved it, and userAsks when they asked for a change. changedByUser lists what the user changed in bro since you last read it; reading clears that list. Call it before you spend anything and whenever the user says they changed the storyboard.",
            json!({
                "project": project,
                "scene": { "type": ["string", "integer"], "description": "Only this scene: its id or its number in the film." }
            }),
            &[],
        ),
        tool(
            "storyboard_plan",
            "Write the film and its scenes onto the storyboard the user sees beside this session. Send it as soon as the idea is agreed, before any picture is made, and again whenever the plan changes. scenes replaces the scene list (append: true adds to it); a scene that keeps its id keeps its keyframes and takes. Scenes run on from each other: give startMs and endMs, or seconds and bro places it after the one before. Returns the scenes as stored and anything that would stop the film: gaps, a scene with no direction, SLOPCANNON's 5 to 15 second scenes.",
            json!({
                "project": project,
                "title": { "type": "string", "description": "The film's title." },
                "concept": { "type": "string", "description": "The treatment: the visual world, who is who, the arc." },
                "aspect": { "type": "string", "description": "The frame, e.g. 16:9 or 9:16." },
                "durationMs": { "type": "integer", "description": "How long the film runs, in milliseconds (a song's length)." },
                "song": { "type": "string", "description": "The song or master audio file it is cut to." },
                "cast": { "type": "array", "items": { "type": "string" }, "description": "Everyone in the film, by the names scenes use." },
                "scenes": {
                    "type": "array",
                    "description": "The scenes, in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "Keep it to keep the scene's keyframes and takes." },
                            "startMs": { "type": "integer" },
                            "endMs": { "type": "integer" },
                            "seconds": { "type": "number", "description": "How long it runs, when startMs and endMs are left out." },
                            "title": { "type": "string" },
                            "script": { "type": "string", "description": "What is heard: the lyric, the lines, the voice-over." },
                            "direction": { "type": "string", "description": "What is seen: staging, camera, performance. For SLOPCANNON this is the scene's complete beat." },
                            "cast": { "type": "array", "items": { "type": "string" } },
                            "credits": { "type": "number" }
                        }
                    }
                },
                "append": { "type": "boolean", "description": "Add to the scenes already there instead of replacing them." }
            }),
            &[],
        ),
        tool(
            "storyboard_scene",
            "Change one scene, named by its id or its number in the film: reword it, retime it, attach a keyframe picture or a filmed take (the file's path; the newest becomes the chosen one), choose between them (selectKeyframe / selectTake, 1 is the first), record what filming was quoted at (credits), mark the user's request done (resolveNote), insert a new scene after it (insert: true; scene: 0 puts it first) or remove it. The user sees each change at once.",
            json!({
                "project": project,
                "scene": { "type": ["string", "integer"], "description": "The scene's id or its number in the film." },
                "title": { "type": "string" },
                "script": { "type": "string", "description": "What is heard." },
                "direction": { "type": "string", "description": "What is seen." },
                "cast": { "type": "array", "items": { "type": "string" } },
                "startMs": { "type": "integer" },
                "endMs": { "type": "integer" },
                "seconds": { "type": "number", "description": "How long it runs." },
                "keyframe": shot("A picture for the scene: its file path, or { path, note }."),
                "take": shot("A filmed clip for the scene: its file path, or { path, note }."),
                "selectKeyframe": { "type": "integer", "description": "Film from this keyframe (1 is the first)." },
                "selectTake": { "type": "integer", "description": "Put this take in the cut (1 is the first)." },
                "approved": { "type": "boolean" },
                "resolveNote": { "type": "boolean", "description": "The user's request for a change in this scene is done." },
                "credits": { "type": "number", "description": "What filming it was quoted at." },
                "insert": { "type": "boolean", "description": "Add a new scene after this one." },
                "remove": { "type": "boolean", "description": "Take the scene out of the film." }
            }),
            &["scene"],
        ),
        tool(
            "storyboard_timed_plan",
            "The storyboard as SLOPCANNON's timed plan (schemaVersion 1: durationMs, and each scene's startMs, endMs, cast and beat), ready for slopcannon_create_quote or its CLI. Lists first everything SLOPCANNON would refuse and the scenes the user has not approved yet, so you can settle those before anything is bought.",
            json!({ "project": project }),
            &[],
        ),
        tool(
            "storyboard_show",
            "Bring the storyboard to the front in bro, on a scene when you name one. Use it when you want the user to look at something: new keyframes, a take to review.",
            json!({
                "project": project,
                "scene": { "type": ["string", "integer"], "description": "The scene to show: its id or its number in the film." }
            }),
            &[],
        ),
    ]
}

/// Whether `name` is one of the storyboard tools.
pub(crate) fn is_tool(name: &str) -> bool {
    name.starts_with("storyboard_")
}

pub(crate) async fn execute(app: &AppState, name: &str, args: &Value) -> Result<String, String> {
    let project = project_of(app, args)?;
    match name {
        "storyboard_get" => get(project, scene_key(args)).await,
        "storyboard_plan" => {
            let text = plan(project.clone(), args.clone()).await?;
            send(app, project, None, false);
            Ok(text)
        }
        "storyboard_scene" => {
            let (scene, text) = scene(project.clone(), args.clone()).await?;
            send(app, project, scene, false);
            Ok(text)
        }
        "storyboard_timed_plan" => timed_plan(project).await,
        "storyboard_show" => {
            let at = match scene_key(args) {
                None => None,
                Some(key) => {
                    let p = project.clone();
                    let found = blocking(move || {
                        let board = storyboard::load(&p).unwrap_or_default();
                        board.find(&key).ok_or_else(|| no_scene(&board, &key))
                    })
                    .await??;
                    Some(found)
                }
            };
            send(app, project, at, true);
            Ok(match at {
                Some(i) => format!("The storyboard is in front of the user, on scene {}.", i + 1),
                None => "The storyboard is in front of the user.".into(),
            })
        }
        other => Err(format!("Unknown tool \"{other}\".")),
    }
}

/// Tell bro a storyboard changed (or should be shown), from the calling session if there is one.
fn send(app: &AppState, project: PathBuf, scene: Option<usize>, focus: bool) {
    app.send_command(BridgeCommand::Storyboard { session: tools::caller_session(), project, scene, focus });
}

/// Run blocking file work off the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| e.to_string())
}

/// The project a call is about: its `project` folder, else the calling session's folder.
fn project_of(app: &AppState, args: &Value) -> Result<PathBuf, String> {
    let given = args.get("project").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty());
    let folder = match given {
        Some(p) => p.to_string(),
        None => tools::caller_session()
            .and_then(|id| app.summary(&id))
            .map(|s| s.cwd)
            .filter(|c| !c.trim().is_empty())
            .ok_or("Say which project: pass project (its folder), or call from a bro session.")?,
    };
    let path = PathBuf::from(&folder);
    if !path.is_dir() {
        return Err(format!("The project folder \"{folder}\" doesn't exist. Pass project as an existing folder."));
    }
    Ok(path)
}

/// The scene a call names: an id, or its number as a string or an integer.
fn scene_key(args: &Value) -> Option<String> {
    match args.get("scene") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// What to say when a scene isn't there (as `Board::apply_scene` says it).
fn no_scene(board: &Board, key: &str) -> String {
    match board.scenes.len() {
        0 => "the storyboard has no scenes yet: write them with storyboard_plan".into(),
        n => format!("no scene \"{key}\": the film has scenes 1 to {n} ({})", board.scenes.iter().map(|s| s.id.as_str()).take(12).collect::<Vec<_>>().join(", ")),
    }
}

/// A blank line, then what to fix before filming, when there is anything.
fn problems_block(board: &Board) -> String {
    let problems = board.problems(true);
    if problems.is_empty() {
        return String::new();
    }
    format!("\n\nTo fix before filming:\n{}", problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"))
}

fn io_error(e: std::io::Error) -> String {
    format!("Couldn't write the storyboard: {e}")
}

async fn get(project: PathBuf, scene: Option<String>) -> Result<String, String> {
    blocking(move || {
        if storyboard::load(&project).is_none() {
            return Ok(NO_BOARD.to_string());
        }
        let (out, _) = storyboard::update(&project, |b| {
            let at = match scene.as_deref().map(|key| b.find(key).ok_or_else(|| no_scene(b, key))) {
                Some(Err(e)) => return (false, Err(e)),
                Some(Ok(i)) => Some(i),
                None => None,
            };
            let mut value = b.for_agent(&project);
            let problems = b.problems(true);
            if let Some(i) = at {
                value = json!({ "scene": value["scenes"][i].clone(), "changedByUser": value["changedByUser"].clone() });
            }
            if !problems.is_empty() {
                value["problems"] = json!(problems);
            }
            let moved = b.take_edits();
            (moved, Ok(value))
        })
        .map_err(io_error)?;
        out.map(|v| serde_json::to_string(&v).unwrap_or_default())
    })
    .await?
}

async fn plan(project: PathBuf, args: Value) -> Result<String, String> {
    blocking(move || {
        let (out, b) = storyboard::update(&project, |b| match b.apply_plan(&args, &project) {
            Ok(()) => (true, Ok(())),
            Err(e) => (false, Err(e)),
        })
        .map_err(io_error)?;
        out?;
        let mut text = format!("Storyboard saved (rev {}): {} scenes, {}.", b.rev, b.scenes.len(), clock(b.runs_ms()));
        for (i, s) in b.scenes.iter().enumerate() {
            let title = if s.title.is_empty() { "untitled" } else { s.title.as_str() };
            text += &format!("\n  {}. {}  {}–{}  {}", i + 1, s.id, clock(s.start_ms), clock(s.end_ms), title);
        }
        text += &problems_block(&b);
        Ok(text)
    })
    .await?
}

/// Returns where the changed scene is in the film (None for a removed one) and the text to answer with.
async fn scene(project: PathBuf, args: Value) -> Result<(Option<usize>, String), String> {
    blocking(move || {
        let removed = args.get("remove").and_then(Value::as_bool).unwrap_or(false);
        let (out, b) = storyboard::update(&project, |b| match b.apply_scene(&args, &project) {
            Ok(i) => (true, Ok(i)),
            Err(e) => (false, Err(e)),
        })
        .map_err(io_error)?;
        let i = out?;
        let mut text = if removed {
            format!("Scene {} removed (rev {}): {} scenes left.", i + 1, b.rev, b.scenes.len())
        } else {
            let s = &b.scenes[i];
            let count = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
            format!(
                "Scene {} ({}) saved (rev {}): {}–{}, {}, {}{}.",
                i + 1,
                s.id,
                b.rev,
                clock(s.start_ms),
                clock(s.end_ms),
                count(s.keyframes.len(), "keyframe"),
                count(s.takes.len(), "take"),
                if s.approved { ", approved" } else { "" }
            )
        };
        text += &problems_block(&b);
        Ok(((!removed).then_some(i), text))
    })
    .await?
}

async fn timed_plan(project: PathBuf) -> Result<String, String> {
    blocking(move || {
        let Some(b) = storyboard::load(&project) else { return NO_BOARD.to_string() };
        let mut text = String::new();
        let refused = b.problems(true);
        if !refused.is_empty() {
            text += &format!("Would be refused:\n{}\n", refused.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"));
        }
        let waiting: Vec<String> = b.scenes.iter().enumerate().filter(|(_, s)| !s.approved).map(|(i, _)| (i + 1).to_string()).collect();
        if !waiting.is_empty() {
            text += &format!("Not approved by the user yet: scene{} {}.\n", if waiting.len() == 1 { "" } else { "s" }, waiting.join(", "));
        }
        text + &serde_json::to_string(&b.timed_plan()).unwrap_or_default()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BridgeConfig;
    use crate::orchestrator::mcp::handle_message;
    use bro_core::storyboard::Field;
    use std::sync::Arc;

    struct Rig {
        _data: tempfile::TempDir,
        project: tempfile::TempDir,
        app: AppState,
        sent: Arc<parking_lot::Mutex<Vec<String>>>,
    }

    fn rig() -> Rig {
        let data = tempfile::tempdir().unwrap();
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = sent.clone();
        let app = AppState::new(
            BridgeConfig { data_root: data.path().to_path_buf(), ..BridgeConfig::default() },
            "token".into(),
            Arc::new(move |command| {
                if let BridgeCommand::Storyboard { .. } = &command {
                    sink.lock().push(format!("{command:?}"));
                }
            }),
        );
        Rig { _data: data, project: tempfile::tempdir().unwrap(), app, sent }
    }

    impl Rig {
        async fn call(&self, name: &str, mut args: Value) -> Result<String, String> {
            args["project"] = json!(self.project.path());
            execute(&self.app, name, &args).await
        }
    }

    fn plan_args() -> Value {
        json!({
            "title": "Night Drive", "durationMs": 20000, "cast": ["CLAUDIA"],
            "scenes": [
                { "title": "Cold open", "script": "We carry the morning home", "direction": "CLAUDIA under a flickering sign.", "seconds": 10, "cast": ["CLAUDIA"] },
                { "title": "The drive", "direction": "The car pulls out.", "seconds": 10, "cast": ["CLAUDIA"] }
            ]
        })
    }

    #[tokio::test]
    async fn a_plan_is_read_back_and_the_users_change_is_news_once() {
        let r = rig();
        assert_eq!(r.call("storyboard_get", json!({})).await.unwrap(), NO_BOARD);
        let text = r.call("storyboard_plan", plan_args()).await.unwrap();
        assert!(text.starts_with("Storyboard saved (rev 1): 2 scenes, 0:20."), "{text}");
        assert!(text.contains("\n  1. s1  0:00–0:10  Cold open\n  2. s2  0:10–0:20  The drive"), "{text}");
        assert!(!text.contains("To fix before filming"), "{text}");
        assert_eq!(r.sent.lock().len(), 1);
        assert!(r.sent.lock()[0].contains("focus: false") && r.sent.lock()[0].contains("scene: None"), "{:?}", r.sent.lock());

        storyboard::update(r.project.path(), |b| (b.set_text(Some(0), Field::Script, "We carry the evening home"), ())).unwrap();
        let first: Value = serde_json::from_str(&r.call("storyboard_get", json!({})).await.unwrap()).unwrap();
        assert_eq!(first["changedByUser"], json!(["scene 1 (s1): the script was rewritten"]));
        assert_eq!(first["scenes"][0]["script"], "We carry the evening home");
        assert_eq!(first["scenes"][1]["n"], 2);
        let second: Value = serde_json::from_str(&r.call("storyboard_get", json!({})).await.unwrap()).unwrap();
        assert_eq!(second["changedByUser"], json!([]));

        // one scene, by number or id; an unknown one is an error that says what exists
        let one: Value = serde_json::from_str(&r.call("storyboard_get", json!({ "scene": 2 })).await.unwrap()).unwrap();
        assert_eq!(one["scene"]["id"], "s2");
        let err = r.call("storyboard_get", json!({ "scene": "nope" })).await.unwrap_err();
        assert!(err.contains("scenes 1 to 2"), "{err}");
    }

    #[tokio::test]
    async fn a_scene_takes_a_keyframe_and_says_where_it_stands() {
        let r = rig();
        r.call("storyboard_plan", plan_args()).await.unwrap();
        r.sent.lock().clear();
        let text = r.call("storyboard_scene", json!({ "scene": "s1", "keyframe": { "path": "slop/keyframes/s1-a.png", "note": "wide" } })).await.unwrap();
        assert_eq!(text, "Scene 1 (s1) saved (rev 2): 0:00–0:10, 1 keyframe, 0 takes.");
        assert!(r.sent.lock()[0].contains("scene: Some(0)"), "{:?}", r.sent.lock());
        let text = r.call("storyboard_scene", json!({ "scene": 2, "approved": true })).await.unwrap();
        assert!(text.ends_with("0 takes, approved."), "{text}");
        let text = r.call("storyboard_scene", json!({ "scene": 2, "remove": true })).await.unwrap();
        assert_eq!(text.lines().next().unwrap(), "Scene 2 removed (rev 4): 1 scenes left.");
        assert!(text.contains("the scenes end at 0:10 and the film runs 0:20"), "{text}");
        assert!(r.sent.lock().last().unwrap().contains("scene: None"));
        let err = r.call("storyboard_scene", json!({ "title": "x" })).await.unwrap_err();
        assert!(err.contains("scene is required"), "{err}");
    }

    #[tokio::test]
    async fn the_timed_plan_names_what_the_user_has_not_approved() {
        let r = rig();
        assert_eq!(r.call("storyboard_timed_plan", json!({})).await.unwrap(), NO_BOARD);
        r.call("storyboard_plan", plan_args()).await.unwrap();
        r.call("storyboard_scene", json!({ "scene": 1, "approved": true })).await.unwrap();
        r.call("storyboard_scene", json!({ "scene": 2, "seconds": 3 })).await.unwrap();
        let text = r.call("storyboard_timed_plan", json!({})).await.unwrap();
        assert!(text.starts_with("Would be refused:\n- scene 2 runs 3s"), "{text}");
        assert!(text.contains("Not approved by the user yet: scene 2.\n{\"schemaVersion\":1,"), "{text}");
        assert!(text.contains("\"beat\":\"CLAUDIA under a flickering sign.\""), "{text}");
    }

    #[tokio::test]
    async fn show_brings_the_storyboard_forward() {
        let r = rig();
        r.call("storyboard_plan", plan_args()).await.unwrap();
        r.sent.lock().clear();
        assert_eq!(r.call("storyboard_show", json!({ "scene": "s2" })).await.unwrap(), "The storyboard is in front of the user, on scene 2.");
        assert!(r.sent.lock()[0].contains("focus: true") && r.sent.lock()[0].contains("scene: Some(1)"));
        assert!(r.call("storyboard_show", json!({ "scene": "zzz" })).await.is_err());
        assert_eq!(r.call("storyboard_show", json!({})).await.unwrap(), "The storyboard is in front of the user.");
    }

    #[tokio::test]
    async fn a_call_must_say_which_project() {
        let r = rig();
        let err = execute(&r.app, "storyboard_get", &json!({})).await.unwrap_err();
        assert_eq!(err, "Say which project: pass project (its folder), or call from a bro session.");
        let missing = r.project.path().join("not-there");
        let err = execute(&r.app, "storyboard_get", &json!({ "project": missing })).await.unwrap_err();
        assert!(err.contains("doesn't exist"), "{err}");
    }

    fn names(answer: &Value) -> Vec<String> {
        answer["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn the_studio_endpoint_offers_only_the_storyboard() {
        use crate::orchestrator::mcp::{Scope, handle_message_for_scope};
        let r = rig();
        let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let studio = handle_message_for_scope(&r.app, &list, Scope::Studio).await.unwrap();
        assert_eq!(names(&studio), ["storyboard_get", "storyboard_plan", "storyboard_scene", "storyboard_timed_plan", "storyboard_show"]);
        let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "list_sessions", "arguments": {} } });
        let refused = handle_message_for_scope(&r.app, &call, Scope::Studio).await.unwrap();
        assert_eq!(refused["result"]["isError"], true);
        assert_eq!(refused["result"]["content"][0]["text"], "That tool isn't part of the storyboard server.");
        let init = json!({ "jsonrpc": "2.0", "id": 3, "method": "initialize", "params": {} });
        let init = handle_message_for_scope(&r.app, &init, Scope::Studio).await.unwrap();
        assert!(init["result"]["instructions"].as_str().unwrap().starts_with("The storyboard of this Create Slop project"));
        assert_eq!(init["result"]["serverInfo"]["name"], "bro");
        // a storyboard call goes through, and the full endpoint lists them too
        let call = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "storyboard_get", "arguments": { "project": r.project.path() } } });
        let ok = handle_message_for_scope(&r.app, &call, Scope::Studio).await.unwrap();
        assert_eq!((ok["result"]["isError"].clone(), ok["result"]["content"][0]["text"].clone()), (json!(false), json!(NO_BOARD)));
        let all = names(&handle_message(&r.app, &list).await.unwrap());
        assert!(all.contains(&"list_sessions".to_string()));
        for tool in ["storyboard_get", "storyboard_plan", "storyboard_scene", "storyboard_timed_plan", "storyboard_show"] {
            assert!(all.contains(&tool.to_string()), "{tool} missing from /mcp");
        }
    }
}
