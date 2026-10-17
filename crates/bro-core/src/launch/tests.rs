//! launch::build across harness × route combinations (all inside a sandboxed home).
use super::*;
use crate::browser::BrowserMode;
use crate::util::test_env::{Sandbox, sandbox};
use serde_json::{Value, json};
use std::path::Path;

const PROXY: &str = "http://127.0.0.1:3458";

/// Focus view and spinner tips at Claude's defaults, so `--settings` only shows up in the tests about it.
fn ctx() -> LaunchCtx {
    let mut claude_env = crate::claude_env::ClaudeEnvSettings::default();
    claude_env.set_choice(crate::claude_env::knob("focus").unwrap(), 1);
    claude_env.set_choice(crate::claude_env::knob("spinner_tips").unwrap(), 1);
    LaunchCtx { proxy_base: Some(PROXY.into()), proxy_token: Some("tok".into()), route_native: false, claude_env, ..Default::default() }
}

fn spec(harness: Harness, profile: Option<&str>, provider: Option<&str>, model: Option<&str>) -> LaunchSpec {
    LaunchSpec {
        harness,
        profile_id: profile.map(str::to_string),
        provider_id: provider.map(str::to_string),
        model: model.map(str::to_string),
        cwd: PathBuf::from("/work"),
        resume: None,
        permission: Permission::Default,
        browser: BrowserMode::Off,
        extra_args: vec![],
        orchestrator: false,
    }
}

fn env<'a>(c: &'a CommandSpec, k: &str) -> Option<&'a str> {
    c.env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

fn removes(c: &CommandSpec, k: &str) -> bool {
    c.env_remove.iter().any(|x| x == k)
}

fn has_seq(args: &[String], seq: &[&str]) -> bool {
    args.windows(seq.len()).any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
}

/// Accounts, a codex profile and provider keys.
fn setup() -> Sandbox {
    let sb = sandbox();
    let acct = crate::paths::claude_accounts_dir();
    for name in ["work", "other"] {
        std::fs::create_dir_all(acct.join(name)).unwrap();
        std::fs::write(acct.join(name).join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a"}}"#).unwrap();
    }
    std::fs::create_dir_all(crate::paths::codex_profiles_dir().join("smol")).unwrap();
    std::fs::create_dir_all(sb.bro()).unwrap();
    std::fs::write(
        crate::paths::config_path(),
        json!({ "keys": { "zai": "z-key", "openai": "o-key", "meta": "m-key" } }).to_string(),
    )
    .unwrap();
    sb
}

#[test]
fn claude_account_profile_native() {
    let _sb = setup();
    let mut s = spec(Harness::Claude, Some("claude:work"), None, Some("opus"));
    s.permission = Permission::Auto;
    s.extra_args = vec!["--verbose".into()];
    let c = build(&s, &ctx()).unwrap();
    assert!(env(&c, "CLAUDE_CONFIG_DIR").unwrap().ends_with("work"));
    for k in ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_DISABLE_1M_CONTEXT"] {
        assert!(removes(&c, k), "{k}");
    }
    assert_eq!(c.args, ["--permission-mode", "auto", "--model", "opus", "--verbose"]);
    assert_eq!(c.program, "claude");
    assert_eq!(c.label, "claude · work · opus");
    assert_eq!(c.cwd, PathBuf::from("/work"));
    assert!(c.route.is_none());
}

#[test]
fn claude_local_removes_config_dir_and_skip_permissions() {
    let _sb = setup();
    let mut s = spec(Harness::Claude, Some("claude:local"), None, None);
    s.permission = Permission::Skip;
    let c = build(&s, &ctx()).unwrap();
    assert!(removes(&c, "CLAUDE_CONFIG_DIR"));
    assert!(env(&c, "CLAUDE_CONFIG_DIR").is_none());
    assert_eq!(c.args, ["--dangerously-skip-permissions"]);
    assert_eq!(c.label, "claude · local");
}

#[test]
fn claude_anthropic_provider_direct() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, None, Some("zai"), Some("glm-5.3")), &ctx()).unwrap();
    assert_eq!(env(&c, "ANTHROPIC_BASE_URL"), Some("https://api.z.ai/api/anthropic"));
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("z-key"));
    assert_eq!(env(&c, "ANTHROPIC_API_KEY"), Some(""));
    assert_eq!(env(&c, "CLAUDE_CODE_DISABLE_1M_CONTEXT"), Some("1"));
    assert!(!removes(&c, "ANTHROPIC_BASE_URL"));
    assert!(c.route.is_none());
    assert_eq!(c.label, "claude · zai · glm-5.3");
    // every Claude Code model slot is pinned, so haiku/subagent calls stay on glm
    for k in claude::MODEL_VARS {
        assert_eq!(env(&c, k), Some("glm-5.3"), "{k}");
    }
}

