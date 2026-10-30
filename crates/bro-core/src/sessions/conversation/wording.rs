//! Tool calls in plain words, for the steps focus mode shows ([`super::claude_step`] and Codex's items): a skill
//! by its name ("Generate voice"), an MCP server's tool by what it does and to what ("Navigate · figma.com",
//! "Type · “omelet”"), and the shell commands people run all the time by what they did ("Tested bro-proxy",
//! "Committed “fix the timer”", "Pushed fix/timer-clock"). Names spelled their owners' way keep it: JustGains,
//! GitHub, PDF.

use super::{LABEL_MAX, StepKind, basename, line};
use serde_json::Value;

/// Words spelled their owners' way, whatever case they come in.
const SPELLED: &[(&str, &str)] = &[
    ("ai", "AI"),
    ("api", "API"),
    ("apis", "APIs"),
    ("ci", "CI"),
    ("cli", "CLI"),
    ("css", "CSS"),
    ("csv", "CSV"),
    ("docx", "DOCX"),
    ("gpu", "GPU"),
    ("html", "HTML"),
    ("id", "ID"),
    ("ids", "IDs"),
    ("ios", "iOS"),
    ("json", "JSON"),
    ("llm", "LLM"),
    ("mcp", "MCP"),
    ("pdf", "PDF"),
    ("pptx", "PPTX"),
    ("pr", "PR"),
    ("qa", "QA"),
    ("sdk", "SDK"),
    ("seo", "SEO"),
    ("sql", "SQL"),
    ("svg", "SVG"),
    ("tts", "TTS"),
    ("ui", "UI"),
    ("url", "URL"),
    ("urls", "URLs"),
    ("ux", "UX"),
    ("xlsx", "XLSX"),
    ("android", "Android"),
    ("chatgpt", "ChatGPT"),
    ("chrome", "Chrome"),
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("elevenlabs", "ElevenLabs"),
    ("figma", "Figma"),
    ("gemini", "Gemini"),
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("javascript", "JavaScript"),
    ("macos", "macOS"),
    ("openai", "OpenAI"),
    ("openrouter", "OpenRouter"),
    ("posthog", "PostHog"),
    ("tiktok", "TikTok"),
    ("typescript", "TypeScript"),
    ("youtube", "YouTube"),
    // Just Superhuman's own
    ("bro", "bro"),
    ("justbrowse", "JustBrowse"),
    ("justcommit", "JustCommit"),
    ("justgains", "JustGains"),
    ("justhost", "JustHost"),
    ("justimagine", "JustImagine"),
    ("justmemes", "JustMemes"),
    ("justposts", "JustPosts"),
    ("justready", "JustReady"),
    ("justtools", "JustTools"),
    ("justui", "JustUI"),
    ("justwatch", "JustWatch"),
];

/// Words a tool's name starts what it does with: what comes before one is the server's prefix (`chrome_` of
/// `chrome_click_element`).
const VERBS: &[&str] = &[
    "add", "analyze", "answer", "append", "approve", "archive", "ask", "browse", "build", "call", "cancel", "capture", "check",
    "clear", "click", "close", "commit", "compare", "compress", "connect", "convert", "copy", "create", "crop", "decompile",
    "delete", "deploy", "describe", "disable", "disconnect", "download", "drag", "draw", "edit", "enable", "evaluate", "exec",
    "execute", "export", "extract", "fetch", "fill", "find", "focus", "format", "generate", "get", "go", "handle", "hover",
    "import", "insert", "inspect", "install", "invoke", "launch", "list", "load", "lookup", "make", "merge", "move", "navigate",
    "open", "parse", "paste", "pause", "play", "post", "press", "publish", "pull", "push", "query", "read", "record", "refresh",
    "reload", "remove", "rename", "render", "replace", "reply", "request", "reset", "resize", "restart", "restore", "resume",
    "run", "save", "scan", "schedule", "screenshot", "scroll", "search", "select", "send", "set", "share", "show", "snapshot",
    "speak", "start", "stop", "submit", "summarize", "swipe", "switch", "sync", "take", "tap", "test", "toggle", "trace",
    "transcribe", "translate", "type", "update", "upload", "use", "validate", "verify", "view", "visit", "wait", "write",
];

