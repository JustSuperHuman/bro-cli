//! `bro --demo`: realistic fake profiles, providers, past sessions, usage, proxy traffic and bridge status, plus
//! agent-looking transcripts for the live sessions (which run a harmless shell). Used for screenshots and the
//! snapshot tests; never touches real accounts or the network.

use super::{Avail, ProxyInfo, Services, State, UsageEntry};
use crate::pane::Activity;
use bro_bridge::BridgeStatus;
use bro_core::Harness;
use bro_core::launch::CommandSpec;
use bro_core::profiles::{Profile, ProfileKind};
use bro_core::providers::{Model, Provider, ProviderMode};
use bro_core::sessions::SessionInfo;
use bro_core::usage::{Headroom, Usage, Window};
use bro_proxy::{ProxyEvent, Route, Upstream};
use std::path::PathBuf;
use crate::util::now_ms;
use std::time::{Duration, SystemTime};

/// Where the demo projects "live".
pub fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/demo")).join("code")
}

fn profile(id: &str, kind: ProfileKind, plan: &str, email: &str) -> Profile {
    let name = id.split(':').nth(1).unwrap_or(id).to_string();
    let home = dirs::home_dir().unwrap_or_default();
    let dir = match kind {
        ProfileKind::ClaudeLocal => home.join(".claude"),
        ProfileKind::ClaudeAccount => home.join(".claude-max-pool").join("accounts").join(&name),
        ProfileKind::CodexLocal => home.join(".codex"),
        ProfileKind::CodexProfile => home.join(".bro").join("codex-profiles").join(&name),
    };
    Profile { id: id.into(), kind, name, dir, authenticated: true, plan: Some(plan.into()), tier: None, identity: None, email: Some(email.into()) }
}

fn model(id: &str, name: &str) -> Model {
    Model { id: id.into(), name: Some(name.into()), context: None, extra: Default::default() }
}

fn provider(id: &str, name: &str, mode: ProviderMode, base: &str, models: Vec<Model>) -> Provider {
    Provider {
        id: id.into(),
        name: name.into(),
        mode,
        base_url: (!base.is_empty()).then(|| base.into()),
        responses_base_url: None,
        key_env: None,
        key_url: None,
        no_key: false,
        disable_1m_context: false,
        env: Default::default(),
        models,
        extra: Default::default(),
    }
}

fn window(pct: f32, resets_in: i64, mins: u32) -> Option<Window> {
    Some(Window { used_pct: pct, resets_at: Some(crate::util::now_secs() + resets_in), window_mins: Some(mins) })
}

fn usage(h5: f32, r5: i64, wk: f32, rwk: i64, plan: &str) -> Usage {
    Usage { five_hour: window(h5, r5, 300), weekly: window(wk, rwk, 10_080), scoped: vec![], plan: Some(plan.into()), credits: None, fetched_at: crate::util::now_secs() }
}

fn past(h: Harness, profile: Option<&str>, project: &str, title: &str, ago_secs: u64) -> SessionInfo {
    let cwd = root().join(project);
    SessionInfo {
        id: format!("{:08x}-demo", (title.len() as u64 * 2_654_435_761) ^ ago_secs),
        harness: h,
        profile_id: profile.map(String::from),
        path: cwd.join(".demo.jsonl"),
        project: Some(super::local_project_for(&cwd)),
        cwd: Some(cwd),
        title: title.into(),
        branch: Some("main".into()),
        modified: SystemTime::now() - Duration::from_secs(ago_secs),
        size: 48_000,
    }
}