#[test]
fn claude_native_login_leaves_model_slots_alone() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, Some("claude:work"), None, None), &ctx()).unwrap();
    for k in claude::MODEL_VARS {
        assert!(removes(&c, k) && env(&c, k).is_none(), "{k}");
    }
}

#[test]
fn claude_missing_key_is_an_error() {
    let _sb = setup();
    assert!(build(&spec(Harness::Claude, None, Some("groq"), None), &ctx()).is_err());
    assert!(build(&spec(Harness::Claude, None, Some("nope"), None), &ctx()).is_err());
    assert!(build(&spec(Harness::Claude, Some("claude:ghost"), None, None), &ctx()).is_err());
}

#[test]
fn claude_openai_provider_routes_through_proxy() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, None, Some("openai"), Some("gpt-4o")), &ctx()).unwrap();
    let route = c.route.clone().unwrap();
    assert_eq!(route.upstream, "openai");
    assert_eq!(route.model.as_deref(), Some("gpt-4o"));
    assert_eq!(route.route_id, route_id("openai", Some("gpt-4o")));
    assert_eq!(env(&c, "ANTHROPIC_BASE_URL").unwrap(), format!("{PROXY}/r/{}", route.route_id));
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("tok"));
    assert_eq!(env(&c, "CLAUDE_CODE_DISABLE_1M_CONTEXT"), Some("1"));
    assert!(removes(&c, "ANTHROPIC_API_KEY"));
    assert!(has_seq(&c.args, &["--model", "gpt-4o"]));
    // Without a proxy the launch is refused.
    assert!(build(&spec(Harness::Claude, None, Some("openai"), Some("gpt-4o")), &LaunchCtx::default()).is_err());
}

#[test]
fn claude_on_codex_login_and_pool() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, None, Some("codex:smol"), Some("gpt-5.3-codex")), &LaunchCtx { proxy_base: Some(PROXY.into()), proxy_token: None, route_native: false, ..Default::default() }).unwrap();
    assert_eq!(c.route.as_ref().unwrap().upstream, "codex:smol");
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("bro"));
    assert_eq!(env(&c, "CLAUDE_CODE_DISABLE_1M_CONTEXT"), Some("1"));
    // A codex profile picked as the login on the claude harness means the same thing.
    let c2 = build(&spec(Harness::Claude, Some("codex:smol"), None, Some("gpt-5.3-codex")), &ctx()).unwrap();
    assert_eq!(c2.route.unwrap().upstream, "codex:smol");

    let p = build(&spec(Harness::Claude, None, Some("pool"), None), &ctx()).unwrap();
    assert_eq!(p.route.as_ref().unwrap().upstream, "pool");
    assert!(env(&p, "CLAUDE_CODE_DISABLE_1M_CONTEXT").is_none());
    assert_eq!(p.label, "claude · pool");
}

fn write_session(dir: &Path, id: &str) {
    let f = dir.join("projects").join("F--work").join(format!("{id}.jsonl"));
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    let line = json!({"type": "user", "cwd": "F:\\work", "message": {"content": "hello"}});
    std::fs::write(&f, format!("{line}\n{}\n", json!({"pad": "x".repeat(600)}))).unwrap();
}

#[test]
fn claude_resume_same_and_cross_profile() {
    let _sb = setup();
    let id = "11111111-1111-4111-8111-111111111111";
    let work = crate::paths::claude_accounts_dir().join("work");
    write_session(&work, id);

    let mut s = spec(Harness::Claude, Some("claude:work"), None, None);
    s.resume = Some(Resume { session_id: id.into(), fork: false });
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(c.args, ["--resume", id]);
    assert!(c.cleanup.is_empty());

    s.profile_id = Some("claude:other".into());
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(c.args, ["--resume", id, "--fork-session"]);
    let staged = crate::paths::claude_accounts_dir().join("other").join("projects").join("F--work").join(format!("{id}.jsonl"));
    assert!(c.cleanup.contains(&staged));
    assert!(staged.exists());
    crate::sessions::cleanup_staged(&c.cleanup);
    assert!(!staged.exists());
    assert!(work.join("projects").join("F--work").join(format!("{id}.jsonl")).exists());
}

/// Codex args minus `--no-daemon`, which depends on whether the test runs elevated.
fn codex_args(c: &CommandSpec) -> Vec<&str> {
    c.args.iter().map(String::as_str).filter(|a| *a != "--no-daemon").collect()
}

