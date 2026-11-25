//! Shell access to JustBrowse for harnesses without MCP. Each pane retains its own
//! MCP session, including tab ownership, across CLI invocations.
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn response(text: &str, id: u64) -> anyhow::Result<Value> {
    let messages = serde_json::from_str::<Value>(text).ok().into_iter().chain(
        text.lines()
            .filter_map(|line| line.strip_prefix("data:").and_then(|s| serde_json::from_str(s.trim()).ok())),
    );
    let reply = messages
        .into_iter()
        .find(|v| v["id"].as_u64() == Some(id))
        .context("JustBrowse returned no matching MCP response")?;
    if let Some(error) = reply.get("error") {
        bail!("JustBrowse: {}", error["message"].as_str().unwrap_or("MCP request failed"));
    }
    reply.get("result").cloned().context("JustBrowse returned no MCP result")
}

fn post(url: &str, session: Option<&str>, body: &Value) -> anyhow::Result<(u16, Option<String>, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-03-26");
    if let Some(pane) = crate::util::env_str("BRO_SESSION_ID") {
        request = request.header("x-bro-session-id", &pane);
    }
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    let mut result = request
        .send_json(body)
        .context("JustBrowse is not reachable; open its extension and click Connect")?;
    let status = result.status().as_u16();
    let session = result
        .headers()
        .get("mcp-session-id")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let text = result.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string()?;
    Ok((status, session, text))
}

fn connect(url: &str) -> anyhow::Result<Option<String>> {
    let (status, session, text) = post(
        url,
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"bro","version":env!("CARGO_PKG_VERSION")}
        }}),
    )?;
    if status != 200 {
        bail!("JustBrowse initialization returned HTTP {status}; reconnect its extension");
    }
    response(&text, 1)?;
    let (status, _, _) = post(
        url,
        session.as_deref(),
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )?;
    if !(200..300).contains(&status) {
        bail!("JustBrowse initialization returned HTTP {status}");
    }
    Ok(session)
}

fn rpc(url: &str, state: &Path, method: &str, params: Value) -> anyhow::Result<Value> {
    let mut session = std::fs::read_to_string(state).ok().filter(|s| !s.trim().is_empty());
    if session.is_none() {
        session = connect(url)?;
        crate::util::atomic_write(state, session.as_deref().unwrap_or("").as_bytes())?;
    }
    let body = json!({"jsonrpc":"2.0","id":2,"method":method,"params":params});
    let (mut status, _, mut text) = post(url, session.as_deref(), &body)?;
    if status == 404 {
        // MCP 404 means the session expired, so this request never ran. Retry only that case.
        session = connect(url)?;
        crate::util::atomic_write(state, session.as_deref().unwrap_or("").as_bytes())?;
        let retry = post(url, session.as_deref(), &body)?;
        status = retry.0;
        text = retry.2;
    }
    if status != 200 {
        bail!("JustBrowse returned HTTP {status}; check its connection in Bro");
    }
    response(&text, 2)
}

