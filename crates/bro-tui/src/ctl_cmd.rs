//! `bro ctl [<tool> [key=value… | json]]` — call bro's session tools (the bridge's `/mcp` endpoint) from a
//! shell. The orchestrator uses it on harnesses without an MCP client (Pi, omp) and as a fallback anywhere;
//! you can use it too. Talks to the running bro's bridge on loopback; never starts the TUI.

use serde_json::{Map, Value, json};
use std::time::Duration;

/// Tools may wait up to 45 s for sessions; leave room for that.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Parsed `bro ctl` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CtlArgs {
    /// None: list the tools.
    pub tool: Option<String>,
    pub arguments: Value,
}

// JSON numbers are never NaN, so equality is total.
impl Eq for CtlArgs {}

pub const HELP: &str = "bro ctl — drive bro's sessions from a shell (the orchestrator's tools)

usage
  bro ctl                         list every tool with its arguments
  bro ctl <tool> [key=value …]    call one: values that parse as JSON (numbers, true/false,
                                  arrays) are sent as JSON, anything else as text
  bro ctl <tool> '<json object>'  or pass the arguments as one JSON object

examples
  bro ctl list_sessions
  bro ctl read_chat sessionId=Toast messages=20
  bro ctl send_input sessionId=Toast text=\"run the tests\"
  bro ctl open_session project=justgains agent=codex task=\"add a loading state\"
  bro ctl wait_for_sessions '{\"sessionIds\":[\"Toast\",\"Pinky\"],\"mode\":\"any\"}'

Finds bro through $BRO_BRIDGE_URL (set in every bro pane), else the bridge's discovery file.
Exits 1 when the tool reports an error.
";

/// `None` = print [`HELP`].
pub fn parse(args: &[String]) -> Result<Option<CtlArgs>, String> {
    let Some(first) = args.first() else { return Ok(Some(CtlArgs { tool: None, arguments: json!({}) })) };
    if matches!(first.as_str(), "-h" | "--help" | "help") {
        return Ok(None);
    }
    if first.starts_with('-') {
        return Err(format!("unknown option '{first}' (bro ctl --help)"));
    }
    let tool = first.clone();
    let rest = &args[1..];
    if let [one] = rest
        && one.trim_start().starts_with('{')
    {
        let v: Value = serde_json::from_str(one).map_err(|e| format!("the arguments aren't valid JSON: {e}"))?;
        return Ok(Some(CtlArgs { tool: Some(tool), arguments: v }));
    }
    let mut map = Map::new();
    for a in rest {
        let (k, v) = a.split_once('=').ok_or_else(|| format!("'{a}' isn't key=value (or pass one JSON object)"))?;
        let k = k.trim();
        if k.is_empty() {
            return Err(format!("'{a}' has no key"));
        }
        // JSON when it parses as anything but a bare string; text otherwise ("Toast", "run the tests")
        let value = match serde_json::from_str::<Value>(v) {
            Ok(Value::String(_)) | Err(_) => Value::String(v.to_string()),
            Ok(parsed) => parsed,
        };
        map.insert(k.to_string(), value);
    }
    Ok(Some(CtlArgs { tool: Some(tool), arguments: Value::Object(map) }))
}

/// One JSON-RPC call to `<bridge>/mcp`.
fn rpc(bridge: &str, method: &str, params: Value) -> Result<Value, String> {
    let url = format!("{}/mcp", bridge.trim_end_matches('/'));
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp = bro_core::http::post_json(&url, &body, TIMEOUT).map_err(|e| format!("bro isn't reachable at {bridge} ({})", e.chain().last().map(|c| c.to_string()).unwrap_or_default()))?;
    if !resp.ok() {
        return Err(format!("bro answered HTTP {}", resp.status));
    }
    if let Some(err) = resp.body.get("error") {
        return Err(err["message"].as_str().unwrap_or("the call failed").to_string());
    }
    Ok(resp.body["result"].clone())
}

