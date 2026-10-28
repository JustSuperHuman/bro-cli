//! Who made the model a session runs, so an agent running another lab's model wears that lab's mark too: Codex
//! on Claude, Claude on GPT, Claude on Gemini, Pi on anything. Names are the logo names [`crate::ui::provider_glyph`]
//! and [`crate::app::gui::kit::brand::maker`] take ("claude", "codex" for OpenAI, "gemini", "deepseek"…).

use bro_core::Harness;

/// The maker of the model a session runs, when it isn't the agent's own: from the model's name, else from the
/// login or provider it runs on. None for a shell, or an agent on its own lab's model.
pub fn badge(harness: Option<Harness>, profile: Option<&str>, model: Option<&str>) -> Option<&'static str> {
    let own = match harness? {
        Harness::Claude => Some("claude"),
        Harness::Codex => Some("codex"),
        Harness::Pi | Harness::Omp => None,
    };
    let maker = model.and_then(of_model).or_else(|| profile.and_then(of_login))?;
    (Some(maker) != own).then_some(maker)
}

/// The lab behind a model id ("claude-opus-5", "anthropic/claude-sonnet-4.5", "gpt-5.5", "o3", "qwen3-coder").
pub fn of_model(model: &str) -> Option<&'static str> {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return None;
    }
    // the name decides; a vendor prefix ("x-ai/", "moonshotai/") only when the name doesn't
    let (vendor, name) = m.rsplit_once('/').unwrap_or(("", &m));
    let reasoning = |s: &str| s.len() >= 2 && s.starts_with('o') && s.as_bytes()[1].is_ascii_digit();
    let lab = |s: &str| -> Option<&'static str> {
        let has = |k: &str| s.contains(k);
        Some(if has("claude") || has("opus") || has("sonnet") || has("haiku") || has("fable") || has("anthropic") {
            "claude"
        } else if has("gemini") || has("gemma") || has("google") {
            "gemini"
        } else if has("gpt") || has("codex") || has("openai") || has("chatgpt") || reasoning(s) {
            "codex"
        } else if has("deepseek") {
            "deepseek"
        } else if has("mistral") || has("stral") || has("mixtral") {
            "mistral"
        } else if has("qwen") || has("qwq") {
            "qwen"
        } else if has("kimi") || has("moonshot") {
            "kimi"
        } else if has("grok") || has("x-ai") || s == "xai" {
            "xai"
        } else if has("glm") || has("zhipu") || has("z-ai") || s == "zai" {
            "zai"
        } else if has("llama") && !has("ollama") || has("meta-") {
            "meta"
        } else {
            return None;
        })
    };
    lab(name).or_else(|| lab(vendor))
}

/// The lab behind a login or provider id ("pool", "claude:work", "codex:me", "gemini-sub:x", "openrouter"): its
/// own lab, or for a host of many labs' models (OpenRouter, Groq, Ollama, LM Studio) the host.
pub fn of_login(id: &str) -> Option<&'static str> {
    let id = id.trim().to_ascii_lowercase();
    if id == "pool" || id.starts_with("claude:") {
        return Some("claude");
    }
    if id.starts_with("codex:") {
        return Some("codex");
    }
    if id.starts_with("gemini") {
        return Some("gemini");
    }
    let has = |k: &str| id.contains(k);
    if has("openrouter") {
        Some("openrouter")
    } else if has("groq") {
        Some("groq")
    } else if has("ollama") {
        Some("ollama")
    } else if has("lmstudio") || has("lm-studio") {
        Some("lmstudio")
    } else if has("dashscope") || has("alibaba") {
        Some("qwen")
    } else if has("bigmodel") || has("z.ai") {
        Some("zai")
    } else {
        of_model(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_on_another_labs_model_wears_its_mark() {
        // its own lab's model: no badge
        assert_eq!(badge(Some(Harness::Claude), Some("claude:work"), Some("opus")), None);
        assert_eq!(badge(Some(Harness::Codex), Some("codex:me"), None), None);
        assert_eq!(badge(Some(Harness::Claude), Some("openrouter"), Some("anthropic/claude-sonnet-4.5")), None);
        // another lab's, by the model or else the login
        assert_eq!(badge(Some(Harness::Codex), Some("pool"), Some("claude-opus-5")), Some("claude"));
        assert_eq!(badge(Some(Harness::Codex), Some("claude:work"), None), Some("claude"));
        assert_eq!(badge(Some(Harness::Claude), Some("codex:me"), Some("gpt-5.5")), Some("codex"));
        assert_eq!(badge(Some(Harness::Claude), Some("gemini-sub:me"), None), Some("gemini"));
        assert_eq!(badge(Some(Harness::Claude), Some("openrouter"), Some("moonshotai/kimi-k2")), Some("kimi"));
        assert_eq!(badge(Some(Harness::Claude), Some("openrouter"), None), Some("openrouter"));
        assert_eq!(badge(Some(Harness::Pi), None, Some("claude-sonnet-5")), Some("claude"));
        // a shell, or nothing to go on
        assert_eq!(badge(None, None, Some("claude-opus-5")), None);
        assert_eq!(badge(Some(Harness::Claude), Some("my-proxy"), None), None);
    }

    #[test]
    fn models_by_lab() {
        for (m, lab) in [
            ("o3", "codex"),
            ("o4-mini", "codex"),
            ("openai/gpt-oss-120b", "codex"),
            ("gemini-2.5-pro", "gemini"),
            ("deepseek-chat", "deepseek"),
            ("devstral-medium", "mistral"),
            ("qwen/qwen3-coder", "qwen"),
            ("x-ai/grok-4", "xai"),
            ("z-ai/glm-4.6", "zai"),
            ("meta-llama/llama-4-maverick", "meta"),
        ] {
            assert_eq!(of_model(m), Some(lab), "{m}");
        }
        assert_eq!(of_model("ollama"), None);
        assert_eq!(of_model("some-model"), None);
    }
}
