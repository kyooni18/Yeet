use super::*;
use crate::harness::tasks::{Schedule, TaskAction, TaskStore, five_hour_reset};

impl HarnessService {
    pub(super) fn manage_tasks(&self, action: Option<TaskAction>) {
        let mutation = action.is_some();
        let result = (|| -> Result<_> {
            let mut store = TaskStore::open()?;
            if let Some(action) = action {
                let reset = if let TaskAction::Save { draft, .. } = &action {
                    if matches!(draft.schedule, Schedule::QuotaReset) {
                        let provider = draft.model.split_once('/').map(|p| p.0).unwrap_or("");
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