/// Fill a state with the demo world.
pub fn fill(st: &mut State) {
    st.profiles = Avail::Ready(vec![
        profile("claude:local", ProfileKind::ClaudeLocal, "max", "me@example.com"),
        profile("claude:personal", ProfileKind::ClaudeAccount, "pro", "me+home@example.com"),
        profile("claude:work", ProfileKind::ClaudeAccount, "max", "me@work.example"),
        profile("codex:local", ProfileKind::CodexLocal, "plus", "me@example.com"),
        profile("codex:team", ProfileKind::CodexProfile, "pro", "team@work.example"),
    ]);
    st.providers = Avail::Ready(vec![
        provider("anthropic", "Claude (Anthropic)", ProviderMode::Native, "", vec![model("claude-opus-5", "Claude Opus 5"), model("claude-sonnet-5", "Claude Sonnet 5"), model("claude-haiku-4-5-20251001", "Claude Haiku 4.5")]),
        provider("openai", "OpenAI", ProviderMode::Openai, "https://api.openai.com/v1", vec![model("gpt-5.2-codex", "GPT-5.2 Codex"), model("gpt-5.2", "GPT-5.2"), model("gpt-5-mini", "GPT-5 mini")]),
        provider("openrouter", "OpenRouter", ProviderMode::Anthropic, "https://openrouter.ai/api", vec![model("moonshotai/kimi-k2", "Kimi K2"), model("qwen/qwen3-coder", "Qwen3 Coder"), model("x-ai/grok-code-fast-1", "Grok Code Fast")]),
        provider("zai", "Z.ai", ProviderMode::Anthropic, "https://api.z.ai/api/anthropic", vec![model("glm-4.6", "GLM 4.6"), model("glm-4.5-air", "GLM 4.5 Air")]),
        provider("deepseek", "DeepSeek", ProviderMode::Openai, "https://api.deepseek.com/v1", vec![model("deepseek-chat", "DeepSeek V3.2")]),
    ]);
    st.models = demo_models(st.providers.ready().map(Vec::as_slice).unwrap_or(&[]));
    st.past = Avail::Ready(vec![
        past(Harness::Claude, Some("claude:work"), "bro-cli-v2", "port the oriel event loop and split tree", 1_800),
        past(Harness::Codex, Some("codex:local"), "bro-cli-v2", "anthropic <-> responses streaming fixtures", 7_200),
        past(Harness::Claude, Some("claude:personal"), "bro-cli-v2", "why does the pool fail over twice?", 26_000),
        past(Harness::Pi, None, "bro-cli-v2", "draft the launcher spec", 90_000),
        past(Harness::Claude, Some("claude:work"), "justgains", "fix the flaky workout timer test", 3_600),
        past(Harness::Codex, Some("codex:team"), "justgains", "migrate settings screen to expo-router", 50_000),
        past(Harness::Claude, Some("claude:local"), "justgains", "ghost workouts after discard", 200_000),
        past(Harness::Omp, None, "terminal", "bridge: add OSC 777 notifications", 12_000),
        past(Harness::Claude, Some("claude:work"), "terminal", "qr pairing for the web client", 400_000),
        past(Harness::Codex, Some("codex:local"), "dotfiles", "tidy the pwsh profile", 900_000),
    ]);
    let mut u = std::collections::BTreeMap::new();
    let mut put = |id: &str, us: Usage, now: f32, week: f32, capped: bool| {
        u.insert(id.to_string(), UsageEntry { usage: Some(us), headroom: Some(Headroom { now, week, capped }), error: None, fetching: false });
    };
    put("claude:work", usage(58.0, 2 * 3600 + 14 * 60, 31.0, 4 * 86_400 + 3 * 3600, "max"), 42.0, 69.0, false);
    put("claude:personal", usage(12.0, 4 * 3600 + 2 * 60, 64.0, 2 * 86_400, "pro"), 36.0, 36.0, true);
    put("claude:local", usage(91.0, 38 * 60, 88.0, 86_400 + 5 * 3600, "max"), 9.0, 12.0, true);
    put("codex:local", usage(12.0, 3 * 3600, 40.0, 5 * 86_400, "plus"), 88.0, 60.0, false);
    put("codex:team", usage(3.0, 4 * 3600 + 40 * 60, 8.0, 6 * 86_400, "pro"), 97.0, 92.0, false);
    st.usage = u;
    st.usage_at = Some(std::time::Instant::now());
    st.picks = (Some("claude:work".into()), Some("claude:personal".into()));
    st.proxy.status = Avail::Ready(ProxyInfo { port: 3458, base: "http://127.0.0.1:3458".into(), token: "demo-token".into() });
    st.proxy.routes = vec![
        Route { id: "r-7f3a".into(), upstream: Upstream::Anthropic { base_url: "https://openrouter.ai/api".into(), api_key: None, bearer: true }, default_model: Some("moonshotai/kimi-k2".into()), small_model: None, model_map: vec![], label: "pi · openrouter · kimi-k2".into() },
        Route { id: "r-c01d".into(), upstream: Upstream::ClaudePool { config_dirs: vec![PathBuf::from("work"), PathBuf::from("personal")] }, default_model: None, small_model: None, model_map: vec![], label: "codex · pool · opus-5".into() },
        Route { id: "r-9e21".into(), upstream: Upstream::ChatGptCodex { codex_home: PathBuf::from("~/.codex") }, default_model: Some("gpt-5.2-codex".into()), small_model: Some("gpt-5-mini".into()), model_map: vec![], label: "claude · codex:local · gpt-5.2".into() },
    ];
    let now = now_ms();
    st.proxy.pool = ["work", "personal"]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut s = bro_proxy::PoolAccountStatus { name: n.to_string(), config_dir: PathBuf::from(n), available: i == 0, rate_limited_until_ms: (i == 1).then(|| now_ms() + 41 * 60_000), usage: Default::default() };
            s.usage.window_requests = [214, 97][i];
            s.usage.window_input_tokens = [8_400_000, 3_100_000][i];
            s.usage.window_output_tokens = [312_000, 88_000][i];
            s.usage.window_cost_usd = [41.2, 12.7][i];
            s.usage.last_error = (i == 1).then(|| "429 rate limited — cooling down".to_string());
            s
        })
        .collect();
    st.proxy.events = (0..14).map(|i| fake_event(now - (14 - i) * 23_000, i as u64)).collect();
    st.bridge.status = Avail::Ready(BridgeStatus {
        running: true,
        port: 10001,
        urls: vec!["http://127.0.0.1:10001/?token=demo-9f3a7c21e4b8".into(), "http://192.168.1.23:10001/?token=demo-9f3a7c21e4b8".into(), "http://100.101.7.4:10001/?token=demo-9f3a7c21e4b8".into()],
        token: "demo-9f3a7c21e4b8".into(),
        clients: 3,
        error: None,
    });
    st.bridge.qr = Some(fake_qr());
    st.bridge.enabled = true;
    st.proxy.enabled = true;
}

