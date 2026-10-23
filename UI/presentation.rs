//! Human-readable provider labels and quota countdowns; identifiers stay unchanged.
pub fn provider_name(provider: &str) -> &str {
    match provider.to_ascii_lowercase().as_str() {
        "openai" => "OpenAI API",
        "codex" | "codex-cli" => "OpenAI Codex",
        "anthropic" => "Claude API",
        "claude" => "Claude Code",
        "google" | "gemini" => "Gemini API",
        "gemini-web" => "Gemini Web",
        "antigravity" => "Google Antigravity",
        "opencode" => "Opencode",
        "opencode-go" => "Opencode Go",
        "openrouter" => "Openrouter",
        _ => provider,
    }
}

pub fn reset_countdown(value: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(reset) = chrono::DateTime::parse_from_rfc3339(value) else {
        return "reset time unavailable".into();
    };
    let seconds = (reset.with_timezone(&chrono::Utc) - now).num_seconds();
    if seconds <= 0 {
        return "reset due".into();
    }
    let minutes = (seconds + 59) / 60;
    let days = minutes / 1440;
    let hours = minutes % 1440 / 60;
    let mins = minutes % 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    format!("reset in {}", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_preserve_custom_provider_ids() {
        assert_eq!(provider_name("codex-cli"), "OpenAI Codex");
        assert_eq!(provider_name("ANTHROPIC"), "Claude API");
        assert_eq!(provider_name("my-local"), "my-local");
    }

    #[test]
    fn quota_reset_is_a_readable_countdown() {
        let now = "2026-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            reset_countdown("2026-01-02T02:03:00Z", now),
            "reset in 1d 2h 3m"
        );
        assert_eq!(reset_countdown("2026-01-01T00:00:01Z", now), "reset in 1m");
        assert_eq!(reset_countdown("2025-12-31T23:00:00Z", now), "reset due");
        assert_eq!(reset_countdown("invalid", now), "reset time unavailable");
    }
}