#[test]
fn codex_profile_native_and_resume() {
    let sb = setup();
    let mut s = spec(Harness::Codex, Some("codex:smol"), None, Some("gpt-5.3-codex"));
    s.permission = Permission::Skip;
    let c = build(&s, &ctx()).unwrap();
    assert!(env(&c, "CODEX_HOME").unwrap().ends_with("smol"));
    assert_eq!(c.args.iter().any(|a| a == "--no-daemon"), codex::elevated(), "--no-daemon exactly when elevated");
    assert_eq!(codex_args(&c), ["--yolo", "--model", "gpt-5.3-codex"]);
    assert_eq!(c.label, "codex · smol · gpt-5.3-codex");

    // A rollout owned by the local login, resumed as smol → staged + fork.
    let id = "01a0ed53-d984-7ad0-badc-5b7024210c1a";
    let dir = sb.home().join(".codex").join("sessions").join("2026").join("09").join("29");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("rollout-x-{id}.jsonl")), json!({"type": "session_meta", "payload": {"id": id}}).to_string()).unwrap();
    s.permission = Permission::Default;
    s.model = None;
    s.resume = Some(Resume { session_id: id.into(), fork: false });
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(codex_args(&c), ["fork", id]);
    assert_eq!(c.cleanup.len(), 1);
    assert!(c.cleanup[0].starts_with(crate::paths::codex_profiles_dir().join("smol")));
    crate::sessions::cleanup_staged(&c.cleanup);

    s.profile_id = Some("codex:local".into());
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(codex_args(&c), ["resume", id]);
    assert!(c.cleanup.is_empty());
}

#[test]
fn codex_openai_provider_direct() {
    let _sb = setup();
    let c = build(&spec(Harness::Codex, None, Some("openai"), Some("gpt-4o")), &ctx()).unwrap();
    assert!(has_seq(&c.args, &["-c", "model_provider=\"bro\""]));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.base_url=\"https://api.openai.com/v1\""]));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.wire_api=\"responses\""]));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.env_key=\"BRO_PROVIDER_API_KEY\""]));
    assert_eq!(env(&c, "BRO_PROVIDER_API_KEY"), Some("o-key"));
    assert!(c.route.is_none());
}

#[test]
fn meta_muse_runs_claude_direct_and_codex_on_responses_with_its_settings() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, None, Some("meta"), Some("muse-wonder-1.0")), &ctx()).unwrap();
    assert_eq!(env(&c, "ANTHROPIC_BASE_URL"), Some("https://api.meta.ai"));
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("m-key"));
    assert_eq!(env(&c, "ENABLE_TOOL_SEARCH"), Some("true"));
    assert_eq!(env(&c, "CLAUDE_CODE_DISABLE_1M_CONTEXT"), None);
    for k in claude::MODEL_VARS {
        assert_eq!(env(&c, k), Some("muse-wonder-1.0"), "{k}");
    }
    assert!(c.route.is_none());

    let mut s = spec(Harness::Codex, None, Some("meta"), Some("muse-wonder-1.0"));
    s.extra_args = vec!["-c".into(), "model_reasoning_effort=\"low\"".into()];
    let c = build(&s, &ctx()).unwrap();
    assert!(c.route.is_none());
    assert_eq!(env(&c, "BRO_PROVIDER_API_KEY"), Some("m-key"));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.base_url=\"https://api.meta.ai/v1\""]));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.wire_api=\"responses\""]));
    for setting in [
        "model_reasoning_effort=\"high\"",
        "model_reasoning_summary=\"auto\"",
        "model_context_window=1048576",
        "model_supports_reasoning_summaries=true",
        "model_auto_compact_token_limit=900000",
    ] {
        assert!(has_seq(&c.args, &["-c", setting]), "{setting}");
    }
    let at = |want: &str| c.args.iter().position(|a| a == want).unwrap();
    assert!(at("model_reasoning_effort=\"high\"") < at("model_reasoning_effort=\"low\""), "your own -c wins");
}

#[test]
fn codex_on_claude_login_and_anthropic_provider_via_proxy() {
    let _sb = setup();
    let c = build(&spec(Harness::Codex, Some("claude:work"), None, Some("claude-opus-5")), &ctx()).unwrap();
    let route = c.route.clone().unwrap();
    assert_eq!(route.upstream, "claude:work");
    let base = format!("model_providers.bro.base_url=\"{PROXY}/r/{}/v1\"", route.route_id);
    assert!(has_seq(&c.args, &["-c", &base]));
    assert!(has_seq(&c.args, &["-c", "model_providers.bro.wire_api=\"responses\""]));
    assert_eq!(env(&c, "BRO_PROVIDER_API_KEY"), Some("tok"));

    let z = build(&spec(Harness::Codex, None, Some("zai"), Some("glm-5.3")), &ctx()).unwrap();
    assert_eq!(z.route.unwrap().upstream, "zai");
}

#[test]
fn pi_openai_provider_writes_models_json() {
    let sb = setup();
    let c = build(&spec(Harness::Pi, None, Some("openai"), None), &ctx()).unwrap();
    assert_eq!(c.args, ["--provider", "bro-openai", "--model", "gpt-4o"]);
    assert_eq!(env(&c, pi::PI_KEY_ENV), Some("o-key"));
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(sb.home().join(".pi").join("agent").join("models.json")).unwrap()).unwrap();
    let entry = &cfg["providers"]["bro-openai"];
    assert_eq!(entry["baseUrl"], "https://api.openai.com/v1");
    assert_eq!(entry["api"], "openai-completions");
    assert_eq!(entry["apiKey"], pi::PI_KEY_ENV);
    assert_eq!(entry["models"][0]["id"], "gpt-4o");
    assert_eq!(c.label, "pi · openai");
}

