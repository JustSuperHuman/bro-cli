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
        ProfileKind::GeminiApi => home.join(".bro").join("gemini-profiles").join(&name),
        ProfileKind::GeminiSubscription => home.join(".bro").join("gemini"),
    };
    Profile { id: id.into(), kind, name, dir, authenticated: true, plan: Some(plan.into()), tier: None, identity: None, email: (!email.is_empty()).then(|| email.into()) }
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
    Usage { five_hour: window(h5, r5, 300), weekly: window(wk, rwk, 10_080), scoped: vec![], plan: Some(plan.into()), credits: None, api: None, subscription: None, fetched_at: crate::util::now_secs() }
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
        profile("gemini-sub:local", ProfileKind::GeminiSubscription, "Google AI Ultra", "me@example.com"),
        profile("gemini:work", ProfileKind::GeminiApi, "API key", ""),
    ]);
    st.providers = Avail::Ready(vec![
        provider("anthropic", "Claude (Anthropic)", ProviderMode::Native, "", vec![model("claude-opus-5", "Claude Opus 5"), model("claude-sonnet-5", "Claude Sonnet 5"), model("claude-haiku-4-5-20251001", "Claude Haiku 4.5")]),
        provider("openai", "OpenAI", ProviderMode::Openai, "https://api.openai.com/v1", vec![model("gpt-5.2-codex", "GPT-5.2 Codex"), model("gpt-5.2", "GPT-5.2"), model("gpt-5-mini", "GPT-5 mini")]),
        provider("openrouter", "OpenRouter", ProviderMode::Anthropic, "https://openrouter.ai/api", vec![model("moonshotai/kimi-k2", "Kimi K2"), model("qwen/qwen3-coder", "Qwen3 Coder"), model("x-ai/grok-code-fast-1", "Grok Code Fast")]),
        provider("zai", "Z.ai", ProviderMode::Anthropic, "https://api.z.ai/api/anthropic", vec![model("glm-4.6", "GLM 4.6"), model("glm-4.5-air", "GLM 4.5 Air")]),
        provider("deepseek", "DeepSeek", ProviderMode::Openai, "https://api.deepseek.com/v1", vec![model("deepseek-chat", "DeepSeek V3.2")]),
        provider("gemini", "Gemini (Google AI subscription)", ProviderMode::Antigravity, "", vec![model("gemini-4-argon", "Gemini 4 Argon"), model("gemini-4-argon-high", "Gemini 4 Argon (high thinking)")]),
    ]);
    st.models = demo_models(st.providers.ready().map(Vec::as_slice).unwrap_or(&[]));
    st.models.insert("gemini:work".into(), vec![bro_core::catalogue::ModelRow { id: "gemini-3.8-flash".into(), name: "Gemini Flash".into(), context: Some(1048576), pricing: None, reasoning: true, created: None }]);
    st.models.insert("gemini-sub:local".into(), vec![bro_core::catalogue::ModelRow { id: "gemini-4-argon-high".into(), name: "Gemini 4 Argon (high)".into(), context: Some(1048576), pricing: None, reasoning: true, created: None }]);
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
    u.insert("gemini:work".into(), UsageEntry { usage: Some(Usage { api: Some(bro_core::gemini_api::ApiUsage { today: bro_core::gemini_api::Totals { requests: 42, input: 120000, output: 34000, cached: 80000, metered: 42, errors: 0 }, month: bro_core::gemini_api::Totals { requests: 680, input: 2100000, output: 470000, cached: 900000, metered: 680, errors: 2 } }), ..Default::default() }), ..Default::default() });
    u.insert("gemini-sub:local".into(), UsageEntry { usage: Some(Usage { plan: Some("Google AI Ultra".into()), subscription: Some(vec![bro_core::gemini::ModelQuota { id: "gemini-4-argon-high".into(), name: Some("Gemini 4 Argon".into()), remaining: Some(0.73), reset: Some(chrono::DateTime::from_timestamp(crate::util::now_secs() + 7200, 0).unwrap().to_rfc3339()) }, bro_core::gemini::ModelQuota { id: "gemini-3.8-flash-high".into(), name: Some("Gemini 3.8 Flash".into()), remaining: Some(0.94), reset: None }]), ..Default::default() }), ..Default::default() });
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
    // proxy mode's logins: two Claude logins sharing the work while the third rests until its reset
    let login = |id: &str, h5: f32, wk: f32, conversations: usize, requests: u64, cooling: Option<i64>| {
        let mut s = bro_proxy::LoginStatus { id: id.into(), dir: PathBuf::from(id), available: cooling.is_none(), cooling_until_ms: cooling, five_hour_pct: Some(h5), weekly_pct: Some(wk), five_hour_resets_at: None, weekly_resets_at: None, conversations, usage: Default::default() };
        s.usage.window_requests = requests;
        s
    };
    st.proxy.logins = vec![
        login("claude:local", 38.0, 22.0, 3, 186, None),
        login("claude:work", 61.0, 47.0, 2, 241, None),
        login("claude:personal", 100.0, 71.0, 0, 128, Some(now + 52 * 60_000)),
        login("codex:team", 12.0, 30.0, 1, 44, None),
    ];
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
    // the Skills view: Theo's tstack is watched (some of its skills installed), and one skill was installed from
    // Skill Issue
    st.settings.skills.repos = vec![TSTACK.into(), REACT_PICK.into()];
    let ago = |secs: u64| std::time::Instant::now().checked_sub(Duration::from_secs(secs));
    let repo = |spec: &str, commit: &str, skills: &[(&str, &str)], secs: u64| super::skills::RepoState {
        skills: Some(bro_core::skill_repos::RepoSkills {
            commit: Some(commit.into()),
            skills: skills.iter().map(|(n, _)| n.to_string()).collect(),
            found: skills.iter().map(|(n, d)| bro_core::skill_repos::RepoSkill { name: n.to_string(), description: Some(d.to_string()), dir: bro_core::skill_repos::cache_dir(spec).join("skills").join(n) }).collect(),
            taken: vec![],
        }),
        synced: ago(secs),
        ..Default::default()
    };
    st.skills.repos.insert(TSTACK.into(), repo(TSTACK, "92e38e2", &TSTACK_SKILLS, 2 * 3600));
    st.skills.repos.insert(REACT_PICK.into(), repo(REACT_PICK, "4f1c9d2", &[("react-best-practices", REACT_ABOUT)], 2 * 3600));
    st.skills.own = ["brand-voice", "humanize-writing", "pdf-forms", "higgsfield-generate"].map(String::from).to_vec();
}

