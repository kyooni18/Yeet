use super::*;

impl HarnessService {
    pub(super) fn select_model(&self, model: String) -> Result<()> {
        let model = self.config.set_model(&model)?;
        let levels = reasoning_levels_for_model(&model);
        let current_reasoning = self
            .shared
            .lock_or_recover()
            .state
            .active_reasoning_level
            .clone();
        let normalized_reasoning =
            if !current_reasoning.is_empty() && !levels.contains(&current_reasoning.as_str()) {
                Some(self.config.set_reasoning_level("high")?)
            } else {
                None
            };
        let model_changed;
        {
            let mut shared = self.shared.lock_or_recover();
            model_changed = model_selection_changes(&shared.state.active_model, &model);
            if !model_changed && normalized_reasoning.is_none() {
                return Ok(());
            }
            if model_changed {
                shared.state.active_model = model.clone();
                shared.state.active_model_context_length = None;
                shared.state.current_context_tokens = None;
                shared.append(ConversationKind::System {
                    content: format!("Model set to {model}"),
                });
            }
            if let Some(level) = normalized_reasoning {
                shared.state.active_reasoning_level = level.clone();
                shared.append(ConversationKind::System {
                    content: format!("Reasoning adjusted to {level} for {model}"),
                });
            }
        }
        if model_changed {
            self.refresh_context_length();
        }
        self.publish_state();
        Ok(())
    }

    pub(super) fn select_reasoning(&self, level: String) -> Result<()> {
        let normalized = normalize_reasoning_level(&level)
            .ok_or_else(|| anyhow!("Unsupported reasoning level: {level}"))?;
        let model = self.shared.lock_or_recover().state.active_model.clone();
        if !model.is_empty() && !reasoning_levels_for_model(&model).contains(&normalized) {
            return Err(anyhow!(
                "Reasoning level {normalized} is not supported for {model}"
            ));
        }
        let level = self.config.set_reasoning_level(normalized)?;
        self.shared.lock_or_recover().state.active_reasoning_level = level;
        self.publish_state();
        Ok(())
    }

    pub(super) fn refresh_context_length(&self) {
        let model = self.shared.lock_or_recover().state.active_model.clone();
        if model.is_empty() {
            return;
        }
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let length = config
                .context_length_override(&model)
                .ok()
                .flatten()
                .or_else(|| bridge.context_length(&model).ok().flatten())
                .or_else(|| config.context_length(&model).ok().flatten());
            if let Ok(mut state) = shared.lock()
                && state.state.active_model == model
            {
                state.state.active_model_context_length = length;
                let _ = tx.send(ServiceEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    pub(super) fn request_sessions(&self) {
        let sessions = self.store.list(&self.workspace_root);
        let workspace_catalog = self.store.list_workspace_catalog(&self.workspace_root);
        let mut shared = self.shared.lock_or_recover();
        match sessions {
            Ok(sessions) => shared.state.saved_sessions = sessions,
            Err(error) => {
                shared.state.error_message = Some(format!("Unable to list sessions: {error}"))
            }
        }
        match workspace_catalog {
            Ok((workspaces, session_groups)) => {
                shared.state.known_workspaces = workspaces;
                shared.state.workspace_session_groups = session_groups;
            }
            Err(error) => {
                shared.state.error_message = Some(format!("Unable to list workspaces: {error}"))
            }
        }
        drop(shared);
        self.publish_state();
    }
}