#[test]
fn pi_on_claude_login_via_proxy_preserves_other_providers() {
    let sb = setup();
    let path = sb.home().join(".pi").join("agent").join("models.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"providers":{"mine":{"api":"x"}},"other":1}"#).unwrap();
    let c = build(&spec(Harness::Pi, Some("claude:work"), None, Some("claude-opus-5")), &ctx()).unwrap();
    let route = c.route.clone().unwrap();
    assert_eq!(route.upstream, "claude:work");
    assert_eq!(c.args, ["--provider", "bro-proxy-claude-work", "--model", "claude-opus-5"]);
    assert_eq!(env(&c, pi::PI_KEY_ENV), Some("tok"));
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(cfg["other"], 1);
    assert_eq!(cfg["providers"]["mine"]["api"], "x");
    let entry = &cfg["providers"]["bro-proxy-claude-work"];
    assert_eq!(entry["api"], "anthropic-messages");
    assert_eq!(entry["baseUrl"], format!("{PROXY}/r/{}", route.route_id));
    // Invalid JSON is reported, not overwritten.
    std::fs::write(&path, "{ nope").unwrap();
    assert!(build(&spec(Harness::Pi, None, Some("openai"), None), &ctx()).is_err());
}

#[test]
fn omp_anthropic_provider_and_codex_login() {
    let sb = setup();
    let mut s = spec(Harness::Omp, None, Some("zai"), Some("glm-5.3"));
    s.permission = Permission::Skip;
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(c.args, ["--yolo", "--model", "zai/glm-5.3"]);
    let yml = std::fs::read_to_string(sb.home().join(".omp").join("agent").join("models.yml")).unwrap();
    assert!(yml.starts_with("providers:\n  zai:\n"));
    assert!(yml.contains("    api: anthropic-messages\n"));
    assert!(yml.contains("    apiKey: \"z-key\"\n"));
    assert!(yml.contains("    disableStrictTools: true\n"));

    let c = build(&spec(Harness::Omp, Some("codex:smol"), None, Some("gpt-5.3-codex")), &ctx()).unwrap();
    assert_eq!(c.route.as_ref().unwrap().upstream, "codex:smol");
    assert_eq!(c.args, ["--model", "proxy-codex-smol/gpt-5.3-codex"]);
    let yml = std::fs::read_to_string(sb.home().join(".omp").join("agent").join("models.yml")).unwrap();
    assert!(yml.contains("  zai:\n") && yml.contains("  proxy-codex-smol:\n"));
}

#[test]
fn pi_native_uses_pis_own_login() {
    let _sb = setup();
    let c = build(&spec(Harness::Pi, None, None, Some("some-model")), &ctx()).unwrap();
    assert_eq!(c.args, ["--model", "some-model"]);
    assert!(c.route.is_none());
    let a = build(&spec(Harness::Pi, None, Some("anthropic"), Some("claude-opus-5")), &ctx()).unwrap();
    assert_eq!(a.args, ["--provider", "anthropic", "--model", "claude-opus-5"]);
}

#[test]
fn trim_routes_native_logins_through_the_proxy_as_themselves() {
    let _sb = setup();
    let trim = LaunchCtx { route_native: true, ..ctx() };

    // Claude account: same CLAUDE_CONFIG_DIR (transcripts stay put), backend = proxy route
    // whose upstream is the same login, no model pinning (first-party models).
    let c = build(&spec(Harness::Claude, Some("claude:work"), None, Some("opus")), &trim).unwrap();
    assert!(env(&c, "CLAUDE_CONFIG_DIR").unwrap().ends_with("work"));
    let route = c.route.clone().expect("routed through the proxy");
    assert_eq!(route.upstream, "claude:work");
    assert_eq!(env(&c, "ANTHROPIC_BASE_URL"), Some(format!("{PROXY}/r/{}", route.route_id).as_str()));
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("tok"));
    assert!(removes(&c, "ANTHROPIC_API_KEY"));
    assert!(env(&c, "CLAUDE_CODE_DISABLE_1M_CONTEXT").is_none(), "Anthropic upstream keeps 1M context");
    assert!(env(&c, "ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none(), "background model slots stay Claude's own");
    assert_eq!(c.label, "claude · work · opus");
    assert!(matches!(resolve_upstream(&route.upstream, None, &crate::config::Config::default()).unwrap(),
        UpstreamTarget::ClaudeOAuth { config_dir } if config_dir.ends_with("work")));

    // The local login routes as claude:local.
    let c = build(&spec(Harness::Claude, Some("claude:local"), None, None), &trim).unwrap();
    assert_eq!(c.route.unwrap().upstream, "claude:local");

    // Codex login → ChatGPT backend through the proxy, same CODEX_HOME.
    let c = build(&spec(Harness::Codex, Some("codex:smol"), None, None), &trim).unwrap();
    assert!(env(&c, "CODEX_HOME").unwrap().ends_with("smol"));
    let route = c.route.clone().unwrap();
    assert_eq!(route.upstream, "codex:smol");
    assert!(has_seq(&c.args, &["-c", &format!("model_providers.bro.base_url=\"{PROXY}/r/{}/v1\"", route.route_id)]));
    assert_eq!(env(&c, "BRO_PROVIDER_API_KEY"), Some("tok"));

    // Provider launches and non-trim launches are unchanged; without a running proxy the
    // native route quietly stays direct.
    let z = build(&spec(Harness::Claude, None, Some("zai"), Some("glm-5.3")), &trim).unwrap();
    assert!(z.route.is_none());
    assert!(build(&spec(Harness::Claude, Some("claude:work"), None, None), &ctx()).unwrap().route.is_none());
    let no_proxy = LaunchCtx { route_native: true, ..LaunchCtx::default() };
    assert!(build(&spec(Harness::Claude, Some("claude:work"), None, None), &no_proxy).unwrap().route.is_none());
    assert!(build(&spec(Harness::Codex, Some("codex:smol"), None, None), &no_proxy).unwrap().route.is_none());
}

#[test]
fn browser_additions_follow_the_route() {
    let _sb = setup();
    let root = crate::browser::browser_root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("state.json"), r#"{"enabled":true,"backend":"claude","owner":"local"}"#).unwrap();
    let mut s = spec(Harness::Claude, Some("claude:local"), None, None);
    s.browser = BrowserMode::Auto;
    assert_eq!(build(&s, &ctx()).unwrap().args, ["--chrome"]);
    s.provider_id = Some("zai".into());
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(c.args[0], "--mcp-config");
    // An explicit --no-chrome wins.
    s.extra_args = vec!["--no-chrome".into()];
    assert_eq!(build(&s, &ctx()).unwrap().args, ["--no-chrome"]);
}

#[test]
fn account_profiles_share_the_root_skills_and_mcp_servers() {
    let sb = setup();
    let root_skill = crate::paths::claude_local_dir().join("skills").join("mine");
    std::fs::create_dir_all(&root_skill).unwrap();
    std::fs::write(root_skill.join("SKILL.md"), "x").unwrap();
    std::fs::write(sb.home().join(".claude.json"), json!({ "mcpServers": { "pw": { "type": "stdio", "command": "pw" } } }).to_string()).unwrap();

    let mut s = spec(Harness::Claude, Some("claude:work"), None, None);
    s.extra_args = vec!["hello".into()];
    let c = build(&s, &ctx()).unwrap();
    let account = Path::new(env(&c, "CLAUDE_CONFIG_DIR").unwrap());
    assert!(account.join("skills").join("mine").join("SKILL.md").is_file());
    let cfg = crate::claude_shared::shared_mcp_path(account);
    assert_eq!(c.args, ["hello", "--mcp-config", &cfg.to_string_lossy()]);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["pw"]["command"], "pw");

    s.extra_args = vec!["--".into(), "hello".into()];
    assert_eq!(build(&s, &ctx()).unwrap().args[0], "--mcp-config");
    s.extra_args = vec!["--strict-mcp-config".into()];
    assert_eq!(build(&s, &ctx()).unwrap().args, ["--strict-mcp-config"]);
    // The machine's own login already has them.
    assert!(build(&spec(Harness::Claude, Some("claude:local"), None, None), &ctx()).unwrap().args.is_empty());
}

#[cfg(windows)]
#[test]
fn cmd_shims_run_through_cmd_exe() {
    let sb = setup();
    let bin = sb.home().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("claude.cmd"), "@echo off").unwrap();
    let c = build(&spec(Harness::Claude, None, None, Some("opus")), &ctx()).unwrap();
    assert_eq!(c.program, "cmd.exe");
    assert_eq!(c.args[0], "/c");
    assert!(c.args[1].to_lowercase().ends_with("claude.cmd"));
    assert_eq!(&c.args[2..], ["--model", "opus"]);
    assert!(crate::util::same_path(&which("claude").unwrap(), &bin.join("claude.cmd")));
}