/// The repo the demo watches, and the skill it picked from Skill Issue.
const TSTACK: &str = "t3dotgg/tstack";
const REACT_PICK: &str = "vercel-labs/agent-skills#skills/react-best-practices";
const REACT_ABOUT: &str = "Vercel's impact-ranked React and Next.js performance rules: waterfalls, bundle size, re-renders.";

/// The tstack skills installed in the demo (sol-loop is disabled; the rest wait to be picked).
const TSTACK_INSTALLED: [&str; 5] = ["babysit-pr", "council", "file-pr", "full-send", "full-stop"];

/// tstack's skills as their SKILL.md files describe them.
const TSTACK_SKILLS: [(&str, &str); 9] = [
    ("babysit-pr", "Monitor a pull request through review and CI. Use when the user asks to monitor, watch, or babysit a PR."),
    ("council", "Trigger this if the user says 'council'."),
    ("file-pr", "File a concise pull request. Use when the user asks to file, open, or create a PR."),
    ("full-send", "Trigger this if the user says 'full send'."),
    ("full-stop", "Trigger this if the user says 'full stop', even as a closing \"Full stop.\" that reads like emphasis."),
    ("leaving-pr-comments", "Use whenever leaving comments on GitHub."),
    ("modernize", "For getting old prs up to date. Use when asked to rebase a pr or \"modernize\"."),
    ("sol-loop", "Use when the user says \"sol loop\"."),
    ("takeover", "Take over existing work. Use when the user asks to take over a PR, branch, or agent thread, or requests a takeover."),
];

/// The Skills view's installed skills in the demo: bro's own, some of tstack's, the one from Skill Issue, Codex's, a
/// few of yours, and two of the demo projects' own; and two disabled ones (one of tstack's, one of yours).
pub fn skill_inventory() -> bro_core::skill_inventory::Inventory {
    use bro_core::disabled_skills::Disabled;
    use bro_core::skill_inventory::{Agent, Inventory, Origin, Place, Skill};
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/demo"));
    let folder = |a: Agent| match a {
        Agent::Claude => home.join(".claude"),
        Agent::Codex => home.join(".codex"),
        Agent::Shared => home.join(".agents"),
        Agent::Pi => home.join(".pi").join("agent"),
        Agent::Omp => home.join(".omp").join("agent"),
    };
    let skill = |name: &str, description: &str, origin: Origin, project: Option<&str>, agents: &[Agent]| {
        let places = agents
            .iter()
            .map(|&agent| {
                let skills = match project {
                    Some(p) => root().join(p).join(match agent {
                        Agent::Codex => ".codex",
                        Agent::Shared => ".agents",
                        _ => ".claude",
                    }),
                    None => folder(agent),
                };
                let dir = match origin {
                    Origin::Codex => skills.join("skills").join(".system").join(name),
                    _ => skills.join("skills").join(name),
                };
                Place { agent, dir }
            })
            .collect();
        Skill { name: name.into(), description: Some(description.into()), origin, project: project.map(|p| root().join(p)), places }
    };
    use Agent::*;
    let mut skills = vec![
        skill("bro-open", "Show the user a file (optionally at a line) in bro's file preview on their desktop, web and phone.", Origin::Bro, None, &[Claude, Codex]),
        skill("justtools", "Tells agents when to reach for the just* commands: images, PDFs, QR codes, ports, archives.", Origin::Bro, None, &[Claude, Codex]),
        skill("generate-images", "Generate images through the local JustImagine server, including the saved cast of characters.", Origin::Bro, None, &[Claude, Codex]),
        skill("generate-video", "Generate video clips through JustImagine with Veo, Seedance, Kling and more.", Origin::Bro, None, &[Claude, Codex]),
        skill("generate-voice", "Generate speech through JustImagine on ElevenLabs, each cast member in their own voice.", Origin::Bro, None, &[Claude, Codex]),
    ];
    skills.extend(TSTACK_SKILLS.iter().filter(|(n, _)| TSTACK_INSTALLED.contains(n)).map(|(n, d)| skill(n, d, Origin::Repo(TSTACK.into()), None, &[Claude, Codex])));
    skills.extend([
        skill("react-best-practices", REACT_ABOUT, Origin::Repo(REACT_PICK.into()), None, &[Claude, Codex]),
        skill("imagegen", "Generate or edit images with OpenAI's image models.", Origin::Codex, None, &[Codex]),
        skill("openai-docs", "Answer questions about OpenAI's APIs from the official docs.", Origin::Codex, None, &[Codex]),
        skill("skill-creator", "Create a new skill, or improve one, with the right layout and frontmatter.", Origin::Codex, None, &[Codex]),
        skill("brand-voice", "Write the way the brand sounds: short sentences, plain words, no hype.", Origin::Yours, None, &[Claude]),
        skill("humanize-writing", "Rewrite text that reads as machine-made so it reads like a person wrote it.", Origin::Yours, None, &[Claude, Shared]),
        skill("pdf-forms", "Fill in PDF forms, flatten them and check every field landed.", Origin::Yours, None, &[Shared]),
        // a description that runs over several lines, as YAML's `|` keeps it
        skill("higgsfield-generate", "Generate images/videos/3D assets/audio via Higgsfield AI. Defaults:\nGPT Image 2 for image/design/text, Seedance 2.0 for video,\nNano Banana 2/Lite/Pro for character/reference images.\n\nUse when: \"generate an image\", \"make a video\", \"animate this photo\".", Origin::Yours, None, &[Claude]),
        skill("release", "Cut a bro release: bump the version, tag it, build the installers and publish them.", Origin::Yours, Some("bro-cli-v2"), &[Claude, Shared]),
        skill("snapshot-tests", "Run the terminal UI's snapshot tests and accept only the changes you meant.", Origin::Yours, Some("bro-cli-v2"), &[Claude]),
        skill("expo-deploy", "Build the JustGains app with EAS and ship it to TestFlight and the Play Store.", Origin::Yours, Some("justgains"), &[Codex]),
    ]);
    // in the order a scan finds them
    skills.sort_by(|a, b| (a.project.is_some(), &a.project, &a.origin, a.name.to_lowercase()).cmp(&(b.project.is_some(), &b.project, &b.origin, b.name.to_lowercase())));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let disabled = |s: Skill, days: u64| Disabled { id: s.name.clone(), name: s.name, description: s.description, origin: s.origin, project: s.project, places: s.places, at: now.saturating_sub(days * 86_400) };
    let sol = TSTACK_SKILLS.iter().find(|(n, _)| *n == "sol-loop").map_or("", |(_, d)| d);
    let disabled = vec![
        disabled(skill("old-release-notes", "Write release notes the old way: a changelog per package, then a summary on top.", Origin::Yours, None, &[Claude]), 9),
        disabled(skill("sol-loop", sol, Origin::Repo(TSTACK.into()), None, &[Claude, Codex]), 3),
    ];
    Inventory { skills, disabled }
}

