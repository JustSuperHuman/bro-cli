//! `bro computers`: the computers on your Just Superhuman account (JustConnect). Every command answers in plain
//! lines a person can read; `--json` anywhere in the arguments prints the server's answer untouched for scripts.

use bro_core::justconnect::{self as jc, health};
use serde_json::{Value, json};

pub const HELP: &str = "bro computers — the computers on your Just Superhuman account

  Add or remove this computer
    bro computers enable [name]           add this computer (asks for administrator permission once)
    bro computers enable --claim-file <file>
                                          add a computer your technician already manages, with their claim
    bro computers disable                 stop remote access to this computer
    bro computers phone on|off            your own phone opens the sessions here (on by default once signed in)
    bro computers status                  whether this computer is reachable, and where `bro ctl` runs

  Work with another computer
    bro computers list                    the computers you can reach
    bro computers status <id>             one computer in detail
    bro computers doctor <id>             why one can't be reached, link by link, and what fixes it
    bro computers select <id|local>       choose where `bro ctl` runs work
    bro computers run <id> <shell> <command>
                                          run a command there (powershell, cmd, bash or sh)
    bro computers desktop <id>            open its desktop in your browser
    bro computers history <id>            recent commands and Bro work
    bro computers install-bro <id>        install or update Bro there (Windows)

  Add --json to any command for the raw answer.

Sign in first with `bro jsh login`. You reach the computers you added; account admins and IT Admins reach
every computer on the account. Keep Bro open on a computer to run sessions there. Turning access off leaves
the monitoring service installed, so turning it back on is instant.
";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    // a connection that failed is said by where it stopped and what fixes it; anything else in its own words
    run_command(args).map_err(|error| if health::explains(&error) { anyhow::anyhow!(health::of_failure(&error, None).render()) } else { error })
}

/// This Bro's version, for telling an older one on another computer.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `bro computers doctor <id>`: each link between here and that computer with how it came out, what to do about
/// the first that fails, and how long a request there and back takes when none does.
fn doctor(id: &str) -> anyhow::Result<(Value, String)> {
    let status = match jc::call("GET", &route(id, "")?, json!({})) {
        Ok(status) => status,
        Err(error) if health::explains(&error) => {
            let d = health::of_failure(&error, None);
            return Ok((json!({"ready":false,"headline":d.headline,"steps":d.steps}), d.render()));
        }
        Err(error) => return Err(error),
    };
    // the account's record of the computer, under the name people know it by
    let mut computer = status["connection"].clone();
    if computer["name"].as_str().is_none_or(str::is_empty) {
        computer["name"] = status["computer"]["name"].clone();
    }
    let d = health::of_computer(&computer, status["computer"]["status"].as_str(), now_ms(), VERSION);
    let mut text = d.render();
    let mut result = json!({"ready":d.ready(),"headline":d.headline,"steps":d.steps,"notes":d.notes});
    if d.ready() {
        // every link answers: time one small request there and back
        match jc::remote_tool_timed(id, "list_projects", json!({})) {
            Ok((job, took)) if job["state"] == "completed" => {
                result["roundTripMs"] = json!(took.as_millis() as u64);
                text.push_str(&format!("\n\nA request there and back took {}.", span(took)));
            }
            Ok((job, _)) => text.push_str(&format!("\n\nA test request came back as \"{}\". {UNANSWERED}", state_label(job["state"].as_str().unwrap_or_default()))),
            Err(error) => {
                let d = health::of_failure(&error, computer["name"].as_str());
                result = json!({"ready":false,"headline":d.headline,"steps":d.steps});
                text = d.render();
            }
        }
    }
    Ok((result, text))
}

/// "0.4 s", "2.1 s", "38 s".
fn span(took: std::time::Duration) -> String {
    let seconds = took.as_secs_f64();
    if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{seconds:.0} s") }
}

