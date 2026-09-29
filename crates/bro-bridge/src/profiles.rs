//! Launch profiles advertised to clients. The reference host read Windows
//! Terminal's settings.json; bro advertises its own launchers instead (a shell
//! plus the agent harnesses) and bro-tui may replace the list with
//! [`crate::Bridge::set_profiles`]. The chosen profile's `id` comes back in
//! [`crate::CreateRequest::profile_id`].

use crate::model::TerminalProfile;

fn default_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        ("pwsh.exe".into(), vec!["-NoLogo".into()])
    } else {
        (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            Vec::new(),
        )
    }
}

fn agent(id: &str, label: &str, description: &str, client_agent: Option<&str>) -> TerminalProfile {
    TerminalProfile {
        id: id.into(),
        label: label.into(),
        shell: id.into(),
        args: Vec::new(),
        group: "agent".into(),
        description: Some(description.into()),
        // The clients only know claude / codex / hermes.
        agent: client_agent.map(str::to_owned),
        terminal_profile_guid: None,
    }
}

/// The built-in list: `shell`, `claude`, `codex`, `pi`, `omp`.
pub fn default_profiles() -> Vec<TerminalProfile> {
    let (shell, args) = default_shell();
    vec![
        TerminalProfile {
            id: "shell".into(),
            label: "Shell".into(),
            shell,
            args,
            group: "shell".into(),
            description: Some("Default shell".into()),
            agent: None,
            terminal_profile_guid: None,
        },
        agent("claude", "Claude Code", "Claude Code agent", Some("claude")),
        agent("codex", "Codex", "OpenAI Codex agent", Some("codex")),
        agent("pi", "Pi", "Pi coding agent", None),
        agent("omp", "omp", "oh-my-pi coding agent", None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiles_serialize_like_just_terminal_profiles() {
        let profiles = default_profiles();
        assert_eq!(profiles[0].group, "shell");
        let value = serde_json::to_value(&profiles[1]).unwrap();
        assert_eq!(value["id"], "claude");
        assert_eq!(value["group"], "agent");
        assert_eq!(value["agent"], "claude");
        assert!(value["args"].is_array());
        assert!(value.get("terminalProfileGuid").is_none());
        assert!(
            serde_json::to_value(&profiles[3])
                .unwrap()
                .get("agent")
                .is_none()
        );
    }
}