/// The skills view's catalogue in the demo: three skills from each of Skill Issue's domains, as fetched.
pub fn skill_catalog() -> bro_core::skill_market::Catalog {
    serde_json::from_str(include_str!("../../assets/demo-skills.json")).unwrap_or_default()
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
        trim_tokens_before: None,
        trim_tokens_after: None,
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

/// The agent team the first demo session leads: three teammates on a review, their task list half done.
pub fn team() -> bro_core::agent_teams::Team {
    use bro_core::agent_teams::{Member, Task, TaskStatus, Team};
    let member = |name: &str, color: &str, agent_type: Option<&str>, model: Option<&str>, active: bool| Member {
        name: name.into(),
        agent_id: format!("{name}@session-demo"),
        agent_type: agent_type.map(String::from),
        color: Some(color.into()),
        model: model.map(String::from),
        active,
        ..Member::default()
    };
    let task = |id: &str, subject: &str, doing: &str, status: TaskStatus, owner: Option<&str>, after: &[&str], description: &str| Task {
        id: id.into(),
        subject: subject.into(),
        description: description.into(),
        active_form: Some(doing.into()),
        owner: owner.map(String::from),
        status,
        blocked_by: after.iter().map(|s| s.to_string()).collect(),
    };
    use TaskStatus::*;
    Team {
        name: "session-demo".into(),
        members: vec![
            Member { name: "team-lead".into(), agent_id: "team-lead@session-demo".into(), lead: true, ..Member::default() },
            member("security", "blue", Some("security-reviewer"), Some("opus"), true),
            member("perf", "green", None, Some("sonnet"), true),
            member("tests", "purple", Some("test-runner"), Some("sonnet"), false),
        ],
        tasks: vec![
            task("1", "Map the auth and session code", "Mapping the auth code", Completed, Some("security"), &[], "List every place tokens are minted, stored or checked."),
            task("2", "Audit token handling", "Auditing token handling", InProgress, Some("security"), &["1"], "JWTs in httpOnly cookies: expiry, rotation, revocation, and what a stolen refresh token can do."),
            task("3", "Profile the request hot path", "Profiling the hot path", InProgress, Some("perf"), &[], "Find what each request allocates and where the time goes under 200 concurrent sessions."),
            task("4", "Cover the new branches with tests", "Writing tests", Completed, Some("tests"), &[], "Every new branch in src/auth gets a test that fails without it."),
            task("5", "Run the suite against the fixes", "Running the suite", Pending, Some("tests"), &["2", "3"], "Once security and perf land their fixes, run everything and report flakes."),
            task("6", "Write up the findings", "Writing the report", Pending, None, &["2", "3", "5"], "One report: each finding with its severity, the fix and the test that covers it."),
        ],
    }
}

/// An agent-looking transcript (ANSI) for a demo pane, at the width the old fixed screens used.
pub fn transcript(h: Option<Harness>) -> String {
    transcript_for(h, 200)
}

/// The width of `s` on screen, its escape sequences not counted.
fn visible_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            w += c.width().unwrap_or(0);
        }
    }
    w
}

/// Where a line's continuation starts: under its text, past a bullet, a `⎿` or a `1.`.
fn hang_of(line: &str) -> usize {
    let mut plain = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    let lead = plain.chars().take_while(|c| *c == ' ').count();
    let rest: String = plain.chars().skip(lead).collect();
    let token = rest.split(' ').next().unwrap_or("");
    let numbered = token.len() > 1 && token.ends_with('.') && token[..token.len() - 1].chars().all(|c| c.is_ascii_digit());
    if !(matches!(token, "●" | "•" | ">" | "✻" | "⎿" | "└" | "›" | "❯") || numbered) {
        return lead;
    }
    let after = rest.chars().skip(token.chars().count()).take_while(|c| *c == ' ').count();
    lead + token.chars().count() + after
}