/// A plausible proxy event; `seed` varies route/status/tokens.
pub fn fake_event(at_ms: i64, seed: u64) -> ProxyEvent {
    let routes = [("r-7f3a", "anthropic", "openrouter", "moonshotai/kimi-k2", None), ("r-c01d", "responses", "claude-pool", "claude-opus-5", Some("work")), ("r-9e21", "anthropic", "chatgpt-codex", "gpt-5.2-codex", None)];
    let (route, inbound, upstream, model, acct) = routes[(seed % 3) as usize];
    let status = match seed % 11 {
        7 => 429,
        10 => 502,
        _ => 200,
    };
    let acct = if seed % 4 == 1 && acct.is_some() { Some("personal") } else { acct };
    ProxyEvent {
        at_ms,
        route_id: route.into(),
        inbound: inbound.into(),
        upstream: upstream.into(),
        model: model.into(),
        status,
        stream: seed % 3 != 2,
        input_tokens: (status == 200).then_some(1_200 + (seed * 7_919) % 48_000),
        output_tokens: (status == 200).then_some(40 + (seed * 104_729) % 3_000),
        cache_read_tokens: (status == 200 && seed.is_multiple_of(2)).then_some(20_000 + (seed * 31) % 9_000),
        latency_ms: 380 + (seed * 7_717) % 9_000,
        error: match status {
            429 => Some("rate limited — failed over".into()),
            502 => Some("upstream closed the stream".into()),
            _ => None,
        },
        account: acct.map(String::from),
    }
}

/// A fake pairing QR (finder squares + deterministic noise) as half-block rows.
pub fn fake_qr() -> Vec<String> {
    const N: usize = 25;
    let mut m = [[false; N]; N];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for row in m.iter_mut() {
        for cell in row.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *cell = seed % 5 < 2;
        }
    }
    for (oy, ox) in [(0, 0), (0, N - 7), (N - 7, 0)] {
        for y in 0..7 {
            for x in 0..7 {
                let edge = y == 0 || y == 6 || x == 0 || x == 6;
                let core = (2..=4).contains(&y) && (2..=4).contains(&x);
                m[oy + y][ox + x] = edge || core;
            }
        }
    }
    let at = |y: isize, x: isize| y >= 0 && x >= 0 && (y as usize) < N && (x as usize) < N && m[y as usize][x as usize];
    (-1..N as isize + 1)
        .step_by(2)
        .map(|y| {
            (-1..N as isize + 1)
                .map(|x| match (at(y, x), at(y + 1, x)) {
                    (true, true) => ' ',
                    (true, false) => '▄',
                    (false, true) => '▀',
                    (false, false) => '█',
                })
                .collect()
        })
        .collect()
}