/// How `word` (lowercase) is spelled: its owner's way, else as it is.
fn spelled(word: &str) -> Option<&'static str> {
    SPELLED.iter().find(|(w, _)| *w == word).map(|(_, s)| *s)
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// `words` (lowercase) as a sentence starts: the first capitalised, each its owner's way.
fn sentence(words: &[String]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| match spelled(w) {
            Some(s) => s.to_string(),
            None if i == 0 => capitalised(w),
            None => w.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A name's words, lowercase: `chrome_click_element`, `ClickBySelector`, `generate-voice`, `container.exec`.
fn split(name: &str) -> Vec<String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | ' ' | '.' | '/') {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            words.push(std::mem::take(&mut word));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A name as words: `NotebookRead` → "Notebook read", `claude-code-guide` → "Claude code guide".
pub fn name(raw: &str) -> String {
    let words = split(raw);
    if words.is_empty() { raw.to_string() } else { line(&sentence(&words), LABEL_MAX) }
}

/// A skill as its name says it: `generate-voice` → "Generate voice", `justgains-app-deployment` → "JustGains app
/// deployment", a plugin's `anthropic-skills:pdf` → "PDF".
pub fn skill_name(skill: &str) -> String {
    let own = skill.trim().trim_start_matches('/');
    let own = own.rsplit(':').next().unwrap_or(own);
    if split(own).is_empty() { "A skill".to_string() } else { name(own) }
}

/// An MCP server as its owner names it: a claude.ai connector's `claude_ai_JustGains` → "JustGains",
/// `claude_ai_Claude_Docs` → "Claude Docs", `android-mcp` → "Android", `justbrowse` → "JustBrowse".
pub fn server_name(server: &str) -> String {
    let own = server.strip_prefix("claude_ai_").unwrap_or(server);
    // "mcp" and "server" say nothing about which one it is
    let parts: Vec<&str> = own.split(['_', '-', ' ']).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("mcp") && !w.eq_ignore_ascii_case("server")).collect();
    if parts.is_empty() {
        return own.to_string();
    }
    // capitals of its own are its spelling
    if own.chars().any(char::is_uppercase) {
        return parts.join(" ");
    }
    parts.iter().map(|p| spelled(p).map_or_else(|| capitalised(p), str::to_string)).collect::<Vec<_>>().join(" ")
}

/// What an MCP tool does, verb first and without its server's prefix (`chrome_click_element` → "Click element"),
/// with the one thing it was given that says most about the call: "Navigate · figma.com", "Type · “omelet”".
pub fn tool_label(tool: &str, input: &Value) -> String {
    let words = split(tool);
    let from = words.iter().position(|w| VERBS.contains(&w.as_str())).unwrap_or(0);
    let what = if words.is_empty() { "A tool".to_string() } else { sentence(&words[from..]) };
    let label = match key_argument(input) {
        Some(arg) => format!("{what} · {arg}"),
        None => what,
    };
    line(&label, LABEL_MAX)
}

/// The argument that says most about a call: where it went, what it looked for or said, the file, the button.
/// What's typed into a form (`value`) stays out: it can be a password.
fn key_argument(input: &Value) -> Option<String> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    if let Some(u) = ["url", "uri", "href", "link"].into_iter().find_map(s) {
        return Some(host(u));
    }
    if let Some(t) = ["query", "q", "search", "pattern", "text", "prompt", "question", "title", "name", "subject", "description", "message"].into_iter().find_map(s) {
        return Some(format!("“{}”", line(t, 40)));
    }
    if let Some(p) = ["filePath", "file_path", "path", "file", "filename", "fileName"].into_iter().find_map(s) {
        return Some(basename(p));
    }
    if let Some(k) = ["button", "key", "keys", "serial", "device", "resourceId", "action"].into_iter().find_map(s) {
        return Some(line(&k.replace('_', " "), 40));
    }
    let n = |k: &str| input.get(k).and_then(Value::as_f64);
    if let (Some(x), Some(y)) = (n("x"), n("y")) {
        return Some(format!("at {}, {}", x.round(), y.round()));
    }
    n("duration").or_else(|| n("seconds")).map(|d| format!("{}s", d.round()))
}

/// Where an address goes: its host (and port), without `www.`; the address itself when it isn't one.
pub fn host(address: &str) -> String {
    match url::Url::parse(address) {
        Ok(u) if u.host_str().is_some() => {
            let h = u.host_str().unwrap_or_default().trim_start_matches("www.");
            match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h.to_string(),
            }
        }
        _ => line(address, 40),
    }
}

/// What a tool search brought in (`select:WebFetch,Monitor` → "Loaded WebFetch and Monitor"; an MCP server's
/// tools by the server), or what it looked for.
pub fn tool_search(query: &str) -> String {
    let query = query.trim();
    let Some(names) = query.strip_prefix("select:") else {
        return if query.is_empty() { "Looked up its tools".to_string() } else { format!("Looked for tools: “{}”", line(query, 40)) };
    };
    let mut loaded: Vec<String> = vec![];
    for n in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let said = match n.strip_prefix("mcp__").and_then(|t| t.split("__").next()) {
            Some(server) => format!("{} tools", server_name(server)),
            None => n.to_string(),
        };
        if !loaded.contains(&said) {
            loaded.push(said);
        }
    }
    match loaded.as_slice() {
        [] => "Looked up its tools".to_string(),
        [one] => format!("Loaded {one}"),
        [a, b] => format!("Loaded {a} and {b}"),
        many => format!("Loaded {} tools", many.len()),
    }
}