fn run_command(args: Vec<String>) -> anyhow::Result<()> {
    let raw = args.iter().any(|a| a == "--json");
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    let (result, text) = match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            print!("{HELP}");
            return Ok(());
        }
        ["list"] => {
            let result = jc::call("GET", "/computers", json!({}))?;
            let text = list(&result, jc::selected().as_deref());
            (result, text)
        }
        ["enable"] => (jc::enable(None)?, ENABLED.to_string()),
        ["enable", "--claim-file", path] => (jc::enable_with_claim(None, Some(std::path::Path::new(path)))?, ENABLED.to_string()),
        ["enable", name] => (jc::enable(Some(name))?, ENABLED.to_string()),
        ["phone", "on"] => (jc::set_sessions(true)?, "Your phone, signed in to this account, lists this computer and opens its sessions.".to_string()),
        ["phone", "off"] => (
            jc::set_sessions(false)?,
            "Your phone can't open the sessions here anymore. Signing in again won't turn it back on; `bro computers phone on` does.".to_string(),
        ),
        ["disable"] => (
            jc::disable()?,
            "Remote access to this computer is off.\nThe monitoring service stays installed. Turn access back on with `bro computers enable`.".to_string(),
        ),
        ["select", "local"] => (jc::select("local")?, "`bro ctl` runs on this computer again.".to_string()),
        ["select", id] => {
            let result = jc::select(id)?;
            let name = result["computer"]["name"].as_str().unwrap_or(id);
            let text = format!("`bro ctl` now runs on {name}.\nCome back with `bro computers select local`.");
            (result, text)
        }
        ["status"] => {
            let c = jc::load();
            let selected = jc::selected();
            let result = json!({"enabled":c.enabled,"computerId":c.computer_id,"selected":selected});
            let mut rows = vec![("Remote access", if c.enabled { "On" } else { "Off" }.to_string())];
            if jc::valid_id(&c.computer_id) {
                rows.push(("Computer ID", c.computer_id.clone()));
            }
            rows.push(("bro ctl runs on", selected.unwrap_or_else(|| "This computer".into())));
            let mut text = pairs(&rows);
            let mut result = result;
            if c.enabled {
                let link = local_link();
                text.push_str(&format!("\n\n{}", link_text(link.as_ref())));
                result["link"] = link.unwrap_or(Value::Null);
            } else {
                text.push_str("\n\nAdd this computer to your account with `bro computers enable`.");
            }
            (result, text)
        }
        ["status", id] => {
            let result = jc::call("GET", &route(id, "")?, json!({}))?;
            let text = status(&result);
            (result, text)
        }
        ["doctor", id] => doctor(id)?,
        ["history", id] => {
            let result = jc::call("GET", &route(id, "/history")?, json!({}))?;
            let text = history(&result);
            (result, text)
        }
        ["install-bro", id] => {
            let result = jc::call("POST", &route(id, "/bro/install")?, json!({"requestId":uuid::Uuid::new_v4().to_string()}))?;
            let text = outcome(&result, "Bro is installed.");
            (result, text)
        }
        ["run", id, shell, command] => {
            let result = jc::call(
                "POST",
                &route(id, "/commands")?,
                json!({"requestId":uuid::Uuid::new_v4().to_string(),"command":command,"shell":shell,"timeout":60}),
            )?;
            let text = outcome(&result, "Finished with no output.");
            (result, text)
        }
        ["desktop", id] => {
            let response = jc::call("POST", &route(id, "/desktop")?, json!({}))?;
            let url = response["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("The desktop link didn't come back. Try again."))?;
            if url::Url::parse(url)?.scheme() != "https" {
                anyhow::bail!("The desktop link that came back isn't secure, so it wasn't opened.");
            }
            crate::panes::links::open(url).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let until = when(&response["expiresAt"]).map_or_else(|| "for 20 minutes".to_string(), |t| format!("until {}", t.format("%-I:%M %p")));
            (json!({"opened":true,"expiresAt":response["expiresAt"]}), format!("The desktop is opening in your browser. Its link works {until}."))
        }
        _ => anyhow::bail!("That isn't a `bro computers` command. See `bro computers --help`."),
    };
    if raw {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

const ENABLED: &str = "This computer is on your account.\nOpen JustConnect in your Superhost to reach it, or run `bro computers list` on another computer.";

fn route(id: &str, suffix: &str) -> anyhow::Result<String> {
    if !jc::valid_id(id) {
        anyhow::bail!("That isn't a computer ID. Copy one from `bro computers list`.");
    }
    Ok(format!("/computers/{id}{suffix}"))
}

fn platform(value: &Value) -> &str {
    match value.as_str().unwrap_or_default() {
        "windows" => "Windows",
        "darwin" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// Label and value lines, the values in one column.
fn pairs(rows: &[(&str, String)]) -> String {
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(label, value)| format!("{label:<width$}  {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A time the server sent: ISO text from the monitoring service, or milliseconds from the account server.
fn when(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    match value {
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|t| t.with_timezone(&chrono::Local)),
        Value::Number(ms) => chrono::DateTime::from_timestamp_millis(ms.as_i64()?)
            .map(|t| t.with_timezone(&chrono::Local)),
        _ => None,
    }
}

/// "4 min ago" for anything recent, a date for anything older.
fn ago(value: &Value) -> Option<String> {
    let at = when(value)?;
    let seconds = (chrono::Local::now() - at).num_seconds().max(0);
    Some(match seconds {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (seconds / 60).max(1)),
        3600..=86_399 => format!("{} hr ago", seconds / 3600),
        86_400..=172_799 => "1 day ago".into(),
        172_800..=604_799 => format!("{} days ago", seconds / 86_400),
        _ => at.format("%b %-d, %Y").to_string(),
    })
}

/// What one computer in the list is doing.
fn state(c: &Value) -> &'static str {
    if c["enabled"] != true {
        "Access off"
    } else if c["broOnline"] == true {
        "Bro is open"
    } else {
        "Bro is closed"
    }
}

fn list(result: &Value, selected: Option<&str>) -> String {
    let computers = result["computers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if computers.is_empty() {
        return "No computers on your account yet.\nAdd this one with `bro computers enable`."
            .into();
    }
    let name = |c: &Value| c["name"].as_str().unwrap_or("Computer").to_string();
    let wide = computers
        .iter()
        .map(|c| name(c).chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, 36);
    let mut lines = vec![format!(
        "  {:<wide$}  {:<8}  {:<13}  ID",
        "NAME", "SYSTEM", "STATUS"
    )];
    for c in computers {
        let id = c["id"].as_str().unwrap_or_default();
        // the one `bro ctl` runs on
        let mark = if selected == Some(id) { '›' } else { ' ' };
        let mut shown: String = name(c).chars().take(wide).collect();
        if name(c).chars().count() > wide {
            shown.pop();
            shown.push('…');
        }
        lines.push(format!(
            "{mark} {shown:<wide$}  {:<8}  {:<13}  {id}",
            platform(&c["platform"]),
            state(c)
        ));
    }
    lines.push(String::new());
    lines.push(match selected {
        Some(_) => {
            "› is where `bro ctl` runs. Return here with `bro computers select local`.".into()
        }
        None => "Run work on one with `bro computers select <id>`.".into(),
    });
    lines.join("\n")
}

fn status(result: &Value) -> String {
    let (c, connection) = (&result["computer"], &result["connection"]);
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let health = if connection["enabled"] != true {
        "Access off".to_string()
    } else {
        match c["status"].as_str().unwrap_or_default() {
            "online" => "Online".into(),
            "offline" => "Offline".into(),
            "overdue" => "Not responding".into(),
            "awaiting_install" => "Setting up".into(),
            other => other.to_string(),
        }
    };
    let mut rows = vec![("Status", health)];
    rows.push((
        "System",
        text(&c["system"]).unwrap_or_else(|| platform(&c["platform"]).to_string()),
    ));
    if let Some(host) = text(&c["hostname"]) {
        rows.push(("Hostname", host));
    }
    rows.push((
        "Last seen",
        ago(&c["lastSeen"]).unwrap_or_else(|| "Not yet".into()),
    ));
    // the service reports whole gigabytes under this name
    if let Some(memory) = c["memoryMb"].as_u64().filter(|m| *m > 0) {
        rows.push((
            "Memory",
            format!("{} GB", if memory > 2048 { memory / 1024 } else { memory }),
        ));
    }
    if let Some(address) = text(&c["publicIp"]) {
        rows.push(("Public address", address));
    }
    if c["needsReboot"] == true {
        rows.push(("Restart", "Needed".into()));
    }
    rows.push((
        "Bro",
        match (
            connection["broOnline"] == true,
            text(&connection["broVersion"]),
        ) {
            (true, Some(version)) => format!("Open · version {version}"),
            (true, None) => "Open".into(),
            (false, _) => "Closed".into(),
        },
    ));
    if let Some(version) = text(&c["agentVersion"]) {
        rows.push(("Monitoring service", format!("Version {version}")));
    }
    rows.push(("Computer ID", text(&c["id"]).unwrap_or_default()));
    let mut out = format!(
        "{}\n\n{}",
        c["name"].as_str().unwrap_or("Computer"),
        pairs(&rows)
    );
    // what stands in the way of running sessions there, and what fixes it
    let mut record = connection.clone();
    if record["name"].as_str().is_none_or(str::is_empty) {
        record["name"] = c["name"].clone();
    }
    let d = health::of_computer(&record, c["status"].as_str(), now_ms(), VERSION);
    if !d.ready() {
        out.push_str(&format!("\n\n{}", d.headline));
        for (i, step) in d.steps.iter().enumerate() {
            out.push_str(&format!("\n  {}. {step}", i + 1));
        }
    }
    for note in &d.notes {
        out.push_str(&format!("\n\n{note}"));
    }
    out
}

/// This computer's own connection to the account, as the running Bro reports it (None: Bro isn't running here,
/// or is an older one that doesn't say).
fn local_link() -> Option<Value> {
    let endpoint = bro_bridge::read_endpoint(&crate::util::bro_dir().join("bridge"))?;
    let answer = bro_core::http::get_json(&format!("{}/api/justconnect", endpoint.local_url()), &[], std::time::Duration::from_secs(3)).ok()?;
    answer.ok().then(|| answer.body["link"].clone()).filter(Value::is_object)
}

/// What the worker's connection means for someone asking this computer to do something.
fn link_text(link: Option<&Value>) -> String {
    let Some(link) = link else {
        return "Bro isn't running here, so nothing asked of this computer starts. Open Bro.".into();
    };
    match link["phase"].as_str().unwrap_or_default() {
        "connected" if link["held"] == true => "Connected. What you ask of this computer starts at once.".into(),
        "connected" => "Connected. What you ask of this computer starts within a few seconds.".into(),
        "connecting" => "Connecting to Just Superhuman.".into(),
        "reconnecting" => "Can't reach Just Superhuman right now. Bro keeps trying, a little less often each time.".into(),
        "refused" => "Just Superhuman doesn't accept this sign-in for this computer. Run `bro computers enable` again.".into(),
        _ => "Waiting to start. Another Bro on this computer may be taking its work.".into(),
    }
}

/// What a state means to the person who asked.
fn state_label(state: &str) -> &str {
    match state {
        "completed" => "Finished",
        "failed" => "Failed",
        "unknown" => "No answer",
        "expired" => "Never delivered",
        "cancelled" => "Cancelled",
        "running" | "leased" => "Running",
        "queued" => "Waiting",
        other => other,
    }
}

const UNANSWERED: &str = "The computer didn't answer in time. This may still have run, so check the computer before you send it again.";

/// A command's output, or what happened to it when there isn't any.
fn outcome(result: &Value, empty: &str) -> String {
    let said = result["result"]["output"]
        .as_str()
        .or(result["result"]["error"].as_str())
        .map(str::trim_end)
        .filter(|s| !s.is_empty());
    match (result["state"].as_str().unwrap_or_default(), said) {
        ("completed", Some(text)) => text.to_string(),
        ("completed", None) => empty.to_string(),
        ("unknown" | "expired", None) => UNANSWERED.to_string(),
        (state, Some(text)) => format!("{}\n\n{text}", state_label(state)),
        (state, None) => state_label(state).to_string(),
    }
}

fn history(result: &Value) -> String {
    let mut items: Vec<(Option<chrono::DateTime<chrono::Local>>, &str, &Value)> = vec![];
    for op in result["operations"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        items.push((when(&op["created_at"]), "Command", op));
    }
    for job in result["jobs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        items.push((when(&job["createdAt"]), "Bro work", job));
    }
    if items.is_empty() {
        return "Nothing yet. Commands and Bro work on this computer are kept here.".into();
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items
        .iter()
        .map(|(at, kind, item)| {
            let state = state_label(item["state"].as_str().unwrap_or_default());
            let first = item["result"]["output"]
                .as_str()
                .or(item["result"]["error"].as_str())
                .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or_default();
            let mut line: String = first.trim().chars().take(72).collect();
            if first.trim().chars().count() > 72 {
                line.push('…');
            }
            let time = at.map_or_else(String::new, |t| t.format("%b %e %l:%M %p").to_string());
            format!("{time:<16}  {kind:<8}  {state:<15}  {line}")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn remote_ctl(args: &crate::ctl_cmd::CtlArgs, id: &str) -> Result<String, String> {
    let Some(tool) = &args.tool else {
        return Ok(format!(
            "Selected computer: {id}\nRemote tools: list_sessions, list_projects, open_session, read_session, read_chat, send_input, send_keys, answer_prompt, close_session\nUse `bro computers select local` to return to this computer."
        ));
    };
    let result: Value = jc::remote_tool(id, tool, args.arguments.clone())
        .map_err(|error| if health::explains(&error) { health::of_failure(&error, None).render() } else { error.to_string() })?;
    if result["state"] != "completed" {
        return Err(unfinished(&result));
    }
    Ok(result["result"]["output"].as_str().unwrap_or("").into())
}

/// What became of remote work that didn't finish, for the person who asked.
fn unfinished(job: &Value) -> String {
    let said = job["result"]["output"].as_str().or(job["result"]["error"].as_str()).map(str::trim).filter(|s| !s.is_empty());
    match (job["state"].as_str().unwrap_or_default(), said) {
        // the tool ran and said why it couldn't
        ("failed", Some(text)) => text.to_string(),
        ("failed", None) => "That didn't work on the other computer, and it gave no reason.".into(),
        ("expired", _) => "Never delivered. Bro on that computer didn't pick this up within two minutes, so nothing ran. `bro computers doctor <id>` shows why.".into(),
        ("cancelled", _) => "Cancelled before it started: access to that computer changed.".into(),
        ("queued" | "leased", _) => "Still running there after three minutes. `bro computers history <id>` shows how it ends.".into(),
        // handed over, and no result came back
        _ => UNANSWERED.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_lines_up_and_marks_where_ctl_runs() {
        let result = json!({"computers":[
            {"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","name":"Studio workstation","platform":"windows","enabled":true,"broOnline":true},
            {"id":"1c0de111-0da5-444f-a10c-9c17afcd48e1","name":"Priya's MacBook Pro","platform":"darwin","enabled":false,"broOnline":false},
        ]});
        let text = list(&result, Some("dbb88f69-0da5-444f-a10c-9c17afcd48e0"));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1],
            format!(
                "› {:<19}  {:<8}  {:<13}  dbb88f69-0da5-444f-a10c-9c17afcd48e0",
                "Studio workstation", "Windows", "Bro is open"
            )
        );
        assert_eq!(
            lines[2],
            format!(
                "  {:<19}  {:<8}  {:<13}  1c0de111-0da5-444f-a10c-9c17afcd48e1",
                "Priya's MacBook Pro", "macOS", "Access off"
            )
        );
        assert!(list(&json!({"computers":[]}), None).contains("bro computers enable"));
    }

    #[test]
    fn a_computer_reads_as_plain_facts() {
        let result = json!({
            "computer":{"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","name":"Studio workstation","platform":"windows","status":"online","hostname":"STUDIO-PC","system":"Windows 11 Pro","memoryMb":64,"agentVersion":"2.9.1","needsReboot":true},
            "connection":{"enabled":true,"broOnline":true,"broVersion":"2.1.28"}
        });
        let text = status(&result);
        assert!(text.starts_with("Studio workstation\n\n"), "{text}");
        for (label, value) in [
            ("Status", "Online"),
            ("Memory", "64 GB"),
            ("Restart", "Needed"),
            ("Bro", "Open · version 2.1.28"),
            ("Monitoring service", "Version 2.9.1"),
        ] {
            let want = format!("{label:<18}  {value}");
            assert!(text.contains(&want), "missing {want:?} in\n{text}");
        }
    }

    #[test]
    fn remote_work_that_did_not_finish_says_what_became_of_it() {
        // the tool's own reason, not a dump of the job
        assert_eq!(unfinished(&json!({"state":"failed","result":{"output":"The target is working. Wait until it is ready."}})), "The target is working. Wait until it is ready.");
        assert!(unfinished(&json!({"state":"expired","result":null})).starts_with("Never delivered."));
        assert!(unfinished(&json!({"state":"cancelled"})).starts_with("Cancelled before it started"));
        assert_eq!(unfinished(&json!({"state":"unknown","result":null})), UNANSWERED);
        assert!(unfinished(&json!({"state":"leased"})).contains("bro computers history"));
    }

    #[test]
    fn this_computers_connection_reads_as_what_it_means_for_work() {
        assert!(link_text(None).contains("Open Bro"));
        assert_eq!(link_text(Some(&json!({"phase":"connected","held":true}))), "Connected. What you ask of this computer starts at once.");
        assert!(link_text(Some(&json!({"phase":"connected","held":false}))).contains("within a few seconds"));
        assert!(link_text(Some(&json!({"phase":"reconnecting"}))).contains("keeps trying"));
        assert!(link_text(Some(&json!({"phase":"refused"}))).contains("bro computers enable"));
    }

    #[test]
    fn a_computer_that_cannot_run_sessions_says_what_to_do_under_its_facts() {
        let closed = json!({
            "computer":{"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","name":"Studio workstation","platform":"windows","status":"online"},
            "connection":{"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","enabled":true,"provisioned":true,"broOnline":false,"broLastSeen":now_ms() - 3 * 3_600_000}
        });
        let text = status(&closed);
        assert!(text.contains("Bro is closed on Studio workstation.\n  1. Open Bro on Studio workstation."), "{text}");
        let ready = json!({
            "computer":{"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","name":"Studio workstation","platform":"windows","status":"online"},
            "connection":{"enabled":true,"provisioned":true,"broOnline":true,"broVersion":"2.0.1"}
        });
        let text = status(&ready);
        assert!(!text.contains("What to do") && !text.contains("1. "), "{text}");
        assert!(text.contains("Studio workstation runs Bro 2.0.1; this computer runs"), "{text}");
    }

    #[test]
    fn a_command_with_no_answer_says_it_may_have_run() {
        assert_eq!(
            outcome(
                &json!({"state":"completed","result":{"output":"STUDIO-PC\r\n"}}),
                "none"
            ),
            "STUDIO-PC"
        );
        assert_eq!(
            outcome(&json!({"state":"unknown","result":{"error":""}}), "none"),
            UNANSWERED
        );
        assert_eq!(
            outcome(
                &json!({"state":"failed","result":{"error":"Access denied"}}),
                "none"
            ),
            "Failed\n\nAccess denied"
        );
    }
}
