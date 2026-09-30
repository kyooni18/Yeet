//! Terminal adapter for shared intents; domain modules never depend on App.
use super::App;
use crate::{agents::actions::Action, backend::Backend};

impl App {
    /// One entry point for frontend automation and shared UI actions.
    /// Commands retain the existing Harness transport/permission pipeline.
    pub fn dispatch_action(&mut self, action: Action, backend: &mut Backend) -> anyhow::Result<()> {
        match action {
            Action::Navigate(target) => {
                self.apply_shared_navigation(target);
                Ok(())
            }
            Action::Command(command) => backend.send(command),
        }
    }
}