pub fn call(tool: Option<&str>, arguments: Value) -> anyhow::Result<Value> {
    let enabled = match crate::util::env_str("BRO_BROWSER_PROVIDER").as_deref() {
        Some("justbrowse") => crate::justbrowse::installed().is_some(),
        Some("off") => false,
        _ => crate::automation::Active::load().browser,
    };
    if !enabled {
        bail!("Install and enable JustBrowse in Bro's Browser and computer settings first");
    }
    let pane =
        crate::util::env_str("BRO_SESSION_ID").context("Run `bro browse` inside a Bro pane so browser tabs belong to that session")?;
    let dir = crate::paths::bro_dir().join("browser-cli").join(crate::util::short_hash(&pane, 24));
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("connection.lock"))?;
    lock.lock()?;
    let (method, params) = match tool {
        Some(name) => ("tools/call", json!({"name":name,"arguments":arguments})),
        None => ("tools/list", json!({})),
    };
    let mut result = rpc(crate::justbrowse::MCP_URL, &dir.join("session"), method, params)?;
    if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
        use base64::Engine;
        for item in content {
            if item["type"] == "image" {
                let data = item["data"].as_str().context("JustBrowse returned an image without data")?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                let ext = match item["mimeType"].as_str() {
                    Some("image/jpeg") => "jpg",
                    Some("image/webp") => "webp",
                    _ => "png",
                };
                let path = dir.join(format!("image-{:x}.{ext}", rand::random::<u64>()));
                crate::util::atomic_write(&path, &bytes)?;
                *item = json!({"type":"text","text":format!("Screenshot saved to {}", path.display())});
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    #[test]
    fn handles_json_sse_and_errors_without_mistaking_notifications_for_results() {
        assert_eq!(response("{\"id\":2,\"result\":{\"tools\":[]}}", 2).unwrap(), json!({"tools":[]}));
        assert_eq!(
            response(
                "event: message\ndata: {\"method\":\"notifications/x\"}\n\ndata: {\"id\":2,\"result\":42}\n\n",
                2
            )
            .unwrap(),
            json!(42)
        );
        assert!(
            response("{\"id\":2,\"error\":{\"message\":\"disconnected\"}}", 2)
                .unwrap_err()
                .to_string()
                .contains("disconnected")
        );
        assert!(response("{\"id\":3,\"result\":42}", 2).is_err());
    }

    fn request(stream: &mut std::net::TcpStream) -> (String, Value) {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            headers.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let lower = headers.to_ascii_lowercase();
        let mut body = vec![];
        if let Some(length) = lower
            .lines()
            .find_map(|s| s.strip_prefix("content-length:").and_then(|s| s.trim().parse().ok()))
        {
            body.resize(length, 0);
            reader.read_exact(&mut body).unwrap();
        } else {
            assert!(lower.contains("transfer-encoding: chunked"));
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let n = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
                if n == 0 {
                    break;
                }
                let at = body.len();
                body.resize(at + n, 0);
                reader.read_exact(&mut body[at..]).unwrap();
                let mut end = [0; 2];
                reader.read_exact(&mut end).unwrap();
            }
        }
        (lower, serde_json::from_slice(&body).unwrap())
    }

    #[test]
    fn retains_mcp_tab_ownership_between_calls_and_recovers_only_expired_sessions() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let steps = [
                (
                    "initialize",
                    None,
                    200,
                    Some("pane-one"),
                    json!({"id":1,"result":{"protocolVersion":"2025-03-26"}}),
                ),
                ("notifications/initialized", Some("pane-one"), 202, None, Value::Null),
                ("tools/list", Some("pane-one"), 200, None, json!({"id":2,"result":{"tools":[]}})),
                ("tools/call", Some("pane-one"), 200, None, json!({"id":2,"result":{"content":[]}})),
                ("tools/call", Some("pane-one"), 404, None, json!({"error":"expired"})),
                (
                    "initialize",
                    None,
                    200,
                    Some("pane-two"),
                    json!({"id":1,"result":{"protocolVersion":"2025-03-26"}}),
                ),
                ("notifications/initialized", Some("pane-two"), 202, None, Value::Null),
                ("tools/call", Some("pane-two"), 200, None, json!({"id":2,"result":{"content":[]}})),
                ("tools/call", Some("pane-two"), 500, None, json!({"error":"failed"})),
            ];
            for (method, session, status, new_session, reply) in steps {
                let (mut stream, _) = listener.accept().unwrap();
                let (headers, body) = request(&mut stream);
                assert_eq!(body["method"], method);
                match session {
                    Some(id) => assert!(headers.contains(&format!("mcp-session-id: {id}"))),
                    None => assert!(!headers.contains("mcp-session-id:")),
                }
                let body = if status == 202 { String::new() } else { reply.to_string() };
                let session_header = new_session.map(|s| format!("Mcp-Session-Id: {s}\r\n")).unwrap_or_default();
                write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n{session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("session");
        rpc(&url, &state, "tools/list", json!({})).unwrap();
        rpc(&url, &state, "tools/call", json!({"name":"chrome_navigate"})).unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap(), "pane-one");
        rpc(&url, &state, "tools/call", json!({"name":"chrome_read_page"})).unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap(), "pane-two");
        assert!(
            rpc(&url, &state, "tools/call", json!({"name":"chrome_click_element"}))
                .unwrap_err()
                .to_string()
                .contains("500")
        );
        server.join().unwrap();
    }
}