/// A plan's progress: "Updated the plan · 3/7 done" ("Finished the plan" once it all is), and what's under way
/// now. `items` is the task list (Claude's `todos`, Codex's `plan`), each with its `status` and its words under
/// one of `what` (the first of them there).
pub fn plan_progress(items: Option<&Value>, what: &[&str]) -> (String, Option<String>) {
    let Some(items) = items.and_then(Value::as_array).filter(|a| !a.is_empty()) else { return ("Updated the plan".to_string(), None) };
    let status = |i: &Value| i.get("status").and_then(Value::as_str).unwrap_or_default().to_string();
    let done = items.iter().filter(|i| status(i) == "completed").count();
    let now = items.iter().find(|i| status(i) == "in_progress").and_then(|i| what.iter().find_map(|k| i.get(*k).and_then(Value::as_str))).map(str::to_string);
    let label = if done == items.len() { "Finished the plan".to_string() } else { format!("Updated the plan · {done}/{} done", items.len()) };
    (label, now)
}

/// A wait as people say it: "45s", "20m", "1h 30m".
pub fn wait(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ if s.is_multiple_of(3600) => format!("{}h", s / 3600),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

// ------------------------------------------------------------------------------------------------ commands

/// A shell command in plain words, when it's one people run all the time: "Tested bro-tui", "Committed “fix”",
/// "Searched for “route”", "Fetched docs.rs". Two in a row read as both ("Staged changes · Committed “fix”").
/// None when the first isn't one of those: it reads as itself then.
pub fn command_label(cmd: &str) -> Option<(StepKind, String)> {
    let first = cmd.lines().map(str::trim).find(|l| !l.is_empty())?;
    let cmds = commands(first);
    let (kind, mut label) = said(cmds.first()?)?;
    if let Some((_, more)) = cmds.get(1).and_then(|c| said(c)) {
        label = format!("{label} · {more}");
    }
    Some((kind, line(&label, LABEL_MAX)))
}

/// A command line's words as a shell splits them: quotes taken off, `&&` `||` `;` `|` `&` words of their own
/// (`2>&1` stays one).
fn shell_words(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut word = String::new();
    let mut quoted = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars().peekable();
    // a word ends: quotes alone (`''`) still make one
    fn end(word: &mut String, quoted: &mut bool, out: &mut Vec<String>) {
        if *quoted || !word.is_empty() {
            out.push(std::mem::take(word));
        }
        *quoted = false;
    }
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                quoted = true;
            }
            c if c.is_whitespace() => end(&mut word, &mut quoted, &mut out),
            '&' if word.ends_with('>') => word.push(c),
            ';' | '|' | '&' => {
                end(&mut word, &mut quoted, &mut out);
                let mut op = c.to_string();
                if c != ';' && chars.peek() == Some(&c) {
                    op.push(c);
                    chars.next();
                }
                out.push(op);
            }
            c => word.push(c),
        }
    }
    end(&mut word, &mut quoted, &mut out);
    out
}

/// The commands a line runs, each as its words (`cd x && cargo test | tail` → `cargo test`): a pipeline is its
/// first command; changing folder, setting a variable or the shell's options isn't one.
fn commands(text: &str) -> Vec<Vec<String>> {
    let mut out = vec![];
    let mut cur: Vec<String> = vec![];
    let mut piped = false;
    for w in shell_words(text) {
        match w.as_str() {
            "&&" | "||" | ";" | "&" => {
                keep(&mut out, std::mem::take(&mut cur));
                piped = false;
            }
            "|" => {
                keep(&mut out, std::mem::take(&mut cur));
                piped = true;
            }
            _ if piped => {}
            _ => cur.push(w),
        }
    }
    keep(&mut out, cur);
    out
}

/// Add command `words` to `out`, without what only sets it up (a variable for it, `sudo`, `timeout 60`).
fn keep(out: &mut Vec<Vec<String>>, mut words: Vec<String>) {
    loop {
        let Some(first) = words.first() else { return };
        let assigns = first.split_once('=').is_some_and(|(n, _)| !n.is_empty() && n.trim_start_matches('$').chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'));
        if assigns || words.get(1).is_some_and(|w| w == "=") {
            // `FOO=1 cmd`, PowerShell's `$x = …`: the rest of a PowerShell assignment isn't a command either
            if !assigns {
                return;
            }
            words.remove(0);
        } else if matches!(first.to_ascii_lowercase().as_str(), "sudo" | "time" | "nice" | "env" | "command" | "exec" | "nohup") {
            words.remove(0);
        } else if first == "timeout" && words.len() > 2 {
            words.drain(..2);
        } else {
            break;
        }
    }
    let setup = matches!(words[0].to_ascii_lowercase().as_str(), "cd" | "set-location" | "sl" | "pushd" | "popd" | "export" | "set" | "source" | "." | "setlocal" | "chcp");
    if !setup {
        out.push(words);
    }
}

/// A program as named on a command line: `C:\…\git.exe` → "git".
fn program(word: &str) -> String {
    let base = basename(word).to_ascii_lowercase();
    [".exe", ".cmd", ".bat"].iter().find_map(|e| base.strip_suffix(*e)).map_or(base.clone(), str::to_string)
}

/// The arguments that aren't options, redirections or counts.
fn plain(args: &[String]) -> Vec<&str> {
    args.iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-') && !a.starts_with('<') && !a.chars().all(|c| c.is_ascii_digit()))
        .filter(|a| !a.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with('>'))
        .collect()
}

/// "a.rs", "a.rs and b.rs", "a.rs and 3 more".
fn list(names: &[&str]) -> String {
    let names: Vec<String> = names.iter().map(|n| basename(n)).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} more", rest.len()),
    }
}

