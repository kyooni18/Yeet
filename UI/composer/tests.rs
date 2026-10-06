use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use crate::shared_ui::composer_session::ComposerSession;
fn environment() -> ComposerEnvironment {
    ComposerEnvironment {
        context: ComposerContext {
            workspace: "workspace".into(),
            session_id: Some("s".into()),
            unsaved_generation: 0,
        },
        supported_destinations: vec![ComposerDestination::Models],
        ..Default::default()
    }
}
fn editor(text: &str) -> EditorSnapshot {
    EditorSnapshot {
        context: environment().context,
        text: text.into(),
        revision: 1,
        ..Default::default()
    }
}
#[test]
fn shared_submit_routes_panels_and_preserves_failed_delivery_and_new_typing() {
    let harness = HarnessState::default();
    let mut session = ComposerSession::new(&harness);
    session.configure(environment(), &harness);
    let prepared = session.prepare(ComposerAction::UpdateEditor(editor("old draft")), &harness);
    session.commit(prepared, &harness);
    let submitted = session.prepare(ComposerAction::Submit(editor("old draft")), &harness);
    assert!(
        matches!(submitted.effect.command,Some(HarnessCommand::Submit{ref text,..}) if text=="old draft")
    );
    assert_eq!(session.projection().view.editor.text, "old draft");
    let mut newer = editor("new typing");
    newer.revision = 2;
    let prepared = session.prepare(ComposerAction::UpdateEditor(newer), &harness);
    session.commit(prepared, &harness);
    let (_, effect) = session.commit(submitted, &harness);
    assert_eq!(session.projection().view.editor.text, "new typing");
    assert_eq!(effect.ui.accepted_editor.unwrap().text, "old draft");
    let stale = session.prepare(ComposerAction::Submit(editor("old draft")), &harness);
    assert!(stale.effect.command.is_none());
    let mut panel = editor("/model");
    panel.revision = 3;
    let prepared = session.prepare(ComposerAction::Submit(panel), &harness);
    assert_eq!(
        prepared.effect.ui.destination,
        Some(ComposerDestination::Models)
    );
    assert!(prepared.effect.command.is_none());
    let mut query = editor("/mod");
    query.revision = 3;
    let selected = session.prepare(
        ComposerAction::SelectSuggestion {
            editor: query.clone(),
            command: "/model".into(),
        },
        &harness,
    );
    assert_eq!(
        selected.effect.ui.destination,
        Some(ComposerDestination::Models)
    );
    assert_eq!(selected.effect.ui.accepted_editor.unwrap().text, query.text);
}
#[test]
fn readiness_context_streaming_and_attachment_edit_are_shared_guards() {
    let mut harness = HarnessState::default();
    let mut state = ComposerState::default();
    let env = environment();
    let mut snapshot = editor("send");
    snapshot.context.workspace = "other".into();
    assert!(
        state
            .apply(ComposerAction::Submit(snapshot), &env, &harness)
            .command
            .is_none()
    );
    let mut snapshot = editor("send");
    snapshot.attachments.push(ComposerAttachment {
        id: "file".into(),
        name: "file".into(),
        attachment_id: None,
        ready: false,
        error: None,
    });
    assert!(
        state
            .apply(ComposerAction::Submit(snapshot), &env, &harness)
            .command
            .is_none()
    );
    let mut snapshot = editor("");
    snapshot.mode = ComposerMode::EditLast {
        has_attachments: true,
    };
    assert!(matches!(
        state
            .apply(ComposerAction::Submit(snapshot.clone()), &env, &harness)
            .command,
        Some(HarnessCommand::EditLast { .. })
    ));
    harness.is_streaming = true;
    assert!(
        state
            .apply(ComposerAction::Submit(snapshot), &env, &harness)
            .command
            .is_none()
    );
    assert!(matches!(
        state
            .apply(ComposerAction::Interrupt, &env, &harness)
            .command,
        Some(HarnessCommand::Interrupt)
    ));
    assert!(
        state
            .apply(ComposerAction::Submit(editor("/model")), &env, &harness)
            .ui
            .destination
            .is_none()
    );
    let mut inspecting = env.clone();
    inspecting.allow_commands_while_streaming = true;
    assert_eq!(
        state
            .apply(
                ComposerAction::Submit(editor("/model")),
                &inspecting,
                &harness
            )
            .ui
            .destination,
        Some(ComposerDestination::Models)
    );
}
#[test]
fn stale_permission_target_cannot_resolve_replacement() {
    let mut harness = HarnessState::default();
    harness.pending_shell_permission = Some(crate::model::ShellPermission {
        id: "new".into(),
        kind: "shell".into(),
        command: "ls".into(),
        operation: "Inspect".into(),
        reason: "requested".into(),
    });
    let mut state = ComposerState::default();
    let env = environment();
    let action = |id: &str| ComposerAction::RespondPermission {
        target: PermissionTarget {
            kind: PermissionKind::Shell,
            id: id.into(),
        },
        allow: true,
    };
    assert!(state.apply(action("old"), &env, &harness).command.is_none());
    assert!(
        matches!(state.apply(action("new"),&env,&harness).command,Some(HarnessCommand::ResolvePermission{request_id,granted:true}) if request_id=="new")
    );
    let view = state.view(&env, &harness);
    assert_eq!(view.permissions[0].controls[0].label, "Deny");
    assert!(view.editable);
    let mut capturing = env.clone();
    capturing.freeze_editor_for_permissions = true;
    assert!(!state.view(&capturing, &harness).editable);
}
