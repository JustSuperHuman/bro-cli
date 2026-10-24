use super::*;
use crate::util::test_env::sandbox;
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

/// A pack `name` in the packs folder: every category with the given files (written as tiny files).
fn pack_with(name: &str, cats: &[(&str, &[&str])]) {
    let dir = packs_dir().join(name);
    let mut categories = Map::new();
    for (cat, files) in cats {
        for f in *files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"RIFF").unwrap();
        }
        let sounds: Vec<Value> = files.iter().map(|f| json!({ "file": f, "label": "line" })).collect();
        categories.insert(cat.to_string(), json!({ "sounds": sounds }));
    }
    let manifest = json!({ "cesp_version": "1.0", "name": name, "display_name": format!("{name} pack"), "version": "1.0.0", "categories": categories });
    std::fs::write(dir.join("openpeon.json"), manifest.to_string()).unwrap();
}

fn peon_pack() {
    pack_with(
        "peon",
        &[
            ("session.start", &["sounds/Ready.wav"]),
            ("task.complete", &["sounds/Done1.wav", "sounds/Done2.wav"]),
            ("input.required", &["sounds/What.wav"]),
            ("user.spam", &["sounds/Angry.wav"]),
            ("task.error", &["sounds/Ugh.wav"]),
            ("resource.limit", &["sounds/Why.wav"]),
        ],
    );
}

fn ev(name: &str, sid: &str) -> String {
    json!({ "hook_event_name": name, "session_id": sid, "cwd": "/work/app" }).to_string()
}

fn played(input: &str, now: f64) -> Option<&'static str> {
    decide(Harness::Claude, input, now).map(|p| p.category)
}

#[test]
fn home_is_peon_pings_folder_when_it_has_one_else_bros() {
    let sb = sandbox();
    assert_eq!(home(), sb.bro().join("peon"));
    std::fs::create_dir_all(peon_ping_dir().join("packs")).unwrap();
    assert_eq!(home(), sb.home().join(".claude").join("hooks").join("peon-ping"));
    assert!(!peon_ping_installed());
    std::fs::write(peon_ping_dir().join("peon.ps1"), "").unwrap();
    assert!(peon_ping_installed());
    unsafe { std::env::set_var("CLAUDE_PEON_DIR", sb.home().join("custom")) };
    assert_eq!(home(), sb.home().join("custom"));
}

#[test]
fn config_keeps_what_bro_doesnt_know() {
    let _sb = sandbox();
    std::fs::create_dir_all(home()).unwrap();
    std::fs::write(home().join("config.json"), "\u{feff}{\"volume\": 0.3, \"mobile_notify\": {\"enabled\": true}, \"annoyed_threshold\": 3, \"active_pack\": \"glados\"}").unwrap();
    let mut c = PeonConfig::load();
    assert_eq!(c.volume(), 0.3);
    assert_eq!(c.default_pack(), Some("glados"));
    assert!(c.enabled() && c.category_on("task.complete"));
    c.set_volume(0.456);
    c.set_category("user.spam", false);
    c.use_pack("peon");
    c.save().unwrap();
    let back: Value = serde_json::from_str(&std::fs::read_to_string(home().join("config.json")).unwrap()).unwrap();
    assert_eq!(back["mobile_notify"]["enabled"], true);
    assert_eq!(back["annoyed_threshold"], 3, "an integer stays an integer");
    assert_eq!(back["volume"], 0.46);
    assert_eq!(back["categories"]["user.spam"], false);
    assert_eq!((back["default_pack"].as_str(), back["active_pack"].as_str()), (Some("peon"), Some("peon")));
    let fresh = PeonConfig::default();
    assert_eq!((fresh.default_pack(), fresh.volume()), (Some(DEFAULT_PACK), 0.5));
}