/// What a command did, when it's one of the usual ones.
fn said(words: &[String]) -> Option<(StepKind, String)> {
    use StepKind::*;
    let prog = program(words.first()?);
    let args = &words[1..];
    let plain = plain(args);
    let run = |s: String| Some((Run, s));
    match prog.as_str() {
        "git" => git(args).and_then(run),
        "cargo" => cargo(args).and_then(run),
        "npm" | "pnpm" | "yarn" | "bun" => js(&prog, args).and_then(run),
        "npx" | "bunx" | "pnpx" | "uvx" => plain.first().map(|t| (Run, format!("Ran {}", basename(t)))),
        "python" | "python3" | "py" | "node" | "deno" | "tsx" | "ts-node" => script(args).and_then(run),
        "pytest" => run("Ran the tests".into()),
        "pip" | "pip3" if plain.first() == Some(&"install") => run(installed(&plain[1..])),
        "curl" | "wget" | "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm" => {
            args.iter().find(|a| a.starts_with("http://") || a.starts_with("https://")).map(|u| (Web, format!("Fetched {}", host(u))))
        }
        "gh" => gh(&plain).and_then(run),
        "docker" | "podman" => docker(&plain).and_then(run),
        "ls" | "dir" | "get-childitem" | "gci" | "tree" => Some((Read, plain.last().map_or("Listed the files".to_string(), |p| format!("Listed {}", basename(p))))),
        "cat" | "type" | "get-content" | "gc" | "head" | "tail" | "less" | "more" | "bat" => {
            written(args).or_else(|| plain.first().map(|_| (Read, format!("Read {}", list(&plain)))))
        }
        "echo" | "printf" | "write-output" | "write-host" | "tee" | "set-content" | "add-content" | "out-file" => written(args).or_else(|| match prog.as_str() {
            "tee" | "set-content" | "add-content" | "out-file" => plain.first().map(|f| (Run, format!("Wrote {}", basename(f)))),
            _ => None,
        }),
        "grep" | "egrep" | "rg" | "findstr" | "select-string" | "sls" | "ag" | "ack" => pattern(args).map(|p| (Search, format!("Searched for “{}”", line(&p, 50)))),
        "find" | "fd" => Some((Search, "Looked for files".into())),
        "mkdir" | "md" | "new-item" | "touch" => plain.first().map(|_| (Run, format!("Made {}", list(&plain)))),
        "rm" | "del" | "erase" | "remove-item" | "rmdir" | "rd" => plain.first().map(|_| (Run, format!("Deleted {}", list(&plain)))),
        "cp" | "copy" | "copy-item" | "robocopy" | "xcopy" => plain.first().map(|f| (Run, format!("Copied {}", basename(f)))),
        "mv" | "move" | "move-item" => plain.first().map(|f| (Run, format!("Moved {}", basename(f)))),
        "ren" | "rename" | "rename-item" => plain.first().map(|f| (Run, format!("Renamed {}", basename(f)))),
        "sleep" | "start-sleep" => slept(args).map(|w| (Run, format!("Waited {w}"))),
        "make" | "just" => run(match plain.first() {
            Some(t) => format!("Ran {prog} {t}"),
            None => format!("Ran {prog}"),
        }),
        "kill" | "pkill" | "killall" | "taskkill" | "stop-process" => run("Stopped a process".into()),
        _ => None,
    }
}

/// Where a command's output went, when that's a file: `echo x > notes.md`, `cat >> log.txt <<'EOF'`.
fn written(args: &[String]) -> Option<(StepKind, String)> {
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        let target = match a {
            ">" | ">>" => it.next(),
            a if a.starts_with('>') && !a.starts_with(">&") => Some(a.trim_start_matches('>')),
            _ => None,
        };
        if let Some(t) = target.filter(|t| !t.is_empty() && *t != "/dev/null" && !t.eq_ignore_ascii_case("nul") && !t.eq_ignore_ascii_case("$null")) {
            return Some((StepKind::Run, format!("Wrote {}", basename(t))));
        }
    }
    None
}

