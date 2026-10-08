//! `bro open <path>[:line[:col]] [--session <id>] [--editor]` — run inside a bro pane (by you or an agent) to
//! show a file to the user: bro's preview on the desktop plus the web and phone clients (or, with `--editor`,
//! the desktop's editor). Talks to the running bro's bridge on loopback; never starts the TUI.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Parsed `bro open` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenArgs {
    pub target: String,
    pub session: Option<String>,
    pub editor: bool,
}

pub const HELP: &str = "bro open — show a file to the user from inside a bro session

usage
  bro open <path>[:line[:col]] [--editor] [--session <id>]

  <path>        relative to where you run it (or the session's folder), or absolute;
                src/app.rs:42, src/app.rs:42:7, src/app.rs#L42 and \"src/app.rs(42,7)\" all work
  --editor, -e  open it in the desktop's editor instead of bro's preview
  --session id  the bro session to open it in (default: $BRO_SESSION_ID, set in every bro pane)

The preview opens in bro if you're looking at that session (else bro says so: prefix f shows it),
and in the web and phone clients connected to bro.
";

/// `None` = print [`HELP`].
pub fn parse(args: &[String]) -> Result<Option<OpenArgs>, String> {
    let (mut target, mut session, mut editor) = (None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "-e" | "--editor" => editor = true,
            "-s" | "--session" => session = Some(it.next().cloned().ok_or("--session needs an id")?),
            s if s.starts_with("--session=") => session = Some(s["--session=".len()..].to_string()),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option '{s}' (bro open --help)")),
            s if target.is_none() => target = Some(s.to_string()),
            s => return Err(format!("unexpected argument '{s}' — one file at a time (quote paths with spaces)")),
        }
    }
    let target = target.filter(|t| !t.trim().is_empty()).ok_or("which file? bro open <path>[:line]")?;
    Ok(Some(OpenArgs { target, session, editor }))
}

/// Where `bro open` runs: the session, the bridge and the working directory.
pub struct Context {
    pub session: Option<String>,
    /// `http://127.0.0.1:<port>`
    pub bridge: Option<String>,
    pub cwd: PathBuf,
}