/// `line` (ANSI) broken at spaces to fit `width`, a continuation hanging under the text and carrying the colours
/// the break interrupted. Unlike the terminal's own wrap it never splits a word that fits on a line.
fn wrap_ansi(line: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }
    let hang = hang_of(line).min(width / 2);
    // (is an escape, text): escapes, words and runs of spaces
    let mut tokens: Vec<(bool, String)> = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            let mut e = String::from("\x1b");
            for n in chars.by_ref() {
                e.push(n);
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
            tokens.push((true, e));
        } else {
            let space = c == ' ';
            match tokens.last_mut() {
                Some((false, t)) if t.starts_with(' ') == space => t.push(c),
                _ => tokens.push((false, c.to_string())),
            }
        }
    }
    let (mut out, mut cur, mut cur_w, mut active) = (Vec::new(), String::new(), 0usize, Vec::<String>::new());
    let flush = |cur: &mut String, cur_w: &mut usize, active: &[String], out: &mut Vec<String>| {
        if !active.is_empty() {
            cur.push_str("\x1b[0m");
        }
        out.push(std::mem::take(cur));
        *cur = format!("{}{}", " ".repeat(hang), active.concat());
        *cur_w = hang;
    };
    for (esc, t) in tokens {
        if esc {
            if t == "\x1b[0m" || t == "\x1b[m" {
                active.clear();
            } else {
                active.push(t.clone());
            }
            cur.push_str(&t);
            continue;
        }
        let w = visible_width(&t);
        if t.starts_with(' ') {
            if cur_w > hang && cur_w + w <= width {
                cur.push_str(&t);
                cur_w += w;
            } else if cur_w > hang {
                flush(&mut cur, &mut cur_w, &active, &mut out);
            } else if out.is_empty() {
                cur.push_str(&t);
                cur_w += w;
            }
            continue;
        }
        if cur_w + w > width && cur_w > hang {
            flush(&mut cur, &mut cur_w, &active, &mut out);
        }
        // a word longer than a whole line breaks after a slash where it can, and is cut where it can't
        let pieces: Vec<&str> = if w + hang > width { t.split_inclusive('/').collect() } else { vec![t.as_str()] };
        for piece in pieces {
            let pw = visible_width(piece);
            if cur_w + pw > width && cur_w > hang && pw + hang <= width {
                flush(&mut cur, &mut cur_w, &active, &mut out);
            }
            for c in piece.chars() {
                let cw = visible_width(&c.to_string());
                if cur_w + cw > width {
                    flush(&mut cur, &mut cur_w, &active, &mut out);
                }
                cur.push(c);
                cur_w += cw;
            }
        }
    }
    if cur_w > hang || out.is_empty() {
        out.push(cur);
    }
    out
}

/// `lines` inside a rounded box of at most `width` columns, edges in `edge`, each line wrapped to fit.
fn boxed(edge: &str, lines: &[&str], width: usize) -> Vec<String> {
    let x = "\x1b[0m";
    let width = width.max(12);
    let inner = width - 4;
    let mut out = vec![format!("{edge}╭{}╮{x}", "─".repeat(width - 2))];
    for l in lines {
        for row in wrap_ansi(l, inner) {
            let pad = inner.saturating_sub(visible_width(&row));
            out.push(format!("{edge}│{x} {row}{x}{} {edge}│{x}", " ".repeat(pad)));
        }
    }
    out.push(format!("{edge}╰{}╯{x}", "─".repeat(width - 2)));
    out
}

/// A demo pane's screen in parts that stay whole: a box or a tool step is shown entire or not at all.
type Blocks = Vec<Vec<String>>;

/// `blocks` as one screen `cols` wide and at most `rows` tall: lines break at words, and when it's taller than the
/// tile the oldest blocks go (the way a conversation scrolls), never half of one.
fn stack(blocks: Blocks, cols: usize, rows: usize) -> String {
    let mut blocks: Vec<Vec<String>> = blocks.into_iter().map(|b| b.iter().flat_map(|l| wrap_ansi(l, cols)).collect()).collect();
    let height = |b: &[Vec<String>]| b.iter().map(Vec::len).sum::<usize>();
    while blocks.len() > 1 && height(&blocks) > rows {
        blocks.remove(0);
    }
    blocks.concat().join("\r\n")
}

/// A block of lines, the last of them followed by a blank one.
fn block(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).chain([String::new()]).collect()
}

/// The transcript for a demo pane `cols` wide: its lines break at words and its box is drawn to fit, so it reads
/// the same at any tile size.
pub fn transcript_for(h: Option<Harness>, cols: u16) -> String {
    let screen = transcript_fit(h, cols, u16::MAX);
    // Claude's ended on a fresh line, below its status
    if matches!(h, Some(Harness::Claude)) { screen + "\r\n" } else { screen }
}

/// [`transcript_for`] in a tile `rows` tall.
pub fn transcript_fit(h: Option<Harness>, cols: u16, rows: u16) -> String {
    let (cols, rows) = (cols as usize, rows as usize);
    let (o, d, g, r, b, x) = ("\x1b[38;2;217;119;87m", "\x1b[2m", "\x1b[32m", "\x1b[31m", "\x1b[1m", "\x1b[0m");
    let blocks: Blocks = match h {
        Some(Harness::Claude) => {
            let mut welcome = boxed(o, &[&format!("{o}✻{x} Welcome to {b}Claude Code{x}!"), &format!("  {d}cwd: ~/code/bro-cli-v2{x}")], cols.min(49));
            welcome.push(String::new());
            vec![
                welcome,
                block(&[&format!("{d}>{x} wire the launcher to the proxy routes")]),
                block(&[&format!("{o}●{x} I'll look at how launches register routes first.")]),
                block(&[&format!("{o}●{x} {b}Read{x}(crates/bro-tui/src/services/launch.rs)"), &format!("  {d}⎿  Read 148 lines{x}")]),
                block(&[&format!("{o}●{x} {b}Update{x}(crates/bro-tui/src/services/launch.rs)"), &format!("  {d}⎿  Updated with {g}12 additions{x}{d} and {r}3 removals{x}")]),
                vec![format!("{o}✻ Pondering…{x} {d}(38s · ↓ 2.1k tokens · esc to interrupt){x}")],
            ]
        }
        Some(Harness::Codex) => vec![
            block(&[&format!("{b}>_ OpenAI Codex{x} {d}(v0.60){x}"), &format!("{d}model: gpt-5.2-codex   directory: ~/code/bro-cli-v2{x}")]),
            block(&[&format!("{b}user{x}"), "add golden fixtures for responses streaming"]),
            block(&[&format!("{g}•{x} Explored"), &format!("  {d}└ Read fixtures/responses_stream.jsonl{x}")]),
            block(&[&format!("{g}•{x} Edited crates/bro-proxy/tests/golden.rs {g}(+86 -0){x}")]),
            block(&[&format!("{b}codex{x}"), "Added 6 fixtures covering text, tool calls and reasoning deltas.", "All 41 proxy tests pass."]),
            vec![format!("{d}› {x}")],
        ],
        Some(Harness::Pi) | Some(Harness::Omp) => vec![
            block(&[&format!("{b}pi{x} {d}· openrouter · kimi-k2{x}")]),
            block(&[&format!("{d}>{x} summarise the bridge protocol")]),
            block(&[&format!("The bridge speaks {b}/ws{x} with hello → snapshot → output events,"), "plus /api/sessions for create / kill / rename."]),
            vec![format!("{d}ready ·{x} ")],
        ],
        None => {
            // a tile too narrow for the whole path shows the folder alone
            let dir = if cols >= 44 { "~/code/bro-cli-v2" } else { "bro-cli-v2" };
            vec![vec![
                format!("{g}PS{x} {dir}> git status --short"),
                " M crates/bro-tui/src/app/mod.rs".to_string(),
                "?? crates/bro-tui/snapshots/".to_string(),
                format!("{g}PS{x} {dir}> "),
            ]]
        }
    };
    stack(blocks, cols, rows)
}

