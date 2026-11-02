//! Cross-process contract: Orbit's actual voice server, host client and tool
//! runner against bro's real loopback host, workspace and scoped readers. A
//! scripted realtime provider stands in for the model; nothing else is faked.
//!
//! Ignored by default because it needs Node and an Orbit checkout:
//! `BRO_ORBIT_SOURCE=F:/orbit-voice-upstream cargo test -p bro-bridge orbit_client -- --ignored --nocapture`
use super::*;
use std::process::Stdio;
use std::sync::mpsc;

const SCROLLBACK: &str = "sibling scrollback: 41 tests passed";
const FOREIGN: &str = "foreign project secret";
const CHAT_ID: &str = "4b1d0c2e-5a6f-4c3b-9d8e-7f6a5b4c3d2e";
const RESULT_TEXT: &str = "Coordinator finished: the unit tests passed";

fn plant_chat(claude: &Path, project: &Path) {
    let slug: String = project.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let file = claude.join("projects").join(slug).join(format!("{CHAT_ID}.jsonl"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let cwd = project.to_string_lossy();
    let lines = [
        json!({"type":"user","cwd":cwd,"sessionId":CHAT_ID,"message":{"role":"user","content":"Which schema did we pick for the inbox?"}}),
        json!({"type":"assistant","cwd":cwd,"sessionId":CHAT_ID,"message":{"role":"assistant","content":[{"type":"text","text":"We kept schema 1 for the durable inbox."}]}}),
        json!({"type":"assistant","pad":"x".repeat(600)}),
    ];
    std::fs::write(file, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
}

#[test]
#[ignore = "needs Node and an Orbit checkout in BRO_ORBIT_SOURCE"]
fn orbit_voice_tool_calls_reach_bros_real_host() {
    let Some(orbit) = std::env::var_os("BRO_ORBIT_SOURCE").map(PathBuf::from) else {
        panic!("Set BRO_ORBIT_SOURCE to an Orbit checkout with node_modules installed");
    };
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let claude = home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    // SAFETY: run on its own (`--ignored` with this name filter); no other test thread reads these.
    unsafe {
        for (key, value) in [
            ("HOME", home.clone()), ("USERPROFILE", home.clone()), ("CLAUDE_CONFIG_DIR", claude.clone()),
            ("CLAUDE_POOL_DIR", home.join(".claude-max-pool")), ("BRO_DIR", home.join(".bro")),
            ("BRO_CODEX_PROFILES_DIR", home.join(".bro/codex-profiles")), ("CODEX_HOME", home.join(".codex")),
            ("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
        ] { std::env::set_var(key, value); }
    }
    let a = root.path().join("project-a");
    let b = root.path().join("project-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    plant_chat(&claude, &a);

    let terminal_commands = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sink = terminal_commands.clone();
    let workspace = crate::Bridge::start_local(
        crate::BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().join("bridge"), web_interface: false },
        Box::new(move |_| { sink.fetch_add(1, Ordering::Relaxed); }),
    ).unwrap();
    for (id, project) in [("source", &a), ("sibling", &a), ("foreign", &b)] {
        workspace.register(crate::SessionMeta { id: id.into(), title: format!("Tab {id}"), shell: "fixture".into(), args: vec![],
            cwd: Some(project.clone()), project: Some(project.clone()), pid: Some(42), cols: 80, rows: 24, agent: None, kind: None });
    }
    workspace.bind_voice_generations(&[("source", "g-source"), ("sibling", "g-sibling"), ("foreign", "g-foreign")]);
    workspace.output("sibling", format!("{SCROLLBACK}\r\n").as_bytes());
    workspace.output("foreign", format!("{FOREIGN}\r\n").as_bytes());

    // bro's real dispatcher: durably queue, then report the coordinator's reply.
    let (tx, rx) = mpsc::channel::<Command>();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let seen = commands.clone();
    let handle = start(root.path().join("bro"), move |command| { seen.lock().push(command.clone()); let _ = tx.send(command); }).unwrap();
    {
        let handle = handle.clone();
        std::thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                if let Command::Submit { id, .. } = command {
                    assert!(handle.queued(&id));
                    std::thread::sleep(Duration::from_millis(300));
                    assert!(handle.finish(&id, RESULT_TEXT, true));
                }
            }
        });
    }
    handle.attach_workspace(Some(workspace.clone()));
    let scope = |p: &Path| bro_core::orbit_voice::project_scope_id(p).unwrap();
    assert!(handle.publish_host(json!({
        "protocol": 1, "instanceId": handle.instance_id(), "name": "bro", "mode": "desktop", "window": null,
        "capabilities": CAPABILITIES, "focusedSessionId": "source",
        "sessions": [
            {"id":"source","generation":"g-source","scope":scope(&a),"title":"Tab source","agent":"claude","activity":"idle"},
            {"id":"sibling","generation":"g-sibling","scope":scope(&a),"title":"Tab sibling","agent":"claude","activity":"working"},
            {"id":"foreign","generation":"g-foreign","scope":scope(&b),"title":"Tab foreign","agent":"codex","activity":"idle"},
        ],
    })));

    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/orbit_voice/tests/orbit_client.mjs");
    let register = orbit.join("test/fuzz/register.mjs");
    let file_url = |p: &Path| format!("file:///{}", p.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
    let mut child = std::process::Command::new(std::env::var_os("BRO_NODE").unwrap_or_else(|| "node".into()))
        .arg("--import").arg(file_url(&register))
        .arg(&driver).arg(handle.descriptor_path()).arg(&orbit).arg(root.path().join("orbit"))
        .arg(serde_json::to_string(&json!({"sibling":SCROLLBACK,"foreign":FOREIGN,"chatId":CHAT_ID,"result":RESULT_TEXT})).unwrap())
        .stdout(Stdio::piped()).stderr(Stdio::inherit())
        .spawn().expect("node");
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() { break status; }
        if Instant::now() > deadline { let _ = child.kill(); panic!("Orbit driver timed out"); }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();
    assert!(status.success(), "Orbit driver failed:\n{stdout}");
    let summary: Value = serde_json::from_str(stdout.lines().rev().find_map(|line| line.strip_prefix("RESULT ")).expect("driver summary")).unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&summary).unwrap());

    // The draft entered bro's dispatcher once, pinned to the session viewed at speech onset.
    let submitted = commands.lock().iter().filter_map(|command| match command {
        Command::Submit { id, text, focused, generation, .. } => Some((id.clone(), text.clone(), focused.clone(), generation.clone())),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(submitted.len(), 1, "{submitted:?}");
    assert_eq!(submitted[0].1, "Run the unit tests");
    assert_eq!(submitted[0].2.as_deref(), Some("source"));
    assert_eq!(submitted[0].3.as_deref(), Some("g-source"));
    assert_eq!(summary["submittedRequestId"], json!(submitted[0].0));
    assert_eq!(terminal_commands.load(Ordering::Relaxed), 0, "voice tools never type into a terminal");

    handle.shutdown();
    workspace.shutdown();
}
