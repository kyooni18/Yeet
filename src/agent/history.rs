//! Context assembly stays append-only within a user turn so provider cache prefixes
//! remain stable. Request-only guidance is turn-scoped and is pruned before the next
//! ordinary user turn; ContextMemory rollover handles larger durable-history compaction.
use crate::core::{Message, MessageRole};

pub(super) const TURN_CONTEXT_BOUNDARY: &str = "New user-turn context boundary. Earlier turn-local Skill instructions, retry/finalization directives, goal checkpoints, and execution provenance are historical context, not active instructions for this turn. Apply only this turn's explicitly activated Skills and current runtime guidance. Preserve prior evidence; do not execute stale directives.";

/// Keep the first full instruction; a repeat is a new, small activation record.
/// Never delete an earlier copy to save tokens.
pub(super) fn append_skill_instruction(history: &mut Vec<Message>, name: &str, instructions: &str) {
    let content = format!(
        "User-invoked Skill: {name}\n{instructions}\nUse its attached support tools only as needed; normal sandbox/approval rules apply."
    );
    let repeated = history.iter().position(|message| {
        message.role == MessageRole::System && message.content.as_deref() == Some(content.as_str())
    });
    let activation = if let Some(index) = repeated {
        format!(
            "User-invoked Skill: {name}\nReactivated for this turn: use the full instructions at context message {index} (zero-based), retained earlier in this window. Normal sandbox/approval rules apply."
        )
    } else {
        content
    };
    history.push(Message::system(activation));
}

/// Runtime/environment changes belong at the tail, not beside an old user message.
/// Retain each submitted update for the current turn; the next turn prunes request-only guidance.
pub(super) fn append_context_updates(
    history: &mut Vec<Message>,
    previous: &mut Vec<Message>,
    updates: &[Message],
) {
    if previous == updates {
        return;
    }
    if let Some(first_changed) = updates
        .iter()
        .enumerate()
        .find_map(|(index, update)| (previous.get(index) != Some(update)).then_some(index))
    {
        history.extend_from_slice(&updates[first_changed..]);
    } else if previous.len() != updates.len() {
        history.extend_from_slice(updates);
    }
    *previous = updates.to_vec();
}

/// Drops turn-local provider guidance before starting a new ordinary user turn.
/// Durable user, assistant, and tool evidence stays in the transcript.
pub(super) fn prune_request_only_history(history: &mut Vec<Message>) -> usize {
    let before = history.len();
    history.retain(|message| message.request_only != Some(true));
    before.saturating_sub(history.len())
}

#[cfg(test)]
mod append_only_tests {
    use super::*;
    use crate::agent::policy::{TaskProfile, request_history_for_profile_at};

    #[test]
    fn completed_turns_and_lane_changes_preserve_all_submitted_messages() {
        let mut history = vec![Message::system("base"), Message::user("first")];
        append_skill_instruction(&mut history, "example", "skill instructions");
        history.push(Message::system("retry directive").request_only());
        history.push(Message::tool(
            "large result".repeat(20_000),
            "call",
            Some("read_file".into()),
        ));
        let submitted = request_history_for_profile_at(&history, TaskProfile::Agent, 1);
        history.push(Message::user("next"));
        history.push(Message::system(TURN_CONTEXT_BOUNDARY).request_only());
        for profile in [TaskProfile::Agent, TaskProfile::Research] {
            let next = request_history_for_profile_at(&history, profile, submitted.len());
            assert_eq!(&next[..submitted.len()], submitted.as_slice());
        }
    }

    #[test]
    fn runtime_changes_append_and_identical_updates_do_not_grow_history() {
        let mut history = vec![Message::system("base")];
        let mut previous = Vec::new();
        let first = vec![Message::system("runtime one").request_only()];
        append_context_updates(&mut history, &mut previous, &first);
        let submitted = history.clone();
        append_context_updates(&mut history, &mut previous, &first);
        assert_eq!(history, submitted);
        append_context_updates(
            &mut history,
            &mut previous,
            &[Message::system("runtime two")],
        );
        assert_eq!(&history[..submitted.len()], submitted.as_slice());
        assert_eq!(history.len(), submitted.len() + 1);
    }

    #[test]
    fn changing_one_stable_overlay_does_not_reappend_unchanged_siblings() {
        let mut history = vec![Message::system("base")];
        let mut previous = Vec::new();
        let first = vec![
            Message::system("orientation").request_only(),
            Message::system("runtime one").request_only(),
        ];
        append_context_updates(&mut history, &mut previous, &first);
        let submitted_len = history.len();

        let second = vec![
            Message::system("orientation").request_only(),
            Message::system("runtime two").request_only(),
        ];
        append_context_updates(&mut history, &mut previous, &second);

        assert_eq!(history.len(), submitted_len + 1);
        assert_eq!(
            history
                .last()
                .and_then(|message| message.content.as_deref()),
            Some("runtime two")
        );
    }
    #[test]
    fn new_turn_prunes_request_only_guidance_but_keeps_durable_evidence() {
        let mut history = vec![
            Message::system("base"),
            Message::user("first"),
            Message::system("runtime overlay").request_only(),
            Message::user("retry correction").request_only(),
            Message::assistant("done", None),
        ];

        assert_eq!(prune_request_only_history(&mut history), 2);
        assert_eq!(history.len(), 3);
        assert!(
            history
                .iter()
                .all(|message| message.request_only != Some(true))
        );
        assert_eq!(history[1].content.as_deref(), Some("first"));
        assert_eq!(history[2].content.as_deref(), Some("done"));
    }

    #[test]
    fn repeated_skills_only_shorten_the_new_activation() {
        let mut history = vec![Message::user("first")];
        let instructions = "original instructions ".repeat(1000);
        append_skill_instruction(&mut history, "example", &instructions);
        let submitted = history.clone();
        append_skill_instruction(&mut history, "example", "changed instructions");
        append_skill_instruction(&mut history, "example", &instructions);
        assert_eq!(&history[..submitted.len()], submitted.as_slice());
        let activation = history.last().unwrap().content.as_deref().unwrap();
        assert!(activation.contains("context message 1 (zero-based)"));
        assert!(activation.len() < 300);
        let mut new_window = vec![];
        append_skill_instruction(&mut new_window, "example", &instructions);
        assert!(
            new_window[0]
                .content
                .as_deref()
                .unwrap()
                .contains(&instructions)
        );
    }
}