/// The screen of the demo session that waits on you (the fourth) for a pane `cols` wide: Claude asking to push,
/// the box at the bottom as Claude Code draws it, so focus mode has a real choice to show.
pub fn blocked_screen_for(cols: u16) -> String {
    blocked_screen_fit(cols, u16::MAX)
}

/// [`blocked_screen_for`] in a tile `rows` tall.
pub fn blocked_screen_fit(cols: u16, rows: u16) -> String {
    let (cols, rows) = (cols as usize, rows as usize);
    let (o, d, b, x) = ("\x1b[38;2;217;119;87m", "\x1b[2m", "\x1b[1m", "\x1b[0m");
    let ask = [
        "Bash command".to_string(),
        String::new(),
        "  git push -u origin fix/timer-clock".to_string(),
        "  Push the fix for CI".to_string(),
        String::new(),
        "Do you want to proceed?".to_string(),
        format!("{o}❯{x} 1. Yes"),
        "  2. Yes, and don't ask again for git push commands".to_string(),
        format!("  3. No, and tell Claude what to do differently {d}(esc){x}"),
    ];
    let ask: Vec<&str> = ask.iter().map(String::as_str).collect();
    // in a tile too short for the box, the choices stand on their own: a box cut by the tile would leave its edges
    let mut prompt_box = boxed("", &ask, cols.min(64));
    if prompt_box.len() > rows {
        prompt_box = ask.iter().flat_map(|l| wrap_ansi(l, cols)).collect();
    }
    let blocks: Blocks = vec![
        block(&[&format!("{d}>{x} fix the flaky workout timer test")]),
        block(&[&format!("{o}●{x} {b}Update{x}(src/features/timer/useTimer.ts)"), &format!("  {d}⎿  Updated with 9 additions and 4 removals{x}")]),
        block(&[&format!("{o}●{x} All 14 timer tests pass now, ten runs in a row. Pushing the branch so CI can confirm.")]),
        prompt_box,
    ];
    stack(blocks, cols, rows)
}

/// What a demo session draws in a tile `cols` by `rows`: its transcript, or the prompt it waits on. The ones
/// with nothing to type into hide the cursor, so none floats under the text.
pub fn scene(harness: Option<Harness>, blocked: bool, cols: u16, rows: u16) -> String {
    let screen = if blocked { blocked_screen_fit(cols, rows) } else { transcript_fit(harness, cols, rows) };
    if blocked || harness == Some(Harness::Claude) { format!("{screen}\x1b[?25l") } else { screen }
}

/// A screenshot for the demo's conversations, drawn here rather than read from anywhere: a test run with one red
/// row. Written once to the demo's own temp folder.
pub fn screenshot() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("bro-demo");
    let path = dir.join("timer-tests.png");
    if path.is_file() {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let (w, h) = (720u32, 440u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]| {
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    };
    fill(0, 0, w, h, [0x16, 0x18, 0x1d]);
    fill(0, 0, w, 40, [0x23, 0x26, 0x2e]);
    for (n, c) in [[0xff, 0x5f, 0x57], [0xfe, 0xbc, 0x2e], [0x28, 0xc8, 0x40]].into_iter().enumerate() {
        let x = 18 + n as u32 * 18;
        fill(x, 15, x + 10, 25, c);
    }
    fill(300, 16, 420, 24, [0x5a, 0x60, 0x6c]);
    for r in 0..9u32 {
        let y = 62 + r * 40;
        let failed = r == 5;
        if failed {
            fill(16, y - 4, w - 16, y + 28, [0x3b, 0x1d, 0x23]);
        }
        fill(32, y + 6, 46, y + 20, if failed { [0xf0, 0x5f, 0x5f] } else { [0x4c, 0xc3, 0x6a] });
        let len = 180 + (r * 53) % 240;
        fill(62, y + 9, 62 + len, y + 17, if failed { [0xe6, 0xc4, 0xc4] } else { [0x9a, 0xa1, 0xad] });
        fill(620, y + 9, 690, y + 17, [0x4a, 0x50, 0x5a]);
    }
    let file = std::fs::File::create(&path).ok()?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(&px).ok()?;
    Some(path)
}

