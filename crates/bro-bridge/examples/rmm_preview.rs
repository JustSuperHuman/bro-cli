//! Isolated real-desktop acceptance host. Never attaches or restarts Bro sessions.
use bro_bridge::{Bridge, BridgeConfig, StartOptions};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let root = std::env::var_os("BRO_RMM_PREVIEW_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| bro_core::paths::bro_dir().join("rmm-preview"));
    let port = std::env::var("BRO_RMM_PREVIEW_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10092);
    let bridge = Bridge::start_with(
        BridgeConfig {
            port,
            automatic_port: false,
            bind: "0.0.0.0".into(),
            data_root: root.join("bridge"),
            web_interface: false,
        },
        StartOptions {
            import_legacy_token: false,
        },
        Box::new(|_| {}),
    )?;
    bridge.set_rmm_settings(bro_core::rmm::Settings {
        enabled: true,
        ..Default::default()
    });
    println!(
        "Isolated RMM preview on port {port}. Pairing token is in the preview root's bridge/.terminal-web-token file."
    );
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(tokio::signal::ctrl_c())?;
    bridge.shutdown();
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
