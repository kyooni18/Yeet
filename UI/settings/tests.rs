use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use crate::shared_ui::settings_session::SettingsSession;
#[test]
fn flex_and_busy_guards_are_shared_with_rendered_controls() {
    let mut harness = HarnessState::default();
    let env = SettingsEnvironment::default();
    let mut state = SettingsState::default();
    assert!(
        !state
            .view(&env, &harness)
            .controls()
            .find(|control| control.id == "openai_flex")
            .unwrap()
            .enabled
    );
    harness.active_model = "openai/gpt".into();
    harness.auth_providers = vec![crate::model::AuthProviderItem {
        provider: "openai".into(),
        authenticated: true,
        method: "api-key".into(),
        expires_at: None,
        usage: None,
        error: None,
    }];
    assert!(matches!(
        state
            .apply(
                SettingsAction::SetToggle {
                    id: "openai_flex".into(),
                    enabled: true
                },
                &env,
                &harness
            )
            .command,
        Some(HarnessCommand::SetOpenAiFlex { enabled: true })
    ));
    harness.settings_working = true;
    let view = state.view(&env, &harness);
    assert!(view.controls().all(|control| !control.enabled));
    assert!(
        state
            .apply(
                SettingsAction::SetChoice {
                    id: "appearance".into(),
                    value: "dark".into()
                },
                &env,
                &harness
            )
            .command
            .is_none()
    );
}
#[test]
fn stable_selection_and_editor_delivery_preserve_failed_changes() {
    let harness = HarnessState::default();
    let env = SettingsEnvironment {
        supported_editors: vec![SettingsEditorKind::ContextLength],
        ..Default::default()
    };
    let mut session = SettingsSession::new(&harness);
    session.configure(env, &harness);
    let prepared = session.prepare(SettingsAction::Activate("context_length".into()), &harness);
    session.commit(prepared, &harness);
    assert_eq!(
        session.projection().view.selected.as_deref(),
        Some("context_length")
    );
    let prepared = session.prepare(SettingsAction::SubmitEditor("128k".into()), &harness);
    assert!(matches!(
        prepared.effect.command,
        Some(HarnessCommand::SetContextLength {
            length: Some(128000)
        })
    ));
    drop(prepared);
    assert!(session.projection().view.editor.is_some());
    let prepared = session.prepare(SettingsAction::SubmitEditor("invalid".into()), &harness);
    assert!(prepared.effect.command.is_none());
    session.commit(prepared, &harness);
    assert!(session.projection().view.editor.is_some());
    let prepared = session.prepare(SettingsAction::SubmitEditor("auto".into()), &harness);
    session.commit(prepared, &harness);
    assert!(session.projection().view.editor.is_none());
    assert!(session.refresh(&harness).is_none());
}
#[test]
fn projected_choices_and_provider_identity_drive_actions_without_native_policy() {
    let mut harness = HarnessState::default();
    harness.auth_providers = vec![crate::model::AuthProviderItem {
        provider: "antigravity".into(),
        authenticated: false,
        method: String::new(),
        expires_at: None,
        usage: None,
        error: None,
    }];
    let env = SettingsEnvironment::default();
    let mut state = SettingsState::default();
    let view = state.view(&env, &harness);
    let control = view
        .controls()
        .find(|control| control.provider.as_deref() == Some("antigravity"))
        .unwrap();
    assert_eq!(control.label, "Google Antigravity");
    assert_eq!(control.action_label, "Log in");
    assert!(
        matches!(state.apply(control.action.clone(),&env,&harness).command,Some(HarnessCommand::AuthLogin{provider})if provider=="antigravity")
    );
    assert!(
        state
            .apply(
                SettingsAction::SetChoice {
                    id: "appearance".into(),
                    value: "invalid".into()
                },
                &env,
                &harness
            )
            .command
            .is_none()
    );
    assert!(
        matches!(state.apply(SettingsAction::Activate("appearance".into()),&env,&harness).command,Some(HarnessCommand::SetAppearance{appearance})if appearance=="light")
    );
}