/// What the demo's agent sessions said and did, as their transcripts would have it (focus mode shows it): `i`
/// is the session's place in [`sessions`].
pub fn conversation(i: usize) -> Option<String> {
    use serde_json::json;
    // times as of now: the working session's last step started a few seconds ago, the done one finished a while back
    let ago_ms: i64 = match i {
        0 => 160_000,
        1 => 31 * 60_000 + 96_000,
        _ => 85_000,
    };
    let base = now_ms() - ago_ms;
    let ts = move |s: u32| chrono::DateTime::from_timestamp_millis(base + s as i64 * 1000).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).unwrap_or_default();
    let asked = |model: &str, effort: &str| json!({"type":"api-request","timestamp":ts(0),"params":{"model":model,"thinking":{"type":"adaptive"},"output_config":{"effort":effort}}});
    let you = |s: u32, text: &str| json!({"type":"user","timestamp":ts(s),"sessionId":"0b6e3c1e-0000-4000-8000-00000000demo","message":{"role":"user","content":text}});
    let said = |s: u32, m: &str, text: &str| json!({"type":"assistant","timestamp":ts(s),"message":{"id":m,"content":[{"type":"text","text":text}]}});
    let call = |s: u32, m: &str, id: &str, name: &str, input: serde_json::Value| json!({"type":"assistant","timestamp":ts(s),"message":{"id":m,"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}});
    let ok = |s: u32, id: &str, result: serde_json::Value| json!({"type":"user","timestamp":ts(s),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"ok"}]},"toolUseResult":result});
    let patch = |add: usize, del: usize| json!({"structuredPatch":[{"lines": std::iter::repeat_n("+x", add).chain(std::iter::repeat_n("-x", del)).collect::<Vec<_>>()}]});
    let worked = |s: u32, ms: u64| json!({"type":"system","subtype":"turn_duration","durationMs":ms,"timestamp":ts(s)});
    let lines: Vec<serde_json::Value> = match i {
        // Claude, mid-turn on the launcher, after answering a question about reloads
        0 => vec![
            json!({"type":"user","timestamp":ts(0),"gitBranch":"main","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"}}),
            asked("claude-opus-5", "high"),
            you(0, "why does a codex session running on claude lose its route after a reload?"),
            said(4, "m0", "I'll trace what a reload does with the route it registered."),
            call(5, "m0", "t0", "Read", json!({"file_path":"crates/bro-tui/src/services/launch.rs"})),
            call(5, "m0", "t1", "Grep", json!({"pattern":"cleanup_route"})),
            ok(7, "t0", json!({})),
            ok(7, "t1", json!({})),
            call(9, "m1", "t2", "Read", json!({"file_path":"crates/bro-proxy/src/state.rs"})),
            ok(10, "t2", json!({})),
            said(40, "m2", "## What happens on a reload\n\nA reload starts a **new** process from the old launch spec, but the route belongs to the old one. When the old process exits, `cleanup_route` takes the route with it, and the new session is left pointing at nothing.\n\n- The new process starts before the old one is gone\n- Both of them share one route id\n- The cleanup at exit doesn't know a successor exists\n\nThe fix is to keep the route alive while a reload is in flight and hand it to the new pane. Want me to make that change?"),
            worked(41, 134_000),
            you(90, "yes, and wire the launcher to the proxy routes while you're there"),
            said(94, "m3", "I'll look at how launches register routes first."),
            call(95, "m3", "t3", "Read", json!({"file_path":"crates/bro-tui/src/launcher/mod.rs"})),
            call(95, "m3", "t4", "Read", json!({"file_path":"crates/bro-core/src/launch/route.rs"})),
            ok(97, "t3", json!({})),
            ok(97, "t4", json!({})),
            call(120, "m4", "t5", "Edit", json!({"file_path":"crates/bro-tui/src/services/launch.rs"})),
            ok(121, "t5", patch(12, 3)),
            call(130, "m5", "t6", "Edit", json!({"file_path":"crates/bro-core/src/launch/route.rs"})),
            ok(131, "t6", patch(5, 1)),
            said(150, "m6", "A reload now keeps its route until the new session takes it, and the launcher asks the proxy for one before it starts. Running the launcher tests next."),
            call(151, "m6", "t7", "Bash", json!({"command":"cargo test -p bro-tui launcher","description":"Run the launcher tests"})),
        ],
        // Codex, done: fixtures added, tests green
        1 => {
            let item = |s: u32, v: serde_json::Value| json!({"type":"event_msg","timestamp":ts(s),"payload":{"type":"item_completed","item":v}});
            let reply = |s: u32, text: &str| json!({"type":"response_item","timestamp":ts(s),"payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}});
            vec![
                json!({"type":"session_meta","timestamp":ts(0),"payload":{"cwd":root().join("bro-cli-v2"),"git":{"branch":"golden-fixtures"},"cli_version":"0.165.0"}}),
                json!({"type":"turn_context","timestamp":ts(0),"payload":{"model":"gpt-5.2-codex","effort":"high"}}),
                json!({"type":"event_msg","timestamp":ts(0),"payload":{"type":"task_started"}}),
                item(0, json!({"type":"UserMessage","id":"u1","content":[{"type":"text","text":"add golden fixtures for responses streaming"}]})),
                reply(3, "I'll read the existing stream fixtures, then add the cases the translator doesn't cover yet."),
                item(5, json!({"type":"CommandExecution","id":"c1","command":["pwsh","-Command","Get-Content fixtures/responses_stream.jsonl"],"parsed_cmd":[{"type":"read","cmd":"Get-Content fixtures/responses_stream.jsonl","name":"responses_stream.jsonl"}],"status":"completed","exit_code":0})),
                item(9, json!({"type":"CommandExecution","id":"c2","command":["pwsh","-Command","rg -n 'reasoning' crates/bro-proxy/src/translate"],"parsed_cmd":[{"type":"search","cmd":"rg -n reasoning crates/bro-proxy/src/translate","query":"reasoning"}],"status":"completed","exit_code":0})),
                item(60, json!({"type":"FileChange","id":"f1","changes":{"crates/bro-proxy/tests/golden.rs":{"type":"update","unified_diff":format!("@@ -1 +1,86 @@\n{}", "+x\n".repeat(86))}},"status":"completed"})),
                item(80, json!({"type":"CommandExecution","id":"c3","command":["pwsh","-Command","cargo test -p bro-proxy"],"parsed_cmd":[{"type":"unknown","cmd":"cargo test -p bro-proxy"}],"status":"completed","exit_code":0})),
                reply(95, "Added six fixtures covering text, tool calls and reasoning deltas. All 41 proxy tests pass.\n\n| Fixture | What it covers |\n|---|---|\n| `text_deltas` | plain text, split mid-word |\n| `tool_call` | one function call and its arguments |\n| `parallel_tools` | two calls in one turn |\n| `reasoning` | reasoning summary deltas |\n| `refusal` | a refusal part |\n| `usage` | token counts at the end |\n\nThe reasoning case caught a real bug: summary parts were joined without a break, so two thoughts ran together. That's fixed in `translate/responses.rs`."),
                json!({"type":"event_msg","timestamp":ts(96),"payload":{"type":"task_complete"}}),
            ]
        }
        // Claude, waiting on you to let it push
        3 => {
            // a screenshot you pasted, which it then looks at
            let shot = screenshot().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let prompt = if shot.is_empty() { "fix the flaky workout timer test".to_string() } else { format!("\"{shot}\" fix the flaky workout timer test, it failed again on CI") };
            // a sound it made you, which plays right there in focus mode
            let chime = chime().map(|p| p.to_string_lossy().into_owned());
            let mut lines = vec![
            json!({"type":"user","timestamp":ts(0),"gitBranch":"fix/timer-clock","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"}}),
            asked("claude-sonnet-5", "medium"),
            ];
            // a workout from the JustGains connector, which shows as its own card (an MCP app) once its page is read
            lines.extend(workout(&ts));
            if let Some(chime) = chime {
                lines.extend([
                    you(0, "make a short, bright finish chime for the workout timer"),
                    call(1, "c0", "tc", "Bash", json!({"command":"node justimagine.mjs run chime.json","description":"Make the finish chime"})),
                    ok(2, "tc", json!({"stdout":chime})),
                    said(2, "c1", &format!("Here's the finish chime: four notes rising (C, E, G, C), 2.6 seconds, ringing out at the end.

`{chime}`

Drop it in `assets/sounds/` and the timer plays it on the last rep.")),
                ]);
            }
            lines.extend([
            you(0, &prompt),
            call(3, "m0", "t9", "Read", json!({"file_path":shot})),
            ok(4, "t9", json!({})),
            said(5, "m0", &format!("Your screenshot shows the 6th case failing: the timer test reads the wall clock, so it fails whenever a second ticks over mid-test. I'll move it onto [the fake clock](https://vitest.dev/api/vi.html#vi-usefaketimers). The run you sent is `{shot}`.")),
            call(6, "m0", "t0", "Read", json!({"file_path":"src/features/timer/timer.test.ts"})),
            call(6, "m0", "t1", "Read", json!({"file_path":"src/features/timer/useTimer.ts"})),
            ok(8, "t0", json!({})),
            ok(8, "t1", json!({})),
            call(30, "m1", "t2", "Edit", json!({"file_path":"src/features/timer/useTimer.ts"})),
            ok(31, "t2", patch(9, 4)),
            call(32, "m2", "t3", "Bash", json!({"command":"npm test -- timer --runs 10","description":"Run the timer tests ten times"})),
            ok(70, "t3", json!({"stdout":"14 passed"})),
            said(72, "m3", "All 14 timer tests pass now, ten runs in a row. Pushing the branch so CI can confirm."),
            call(73, "m3", "t4", "Bash", json!({"command":"git push -u origin fix/timer-clock","description":"Push the fix for CI"})),
            ]);
            lines
        }
        _ => return None,
    };
    Some(lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n"))
}

/// The JustGains connector making a workout, as Claude Code records it: the call, its result (what JustGains
/// sent back: the workout, in pounds) with the page it shows in, and what Claude said of it. The card shows the
/// page when bro has read it before (`~/.bro/mcp-apps`); without it, the demo shows the step alone.
fn workout(ts: &dyn Fn(u32) -> String) -> Vec<serde_json::Value> {
    use serde_json::json;
    let result: serde_json::Value = serde_json::from_str(include_str!("demo_workout.json")).unwrap_or_default();
    let sc = result.get("structuredContent").cloned().unwrap_or_default();
    vec![
        json!({"type":"user","timestamp":ts(0),"message":{"role":"user","content":"make me a 30 minute upper body dumbbell workout for tonight"}}),
        json!({"type":"assistant","timestamp":ts(1),"message":{"id":"w0","content":[{"type":"tool_use","id":"tw","name":"mcp__claude_ai_JustGains__generate_workout","input":{"prompt":"30 minute upper body dumbbell workout"}}]}}),
        json!({"type":"user","timestamp":ts(2),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tw","content":sc.to_string()}]},"toolUseResult":sc.to_string(),"mcpMeta":result}),
        json!({"type":"assistant","timestamp":ts(3),"message":{"id":"w1","content":[{"type":"text","text":"Here's **Upper Body Dumbbell Finisher**: about 26 minutes, 15 sets across seven moves. It warms your shoulders up first, then pairs presses with rows so chest and back get equal work, and finishes on curls and overhead extensions. It's saved to your JustGains templates."}]}}),
        json!({"type":"system","subtype":"turn_duration","durationMs":9_000,"timestamp":ts(3)}),
    ]
}

/// A sound for the demo's conversations, made here rather than read from anywhere: a bright rising chime (C, E, G,
/// C), 16-bit mono WAV. Written once to the demo's own temp folder.
pub fn chime() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("bro-demo");
    let path = dir.join("finish-chime.wav");
    if path.is_file() {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let rate = 44_100u32;
    let n = (rate as f32 * 2.6) as usize;
    let notes = [1046.5f32, 1318.5, 1568.0, 2093.0];
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let v: f32 = notes
                .iter()
                .enumerate()
                .map(|(k, f)| {
                    let at = t - k as f32 * 0.18;
                    if at < 0.0 {
                        return 0.0;
                    }
                    // a quick attack, a long ring, a soft octave over it
                    let env = (at / 0.004).min(1.0) * (-at / 0.55).exp();
                    let w = std::f32::consts::TAU * f * at;
                    env * (w.sin() + 0.25 * (2.0 * w).sin())
                })
                .sum();
            (v * 0.22 * 32_767.0).clamp(-32_767.0, 32_767.0) as i16
        })
        .collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for v in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), rate.to_le_bytes().to_vec(), (rate * 2).to_le_bytes().to_vec(), 2u16.to_le_bytes().to_vec(), 16u16.to_le_bytes().to_vec()] {
        out.extend_from_slice(&v);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    std::fs::write(&path, out).ok()?;
    Some(path)
}

/// A harmless command for a demo session: print the transcript, then an interactive shell.
pub fn shell_command(h: Option<Harness>, label: &str, cwd: PathBuf) -> CommandSpec {
    shell_showing(h, label, cwd, &transcript(h), None)
}

/// [`shell_command`] printing `screen` instead; `quiet` names its file and leaves the shell's prompt off, so what
/// it printed is the bottom of the screen (a prompt the agent waits on).
pub fn shell_showing(h: Option<Harness>, label: &str, cwd: PathBuf, screen: &str, quiet: Option<&str>) -> CommandSpec {
    let dir = std::env::temp_dir().join("bro-demo");
    let _ = std::fs::create_dir_all(&dir);
    let name = quiet.map(str::to_string).unwrap_or_else(|| h.map(|h| h.label()).unwrap_or("shell").to_string());
    let file = dir.join(format!("{name}.ans"));
    let _ = std::fs::write(&file, screen);
    let f = file.to_string_lossy().to_string();
    let (program, args) = if cfg!(windows) {
        let (sh, _) = crate::util::default_shell(None);
        if sh.contains("cmd") {
            (sh, vec!["/k".into(), format!("type \"{f}\"")])
        } else {
            let prompt = if quiet.is_some() { "function global:prompt { ' ' }; " } else { "" };
            (sh, vec!["-NoLogo".into(), "-NoExit".into(), "-Command".into(), format!("{prompt}[Console]::OutputEncoding = [Text.Encoding]::UTF8; [Console]::Out.Write([IO.File]::ReadAllText('{}'))", f.replace('\'', "''"))])
        }
    } else if quiet.is_some() {
        ("sh".into(), vec!["-c".into(), format!("cat '{}'; PS1=' ' exec sh", f.replace('\'', "'\\''"))])
    } else {
        ("sh".into(), vec!["-c".into(), format!("cat '{}'; exec \"${{SHELL:-sh}}\"", f.replace('\'', "'\\''"))])
    };
    let cwd = if cwd.is_dir() { cwd } else { dirs::home_dir().unwrap_or_else(std::env::temp_dir) };
    CommandSpec { program, args, env: vec![], env_remove: vec![], cwd, label: label.into(), route: None, cleanup: vec![], caveman: None, store: None }
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

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn rows(screen: &str, cols: u16) -> Vec<String> {
        let mut p = vt100::Parser::new(60, cols, 0);
        p.process(screen.as_bytes());
        p.screen().rows(0, cols).map(|r| r.trim_end().to_string()).filter(|r| !r.is_empty()).collect()
    }

    /// The demo screens at every width a tile takes: each line fits whole, and a box is a whole box.
    #[test]
    fn screens_fit_every_width_with_whole_boxes() {
        let harnesses = [Some(Harness::Claude), Some(Harness::Codex), Some(Harness::Pi), None];
        for cols in [20u16, 24, 30, 40, 52, 66, 80, 140] {
            let mut screens: Vec<String> = harnesses.iter().map(|h| transcript_for(*h, cols)).collect();
            screens.push(blocked_screen_for(cols));
            for screen in screens {
                let r = rows(&screen, cols);
                // nothing is left for the terminal to wrap
                assert!(screen.split("\r\n").all(|l| visible_width(l) <= cols as usize), "{cols} cols: a line the terminal would wrap\n{}", r.join("\n"));
                let boxed: Vec<&String> = r.iter().filter(|l| l.starts_with(['╭', '│', '╰'])).collect();
                let tops = r.iter().filter(|l| l.starts_with('╭')).count();
                assert_eq!(tops, r.iter().filter(|l| l.starts_with('╰')).count(), "{cols} cols: half a box");
                if let Some(first) = boxed.first() {
                    assert!(boxed.iter().all(|l| l.width() == first.width()), "{cols} cols: ragged box\n{}", r.join("\n"));
                    assert!(boxed.iter().all(|l| l.ends_with(['╮', '│', '╯'])), "{cols} cols: box without its right edge");
                }
            }
        }
    }

    fn rows_of(screen: &str, cols: u16, rows: u16) -> Vec<String> {
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process(screen.as_bytes());
        p.screen().rows(0, cols).map(|r| r.trim_end().to_string()).filter(|r| !r.is_empty()).collect()
    }

    /// In a tile shorter than the screen the oldest parts go whole: never half a box, nothing scrolled off.
    #[test]
    fn short_tiles_drop_whole_blocks() {
        for cols in [24u16, 40, 80] {
            for rows in [18u16, 24, 30] {
                for (h, blocked) in [(Some(Harness::Claude), false), (Some(Harness::Codex), false), (Some(Harness::Pi), false), (Some(Harness::Claude), true)] {
                    let screen = scene(h, blocked, cols, rows);
                    let r = rows_of(&screen, cols, rows);
                    let tops = r.iter().filter(|l| l.starts_with('╭')).count();
                    assert_eq!(tops, r.iter().filter(|l| l.starts_with('╰')).count(), "{cols}x{rows}: half a box\n{}", r.join("\n"));
                    let lines = screen.split("\r\n").count();
                    assert!(lines <= rows as usize, "{cols}x{rows}: {lines} lines\n{}", r.join("\n"));
                }
            }
        }
    }

    #[test]
    fn wide_claude_screen_keeps_the_original_box() {
        let wide = transcript_for(Some(Harness::Claude), 120);
        assert!(wide.starts_with("\x1b[38;2;217;119;87m╭───────────────────────────────────────────────╮"));
        assert!(wide.contains("Welcome to \x1b[1mClaude Code\x1b[0m!"));
    }
}
