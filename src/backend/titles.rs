//! Automatic and fallback titles for persisted sessions.
//!
//! Title generation is intentionally isolated from session execution so its
//! provider request, input bounding, and normalization rules stay auditable.

use super::*;

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
    let compact = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 60 {
        compact
    } else {
        format!("{}…", compact.chars().take(59).collect::<String>())
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
    if compact.chars().count() <= 56
        && compact.split_whitespace().count() <= 8
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
    bridge: &BridgeClient,
    title: &TitleRequest,
) -> Result<String> {
    let system = "Generate a concise title for a saved Yeet coding session. Return exactly one plain-text title and nothing else. Use 2-6 words and at most 56 characters. Capture the user's main task, preserve important project or symbol names, and use the same language as the request when natural. Do not use quotes, Markdown, a trailing period, or a 'Title:' prefix.";
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
    let result = bridge.complete(&request)?;
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
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    let mut title: String = title.chars().take(56).collect();
    title = title.trim().to_owned();
    (!title.is_empty()).then_some(title)
}
