//! Native editor identity and shared composer delivery. Cursor/history remain terminal state.
use super::{App, Backend, Mode};
use crate::shared_ui::composer::*;

#[derive(Default)]
pub(crate) struct ComposerAdapter {
    observed: Option<EditorSnapshot>,
    revision: u64,
}
impl App {
    pub(crate) fn composer_editor_snapshot(&mut self) -> EditorSnapshot {
        let context = ComposerContext {
            workspace: self
                .state
                .known_workspaces
                .iter()
                .find(|workspace| workspace.is_current)
                .map(|workspace| workspace.path.clone())
                .unwrap_or_default(),
            session_id: self.state.current_session_id.clone(),
            unsaved_generation: self
                .application
                .composer_environment()
                .context
                .unsaved_generation,
        };
        let changed = self
            .composer
            .observed
            .as_ref()
            .is_none_or(|previous| previous.context != context || previous.text != self.input);
        if changed {
            self.composer.revision = self.composer.revision.saturating_add(1);
        }
        let snapshot = EditorSnapshot {
            context,
            text: self.input.clone(),
            revision: self.composer.revision,
            ..Default::default()
        };
        self.composer.observed = Some(snapshot.clone());
        snapshot
    }
    pub(crate) fn sync_composer(&mut self) {
        let editor = self.composer_editor_snapshot();
        self.application.configure_composer(
            ComposerEnvironment {
                context: editor.context.clone(),
                available: true,
                supported_destinations: vec![
                    ComposerDestination::Models,
                    ComposerDestination::Sessions,
                    ComposerDestination::Settings,
                    ComposerDestination::Reasoning,
                    ComposerDestination::Goal,
                    ComposerDestination::Agents,
                    ComposerDestination::Files,
                    ComposerDestination::Views,
                    ComposerDestination::Capabilities,
                    ComposerDestination::Permissions,
                    ComposerDestination::Status,
                    ComposerDestination::Auth,
                    ComposerDestination::Providers,
                    ComposerDestination::Debate,
                    ComposerDestination::Help,
                ],
                allow_commands_while_streaming: true,
                freeze_editor_for_permissions: true,
            },
            &self.state,
        );
        if self.application.composer_projection().view.editor != editor {
            let prepared = self
                .application
                .prepare_composer(ComposerAction::UpdateEditor(editor), &self.state);
            self.application.commit_composer(prepared, &self.state);
        }
    }
    pub(crate) fn send_composer_action(
        &mut self,
        backend: &mut Backend,
        action: ComposerAction,
    ) -> anyhow::Result<()> {
        self.sync_composer();
        let prepared = self.application.prepare_composer(action, &self.state);
        let new_session = matches!(
            prepared.effect.command,
            Some(crate::model::FrontendCommand::NewSession)
        );
        if let Some(command) = prepared.effect.command.clone() {
            backend.send(command)?;
        }
        if let Some(destination) = prepared.effect.ui.destination {
            match destination {
                ComposerDestination::Help => self.mode = Mode::Help,
                ComposerDestination::Models => self.open_models(backend)?,
                ComposerDestination::Sessions => self.open_sessions(backend)?,
                ComposerDestination::Settings => self.open_settings(backend)?,
                ComposerDestination::Reasoning => self.open_reasoning(),
                ComposerDestination::Goal => self.open_goal(),
                ComposerDestination::Agents => {
                    let command = self.open_agent_group();
                    backend.send(command)?;
                }
                ComposerDestination::Files => self.open_files(),
                ComposerDestination::Views => self.open_views(),
                ComposerDestination::Capabilities => self.open_capabilities(backend)?,
                ComposerDestination::Permissions => {
                    self.open_sandbox_presets();
                    backend.send(crate::model::FrontendCommand::RequestSandbox)?;
                }
                ComposerDestination::Status => self.open_status(backend)?,
                ComposerDestination::Auth => self.open_auth(backend)?,
                ComposerDestination::Providers => self.open_providers(backend)?,
                ComposerDestination::Debate => {
                    self.open_debate();
                    backend.send(crate::model::FrontendCommand::RequestModels)?;
                }
            }
        }
        let (_, effect) = self.application.commit_composer(prepared, &self.state);
        self.apply_composer_effect(effect.ui);
        if new_session {
            self.application.observe_composer_new_session(&self.state);
        }
        Ok(())
    }
    fn apply_composer_effect(&mut self, effect: ComposerUiEffect) {
        if let Some(accepted) = effect.accepted_editor {
            self.record_input_history(&accepted.text);
            if self.composer_editor_snapshot() == accepted {
                self.input.clear();
                self.cursor = 0;
                self.command_index = 0;
                self.follow_tail = true;
                if effect.destination.is_none() {
                    self.home_override = Some(false);
                }
            }
        }
        if let Some(cancelled) = effect.cancel_edit {
            if self.composer_editor_snapshot() == cancelled {
                self.input.clear();
                self.cursor = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepted_submission_preserves_newer_native_text_and_context() {
        let mut app = App::default();
        app.input = "original prompt".into();
        let accepted = app.composer_editor_snapshot();
        app.input = "newer draft".into();
        app.cursor = 4;
        app.apply_composer_effect(ComposerUiEffect {
            accepted_editor: Some(accepted.clone()),
            ..Default::default()
        });
        assert_eq!(app.input, "newer draft");
        assert_eq!(app.cursor, 4);
        assert_eq!(app.input_history, vec!["original prompt"]);
        app.input = accepted.text.clone();
        app.state.current_session_id = Some("another-session".into());
        app.apply_composer_effect(ComposerUiEffect {
            accepted_editor: Some(accepted),
            ..Default::default()
        });
        assert_eq!(app.input, "original prompt");
        let matching = app.composer_editor_snapshot();
        app.apply_composer_effect(ComposerUiEffect {
            accepted_editor: Some(matching),
            ..Default::default()
        });
        assert!(app.input.is_empty());
        assert_eq!(app.cursor, 0);
    }
}