#[test]
fn upstream_resolution() {
    let _sb = setup();
    let cfg = crate::config::Config::load().unwrap();
    match resolve_upstream("openai", None, &cfg).unwrap() {
        UpstreamTarget::OpenAiChat { base_url, api_key } => {
            assert_eq!(base_url, "https://api.openai.com/v1");
            assert_eq!(api_key.as_deref(), Some("o-key"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(resolve_upstream("zai", None, &cfg).unwrap(), UpstreamTarget::Anthropic { bearer: true, .. }));
    assert!(matches!(resolve_upstream("claude:work", None, &cfg).unwrap(), UpstreamTarget::ClaudeOAuth { .. }));
    assert!(matches!(resolve_upstream("codex:smol", None, &cfg).unwrap(), UpstreamTarget::ChatGptCodex { .. }));
    match resolve_upstream("pool", None, &cfg).unwrap() {
        UpstreamTarget::ClaudePool { config_dirs } => assert_eq!(config_dirs.len(), 2),
        other => panic!("{other:?}"),
    }
    assert_ne!(route_id("a", Some("m")), route_id("a", None));
    assert_eq!(route_id("a", Some("m")), route_id("a", Some("m")));
}

#[test]
fn launches_install_the_bro_open_skill_in_the_harness_home() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, Some("claude:work"), None, None), &ctx()).unwrap();
    let claude_home = Path::new(env(&c, "CLAUDE_CONFIG_DIR").unwrap());
    assert!(crate::skills::bro_open_skill_path(claude_home).is_file());
    build(&spec(Harness::Claude, Some("claude:local"), None, None), &ctx()).unwrap();
    assert!(crate::skills::bro_open_skill_path(&crate::paths::claude_local_dir()).is_file());
    let c = build(&spec(Harness::Codex, Some("codex:smol"), None, None), &ctx()).unwrap();
    let codex_home = Path::new(env(&c, "CODEX_HOME").unwrap());
    assert!(crate::skills::bro_open_skill_path(codex_home).is_file());
}