#[test]
fn events_from_every_harness_read_alike() {
    let e = parse_event(Harness::Codex, r#"{"hook_event_name":"SessionStart","session_id":"019a","cwd":"C:\\w","source":"startup"}"#).unwrap();
    assert_eq!((e.name.as_str(), e.session.as_str(), e.source.as_deref()), ("SessionStart", "codex-019a", Some("startup")));
    let e = parse_event(Harness::Omp, r#"{"hook_event_name":"Stop","session_id":"omp-12"}"#).unwrap();
    assert_eq!(e.session, "omp-12", "the extension's id already carries the harness");
    let e = parse_event(Harness::Claude, r#"{"hookEventName":"beforeSubmitPrompt","sessionId":"abc"}"#).unwrap();
    assert_eq!((e.name.as_str(), e.session.as_str()), ("UserPromptSubmit", "abc"));
    assert!(parse_event(Harness::Claude, "not json").is_none());
    assert!(parse_event(Harness::Claude, "{}").is_none());
}

#[test]
fn events_map_to_categories_like_peon_ping() {
    let _sb = sandbox();
    peon_pack();
    assert_eq!(played(&ev("SessionStart", "a"), 1000.0), Some("session.start"));
    // several sessions opening together greet once
    assert_eq!(played(&ev("SessionStart", "b"), 1010.0), None);
    assert_eq!(played(&ev("SessionStart", "b"), 1100.0), Some("session.start"));
    assert_eq!(played(&json!({"hook_event_name":"SessionStart","session_id":"c","source":"compact"}).to_string(), 2000.0), None);
    assert_eq!(played(&ev("Stop", "a"), 3000.0), Some("task.complete"));
    assert_eq!(played(&ev("Stop", "a"), 3002.0), None, "stops are debounced");
    assert_eq!(played(&ev("PermissionRequest", "a"), 3003.0), Some("input.required"));
    let note = |t: &str| json!({"hook_event_name":"Notification","session_id":"a","notification_type":t}).to_string();
    assert_eq!(played(&note("permission_prompt"), 3004.0), None);
    assert_eq!(played(&note("idle_prompt"), 3005.0), None);
    assert_eq!(played(&ev("PostToolUseFailure", "a"), 3006.0), Some("task.error"));
    assert_eq!(played(&ev("PreCompact", "a"), 3007.0), Some("resource.limit"));
    // the pack has no acknowledge or end sounds: silence
    assert_eq!(played(&ev("SubagentStart", "a"), 3008.0), None);
    assert_eq!(played(&ev("SessionEnd", "a"), 3009.0), None);
    // three prompts in ten seconds: annoyed
    assert_eq!(played(&ev("UserPromptSubmit", "s"), 4000.0), None);
    assert_eq!(played(&ev("UserPromptSubmit", "s"), 4001.0), None);
    assert_eq!(played(&ev("UserPromptSubmit", "s"), 4002.0), Some("user.spam"));
    assert_eq!(played(&ev("UserPromptSubmit", "t"), 4003.0), None, "per session");
}

#[test]
fn switches_volume_and_exclusions_hold() {
    let _sb = sandbox();
    peon_pack();
    let mut c = PeonConfig::default();
    c.set_volume(0.8);
    c.set_category("task.error", false);
    c.raw.insert("exclude_dirs".into(), json!(["/scratch"]));
    c.save().unwrap();
    let p = decide(Harness::Claude, &ev("PermissionRequest", "a"), 10.0).unwrap();
    assert_eq!((p.volume, p.pack.as_str()), (0.8, "peon"));
    assert!(p.path.ends_with("What.wav") && p.path.is_file());
    assert_eq!(played(&ev("PostToolUseFailure", "a"), 11.0), None, "category off");
    let inside = json!({"hook_event_name":"PermissionRequest","session_id":"a","cwd":"/scratch/x"}).to_string();
    assert_eq!(played(&inside, 12.0), None, "excluded folder");
    c.set_enabled(false);
    c.save().unwrap();
    assert_eq!(played(&ev("PermissionRequest", "a"), 13.0), None);
}

#[test]
fn no_line_twice_in_a_row() {
    let _sb = sandbox();
    peon_pack();
    let mut last = None;
    for i in 0..8 {
        let p = decide(Harness::Claude, &ev("Stop", "a"), 100.0 + 10.0 * i as f64).unwrap();
        assert_ne!(Some(p.path.clone()), last);
        last = Some(p.path);
    }
}

#[test]
fn packs_pick_by_session_path_harness_then_default() {
    let _sb = sandbox();
    peon_pack();
    pack_with("glados", &[("task.complete", &["sounds/g.mp3"]), ("session.start", &["sounds/hello.mp3"])]);
    pack_with("axe", &[("task.complete", &["sounds/a.ogg"])]);
    let mut c = PeonConfig::default();
    c.raw.insert("ide_rules".into(), json!([{ "ide": "codex", "pack": "glados" }]));
    c.raw.insert("path_rules".into(), json!([{ "pattern": "*/games/*", "pack": "axe" }, { "pattern": "*", "pack": "missing" }]));
    c.save().unwrap();
    let stop = |h: Harness, cwd: &str, now: f64| decide(h, &json!({"hook_event_name":"Stop","session_id":"x","cwd":cwd}).to_string(), now).map(|p| p.pack);
    assert_eq!(stop(Harness::Claude, "/work/app", 100.0).as_deref(), Some("peon"));
    assert_eq!(stop(Harness::Codex, "/work/app", 200.0).as_deref(), Some("glados"));
    assert_eq!(stop(Harness::Codex, "/home/me/games/x", 300.0).as_deref(), Some("axe"), "a path rule beats the harness");
    // a pack picked for a session (peon-ping's skill) wins in session_override mode
    c.raw.insert("pack_rotation_mode".into(), json!("session_override"));
    c.save().unwrap();
    std::fs::write(home().join(".state.json"), json!({ "session_packs": { "codex-x": { "pack": "peon" } } }).to_string()).unwrap();
    assert_eq!(stop(Harness::Codex, "/home/me/games/x", 400.0).as_deref(), Some("peon"));
    // the default gone: the first installed
    c.use_pack("nope");
    c.raw.remove("ide_rules");
    c.raw.remove("path_rules");
    c.save().unwrap();
    std::fs::remove_dir_all(packs_dir().join("peon")).unwrap();
    assert_eq!(stop(Harness::Claude, "/w", 500.0).as_deref(), Some("axe"));
}

#[test]
fn globs_like_powershell() {
    assert!(glob("*/games/*", "C:\\Users\\me\\games\\doom"));
    assert!(glob("C:/Work*", "c:\\work\\app"));
    assert!(glob("a?c", "abc") && !glob("a?c", "abbc"));
    assert!(!glob("*/games", "/x/games/y"));
    assert!(glob("*", ""));
}

#[test]
fn manifests_never_point_outside_their_pack() {
    let m = pack::parse_manifest(
        r#"{"name":"x","author":{"name":"me"},"categories":{"task.complete":{"sounds":[{"file":"sounds/ok.wav"},{"file":"../../evil.wav"},{"file":"/etc/passwd"},{"file":"C:\\x.wav"}]},"session.start":["sounds/hi.mp3"]}}"#,
    )
    .unwrap();
    assert_eq!(m.author.as_deref(), Some("me"));
    assert_eq!(m.sounds("task.complete").iter().map(|s| s.file.as_str()).collect::<Vec<_>>(), ["sounds/ok.wav"]);
    assert_eq!(m.sounds("session.start")[0].file, "sounds/hi.mp3");
    assert!(pack::sound_path(Path::new("/p"), "../x.wav").is_none());
    assert!(pack::valid_name("hd2_helldiver") && pack::valid_name("abes-oddysee-abe"));
    assert!(!pack::valid_name("../x") && !pack::valid_name(".hidden") && !pack::valid_name("a/b") && !pack::valid_name(""));
}

#[test]
fn installed_packs_and_removal() {
    let _sb = sandbox();
    peon_pack();
    pack_with("glados", &[("task.complete", &["sounds/g.mp3"])]);
    std::fs::create_dir_all(packs_dir().join("no-manifest")).unwrap();
    let all = pack::installed();
    assert_eq!(all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["glados", "peon"]);
    assert_eq!((all[1].title(), all[1].sounds, all[1].covers("task.complete"), all[1].covers("session.end")), ("peon pack", 7, true, false));
    pack::remove("glados").unwrap();
    assert!(pack::remove("glados").is_err());
    assert!(pack::remove("../peon").is_err());
    assert_eq!(pack::installed_names(), ["peon"]);
    assert_eq!(sample(Some("task.complete")).map(|p| p.category), Some("task.complete"));
}

#[test]
fn play_args_round_trip() {
    let a = play_args(Path::new("/p/s.wav"), 0.25);
    assert_eq!(&a[..3], ["hook", "peon", "play"]);
    assert_eq!(parse_play(&a[3..]), Some((PathBuf::from("/p/s.wav"), 0.25)));
    assert_eq!(parse_play(&["x".into(), "9".into()]), Some((PathBuf::from("x"), 1.0)));
    assert_eq!(parse_play(&[]), None);
}

// ------------------------------------------------------------------ wiring

#[test]
fn claude_hooks_join_the_launch_settings() {
    let mut s = Map::new();
    s.insert("hooks".into(), json!({ "Stop": [{ "hooks": [{ "type": "command", "command": "mine" }] }] }));
    wire::add_claude_hooks(&mut s, Path::new("/opt/bro/bro"));
    let stop = s["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2, "next to the hooks already there");
    assert_eq!(stop[1]["hooks"][0]["args"], json!(["hook", "peon", "claude"]));
    assert_eq!(stop[1]["hooks"][0]["command"], "/opt/bro/bro");
    for e in wire::CLAUDE_EVENTS {
        assert!(s["hooks"][e].is_array(), "{e}");
    }
}

/// Matches Codex's own `version_for_toml` over the hook identity: this vector's hook was run by codex-cli
/// 0.164 from `-c` with only this trust (no `/hooks` review, no bypass flag).
#[test]
fn codex_trust_hash_matches_codex() {
    let cmd = r"echo fired>>C:\Users\james\AppData\Local\Temp\claude\F--bro-cli-v2\6acedb8f-daaa-4827-8a73-0210bb3f729c\scratchpad\fired.txt";
    assert_eq!(wire::codex_trust_hash("session_start", cmd, 10), "sha256:46afcecae9eafb0d6b13502d6f34529d3c0bd294a6b5e1de103cc75c29d20892");
}

#[test]
fn codex_args_hook_every_event_and_trust_them() {
    let args = wire::codex_args(Path::new("/opt/bro/bro"));
    let values: Vec<&String> = args.iter().skip(1).step_by(2).collect();
    assert!(args.iter().step_by(2).all(|a| a == "-c"));
    assert_eq!(values.len(), wire::CODEX_EVENTS.len() + 1);
    let cmd = wire::codex_command(Path::new("/opt/bro/bro"));
    assert!(cmd.ends_with(" hook peon codex"));
    for (i, (event, label)) in wire::CODEX_EVENTS.iter().enumerate() {
        let (key, value) = values[i].split_once('=').unwrap();
        assert_eq!(key, format!("hooks.{event}"));
        let parsed: toml::Table = format!("v = {value}").parse().unwrap();
        assert_eq!(parsed["v"][0]["hooks"][0]["command"].as_str(), Some(cmd.as_str()));
        assert_eq!(parsed["v"][0]["hooks"][0]["timeout"].as_integer(), Some(10));
        let state: toml::Table = format!("v = {}", values.last().unwrap().strip_prefix("hooks.state=").unwrap()).parse().unwrap();
        let source = if cfg!(windows) { r"C:\<session-flags>\config.toml" } else { "/<session-flags>/config.toml" };
        let key = format!("{source}:{label}:0:0");
        let entry = &state["v"][key.as_str()];
        assert_eq!(entry["trusted_hash"].as_str(), Some(wire::codex_trust_hash(label, &cmd, 10).as_str()));
    }
}

#[test]
fn logins_with_their_own_peon_ping_are_left_alone() {
    let sb = sandbox();
    let claude = sb.home().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    assert_eq!(wire::claude_own(&claude), wire::Own::None);
    let peon = r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"powershell -NoProfile -File \"C:\\Users\\me\\.claude\\hooks\\peon-ping\\peon.ps1\""},{"type":"command","command":"curl x"}]}],
        "UserPromptSubmit":[{"hooks":[{"type":"command","command":"powershell -File C:\\Users\\me\\.claude\\hooks\\peon-ping\\scripts\\hook-handle-use.ps1"}]}],
        "CwdChanged":[{"hooks":[{"type":"command","command":"other.ps1"}]}]},"theme":"dark"}"#;
    std::fs::write(claude.join("settings.json"), peon).unwrap();
    assert!(matches!(wire::claude_own(&claude), wire::Own::PeonPing(_)));

    // taking over: peon-ping's handlers go, the rest stays, bro's own go in everywhere
    let removed = wire::take_over_claude(&claude, Path::new("/opt/bro/bro")).unwrap();
    assert_eq!(removed, 2);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(claude.join("settings.json")).unwrap()).unwrap();
    assert_eq!(doc["theme"], "dark");
    assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], "curl x");
    assert_eq!(doc["hooks"]["Stop"][1]["hooks"][0]["args"], json!(["hook", "peon", "claude"]));
    assert_eq!(doc["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1, "the emptied group went, bro's came");
    assert_eq!(doc["hooks"]["CwdChanged"][0]["hooks"][0]["command"], "other.ps1");
    assert!(matches!(wire::claude_own(&claude), wire::Own::Bro(_)));
    assert_eq!(std::fs::read_to_string(wire::backup_path(&claude.join("settings.json"))).unwrap(), peon);
    // twice changes nothing more
    wire::take_over_claude(&claude, Path::new("/opt/bro/bro")).unwrap();
    let again: Value = serde_json::from_str(&std::fs::read_to_string(claude.join("settings.json")).unwrap()).unwrap();
    assert_eq!(again, doc);

    let codex = sb.home().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("config.toml"), "model = \"x\"\n").unwrap();
    assert_eq!(wire::codex_own(&codex), wire::Own::None);
    std::fs::write(codex.join("config.toml"), "notify = [\"bash\", \"/home/me/.claude/hooks/peon-ping/adapters/codex.sh\"]\n").unwrap();
    assert!(wire::codex_own(&codex).covered());

    assert_eq!(wire::pi_own(Harness::Omp), wire::Own::None);
    std::fs::create_dir_all(crate::paths::omp_agent_dir().join("extensions").join("peon-ping")).unwrap();
    assert!(wire::pi_own(Harness::Omp).covered());
    let logins = wire::logins();
    assert!(logins.iter().any(|l| l.harness == Harness::Claude && l.name == "local" && matches!(l.own, wire::Own::Bro(_))));
    assert!(logins.iter().any(|l| l.harness == Harness::Omp && l.own.covered()));
}

