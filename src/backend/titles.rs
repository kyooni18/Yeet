//! Automatic and fallback titles for persisted sessions.
//!
//! Title generation is intentionally isolated from session execution so its
//! provider request, input bounding, and normalization rules stay auditable.

use super::*;

const TITLE_MAX_WORDS: usize = 4;
const TITLE_MAX_CHARS: usize = 32;

pub(super) const TITLE_INPUT_HEAD_CHARS: usize = 1_600;
pub(super) const TITLE_INPUT_TAIL_CHARS: usize = 800;

/// Inputs needed to generate a title without holding the session lock.
#[derive(Debug, Clone)]
pub(super) struct TitleRequest {
    pub(super) session_id: String,
    model: String,
    user_text: String,
}

/// Creates a deterministic short title from the first user message.
pub(super) fn fallback_title(conversation: &[ConversationEntry]) -> String {
    let first = conversation
        .iter()
        .find_map(|entry| {
            if let ConversationKind::User { content } = &entry.kind {
                Some(content.trim())
            } else {
                None
            }
        })
        .unwrap_or("New chat");
    let title = compact_title(first);
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

/// Captures title-generation inputs once a session has enough context to name.
pub(super) fn prepare_title_request(state: &mut SharedSession) -> Option<TitleRequest> {
    if state.meta.title_generation_attempted {
        return None;
    }
    let session_id = state.state.current_session_id.clone()?;
    let user_text = state
        .state
        .conversation
        .as_ref()?
        .iter()
        .find_map(|entry| {
            if let ConversationKind::User { content } = &entry.kind {
                Some(content.trim().to_owned())
            } else {
                None
            }
        })?;
    let compact = user_text.split_whitespace().collect::<Vec<_>>().join(" ");
    state.meta.title_generation_attempted = true;
    if compact.chars().count() <= TITLE_MAX_CHARS
        && compact.split_whitespace().count() <= TITLE_MAX_WORDS
        && !compact.contains('{')
        && !compact.contains(';')
    {
        return None;
    }
    let model = state.state.active_model.trim().to_owned();
    if model.is_empty() {
        return None;
    }
    Some(TitleRequest {
        session_id,
        model,
        user_text,
    })
}

/// Calls the provider for a concise session title and normalizes its output.
pub(super) fn generate_session_title(
    bridge: &BridgeHandle,
    title: &TitleRequest,
) -> Result<String> {
    let system = format!(
        "Generate a short label for coding session, not a summary or sentence. Return exactly one plain-text title and nothing else. Aim for 2-3 words; never exceed {TITLE_MAX_WORDS} words or {TITLE_MAX_CHARS} characters. Name only the core task or topic. Start directly with that task or topic, never narration such as 'The user wants a', 'The user asks', or 'I need to'. Omit filler, request phrasing, explanations, and secondary details. Preserve a project or symbol name only when essential, and use the same language as the request when natural. Examples: 'Fix login redirect', 'Shorter session titles', 'Rust TUI migration'. Do not use quotes, Markdown, a trailing period, or a 'Title:' prefix."
    );
    let excerpt = title_prompt_excerpt(&title.user_text);
    let mut request = CallRequest::simple(
        title.model.clone(),
        vec![
            Message::system(system),
            Message::user(format!("User request:\n{excerpt}")),
        ],
    );
    request.context_key = Some(format!("session-title-{}", title.session_id));
    // Title generation is deliberately one-shot; never pay a cache-write
    // premium for a prefix that will not be reused.
    request.prompt_cache = Some(false);
    request.temperature = Some(0.2);
    request.max_tokens = Some(64);
    request.timeout_ms = Some(15_000);
    request.metadata = Some(HashMap::from([("purpose".into(), "session-title".into())]));
    request.attached_capabilities = Some(Vec::new());
    let result = bridge.client()?.complete(&request)?;
    normalize_generated_title(&result.text)
        .ok_or_else(|| anyhow!("title generator returned an unusable title"))
}

/// Keeps both ends of a long user request while bounding title-model context.
pub(super) fn title_prompt_excerpt(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let limit = TITLE_INPUT_HEAD_CHARS + TITLE_INPUT_TAIL_CHARS;
    if chars.len() <= limit {
        return value.to_owned();
    }
    let head = chars[..TITLE_INPUT_HEAD_CHARS].iter().collect::<String>();
    let tail = chars[chars.len() - TITLE_INPUT_TAIL_CHARS..]
        .iter()
        .collect::<String>();
    format!("{head}\n[... title input omitted ...]\n{tail}")
}

/// Sanitizes provider title output into the persisted title contract.
pub(super) fn normalize_generated_title(value: &str) -> Option<String> {
    let mut title = value
        .lines()
        .next()?
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim()
        .to_owned();
    for prefix in ["Title:", "title:"] {
        if let Some(rest) = title.strip_prefix(prefix) {
            title = rest.trim().to_owned();
        }
    }
    while title.ends_with('.') {
        title.pop();
        title = title.trim_end().to_owned();
    }
    // Reject request narration before truncation: otherwise its filler alone
    // can fill the four-word budget (e.g. "The user wants a"). Returning None
    // leaves the deterministic user-message fallback in place.
    let lowercase = title.to_ascii_lowercase();
    let narration = lowercase.strip_prefix("the ").unwrap_or(&lowercase);
    if [
        "user wants",
        "user needs",
        "user asks",
        "user requested",
        "user is asking",
        "user would like",
        "i need to",
        "we need to",
    ]
    .iter()
    .any(|prefix| {
        narration == *prefix
            || narration
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    }) {
        return None;
    }
    let title = compact_title(&title);
    (!title.is_empty()).then_some(title)
}

/// Enforces the same bounds for generated and fallback labels, keeping whole
/// words unless a single token exceeds the character budget (e.g. CJK or a path).
fn compact_title(value: &str) -> String {
    let mut title = String::new();
    let mut chars = 0;
    for word in value.split_whitespace().take(TITLE_MAX_WORDS) {
        let word_chars = word.chars().count();
        if title.is_empty() {
            title.extend(word.chars().take(TITLE_MAX_CHARS));
            chars = word_chars.min(TITLE_MAX_CHARS);
        } else if chars + 1 + word_chars <= TITLE_MAX_CHARS {
            title.push(' ');
            title.push_str(word);
            chars += 1 + word_chars;
        } else {
            break;
        }
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_title_rejects_request_narration_before_truncation() {
        for output in [
            "The user wants a",
            "The user wants a fix for login redirects.",
            "Title: THE USER WANTS a shorter session title",
            "User asks for better title generation",
            "The user is asking to fix login",
            "The user would like shorter titles",
            "I need to generate a concise title",
            "We need to summarize the request",
        ] {
            assert_eq!(normalize_generated_title(output), None, "{output}");
        }
    }

    #[test]
    fn generated_title_keeps_task_labels_and_bounds() {
        for (output, expected) in [
            ("Fix login redirect", "Fix login redirect"),
            ("User preferences", "User preferences"),
            ("User requests API", "User requests API"),
            ("Title: Shorter session titles.", "Shorter session titles"),
            ("\"Rust TUI migration\"", "Rust TUI migration"),
            (
                "Fix session title generation regression",
                "Fix session title generation",
            ),
            ("세션 제목 수정", "세션 제목 수정"),
        ] {
            assert_eq!(normalize_generated_title(output).as_deref(), Some(expected));
        }
        assert_eq!(normalize_generated_title(""), None);
        assert_eq!(normalize_generated_title("Title: ..."), None);
    }
}
