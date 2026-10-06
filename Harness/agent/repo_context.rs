//! Once-per-session overlays: the repository map and detected project checks.
//!
//! Stable overlays are re-appended on every user turn, so a large one would be
//! paid uncached each turn. The map is attached only when a byte-identical copy
//! is not already in the append-only history: once per session while the
//! public surface is unchanged, and again after a rollover replaces history.

use super::*;

const SMALL_TALK_MAX_WORDS: usize = 3;
// A/B (3 runs per arm): on 3-4 file fixtures the map added ~2-4k uncached
// tokens (+14-17% cost) because the first listing already shows everything;
// on a 374-file repo cost fell ~11% (within noise), and an identifier lookup
// paid ~1.4k extra input tokens per model call for no fewer calls. The map is
// therefore opt-in (context.repoMap), and small repos skip it even then.
const MIN_SOURCE_FILES: usize = 25;

pub(super) fn repo_map_overlay(
    history: &[Message],
    workspace_root: &str,
    input: &str,
    coding_signal: bool,
) -> Option<Message> {
    if !coding_signal && input.split_whitespace().count() <= SMALL_TALK_MAX_WORDS {
        return None;
    }
    let map = crate::tools::repo_map::build(
        std::path::Path::new(workspace_root),
        crate::tools::repo_map::DEFAULT_TOKEN_BUDGET,
    )?;
    if map.source_files < MIN_SOURCE_FILES {
        return None;
    }
    unsent_overlay(history, map.text)
}

/// A request-only overlay for `text`, unless a byte-identical system message
/// is already in the append-only history (and so still on the wire).
pub(super) fn unsent_overlay(history: &[Message], text: String) -> Option<Message> {
    let already_sent = history.iter().any(|message| {
        message.role == MessageRole::System && message.content.as_deref() == Some(text.as_str())
    });
    (!already_sent).then(|| Message::system(text).request_only())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_map_is_attached_once_until_history_loses_it() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mut history = vec![Message::user("fix the parser")];
        let overlay = repo_map_overlay(&history, root, "fix the parser", true).unwrap();
        history.push(overlay);
        assert!(repo_map_overlay(&history, root, "now add a test", true).is_none());
        assert!(repo_map_overlay(&[], root, "hi", false).is_none());
        assert!(repo_map_overlay(&[], root, "now add a test", false).is_some());
    }
}
