//! Regenerate / edit-last replay of the latest visible top-level run.
//!
//! A replay rewinds both the model history and the transcript to the run's
//! recorded checkpoint and resubmits the same (or edited) user request. It is
//! only allowed when every identity check still holds: the checkpoint's user
//! entry, transcript position and model-history message must all describe
//! the same visible request, and no later top-level run other than an
//! automatic goal resume may follow it. Rewinding the coordinator starts a
//! fresh context window, so the rewrite never masquerades as an append to an
//! already-submitted provider prefix.

use super::*;

/// Recorded positions of the latest replayable visible user turn.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReplayCheckpoint {
    pub(super) history_start: usize,
    pub(super) conversation_start: usize,
    pub(super) run_index: usize,
    pub(super) visible_content: String,
}

/// Locates and validates the checkpoint against the live transcript.
pub(super) fn latest_replay_checkpoint(session: &SharedSession) -> Result<ReplayCheckpoint> {
    let Some((index, run)) = session.meta.runs.iter().enumerate().rev().find(|(_, run)| {
        run.kind == "agent"
            && run.user_entry_id.is_some()
            && run.history_start.is_some()
            && run.conversation_start.is_some()
    }) else {
        return Err(anyhow!("the latest visible turn has no replay checkpoint"));
    };
    if session.meta.runs[index + 1..]
        .iter()
        .any(|later| later.kind != "goal-resume")
    {
        return Err(anyhow!(
            "the latest visible turn is followed by another top-level run and cannot be replayed"
        ));
    }
    let conversation_start = run.conversation_start.expect("checked");
    let conversation = session
        .state
        .conversation
        .as_ref()
        .ok_or_else(|| anyhow!("replay checkpoint has no transcript"))?;
    let entry = conversation
        .get(conversation_start)
        .ok_or_else(|| anyhow!("replay checkpoint no longer matches transcript"))?;
    if entry.id != run.user_entry_id.as_deref().expect("checked") {
        return Err(anyhow!(
            "replay checkpoint user identity no longer matches transcript"
        ));
    }
    let ConversationKind::User { content } = &entry.kind else {
        return Err(anyhow!(
            "replay checkpoint does not point to a visible user entry"
        ));
    };
    Ok(ReplayCheckpoint {
        history_start: run.history_start.expect("checked"),
        conversation_start,
        run_index: index,
        visible_content: content.clone(),
    })
}

/// Returns the model-history user request at the checkpoint, verifying it is
/// the same request the transcript shows.
pub(super) fn checkpoint_request(
    checkpoint: &ReplayCheckpoint,
    history: &[Message],
) -> Result<Message> {
    let Some(message) = history.get(checkpoint.history_start).cloned() else {
        return Err(anyhow!("replay checkpoint no longer matches model history"));
    };
    if message.role != crate::core::MessageRole::User || message.request_only == Some(true) {
        return Err(anyhow!(
            "replay checkpoint does not point to a user request"
        ));
    }
    let images = message.images.as_deref().unwrap_or_default();
    if visible_user_content(message.content.as_deref().unwrap_or_default(), images)
        != checkpoint.visible_content
    {
        return Err(anyhow!(
            "replay checkpoint content no longer matches the visible user turn"
        ));
    }
    Ok(message)
}

/// Builds the resubmitted input. An edit replaces the visible text but keeps
/// any Remote file context that was attached to the original request.
pub(super) fn replay_input(
    original: Message,
    replacement_text: Option<String>,
) -> Result<(String, Vec<ImageAttachment>)> {
    let images = original.images.unwrap_or_default();
    let original_content = original.content.unwrap_or_default();
    let (_, remote_file_payload) = split_remote_file_context(&original_content);
    let remote_file_payload = remote_file_payload.map(str::to_owned);
    let mut text = replacement_text.unwrap_or(original_content);
    if let Some(payload) = remote_file_payload
        && split_remote_file_context(&text).1.is_none()
    {
        text.push_str(REMOTE_FILE_CONTEXT_OPEN);
        text.push_str(&payload);
        text.push_str(REMOTE_FILE_CONTEXT_CLOSE);
    }
    if text.is_empty() && images.is_empty() {
        return Err(anyhow!("edited message cannot be empty"));
    }
    Ok((text, images))
}