/// The tool list as help text: name, description, then its arguments.
fn describe(tools: &Value) -> String {
    let mut out = String::new();
    for t in tools.as_array().into_iter().flatten() {
        let name = t["name"].as_str().unwrap_or_default();
        out.push_str(&format!("{name}\n  {}\n", t["description"].as_str().unwrap_or_default()));
        let required: Vec<&str> = t["inputSchema"]["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if let Some(props) = t["inputSchema"]["properties"].as_object() {
            for (k, p) in props {
                let kind = p["type"].as_str().unwrap_or("any");
                let need = if required.contains(&k.as_str()) { ", required" } else { "" };
                let what = p["description"].as_str().map(|d| format!(": {d}")).unwrap_or_default();
                out.push_str(&format!("    {k} ({kind}{need}){what}\n"));
            }
        }
        out.push('\n');
    }
    out
}

/// Run it. Ok = what to print on stdout; Err = the failure (the tool's own error text when it reports one).
pub fn run(a: &CtlArgs, bridge: Option<&str>) -> Result<String, String> {
    let bridge = bridge.ok_or("bro isn't running (no bridge found) — start bro, or turn its bridge on")?;
    let Some(tool) = &a.tool else {
        let listed = rpc(bridge, "tools/list", json!({}))?;
        return Ok(describe(&listed["tools"]));
    };
    let result = rpc(bridge, "tools/call", json!({ "name": tool, "arguments": a.arguments }))?;
    let text = result["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
    if result["isError"].as_bool() == Some(true) {
        return Err(text);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Option<CtlArgs>, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_pairs_json_and_help() {
        assert_eq!(args(&[]).unwrap().unwrap().tool, None);
        assert_eq!(args(&["--help"]), Ok(None));
        let a = args(&["send_input", "sessionId=Toast", "text=run the tests", "submit=false", "lines=40"]).unwrap().unwrap();
        assert_eq!(a.tool.as_deref(), Some("send_input"));
        assert_eq!(a.arguments, json!({ "sessionId": "Toast", "text": "run the tests", "submit": false, "lines": 40 }));
        let a = args(&["wait_for_sessions", r#"{"sessionIds":["a"],"mode":"any"}"#]).unwrap().unwrap();
        assert_eq!(a.arguments["sessionIds"], json!(["a"]));
        // a quoted JSON string stays the text it was typed as
        assert_eq!(args(&["x", "text=\"hi\""]).unwrap().unwrap().arguments["text"], "\"hi\"");
        assert!(args(&["x", "oops"]).is_err());
        assert!(args(&["--nope"]).is_err());
    }

    #[test]
    fn calls_the_running_bridge() {
        crate::testkit::isolate();
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = bro_bridge::BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: dir.path().join("bridge"), web_interface: false };
        let bridge = bro_bridge::Bridge::start_with(cfg, bro_bridge::StartOptions { import_legacy_token: false }, Box::new(move |c| {
            let _ = tx.send(c);
        }))
        .unwrap();
        let url = bridge.local_url().unwrap();
        let listed = run(&CtlArgs { tool: None, arguments: json!({}) }, Some(&url)).unwrap();
        assert!(listed.contains("list_sessions") && listed.contains("sessionId (string, required)"), "{listed}");
        let sessions = run(&args(&["list_sessions"]).unwrap().unwrap(), Some(&url)).unwrap();
        assert_eq!(sessions.trim(), "[]");
        let err = run(&args(&["read_session", "sessionId=nobody"]).unwrap().unwrap(), Some(&url)).unwrap_err();
        assert!(err.contains("No session matches"), "{err}");
        assert!(run(&args(&["list_sessions"]).unwrap().unwrap(), None).unwrap_err().contains("isn't running"));
        bridge.shutdown();
    }

    /// The orchestrator's real wiring end to end: a live bridge with one worker session, and the real Claude Code
    /// and Codex CLIs given exactly the prompt and MCP settings an orchestrator launch gets. Each must find the
    /// worker through bro's tools and type into it. Runs on your logins (network); `--ignored` to run.
    #[test]
    #[ignore = "runs the real claude and codex CLIs (network, uses your logins)"]
    fn real_agents_drive_bro_through_the_orchestrator_wiring() {
        use bro_core::Harness;
        use bro_core::orchestrator as orch;
        crate::testkit::isolate();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = bro_bridge::BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: dir.path().join("bridge"), web_interface: false };
        let bridge = bro_bridge::Bridge::start_with(cfg, bro_bridge::StartOptions { import_legacy_token: false }, Box::new(move |c| {
            let _ = tx.send(c);
        }))
        .unwrap();
        let url = bridge.local_url().unwrap();
        let meta = |id: &str, title: &str, kind: Option<&str>| bro_bridge::SessionMeta { id: id.into(), title: title.into(), shell: "claude".into(), args: vec![], cwd: Some(dir.path().to_path_buf()), project: None, pid: None, cols: 100, rows: 30, agent: Some("claude".into()), kind: kind.map(String::from) };
        bridge.register(meta("worker-1", "Toast · demo", None));
        bridge.register(meta("boss-1", "orchestrator", Some("orchestrator")));
        let ask = "Use your bro tools: find the session named Toast and type exactly the text PING-FROM-ORCHESTRATOR into it (submit it). Then reply with the word DONE.";

        let typed_into_toast = |rx: &std::sync::mpsc::Receiver<bro_bridge::BridgeCommand>| {
            let until = std::time::Instant::now() + Duration::from_secs(5);
            let mut text = String::new();
            while let Ok(c) = rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
                if let bro_bridge::BridgeCommand::Input { id, data } = c {
                    assert_eq!(id, "worker-1", "never into the orchestrator itself");
                    text.push_str(&String::from_utf8_lossy(&data));
                }
            }
            text
        };

        // Claude Code: --append-system-prompt-file + --mcp-config + --allowedTools mcp__bro
        let prompt = orch::write_prompt(Harness::Claude).unwrap();
        let mcp = orch::write_mcp_config(&url).unwrap();
        let (program, cargs) = bro_core::launch::program_and_args("claude", vec![
            "-p".into(), ask.into(), "--model".into(), "sonnet".into(),
            "--append-system-prompt-file".into(), prompt.to_string_lossy().into_owned(),
            "--allowedTools".into(), "mcp__bro".into(),
            "--mcp-config".into(), mcp.to_string_lossy().into_owned(),
        ]);
        let out = std::process::Command::new(program).args(cargs).current_dir(dir.path()).env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT").output().unwrap();
        let reply = String::from_utf8_lossy(&out.stdout);
        assert!(reply.contains("DONE"), "claude: {reply}\n{}", String::from_utf8_lossy(&out.stderr));
        let typed = typed_into_toast(&rx);
        assert!(typed.contains("PING-FROM-ORCHESTRATOR"), "claude typed {typed:?}");

        // Codex: -c mcp_servers.bro.* + developer_instructions
        let mut cargs = vec!["exec".to_string(), "--skip-git-repo-check".into(), "--dangerously-bypass-approvals-and-sandbox".into()];
        cargs.extend(orch::codex_mcp_args(&url));
        cargs.extend(["-c".into(), format!("developer_instructions={}", serde_json::to_string(&orch::prompt(Harness::Codex)).unwrap())]);
        cargs.push(ask.into());
        let (program, cargs) = bro_core::launch::program_and_args("codex", cargs);
        let out = std::process::Command::new(program).args(cargs).current_dir(dir.path()).stdin(std::process::Stdio::null()).output().unwrap();
        let reply = String::from_utf8_lossy(&out.stdout);
        assert!(reply.contains("DONE"), "codex: {reply}\n{}", String::from_utf8_lossy(&out.stderr));
        let typed = typed_into_toast(&rx);
        assert!(typed.contains("PING-FROM-ORCHESTRATOR"), "codex typed {typed:?}");
        bridge.shutdown();
    }
}