#[test]
fn the_extension_is_written_once_per_version() {
    let sb = sandbox();
    let (args, env) = wire::pi_launch(Harness::Omp, Path::new("/opt/bro/bro")).unwrap();
    assert_eq!(args[0], "-e");
    let file = PathBuf::from(&args[1]);
    assert!(file.starts_with(sb.bro().join("peon-ext")) && file.is_file());
    assert!(std::fs::read_to_string(&file).unwrap().contains("BRO_PEON_BIN"));
    assert!(env.contains(&("BRO_PEON_HARNESS".into(), "omp".into())));
    assert!(env.contains(&("BRO_PEON_BIN".into(), "/opt/bro/bro".into())));
}

// ------------------------------------------------------------------ registry

/// Serves `files` (path → body) on loopback until the test ends; returns the base URL.
fn serve(files: HashMap<String, Vec<u8>>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let mut out = stream;
            match files.get(&path) {
                Some(body) => {
                    let _ = write!(out, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = out.write_all(body);
                }
                None => {
                    let _ = write!(out, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
    });
    base
}

fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn registry_index_parses_sorts_and_searches() {
    let text = json!({ "version": 1, "packs": [
        { "name": "zelda", "display_name": "Ocarina", "source_repo": "a/b", "source_ref": "v1", "tags": ["nintendo"], "author": { "github": "link" } },
        { "name": "../bad", "source_repo": "a/b", "source_ref": "v1" },
        { "name": "abbot", "display_name": "The Abbot", "source_repo": "o/abbot", "source_ref": "v1.2.1", "source_path": ".", "description": "Stronghold monk", "preview_sounds": ["a.mp3", { "file": "sounds/b.mp3", "label": "B" }] },
        { "name": "odd", "source_repo": "a/b", "source_ref": "v1", "categories": "not a list" },
    ]})
    .to_string();
    let packs = registry::parse_index(&text).unwrap();
    assert_eq!(packs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["zelda", "abbot"], "by title; bad names and odd shapes dropped");
    assert_eq!(packs[1].preview_sounds, ["a.mp3", "b.mp3"], "names or {{file, label}}");
    assert!(packs[1].matches("stronghold MONK") && !packs[1].matches("zelda"));
    assert!(packs[0].matches("nintendo link"));
    assert_eq!(packs[1].base_url().unwrap(), "https://raw.githubusercontent.com/o/abbot/v1.2.1");
    let odd = registry::RegistryPack { name: "x".into(), source_repo: "../x".into(), source_ref: "v1".into(), ..Default::default() };
    assert!(odd.base_url().is_err());
    assert!(registry::outdated(Some("1.0.0"), Some("1.2.0")) && !registry::outdated(Some("v2.0"), Some("1.9.9")) && !registry::outdated(None, Some("1")));
    assert_eq!(registry::size_label(440_163), "429 KB");
}

#[test]
fn installs_check_every_file_and_land_whole() {
    let _sb = sandbox();
    let wav = b"RIFFdata".to_vec();
    let mp3 = b"ID3data".to_vec();
    let manifest = json!({ "name": "abbot", "version": "1.2.1", "categories": {
        "task.complete": { "sounds": [{ "file": "sounds/a.wav", "sha256": sha(&wav) }, { "file": "sounds/b.mp3", "sha256": sha(&mp3) }] },
        "session.start": { "sounds": [{ "file": "sounds/a.wav", "sha256": sha(&wav) }] } } })
    .to_string()
    .into_bytes();
    let mut files = HashMap::new();
    files.insert("/o/abbot/v1/openpeon.json".to_string(), manifest.clone());
    files.insert("/o/abbot/v1/sounds/a.wav".to_string(), wav.clone());
    files.insert("/o/abbot/v1/sounds/b.mp3".to_string(), mp3.clone());
    files.insert("/o/bad/v1/openpeon.json".to_string(), manifest.clone());
    files.insert("/o/bad/v1/sounds/a.wav".to_string(), b"tampered".to_vec());
    files.insert("/o/bad/v1/sounds/b.mp3".to_string(), mp3);
    let base = serve(files);
    unsafe { std::env::set_var("BRO_PEON_RAW", &base) };
    let p = registry::RegistryPack { name: "abbot".into(), source_repo: "o/abbot".into(), source_ref: "v1".into(), manifest_sha256: Some(sha(&manifest)), preview_sounds: vec!["b.mp3".into()], ..Default::default() };

    // a preview before installing comes from the cache
    let pv = registry::preview(&p, 0).unwrap();
    assert!(pv.ends_with("b.mp3") && pv.starts_with(crate::paths::bro_dir()));

    let mut steps = vec![];
    let dir = registry::install(&p, |d, t| steps.push((d, t))).unwrap();
    assert_eq!(steps, [(0, 2), (1, 2), (2, 2)]);
    assert_eq!(std::fs::read(dir.join("sounds/a.wav")).unwrap(), wav);
    assert_eq!(pack::installed_names(), ["abbot"]);
    // once installed, a preview plays its own copy
    assert!(registry::preview(&p, 0).unwrap().starts_with(packs_dir()));

    // a file that doesn't match its hash: nothing lands, nothing is left behind
    let bad = registry::RegistryPack { name: "bad".into(), source_repo: "o/bad".into(), ..p.clone() };
    let err = registry::install(&bad, |_, _| {}).unwrap_err().to_string();
    assert!(err.contains("SHA-256"), "{err}");
    // the registry's manifest hash is checked too
    let changed = registry::RegistryPack { manifest_sha256: Some("00".into()), ..p.clone() };
    assert!(registry::install(&changed, |_, _| {}).is_err());
    assert_eq!(pack::installed_names(), ["abbot"]);
    assert_eq!(std::fs::read_dir(packs_dir()).unwrap().count(), 1, "no staging folder left");

    // the index: fetched once, then from the cache
    let index = json!({ "packs": [{ "name": "abbot", "source_repo": "o/abbot", "source_ref": "v1" }] }).to_string();
    let reg = serve(HashMap::from([("/index.json".to_string(), index.into_bytes())]));
    unsafe { std::env::set_var("BRO_PEON_REGISTRY", format!("{reg}/index.json")) };
    assert_eq!(registry::load(false).unwrap().len(), 1);
    unsafe { std::env::set_var("BRO_PEON_REGISTRY", "http://127.0.0.1:9/gone") };
    assert_eq!(registry::load(false).unwrap().len(), 1, "fresh cache");
    assert_eq!(registry::load(true).unwrap().len(), 1, "a failed refresh falls back to the cache");
}

/// The live registry still has the shape bro reads.
#[test]
#[ignore = "network"]
fn live_registry_parses() {
    let _sb = sandbox();
    let packs = registry::load(true).unwrap();
    assert!(packs.len() > 100);
    assert!(packs.iter().any(|p| p.name == "peon"));
    assert!(packs.iter().all(|p| p.base_url().is_ok() || p.source_repo.is_empty()));
}

/// Codex runs bro's `-c` hooks with no `/hooks` review: point `BRO_TEST_CODEX` at the codex executable and
/// `BRO_TEST_BIN` at a built bro. The provider is a dead port, so nothing leaves the machine; SessionStart and
/// UserPromptSubmit still fire.
#[test]
#[ignore = "needs codex and a built bro (BRO_TEST_CODEX, BRO_TEST_BIN)"]
fn codex_runs_bros_hooks_without_asking() {
    let (Some(codex), Some(bro)) = (std::env::var_os("BRO_TEST_CODEX"), std::env::var_os("BRO_TEST_BIN")) else { return };
    let path = std::env::var_os("PATH");
    let sb = sandbox();
    unsafe {
        if let Some(p) = &path {
            std::env::set_var("PATH", p);
        }
        std::env::set_var("CLAUDE_PEON_DIR", sb.home().join("peon"));
    }
    peon_pack();
    PeonConfig::default().save().unwrap();
    let home = sb.home().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    let mut args: Vec<String> = ["exec", "--skip-git-repo-check", "-c", "model_provider=\"x\"", "-c", "model_providers.x.name=\"x\"", "-c", "model_providers.x.base_url=\"http://127.0.0.1:9/v1\"", "-c", "model_providers.x.wire_api=\"responses\""]
        .map(String::from)
        .to_vec();
    args.extend(wire::codex_args(Path::new(&bro)));
    args.push("hi".into());
    let mut child = std::process::Command::new(codex)
        .args(&args)
        .env("CODEX_HOME", &home)
        .current_dir(sb.home())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let state = home.parent().unwrap().join("peon").join(".state.json");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    let mut seen = None;
    while std::time::Instant::now() < deadline {
        if let Some(s) = crate::util::read_json(&state).filter(|s| s["prompt_timestamps"].as_object().is_some_and(|m| m.keys().any(|k| k.starts_with("codex-")))) {
            seen = Some(s);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    let s = seen.expect("codex ran bro's UserPromptSubmit hook");
    assert!(s["last_session_start_time"].as_i64().is_some(), "and its SessionStart: {s}");
}

/// Codex hands the command to PowerShell on Windows, where `"path" args` is only a string: the path goes in bare.
#[test]
fn codex_commands_run_in_every_shell() {
    let sb = sandbox();
    if cfg!(windows) {
        assert_eq!(wire::codex_command(Path::new(r"C:\Users\me\.bro\bin\bro.exe")), r"C:\Users\me\.bro\bin\bro.exe hook peon codex");
        let spaced = sb.home().join("my apps").join("bro.exe");
        std::fs::create_dir_all(spaced.parent().unwrap()).unwrap();
        std::fs::write(&spaced, b"").unwrap();
        let cmd = wire::codex_command(&spaced);
        assert!(cmd.ends_with(" hook peon codex"));
        let exe = cmd.trim_end_matches(" hook peon codex");
        // its 8.3 name where the volume keeps them, else PowerShell's call operator
        assert!(!exe.contains(' ') || exe.starts_with("& '"), "{cmd}");
        let missing = wire::codex_command(Path::new(r"C:\nowhere at all\bro.exe"));
        assert_eq!(missing, r"& 'C:\nowhere at all\bro.exe' hook peon codex");
    } else {
        assert_eq!(wire::codex_command(Path::new("/opt/my bro/bro")), "'/opt/my bro/bro' hook peon codex");
        assert_eq!(wire::codex_command(Path::new("/x/it's/bro")), r"'/x/it'\''s/bro' hook peon codex");
    }
}

/// A turn's hooks run at once; none of their state writes is lost.
#[test]
fn hooks_at_once_keep_every_update() {
    let _sb = sandbox();
    peon_pack();
    std::thread::scope(|s| {
        for i in 0..8 {
            s.spawn(move || decide(Harness::Claude, &ev("UserPromptSubmit", &format!("s{i}")), 50.0));
        }
    });
    let st = crate::util::read_json(&home().join(".state.json")).unwrap();
    assert_eq!(st["prompt_timestamps"].as_object().unwrap().len(), 8, "{st}");
    assert!(!home().join(".state.lock").exists());
}

/// A real pack from the registry installs whole, every file matching its hash, and plays from the packs folder.
#[test]
#[ignore = "network"]
fn live_pack_installs_and_plays_from_disk() {
    let _sb = sandbox();
    let packs = registry::load(true).unwrap();
    let p = packs.iter().find(|p| p.name == "abbot").or_else(|| packs.iter().find(|p| p.total_size_bytes.is_some_and(|b| b < 1_000_000))).unwrap();
    let dir = registry::install(p, |_, _| {}).unwrap();
    let m = pack::load_manifest(&dir).unwrap();
    assert!(m.files().iter().all(|s| pack::sound_path(&dir, &s.file).is_some()));
    let mut c = PeonConfig::default();
    c.use_pack(&p.name);
    c.save().unwrap();
    let cat = m.categories.iter().find(|(_, s)| !s.is_empty()).map(|(c, _)| c.clone()).unwrap();
    assert!(sample(Some(&cat)).is_some_and(|s| s.path.starts_with(&dir)));
    assert!(registry::preview(p, 1).unwrap().starts_with(&dir));
}