/// What a search command looked for: `-e`'s or `-Pattern`'s value, else its first word that isn't an option
/// (nor an option's value).
fn pattern(args: &[String]) -> Option<String> {
    const TAKES: &[&str] = &[
        "-g", "--glob", "-t", "--type", "-T", "--type-not", "-A", "-B", "-C", "-m", "--max-count", "-f", "--file", "--include", "--exclude", "-d", "--max-depth",
        "-path", "-literalpath", "-encoding", "-context",
    ];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let lower = a.to_ascii_lowercase();
        if matches!(lower.as_str(), "-e" | "--regexp" | "-pattern") {
            return it.next().cloned();
        }
        if TAKES.contains(&lower.as_str()) || TAKES.contains(&a.as_str()) {
            it.next();
        } else if !a.starts_with('-') {
            return Some(a.clone());
        }
    }
    None
}

/// How long `sleep 5`, `Start-Sleep -Seconds 5` or `Start-Sleep -Milliseconds 500` waits.
fn slept(args: &[String]) -> Option<String> {
    let ms = args.iter().any(|a| a.eq_ignore_ascii_case("-milliseconds") || a.eq_ignore_ascii_case("-m"));
    let n: f64 = args.iter().find_map(|a| a.trim_end_matches('s').parse().ok())?;
    Some(if ms { format!("{}ms", n.round()) } else { wait(n) })
}

/// "Installed react and zod", "Installed packages".
fn installed(pkgs: &[&str]) -> String {
    if pkgs.is_empty() { "Installed packages".to_string() } else { format!("Installed {}", list(pkgs)) }
}

/// `git`'s usual commands.
fn git(args: &[String]) -> Option<String> {
    // `git -C dir …`, `git -c key=value …`
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        i += if matches!(args[i].as_str(), "-C" | "-c") { 2 } else { 1 };
    }
    let sub = args.get(i)?.as_str();
    let rest = &args[(i + 1).min(args.len())..];
    let plain = plain(rest);
    let has = |f: &[&str]| rest.iter().any(|a| f.contains(&a.as_str()));
    let after = |f: &dyn Fn(&str) -> bool| rest.iter().position(|a| f(a)).and_then(|p| rest.get(p + 1)).map(String::as_str);
    Some(match sub {
        "status" => "Checked git status".into(),
        "diff" => "Looked at the diff".into(),
        "log" | "shortlog" | "reflog" => "Read the git log".into(),
        "show" => "Looked at a commit".into(),
        "add" => "Staged changes".into(),
        "commit" => {
            // `-m`, `--message`, and `-m` at the end of short options run together (`-am`)
            let message = after(&|a| a == "-m" || a == "--message" || (a.starts_with('-') && !a.starts_with("--") && a.ends_with('m')));
            match message.filter(|m| !m.starts_with("$(")) {
                Some(m) => format!("Committed “{}”", line(m, 50)),
                None if has(&["--amend"]) => "Amended the last commit".into(),
                None => "Committed".into(),
            }
        }
        "push" if has(&["--tags"]) => "Pushed tags".into(),
        "push" => match plain.as_slice() {
            [_, .., what] => format!("Pushed {what}"),
            _ => "Pushed".into(),
        },
        "pull" => "Pulled".into(),
        "fetch" => "Fetched the remote".into(),
        "checkout" | "switch" => match after(&|a| matches!(a, "-b" | "-B" | "-c" | "-C")) {
            Some(b) => format!("Made branch {b}"),
            None if has(&["--"]) => "Restored files".into(),
            None => match plain.first() {
                Some(b) => format!("Switched to {b}"),
                None => "Switched branches".into(),
            },
        },
        "branch" if has(&["-d", "-D", "--delete"]) => format!("Deleted branch {}", list(&plain)),
        "branch" => match plain.first() {
            Some(b) => format!("Made branch {b}"),
            None => "Listed branches".into(),
        },
        "stash" => match plain.first() {
            Some(&"pop") | Some(&"apply") => "Restored stashed changes".into(),
            Some(&"list") => "Listed stashes".into(),
            _ => "Stashed changes".into(),
        },
        "merge" => plain.first().map_or("Merged".to_string(), |b| format!("Merged {b}")),
        "rebase" => plain.first().map_or("Rebased".to_string(), |b| format!("Rebased onto {b}")),
        "reset" => "Reset changes".into(),
        "restore" => "Restored files".into(),
        "revert" => "Reverted a commit".into(),
        "cherry-pick" => "Picked a commit".into(),
        "tag" => plain.first().map_or("Listed tags".to_string(), |t| format!("Tagged {t}")),
        "clone" => plain.first().map_or("Cloned a repo".to_string(), |u| format!("Cloned {}", basename(u).trim_end_matches(".git"))),
        "init" => "Made a repo".into(),
        "worktree" => "Changed worktrees".into(),
        "blame" => "Read the blame".into(),
        "remote" => "Checked the remotes".into(),
        "rev-parse" | "describe" => "Checked the repo".into(),
        "ls-files" => "Listed the files".into(),
        "grep" => pattern(rest).map_or("Searched the repo".to_string(), |p| format!("Searched for “{}”", line(&p, 50))),
        "apply" | "am" => "Applied a patch".into(),
        _ => return None,
    })
}