#[test]
fn claude_env_preset_applies_and_provider_env_wins() {
    let _sb = setup();
    let mut ctx = ctx();
    ctx.claude_env.use_preset("private");
    ctx.claude_env.extra.insert("CLAUDE_CODE_DISABLE_1M_CONTEXT".into(), "0".into());
    let c = build(&spec(Harness::Claude, Some("claude:work"), None, None), &ctx).unwrap();
    assert_eq!(env(&c, "DISABLE_TELEMETRY"), Some("1"));
    assert!(env(&c, "ENABLE_TOOL_SEARCH").is_none(), "direct login: Claude Code keeps tool search itself");
    // Backend vars the launch already decided aren't overridden by `extra`.
    let z = build(&spec(Harness::Claude, None, Some("zai"), Some("glm-5.3")), &ctx).unwrap();
    assert_eq!(env(&z, "CLAUDE_CODE_DISABLE_1M_CONTEXT"), Some("1"));
    assert!(env(&z, "ENABLE_TOOL_SEARCH").is_none());
    // Through the proxy to a Claude upstream, tool search is turned back on; not for OpenAI upstreams.
    let p = build(&spec(Harness::Claude, None, Some("pool"), None), &ctx).unwrap();
    assert_eq!(env(&p, "ENABLE_TOOL_SEARCH"), Some("true"));
    let o = build(&spec(Harness::Claude, None, Some("openai"), Some("gpt-4o")), &ctx).unwrap();
    assert!(env(&o, "ENABLE_TOOL_SEARCH").is_none());
    assert_eq!(env(&o, "DISABLE_TELEMETRY"), Some("1"));
}

fn caveman_ctx(level: crate::caveman::Level) -> LaunchCtx {
    let mut c = ctx();
    c.caveman.level = level;
    c.caveman.set_on(true);
    c
}

/// The value after `flag`.
fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

#[test]
fn caveman_off_changes_nothing() {
    let _sb = setup();
    let c = build(&spec(Harness::Claude, None, None, None), &ctx()).unwrap();
    assert!(c.caveman.is_none());
    assert!(!c.args.iter().any(|a| a == "--append-system-prompt-file" || a == "--settings"), "{:?}", c.args);
    assert!(env(&c, "CAVEMAN_DEFAULT_MODE").is_none());
}

#[test]
fn claude_runs_fullscreen_in_focus_view_without_tips_unless_turned_off() {
    let _sb = setup();
    let mut cx = ctx();
    cx.claude_env = Default::default();
    let c = build(&spec(Harness::Claude, None, None, None), &cx).unwrap();
    assert_eq!(env(&c, "CLAUDE_CODE_NO_FLICKER"), Some("1"));
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(arg_after(&c.args, "--settings").unwrap()).unwrap()).unwrap();
    assert_eq!(settings, serde_json::json!({ "viewMode": "focus", "spinnerTipsEnabled": false }), "caveman off: no hooks");
    let c = build(&spec(Harness::Claude, None, None, None), &ctx()).unwrap();
    assert!(c.args.iter().all(|a| a != "--settings"), "{:?}", c.args);
    assert_eq!(env(&c, "CLAUDE_CODE_NO_FLICKER"), Some("1"));
}