/// Keep the demo feeling alive: a fake proxy request every few seconds.
pub fn spawn_ticker(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        let mut seed = 100u64;
        loop {
            std::thread::sleep(Duration::from_millis(2_500 + (seed % 5) * 900));
            seed += 1;
            let e = fake_event(crate::util::now_ms(), seed);
            svc.update(|st| {
                st.proxy.events.push_back(e);
                while st.proxy.events.len() > super::EVENT_CAP {
                    st.proxy.events.pop_front();
                }
            });
        }
    });
}

/// A demo session for the sidebar: (harness, profile, model, project, activity, done-unseen).
pub struct DemoSession {
    pub harness: Option<Harness>,
    pub profile: Option<&'static str>,
    pub model: Option<&'static str>,
    pub project: &'static str,
    pub activity: Option<Activity>,
    pub done: bool,
}

/// The live sessions `bro --demo` opens.
pub fn sessions() -> Vec<DemoSession> {
    use Activity::*;
    let s = |harness, profile, model, project, activity, done| DemoSession { harness, profile, model, project, activity, done };
    vec![
        s(Some(Harness::Claude), Some("claude:work"), Some("claude-opus-5"), "bro-cli-v2", Some(Working), false),
        s(Some(Harness::Codex), Some("codex:local"), Some("gpt-5.2-codex"), "bro-cli-v2", Some(Idle), true),
        s(None, None, None, "bro-cli-v2", None, false),
        s(Some(Harness::Claude), Some("claude:personal"), Some("claude-sonnet-5"), "justgains", Some(Blocked), false),
        s(Some(Harness::Pi), Some("openrouter"), Some("moonshotai/kimi-k2"), "terminal", Some(Idle), false),
    ]
}

/// An agent-looking transcript (ANSI) for a demo pane.
pub fn transcript(h: Option<Harness>) -> String {
    let (o, d, g, r, b, x) = ("\x1b[38;2;217;119;87m", "\x1b[2m", "\x1b[32m", "\x1b[31m", "\x1b[1m", "\x1b[0m");
    match h {
        Some(Harness::Claude) => format!(
            "{o}╭───────────────────────────────────────────────╮{x}\r\n{o}│{x} {o}✻{x} Welcome to {b}Claude Code{x}!                     {o}│{x}\r\n{o}│{x}   {d}cwd: ~/code/bro-cli-v2{x}                      {o}│{x}\r\n{o}╰───────────────────────────────────────────────╯{x}\r\n\r\n\
             {d}>{x} wire the launcher to the proxy routes\r\n\r\n\
             {o}●{x} I'll look at how launches register routes first.\r\n\r\n\
             {o}●{x} {b}Read{x}(crates/bro-tui/src/services/launch.rs)\r\n  {d}⎿  Read 148 lines{x}\r\n\r\n\
             {o}●{x} {b}Update{x}(crates/bro-tui/src/services/launch.rs)\r\n  {d}⎿  Updated with {g}12 additions{x}{d} and {r}3 removals{x}\r\n\r\n\
             {o}✻ Pondering…{x} {d}(38s · ↓ 2.1k tokens · esc to interrupt){x}\r\n"
        ),
        Some(Harness::Codex) => format!(
            "{b}>_ OpenAI Codex{x} {d}(v0.60){x}\r\n{d}model: gpt-5.2-codex   directory: ~/code/bro-cli-v2{x}\r\n\r\n\
             {b}user{x}\r\nadd golden fixtures for responses streaming\r\n\r\n\
             {g}•{x} Explored\r\n  {d}└ Read fixtures/responses_stream.jsonl{x}\r\n\r\n\
             {g}•{x} Edited crates/bro-proxy/tests/golden.rs {g}(+86 -0){x}\r\n\r\n\
             {b}codex{x}\r\nAdded 6 fixtures covering text, tool calls and reasoning deltas.\r\nAll 41 proxy tests pass.\r\n\r\n{d}› {x}\r\n"
        ),
        Some(Harness::Pi) | Some(Harness::Omp) => format!(
            "{b}pi{x} {d}· openrouter · kimi-k2{x}\r\n\r\n{d}>{x} summarise the bridge protocol\r\n\r\n\
             The bridge speaks {b}/ws{x} with hello → snapshot → output events,\r\nplus /api/sessions for create / kill / rename.\r\n\r\n{d}ready ·{x} \r\n"
        ),
        None => format!("{g}PS{x} ~/code/bro-cli-v2> git status --short\r\n M crates/bro-tui/src/app/mod.rs\r\n?? crates/bro-tui/snapshots/\r\n{g}PS{x} ~/code/bro-cli-v2> "),
    }
}