impl BackendService {
    pub(super) fn regenerate_last(&mut self) -> Result<()> {
        self.replay_last_visible_turn(None)
    }

    pub(super) fn edit_last(&mut self, text: String) -> Result<()> {
        self.replay_last_visible_turn(Some(text.trim().to_owned()))
    }

    fn replay_last_visible_turn(&mut self, replacement_text: Option<String>) -> Result<()> {
        if self.shared.lock_or_recover().state.is_streaming {
            return Err(anyhow!("cannot replay while a response is streaming"));
        }
        let checkpoint = latest_replay_checkpoint(&self.shared.lock_or_recover())?;
        let original = {
            let mut coordinator = self.coordinator.lock_or_recover();
            let message = checkpoint_request(&checkpoint, &coordinator.model_history())?;
            coordinator.rewind_model_history(checkpoint.history_start)?;
            message
        };
        let (text, images) = replay_input(original, replacement_text)?;

        {
            let mut shared = self.shared.lock_or_recover();
            if checkpoint.conversation_start > shared.conversation_mut().len() {
                return Err(anyhow!("replay checkpoint no longer matches transcript"));
            }
            shared
                .conversation_mut()
                .truncate(checkpoint.conversation_start);
            shared.meta.runs.truncate(checkpoint.run_index);
            shared.state.conversation_revision = shared.state.conversation_revision.wrapping_add(1);
            shared.state.active_assistant_entry_id = None;
            shared.state.active_assistant_text.clear();
            shared.state.active_reasoning_entry_id = None;
            shared.state.active_reasoning_text.clear();
            shared.state.active_reasoning_summary.clear();
            shared.state.active_activity_entry_id = None;
            shared.state.error_message = None;
        }
        self.publish_state();

        self.submit_agent_with_images(text, images, true, "agent", false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_turn(later_kind: Option<&str>) -> SharedSession {
        let mut session = SharedSession::new("openai/test".into(), "auto".into());
        session.start_run("run-1".into(), "agent", "openai/test".into(), 3, 0);
        let entry = session.append(ConversationKind::User {
            content: "hello".into(),
        });
        session.bind_run_user_entry("run-1", entry);
        session.finish_run(RunStatus::Completed, None);
        if let Some(kind) = later_kind {
            session.start_run("run-2".into(), kind, "openai/test".into(), 5, 1);
            session.finish_run(RunStatus::Completed, None);
        }
        session
    }

    #[test]
    fn only_goal_resumes_may_follow_a_replayable_turn() {
        let checkpoint = latest_replay_checkpoint(&session_with_turn(Some("goal-resume"))).unwrap();
        assert_eq!(checkpoint.history_start, 3);
        assert_eq!(checkpoint.visible_content, "hello");
        assert!(latest_replay_checkpoint(&session_with_turn(Some("agent-notification"))).is_err());
    }

    #[test]
    fn checkpoint_must_match_the_model_history_request() {
        let checkpoint = latest_replay_checkpoint(&session_with_turn(None)).unwrap();
        let mut history = vec![Message::system("s"); 3];
        history.push(Message::user("hello"));
        assert!(checkpoint_request(&checkpoint, &history).is_ok());
        history[3] = Message::user("something else");
        assert!(checkpoint_request(&checkpoint, &history).is_err());
        history[3] = Message::user("hello").request_only();
        assert!(checkpoint_request(&checkpoint, &history).is_err());
    }

    #[test]
    fn edits_keep_attached_remote_file_context() {
        let original = Message::user(format!(
            "question{REMOTE_FILE_CONTEXT_OPEN}file body{REMOTE_FILE_CONTEXT_CLOSE}"
        ));
        let (text, _) = replay_input(original, Some("edited".into())).unwrap();
        assert!(text.starts_with("edited"));
        assert!(text.contains("file body"));
        assert!(replay_input(Message::user(""), Some(String::new())).is_err());
    }
}