/// `cargo`'s usual commands, about the package `-p` names when one does.
fn cargo(args: &[String]) -> Option<String> {
    let sub = args.iter().map(String::as_str).find(|a| !a.starts_with('-') && !a.starts_with('+'))?;
    let value = |f: &[&str]| {
        args.iter().enumerate().find_map(|(i, a)| match a.split_once('=') {
            Some((k, v)) if f.contains(&k) => Some(v),
            _ if f.contains(&a.as_str()) => args.get(i + 1).map(String::as_str),
            _ => None,
        })
    };
    let pkg = value(&["-p", "--package"]);
    let rest: Vec<String> = args.iter().skip_while(|a| a.as_str() != sub).skip(1).cloned().collect();
    let first = plain(&rest).first().map(|s| s.to_string());
    let about = |done: &str, nothing: &str| pkg.map_or(nothing.to_string(), |p| format!("{done} {p}"));
    Some(match sub {
        "test" | "t" | "nextest" => about("Tested", "Ran the tests"),
        "build" | "b" => about("Built", "Built the project"),
        "check" | "c" => about("Checked", "Checked the build"),
        "clippy" => about("Linted", "Linted the code"),
        "fmt" => "Formatted the code".into(),
        "run" | "r" => match pkg.or(value(&["--bin"])) {
            Some(p) => format!("Ran {p}"),
            None => "Ran the project".into(),
        },
        "add" => first.map_or("Added a dependency".into(), |d| format!("Added {d}")),
        "remove" | "rm" => first.map_or("Removed a dependency".into(), |d| format!("Removed {d}")),
        "update" => "Updated dependencies".into(),
        "install" => first.map_or("Installed a crate".into(), |d| format!("Installed {d}")),
        "bench" => "Ran the benchmarks".into(),
        "doc" => "Built the docs".into(),
        "clean" => "Cleaned the build".into(),
        "publish" => about("Published", "Published the crate"),
        "tree" => "Listed dependencies".into(),
        _ => return None,
    })
}

/// npm, pnpm, yarn and bun: installs, scripts, and (bun) a script file run.
fn js(prog: &str, args: &[String]) -> Option<String> {
    let plain = plain(args);
    let script_said = |s: &str| match s {
        "test" | "t" => "Ran the tests".to_string(),
        "build" => "Built the project".into(),
        "dev" | "start" | "serve" | "preview" => "Started the dev server".into(),
        "lint" => "Linted the code".into(),
        "typecheck" | "tsc" | "check" => "Checked the types".into(),
        "format" | "fmt" => "Formatted the code".into(),
        s => format!("Ran {s}"),
    };
    let first = *plain.first()?;
    Some(match first {
        "install" | "i" | "add" | "ci" => installed(&plain[1..]),
        "remove" | "rm" | "uninstall" | "un" => format!("Removed {}", list(&plain[1..])),
        "run" | "run-script" => script_said(plain.get(1)?),
        "x" | "dlx" | "exec" => format!("Ran {}", basename(plain.get(1)?)),
        f if is_script(f) => format!("Ran {}", basename(f)),
        // `npm test` and `npm start` are npm's own; yarn, pnpm and bun run any script by its name
        f if prog != "npm" || matches!(f, "test" | "t" | "start") => script_said(f),
        _ => return None,
    })
}

/// A file a script runner runs.
fn is_script(a: &str) -> bool {
    const EXT: &[&str] = &[".py", ".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".tsx", ".jsx", ".sh", ".ps1", ".rb", ".pl"];
    let lower = a.to_ascii_lowercase();
    EXT.iter().any(|e| lower.ends_with(e))
}

/// `python x.py`, `node build.mjs`, `python -m pytest`: what it runs. A one-liner (`-c`, `-e`) reads as itself.
fn script(args: &[String]) -> Option<String> {
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-m" => {
                let module = it.next()?;
                return Some(if module == "pytest" { "Ran the tests".to_string() } else { format!("Ran {module}") });
            }
            "-c" | "-e" | "--eval" | "-p" | "--print" => return None,
            a if a.starts_with('-') => {}
            a => return is_script(a).then(|| format!("Ran {}", basename(a))),
        }
    }
    None
}

/// The GitHub CLI's usual commands (`plain`: its words that aren't options).
fn gh(plain: &[&str]) -> Option<String> {
    Some(match (*plain.first()?, plain.get(1).copied()) {
        ("pr", Some("create")) => "Opened a pull request".into(),
        ("pr", Some("merge")) => "Merged the pull request".into(),
        ("pr", Some("list")) => "Listed pull requests".into(),
        ("pr", _) => "Checked the pull request".into(),
        ("run", Some("list")) => "Listed CI runs".into(),
        ("run", _) => "Checked CI".into(),
        ("release", Some("create")) => plain.get(2).map_or("Published a release".to_string(), |t| format!("Published release {t}")),
        ("release", _) => "Checked the releases".into(),
        ("issue", Some("create")) => "Opened an issue".into(),
        ("issue", _) => "Checked the issues".into(),
        ("repo", Some("clone")) => "Cloned a repo".into(),
        ("api", _) => "Called the GitHub API".into(),
        ("auth", _) => "Checked GitHub sign-in".into(),
        ("workflow", _) => "Checked the workflows".into(),
        _ => return None,
    })
}