/// A harmless command for a demo session: print the transcript, then an interactive shell.
pub fn shell_command(h: Option<Harness>, label: &str, cwd: PathBuf) -> CommandSpec {
    let dir = std::env::temp_dir().join("bro-demo");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{}.ans", h.map(|h| h.label()).unwrap_or("shell")));
    let _ = std::fs::write(&file, transcript(h));
    let f = file.to_string_lossy().to_string();
    let (program, args) = if cfg!(windows) {
        let (sh, _) = crate::util::default_shell(None);
        if sh.contains("cmd") {
            (sh, vec!["/k".into(), format!("type \"{f}\"")])
        } else {
            (sh, vec!["-NoLogo".into(), "-NoExit".into(), "-Command".into(), format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8; [Console]::Out.Write([IO.File]::ReadAllText('{}'))", f.replace('\'', "''"))])
        }
    } else {
        ("sh".into(), vec!["-c".into(), format!("cat '{}'; exec \"${{SHELL:-sh}}\"", f.replace('\'', "'\\''"))])
    };
    let cwd = if cwd.is_dir() { cwd } else { dirs::home_dir().unwrap_or_else(std::env::temp_dir) };
    CommandSpec { program, args, env: vec![], env_remove: vec![], cwd, label: label.into(), route: None, cleanup: vec![] }
}

/// Launcher model lists for the demo (never reads the real OpenRouter cache).
pub fn demo_models(providers: &[Provider]) -> std::collections::BTreeMap<String, Vec<bro_core::catalogue::ModelRow>> {
    use bro_core::catalogue::ModelRow;
    let row = |id: &str, name: &str, ctx: Option<u64>, price: Option<(f64, f64)>| ModelRow { id: id.into(), name: name.into(), context: ctx, pricing: price, reasoning: false, created: None };
    let mut m: std::collections::BTreeMap<String, Vec<ModelRow>> =
        providers.iter().map(|p| (p.id.clone(), p.models.iter().map(|x| row(&x.id, x.name.as_deref().unwrap_or(&x.id), None, None)).collect())).collect();
    m.insert(
        "openrouter".into(),
        vec![
            row("moonshotai/kimi-k2.7-code", "MoonshotAI: Kimi K2.7 Code", Some(262_144), Some((0.6, 2.5))),
            row("qwen/qwen3-coder", "Qwen: Qwen3 Coder", Some(262_144), Some((0.22, 0.95))),
            row("x-ai/grok-code-fast-1", "xAI: Grok Code Fast 1", Some(256_000), Some((0.2, 1.5))),
            row("deepseek/deepseek-v4", "DeepSeek: V4", Some(1_048_576), Some((0.58, 1.73))),
            row("z-ai/glm-5.3", "Z.AI: GLM 5.3", Some(200_000), Some((0.6, 2.2))),
            row("google/gemini-3-pro", "Google: Gemini 3 Pro", Some(1_048_576), Some((1.25, 10.0))),
        ],
    );
    m.insert("codex".into(), vec![row("gpt-5.2-codex", "GPT-5.2 Codex", None, None), row("gpt-5.2", "GPT-5.2", None, None), row("gpt-5-mini", "GPT-5 mini", None, None)]);
    let claude = m.get("anthropic").cloned().unwrap_or_default();
    m.insert("claude".into(), claude);
    m
}