impl Context {
    /// `BRO_SESSION_ID`, `BRO_BRIDGE_URL` (else the bridge's discovery file under `~/.bro/bridge`), the cwd.
    pub fn from_env() -> Context {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Context {
            session: var("BRO_SESSION_ID"),
            bridge: var("BRO_BRIDGE_URL").or_else(|| bro_bridge::read_endpoint(&crate::util::bro_dir().join("bridge")).map(|e| e.local_url())),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// What to send for `raw`: an existing path made absolute against the cwd (the agent may have `cd`ed away
/// from the session's folder); anything else as typed, for the bridge to resolve (`a/` prefixes, suffixes).
fn wire_path(raw: &str, cwd: &Path) -> String {
    let p = Path::new(raw);
    if p.is_relative() && !raw.starts_with('~') {
        let abs = cwd.join(p);
        if abs.exists() {
            return abs.to_string_lossy().into_owned();
        }
    }
    raw.to_string()
}

/// Ask bro to open it. Ok = a one-line confirmation for stdout.
pub fn run(a: &OpenArgs, ctx: &Context) -> Result<String, String> {
    let session = a.session.clone().or_else(|| ctx.session.clone()).ok_or("not inside a bro session ($BRO_SESSION_ID isn't set) — run it in a bro pane or pass --session <id>")?;
    let base = ctx.bridge.clone().ok_or("bro isn't running (no bridge found) — start bro, or turn its bridge on")?;
    let (path, line, column) = bro_bridge::file_links::split_location(a.target.trim());
    let mut body = serde_json::json!({ "path": wire_path(&path, &ctx.cwd), "source": "agent" });
    if let Some(l) = line {
        body["line"] = l.into();
    }
    if let Some(c) = column {
        body["column"] = c.into();
    }
    let id: String = url::form_urlencoded::byte_serialize(session.as_bytes()).collect();
    let route = if a.editor { "open-in-editor" } else { "open" };
    let url = format!("{}/api/sessions/{id}/{route}", base.trim_end_matches('/'));
    let resp = bro_core::http::post_json(&url, &body, Duration::from_secs(8)).map_err(|e| format!("bro isn't reachable at {base} ({})", root_cause(&e)))?;
    let shown = match line {
        Some(l) => format!("{path}:{l}"),
        None => path.clone(),
    };
    if resp.ok() {
        return Ok(if a.editor {
            format!("opened {shown} in the desktop editor")
        } else {
            let clients = resp.body["clients"].as_u64().unwrap_or(0);
            let also = match clients {
                0 => String::new(),
                1 => " and 1 connected client".to_string(),
                n => format!(" and {n} connected clients"),
            };
            format!("opened {shown} in bro{also}")
        });
    }
    let message = resp.body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {}", resp.status));
    Err(match resp.status {
        404 if message.to_lowercase().contains("session") => format!("{message} (is {session} a session in the running bro?)"),
        _ => format!("{shown}: {message}"),
    })
}

fn root_cause(e: &anyhow::Error) -> String {
    e.chain().last().map(|c| c.to_string()).unwrap_or_else(|| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn args(a: &[&str]) -> Result<Option<OpenArgs>, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(args(&["src/a.rs:4"]), Ok(Some(OpenArgs { target: "src/a.rs:4".into(), session: None, editor: false })));
        assert_eq!(args(&["-e", "--session", "s1", "x.md"]), Ok(Some(OpenArgs { target: "x.md".into(), session: Some("s1".into()), editor: true })));
        assert_eq!(args(&["--session=s2", "x.md"]).unwrap().unwrap().session.as_deref(), Some("s2"));
        assert_eq!(args(&["--help"]), Ok(None));
        assert!(args(&[]).is_err());
        assert!(args(&["a", "b"]).is_err());
        assert!(args(&["--nope", "a"]).is_err());
        assert!(args(&["--session"]).is_err());
    }

    /// One HTTP exchange on loopback: returns (port, the request it got, as text).
    fn fake_bridge(status: &str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut buf = vec![0; length];
            reader.read_exact(&mut buf).unwrap();
            let mut stream = stream;
            write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
            format!("{head}{}", String::from_utf8_lossy(&buf))
        });
        (port, handle)
    }

    #[test]
    fn posts_the_file_and_line_to_the_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.rs"), "x").unwrap();
        let (port, server) = fake_bridge("200 OK", r#"{"opened":true,"clients":2}"#);
        let ctx = Context { session: Some("abc".into()), bridge: Some(format!("http://127.0.0.1:{port}")), cwd: dir.path().to_path_buf() };
        let out = run(&parse_one("src/app.rs:42:7"), &ctx).unwrap();
        assert_eq!(out, "opened src/app.rs:42 in bro and 2 connected clients");
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/sessions/abc/open HTTP/1.1"), "{req}");
        let json: serde_json::Value = serde_json::from_str(req.rsplit("\r\n\r\n").next().unwrap()).unwrap();
        assert_eq!((json["line"].as_u64(), json["column"].as_u64(), json["source"].as_str()), (Some(42), Some(7), Some("agent")));
        assert_eq!(Path::new(json["path"].as_str().unwrap()), dir.path().join("src/app.rs"), "an existing relative path goes absolute");
    }

    #[test]
    fn editor_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        let (port, server) = fake_bridge("204 No Content", "");
        let ctx = Context { session: Some("abc".into()), bridge: Some(format!("http://127.0.0.1:{port}/")), cwd: dir.path().to_path_buf() };
        let a = OpenArgs { target: "a/src/x.rs".into(), session: None, editor: true };
        assert_eq!(run(&a, &ctx).unwrap(), "opened a/src/x.rs in the desktop editor");
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/sessions/abc/open-in-editor "), "{req}");
        let json: serde_json::Value = serde_json::from_str(req.rsplit("\r\n\r\n").next().unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({ "path": "a/src/x.rs", "source": "agent" }), "a path that isn't there is sent as typed");

        let (port, server) = fake_bridge("404 Not Found", r#"{"message":"File not found or unavailable."}"#);
        let ctx = Context { bridge: Some(format!("http://127.0.0.1:{port}")), ..ctx };
        assert_eq!(run(&parse_one("nope.rs:3"), &ctx).unwrap_err(), "nope.rs:3: File not found or unavailable.");
        server.join().unwrap();

        let none = Context { session: None, bridge: Some("http://127.0.0.1:9".into()), cwd: dir.path().to_path_buf() };
        assert!(run(&parse_one("x"), &none).unwrap_err().contains("BRO_SESSION_ID"));
        let down = Context { session: Some("abc".into()), bridge: None, cwd: dir.path().to_path_buf() };
        assert!(run(&parse_one("x"), &down).unwrap_err().contains("isn't running"));
    }

    /// The real thing: a bridge on loopback, found through its discovery file, hands bro the resolved file.
    #[test]
    fn a_running_bridge_turns_bro_open_into_an_open_file_command() {
        crate::testkit::isolate();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crates/x/src")).unwrap();
        std::fs::write(dir.path().join("crates/x/src/lib.rs"), "pub fn x() {}\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = bro_bridge::BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: dir.path().join("bridge"), web_interface: false };
        let bridge = bro_bridge::Bridge::start_with(cfg, bro_bridge::StartOptions { import_legacy_token: false }, Box::new(move |c| {
            let _ = tx.send(c);
        }))
        .unwrap();
        bridge.register(bro_bridge::SessionMeta { id: "s1".into(), title: "t".into(), shell: "sh".into(), args: vec![], cwd: Some(dir.path().to_path_buf()), project: Some(dir.path().to_path_buf()), pid: None, cols: 80, rows: 24, agent: Some("claude".into()) });
        let found = bro_bridge::read_endpoint(&dir.path().join("bridge")).expect("discovery file");
        // from a subfolder the agent cd'ed into, and by a unique suffix the bridge resolves
        let ctx = Context { session: Some("s1".into()), bridge: Some(found.local_url()), cwd: dir.path().join("crates/x") };
        assert_eq!(run(&parse_one("src/lib.rs:1"), &ctx).unwrap(), "opened src/lib.rs:1 in bro");
        let ctx = Context { cwd: dir.path().to_path_buf(), ..ctx };
        run(&parse_one("x/src/lib.rs#L1C5"), &ctx).unwrap();
        let want = dir.path().join("crates/x/src/lib.rs").canonicalize().map(crate::util::strip_verbatim).unwrap();
        for (line, column) in [(Some(1), None), (Some(1), Some(5))] {
            match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
                bro_bridge::BridgeCommand::OpenFile { id, path, line: l, column: c } => assert_eq!((id.as_str(), path, l, c), ("s1", want.clone(), line, column)),
                other => panic!("{other:?}"),
            }
        }
        assert!(run(&parse_one("missing.rs"), &ctx).unwrap_err().contains("not found"));
        let editor = OpenArgs { target: "src/lib.rs:1".into(), session: None, editor: true };
        run(&editor, &Context { cwd: dir.path().join("crates/x"), ..ctx }).unwrap();
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), bro_bridge::BridgeCommand::OpenInEditor { .. }));
        bridge.shutdown();
    }

    fn parse_one(target: &str) -> OpenArgs {
        OpenArgs { target: target.into(), session: None, editor: false }
    }
}
