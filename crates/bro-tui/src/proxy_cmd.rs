//! `bro proxy <upstream> [--model M] [--small-model M] [--port N]` — run bro-proxy headless with one
//! route and print how to point each harness at it. Runs until Ctrl-C.

use bro_proxy::{ProxyConfig, ProxyHandle, Route};

/// Parsed `bro proxy` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyArgs {
    /// "pool", "claude:<profile>", "codex:<profile>" or a provider id
    pub upstream: String,
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub port: u16,
}

pub fn parse(args: &[String]) -> Result<ProxyArgs, String> {
    let mut upstream = None;
    let mut model = None;
    let mut small_model = None;
    let mut port = 0u16;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-m" | "--model" => model = Some(value("--model")?),
            "--small-model" => small_model = Some(value("--small-model")?),
            "--port" => port = value("--port")?.parse().map_err(|_| "--port must be a number".to_string())?,
            s if s.starts_with('-') => return Err(format!("unknown proxy option '{s}'")),
            s if upstream.is_none() => upstream = Some(s.to_string()),
            s => return Err(format!("unexpected argument '{s}'")),
        }
    }
    let upstream = upstream.ok_or("bro proxy needs an upstream: pool, claude:<profile>, codex:<profile> or a provider id")?;
    Ok(ProxyArgs { upstream, model, small_model, port })
}

pub fn run(args: ProxyArgs) -> anyhow::Result<()> {
    let cfg = bro_core::config::Config::load().unwrap_or_default();
    let target = bro_core::launch::resolve_upstream(&args.upstream, args.model.as_deref(), &cfg)?;
    let upstream = crate::services::launch::to_proxy(target);
    let handle = ProxyHandle::start(ProxyConfig { bind: "127.0.0.1".into(), port: args.port, token: None })?;
    let id = bro_core::launch::route_id(&args.upstream, args.model.as_deref());
    handle.upsert_route(Route {
        id: id.clone(),
        label: args.upstream.clone(),
        upstream,
        default_model: args.model.clone(),
        small_model: args.small_model.clone().or_else(|| args.model.clone()),
        model_map: Vec::new(),
    });
    handle.on_event(Box::new(|e| {
        let tokens = match (e.input_tokens, e.output_tokens) {
            (Some(i), Some(o)) => format!(" {i}→{o} tok"),
            _ => String::new(),
        };
        let err = e.error.map(|m| format!("  {m}")).unwrap_or_default();
        eprintln!("{} {} → {} {} {}ms{tokens}{err}", e.status, e.inbound, e.upstream, e.model, e.latency_ms);
    }));
    let base = handle.route_url(&id);
    println!("bro proxy → {} on {}", args.upstream, handle.base_url());
    println!();
    println!("  claude:  ANTHROPIC_BASE_URL={base} ANTHROPIC_AUTH_TOKEN=bro claude");
    println!("  openai:  OPENAI_BASE_URL={base}/v1 OPENAI_API_KEY=bro");
    println!();
    println!("ctrl+c to stop");
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    ctrlc_wait(tx);
    let _ = rx.recv();
    handle.shutdown();
    Ok(())
}

/// Block until Ctrl-C using a tiny tokio runtime (already a dependency).
fn ctrlc_wait(tx: std::sync::mpsc::Sender<()>) {
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            let _ = rt.block_on(tokio::signal::ctrl_c());
        }
        let _ = tx.send(());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<ProxyArgs, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_proxy_args() {
        let a = p(&["codex:local", "-m", "gpt-5.5", "--port", "3460"]).unwrap();
        assert_eq!(a.upstream, "codex:local");
        assert_eq!(a.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(a.port, 3460);
        assert!(p(&[]).is_err());
        assert!(p(&["pool", "--bogus"]).is_err());
    }
}