#[test]
fn caveman_claude_gets_rules_hooks_and_quiets_the_plugin() {
    let _sb = setup();
    let mut cx = caveman_ctx(crate::caveman::Level::Ultracave);
    cx.claude_env = Default::default();
    let c = build(&spec(Harness::Claude, None, None, None), &cx).unwrap();
    assert_eq!(c.caveman, Some(crate::caveman::Level::Ultracave));
    let prompt = arg_after(&c.args, "--append-system-prompt-file").unwrap();
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), crate::caveman::Level::Ultracave.rules());
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(arg_after(&c.args, "--settings").unwrap()).unwrap()).unwrap();
    assert!(settings["hooks"]["SubagentStart"].is_array());
    assert_eq!(settings["viewMode"], "focus", "one --settings file carries both");
    assert_eq!(env(&c, "CAVEMAN_DEFAULT_MODE"), Some("off"));
}

#[test]
fn caveman_merges_with_other_prompt_files() {
    let sb = setup();
    let mine = sb.home().join("mine.md");
    std::fs::write(&mine, "MY RULES").unwrap();
    let mut s = spec(Harness::Claude, None, None, None);
    s.extra_args = vec!["--append-system-prompt-file".into(), mine.to_string_lossy().into_owned(), "--".into(), "--append-system-prompt-file".into()];
    let c = build(&s, &caveman_ctx(crate::caveman::Level::Caveman)).unwrap();
    let dashdash = c.args.iter().position(|a| a == "--").unwrap();
    let flags: Vec<usize> = c.args.iter().enumerate().filter(|(_, a)| *a == "--append-system-prompt-file").map(|(i, _)| i).collect();
    assert_eq!(flags.len(), 2, "one merged flag, plus the prompt text after -- left alone: {:?}", c.args);
    assert!(flags[0] < dashdash && flags[1] > dashdash);
    let merged = std::fs::read_to_string(&c.args[flags[0] + 1]).unwrap();
    assert!(merged.starts_with("MY RULES\n\n# Caveman mode"), "{merged}");

    // Without caveman, a single file of yours passes through untouched.
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(arg_after(&c.args, "--append-system-prompt-file"), Some(mine.to_string_lossy().as_ref()));
}

#[test]
fn caveman_respects_your_own_settings_flag() {
    let _sb = setup();
    let mut s = spec(Harness::Claude, None, None, None);
    s.extra_args = vec!["--settings".into(), "mine.json".into()];
    let c = build(&s, &caveman_ctx(crate::caveman::Level::Caveman)).unwrap();
    assert_eq!(c.args.iter().filter(|a| *a == "--settings").count(), 1);
    assert!(c.caveman.is_some(), "the system prompt still carries it");
}

#[test]
fn caveman_resume_with_it_off_says_so() {
    let _sb = setup();
    let mut c = ctx();
    c.caveman.used = true;
    let mut s = spec(Harness::Claude, None, None, None);
    assert!(build(&s, &c).unwrap().args.iter().all(|a| a != "--settings"), "fresh session: nothing to undo");
    s.resume = Some(Resume { session_id: "abc".into(), fork: false });
    let cmd = build(&s, &c).unwrap();
    let settings = std::fs::read_to_string(arg_after(&cmd.args, "--settings").unwrap()).unwrap();
    assert!(settings.contains("resume-off.json"), "{settings}");
    assert!(cmd.caveman.is_none());
}

#[test]
fn caveman_codex_keeps_your_developer_instructions() {
    let _sb = setup();
    let home = crate::paths::codex_local_dir();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "developer_instructions = \"Use tabs.\"\n").unwrap();
    let c = build(&spec(Harness::Codex, None, None, None), &caveman_ctx(crate::caveman::Level::Caveman)).unwrap();
    let v = c.args.iter().find_map(|a| a.strip_prefix("developer_instructions=")).unwrap();
    let parsed: toml::Table = format!("v = {v}").parse().unwrap();
    let text = parsed["v"].as_str().unwrap();
    assert!(text.starts_with("Use tabs.\n\n# Caveman mode"), "{text}");
    assert_eq!(c.caveman, Some(crate::caveman::Level::Caveman));
}

#[test]
fn caveman_pi_and_omp_append_the_rules_file() {
    let _sb = setup();
    let c = build(&spec(Harness::Pi, None, None, None), &caveman_ctx(crate::caveman::Level::Megacave)).unwrap();
    let f = arg_after(&c.args, "--append-system-prompt").unwrap();
    assert!(f.ends_with("megacave.md"), "{f}");
    let c = build(&spec(Harness::Omp, None, None, None), &caveman_ctx(crate::caveman::Level::Caveman)).unwrap();
    assert!(c.args.iter().any(|a| a.starts_with("--append-system-prompt=") && a.ends_with("caveman.md")), "{:?}", c.args);
}

fn orchestrator_ctx() -> LaunchCtx {
    LaunchCtx { bridge_url: Some("http://127.0.0.1:7357".into()), ..ctx() }
}

fn orchestrator(harness: Harness) -> LaunchSpec {
    LaunchSpec { orchestrator: true, ..spec(harness, None, None, None) }
}