/// Docker's (and Podman's) usual commands.
fn docker(plain: &[&str]) -> Option<String> {
    Some(match (*plain.first()?, plain.get(1).copied()) {
        ("build", _) | ("buildx", _) => "Built an image".into(),
        ("run", _) => "Ran a container".into(),
        ("compose", Some("up" | "start")) => "Started the services".into(),
        ("compose", Some("down" | "stop")) => "Stopped the services".into(),
        ("compose", Some("logs")) | ("logs", _) => "Read the logs".into(),
        ("compose", Some("ps")) | ("ps", _) => "Listed the containers".into(),
        ("compose", Some("exec")) | ("exec", _) => "Ran a command in a container".into(),
        ("pull", _) => "Pulled an image".into(),
        ("push", _) => "Pushed an image".into(),
        ("stop" | "kill", _) => "Stopped a container".into(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cmd(c: &str) -> Option<String> {
        command_label(c).map(|(_, l)| l)
    }

    #[test]
    fn skills_and_servers_read_as_their_names() {
        assert_eq!(skill_name("generate-voice"), "Generate voice");
        assert_eq!(skill_name("justgains-app-deployment"), "JustGains app deployment");
        assert_eq!(skill_name("anthropic-skills:pdf"), "PDF", "a plugin's skill without its plugin");
        assert_eq!(skill_name("claude-api"), "Claude API");
        assert_eq!(skill_name("bro-open"), "bro open", "bro is bro");
        assert_eq!(skill_name(""), "A skill");
        assert_eq!(server_name("claude_ai_JustGains"), "JustGains");
        assert_eq!(server_name("claude_ai_Claude_Docs"), "Claude Docs");
        assert_eq!(server_name("android-mcp"), "Android");
        assert_eq!(server_name("justbrowse"), "JustBrowse");
        assert_eq!(server_name("brave-search"), "Brave Search");
        assert_eq!(name("NotebookRead"), "Notebook read");
        assert_eq!(name("claude-code-guide"), "Claude code guide");
    }

    #[test]
    fn mcp_calls_say_what_they_did_and_to_what() {
        // as JustBrowse's and android-mcp's calls come in transcripts
        let t = |tool: &str, input: Value| tool_label(tool, &input);
        assert_eq!(t("chrome_navigate", json!({"url":"https://www.figma.com/design/gW6z/Just-Gains?node-id=92-1255","background":true})), "Navigate · figma.com");
        assert_eq!(t("chrome_click_element", json!({"tabId":460616203,"ref":"ref_4"})), "Click element");
        assert_eq!(t("chrome_get_web_content", json!({"tabId":1})), "Get web content");
        assert_eq!(t("chrome_fill_or_select", json!({"ref":"ref_3","value":"LocalTest!2026n8n"})), "Fill or select", "never what's typed into a form");
        assert_eq!(t("chrome_upload_file", json!({"filePath":"C:\\Temp\\video-final\\justgains-n8n-demo.mp4"})), "Upload file · justgains-n8n-demo.mp4");
        assert_eq!(t("chrome_history", json!({"text":"claim recovery","maxResults":20})), "Chrome history · “claim recovery”");
        assert_eq!(t("chrome_handle_dialog", json!({"action":"dismiss"})), "Handle dialog · dismiss");
        assert_eq!(t("get_windows_and_tabs", json!({})), "Get windows and tabs");
        assert_eq!(t("Click", json!({"x":380,"y":2440})), "Click · at 380, 2440");
        assert_eq!(t("ClickBySelector", json!({"resourceId":"tab-fuel"})), "Click by selector · tab-fuel");
        assert_eq!(t("ClickBySelector", json!({"description":"Development bypass Pro screen","timeout":5})), "Click by selector · “Development bypass Pro screen”");
        assert_eq!(t("Type", json!({"text":"omelet","x":624,"y":312})), "Type · “omelet”");
        assert_eq!(t("Wait", json!({"duration":30})), "Wait · 30s");
        assert_eq!(t("generate_workout", json!({"prompt":"30 minute upper body"})), "Generate workout · “30 minute upper body”");
        assert_eq!(t("server_localhost", json!({"url":"http://localhost:3000/a"})), "Server localhost · localhost:3000");
    }

    #[test]
    fn tool_searches_and_plans_say_what_came_of_them() {
        assert_eq!(tool_search("select:WebSearch"), "Loaded WebSearch");
        assert_eq!(tool_search("select:WebFetch,Monitor"), "Loaded WebFetch and Monitor");
        assert_eq!(tool_search("select:mcp__justbrowse__chrome_navigate,mcp__justbrowse__chrome_screenshot"), "Loaded JustBrowse tools");
        assert_eq!(tool_search("select:A,B,C"), "Loaded 3 tools");
        assert_eq!(tool_search("slack send"), "Looked for tools: “slack send”");
        let todos = json!([{"content":"Write it","status":"completed"},{"content":"Test it","status":"in_progress","activeForm":"Testing it"},{"content":"Ship it","status":"pending"}]);
        assert_eq!(plan_progress(Some(&todos), &["activeForm", "content"]), ("Updated the plan · 1/3 done".to_string(), Some("Testing it".to_string())));
        let done = json!([{"step":"a","status":"completed"}]);
        assert_eq!(plan_progress(Some(&done), &["step"]).0, "Finished the plan");
        assert_eq!(plan_progress(None, &["step"]).0, "Updated the plan");
        assert_eq!((wait(45.0), wait(1200.0), wait(90.0), wait(5400.0)), ("45s".into(), "20m".into(), "1m 30s".into(), "1h 30m".into()));
    }

    #[test]
    fn the_usual_commands_read_as_what_they_did() {
        assert_eq!(cmd("cargo test -p bro-proxy"), Some("Tested bro-proxy".into()));
        assert_eq!(cmd("cargo build --locked --release -p bro-tui --bins"), Some("Built bro-tui".into()));
        assert_eq!(cmd("$ErrorActionPreference='Stop'; cargo clippy --all-targets"), Some("Linted the code".into()), "what only sets the shell up isn't the command");
        assert_eq!(cmd("git push -u origin fix/timer-clock"), Some("Pushed fix/timer-clock".into()));
        assert_eq!(cmd("cd /repo && git add -A && git commit -m \"fix the timer\""), Some("Staged changes · Committed “fix the timer”".into()));
        assert_eq!(cmd("git commit -am 'tidy'"), Some("Committed “tidy”".into()));
        assert_eq!(cmd("git commit -m \"$(cat <<'EOF'"), Some("Committed".into()), "a message written out below isn't one to quote");
        assert_eq!(cmd("git -C /repo status --short"), Some("Checked git status".into()));
        assert_eq!(cmd("git checkout -b voice-cards"), Some("Made branch voice-cards".into()));
        assert_eq!(command_label("rg -n 'reasoning' crates/bro-proxy/src/translate"), Some((StepKind::Search, "Searched for “reasoning”".into())));
        assert_eq!(cmd("rg -g '*.rs' cleanup_route"), Some("Searched for “cleanup_route”".into()), "an option's value isn't the pattern");
        assert_eq!(cmd("Select-String -Path x.log -Pattern 'timeout'"), Some("Searched for “timeout”".into()));
        assert_eq!(command_label("curl -s https://docs.rs/wgpu | head -50"), Some((StepKind::Web, "Fetched docs.rs".into())));
        assert_eq!(cmd("npm install"), Some("Installed packages".into()));
        assert_eq!(cmd("pnpm add zod react"), Some("Installed zod and react".into()));
        assert_eq!(cmd("bun run build"), Some("Built the project".into()));
        assert_eq!(cmd("bun scripts/email-comeback-preview.ts --audience-date=2026-10-24"), Some("Ran email-comeback-preview.ts".into()));
        assert_eq!(cmd("npm run lint"), Some("Linted the code".into()));
        assert_eq!(cmd("python -m pytest -q"), Some("Ran the tests".into()));
        assert_eq!(cmd("node justimagine.mjs run chime.json"), Some("Ran justimagine.mjs".into()));
        assert_eq!(cmd("gh run watch 40752129906 --exit-status"), Some("Checked CI".into()));
        assert_eq!(cmd("gh release create v2.1.15 --notes-file x"), Some("Published release v2.1.15".into()));
        assert_eq!(cmd("cat > notes.md <<'EOF'"), Some("Wrote notes.md".into()));
        assert_eq!(cmd("head -n 40 src/main.rs"), Some("Read main.rs".into()));
        assert_eq!(cmd("Start-Sleep -Seconds 20"), Some("Waited 20s".into()));
        assert_eq!(cmd("rm -rf target/tmp build"), Some("Deleted tmp and build".into()));
        assert_eq!(cmd("& \"C:\\Program Files\\Git\\bin\\git.exe\" log --oneline -5"), Some("Read the git log".into()));
        assert_eq!(cmd("make test 2>&1 | tail"), Some("Ran make test".into()));
        // anything else reads as itself
        assert_eq!(cmd("./run.sh --fast"), None);
        assert_eq!(cmd("echo hi"), None);
        assert_eq!(cmd("python -c 'print(1)'"), None);
        assert_eq!(cmd(""), None);
    }

    #[test]
    fn a_command_line_splits_as_a_shell_would() {
        assert_eq!(shell_words("a 'b c' \"d\" e;f&&g|h 2>&1"), ["a", "b c", "d", "e", ";", "f", "&&", "g", "|", "h", "2>&1"]);
        assert_eq!(commands("cd x && FOO=1 sudo cargo test | tail; git status"), [vec!["cargo".to_string(), "test".into()], vec!["git".into(), "status".into()]]);
    }
}
