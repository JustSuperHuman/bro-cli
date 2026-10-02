//! launch::build across harness × route combinations (all inside a sandboxed home).
use super::*;
use crate::browser::BrowserMode;
use crate::util::test_env::{Sandbox, sandbox};
use serde_json::{Value, json};
use std::path::Path;

const PROXY: &str = "http://127.0.0.1:3458";

fn ctx() -> LaunchCtx {
    LaunchCtx { proxy_base: Some(PROXY.into()), proxy_token: Some("tok".into()) }
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
        json!({ "keys": { "zai": "z-key", "openai": "o-key" } }).to_string(),
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
    let c = build(&spec(Harness::Claude, None, Some("codex:smol"), Some("gpt-5.3-codex")), &LaunchCtx { proxy_base: Some(PROXY.into()), proxy_token: None }).unwrap();
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

#[test]
fn codex_profile_native_and_resume() {
    let sb = setup();
    let mut s = spec(Harness::Codex, Some("codex:smol"), None, Some("gpt-5.3-codex"));
    s.permission = Permission::Skip;
    let c = build(&s, &ctx()).unwrap();
    assert!(env(&c, "CODEX_HOME").unwrap().ends_with("smol"));
    assert_eq!(c.args, ["--yolo", "--model", "gpt-5.3-codex"]);
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
    assert_eq!(c.args, ["fork", id]);
    assert_eq!(c.cleanup.len(), 1);
    assert!(c.cleanup[0].starts_with(crate::paths::codex_profiles_dir().join("smol")));
    crate::sessions::cleanup_staged(&c.cleanup);

    s.profile_id = Some("codex:local".into());
    let c = build(&s, &ctx()).unwrap();
    assert_eq!(c.args, ["resume", id]);
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