#[test]
fn orchestrator_needs_the_bridge() {
    let _sb = setup();
    let e = build(&orchestrator(Harness::Claude), &ctx()).unwrap_err();
    assert!(format!("{e:#}").contains("bridge"), "{e:#}");
}

#[test]
fn orchestrator_claude_gets_prompt_skill_and_mcp() {
    let _sb = setup();
    let c = build(&orchestrator(Harness::Claude), &orchestrator_ctx()).unwrap();
    assert_eq!(env(&c, "BRO_ORCHESTRATOR"), Some("1"));
    assert_eq!(c.cwd, crate::orchestrator::workspace(), "runs in ~/orchestrator, not the project");
    assert!(c.cwd.is_dir() && c.cwd.ends_with("orchestrator"));
    assert!(c.label.starts_with("orchestrator · claude"), "{}", c.label);
    let prompt = std::fs::read_to_string(arg_after(&c.args, "--append-system-prompt-file").unwrap()).unwrap();
    assert!(prompt.contains("You are the bro orchestrator") && prompt.contains("mcp__bro__"), "{prompt}");
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(arg_after(&c.args, "--mcp-config").unwrap()).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["bro"]["url"], "http://127.0.0.1:7357/mcp");
    assert_eq!(arg_after(&c.args, "--allowedTools"), Some("mcp__bro"));
    assert!(crate::orchestrator::skill_path(&crate::paths::claude_local_dir()).is_file());
    // the variadic options come last, after every other option
    let mcp_at = c.args.iter().position(|a| a == "--mcp-config").unwrap();
    assert!(c.args[..mcp_at].iter().any(|a| a == "--append-system-prompt-file"));

    // A plain launch gets none of it.
    let c = build(&spec(Harness::Claude, None, None, None), &orchestrator_ctx()).unwrap();
    assert!(env(&c, "BRO_ORCHESTRATOR").is_none() && !c.args.iter().any(|a| a == "--mcp-config"), "{:?}", c.args);
}

#[test]
fn orchestrator_claude_merges_with_caveman() {
    let _sb = setup();
    let mut cx = caveman_ctx(crate::caveman::Level::Caveman);
    cx.bridge_url = Some("http://127.0.0.1:7357".into());
    let c = build(&orchestrator(Harness::Claude), &cx).unwrap();
    assert_eq!(c.args.iter().filter(|a| *a == "--append-system-prompt-file").count(), 1, "{:?}", c.args);
    let merged = std::fs::read_to_string(arg_after(&c.args, "--append-system-prompt-file").unwrap()).unwrap();
    assert!(merged.contains("You are the bro orchestrator") && merged.contains("# Caveman mode"), "{merged}");
}

#[test]
fn orchestrator_codex_gets_mcp_server_and_instructions() {
    let _sb = setup();
    let home = crate::paths::codex_local_dir();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "developer_instructions = \"Use tabs.\"\n").unwrap();
    let mut cx = caveman_ctx(crate::caveman::Level::Caveman);
    cx.bridge_url = Some("http://127.0.0.1:7357/".into());
    let c = build(&orchestrator(Harness::Codex), &cx).unwrap();
    assert!(has_seq(&c.args, &["-c", "mcp_servers.bro.url=\"http://127.0.0.1:7357/mcp\""]), "{:?}", c.args);
    let all: Vec<&String> = c.args.iter().filter(|a| a.starts_with("developer_instructions=")).collect();
    assert_eq!(all.len(), 1, "one -c developer_instructions carries everything");
    let parsed: toml::Table = format!("v = {}", all[0].strip_prefix("developer_instructions=").unwrap()).parse().unwrap();
    let text = parsed["v"].as_str().unwrap();
    assert!(text.starts_with("Use tabs.\n\n# You are the bro orchestrator"), "{text}");
    assert!(text.contains("# Caveman mode"));
    assert!(crate::orchestrator::skill_path(&home).is_file());
}

#[test]
fn orchestrator_pi_and_omp_get_the_ctl_brief() {
    let _sb = setup();
    let c = build(&orchestrator(Harness::Pi), &orchestrator_ctx()).unwrap();
    let f = arg_after(&c.args, "--append-system-prompt").unwrap();
    assert!(std::fs::read_to_string(f).unwrap().contains("bro ctl list_sessions"));
    let mut cx = caveman_ctx(crate::caveman::Level::Caveman);
    cx.bridge_url = Some("http://127.0.0.1:7357".into());
    let c = build(&orchestrator(Harness::Omp), &cx).unwrap();
    let flags: Vec<&String> = c.args.iter().filter(|a| a.starts_with("--append-system-prompt=")).collect();
    assert_eq!(flags.len(), 1, "{:?}", c.args);
    let body = std::fs::read_to_string(flags[0].strip_prefix("--append-system-prompt=").unwrap()).unwrap();
    assert!(body.contains("You are the bro orchestrator") && body.contains("# Caveman mode"));
}
