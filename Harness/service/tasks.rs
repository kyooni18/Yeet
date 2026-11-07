use super::*;
use crate::harness::tasks::{Schedule, TaskAction, TaskStore, five_hour_reset};

impl HarnessService {
    pub(crate) fn configure_scheduled_run(&mut self, model: &str, session_id: &str) -> Result<()> {
        let model = crate::config::validate_model_id(model)?;
        self.new_session();
        let mut shared = self.shared.lock_or_recover();
        shared.state.current_session_id = Some(session_id.to_owned());
        shared.state.active_model = model.clone();
        shared.state.active_model_context_length = self
            .config
            .context_length_override(&model)?
            .or(self.config.context_length(&model)?);
        shared.state.current_context_tokens = None;
        if !reasoning_levels_for_model(&model)
            .contains(&shared.state.active_reasoning_level.as_str())
        {
            shared.state.active_reasoning_level = "auto".into();
        }
        drop(shared);
        self.publish_state();
        Ok(())
    }

    pub(super) fn manage_tasks(&self, action: Option<TaskAction>, request_id: Option<String>) {
        let mutation = action.is_some();
        let result = (|| -> Result<_> {
            let mut store = TaskStore::open()?;
            if let Some(action) = action {
                let reset = if let TaskAction::Save { draft, .. } = &action {
                    if matches!(draft.schedule, Schedule::QuotaReset) {
                        let model = crate::config::validate_model_id(&draft.model)?;
                        let provider = model.split_once('/').map(|p| p.0).unwrap_or("");
                        five_hour_reset(&self.bridge.provider_usage(provider)?, Utc::now())
                    } else {
                        None
                    }
                } else {
                    None
                };
                store.apply(action, reset, Utc::now())?;
            }
            store.list()
        })();
        let mut shared = self.shared.lock_or_recover();
        if mutation {
            shared.state.tasks_response_id = request_id;
        }
        shared.state.tasks_scheduler_running = crate::harness::tasks::scheduler_running();
        match result {
            Ok(tasks) => {
                shared.state.scheduled_tasks = tasks;
                if mutation {
                    shared.state.tasks_notice = Some("Task updated".into());
                }
            }
            Err(error) => shared.state.tasks_notice = Some(format!("Tasks: {error:#}")),
        }
        drop(shared);
        self.publish_state();
    }
}
