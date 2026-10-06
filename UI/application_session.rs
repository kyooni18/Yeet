//! One application controller for native and Remote hosts.
//! Owns navigation, shell placement and agent interaction state. Harness event
//! cursors stay in Harness/transport; these revisions describe only UI intent.
use super::{
    agents::{AgentAction, AgentEffect, AgentState},
    agents_session::{AgentProjection, AgentSession, PreparedAgentAction},
    application::ApplicationView,
    composer::{ComposerAction, ComposerEffect, ComposerEnvironment},
    composer_session::{ComposerProjection, ComposerSession, PreparedComposerAction},
    conversation::{ConversationAction, ConversationEffect, ConversationState},
    conversation_session::{
        ConversationProjection, ConversationSession, PreparedConversationAction,
    },
    navigation::{NavigationState, WorkbenchResources},
    settings::{SettingsAction, SettingsEffect, SettingsEnvironment, SettingsState},
    settings_session::{PreparedSettingsAction, SettingsProjection, SettingsSession},
    shell::{ShellAction, ShellState, ShellView, Surface},
    workbench::WorkbenchTab,
};
use crate::harness::HarnessState;
use serde::{Deserialize, Serialize};

/// Existing shell channel shape, retained during adapter migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellProjection {
    pub ui_revision: u64,
    pub state: ShellState,
    pub view: ShellView,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationProjection {
    pub ui_revision: u64,
    pub navigation: NavigationState,
    pub state: ShellState,
    pub view: ShellView,
    pub application: ApplicationView,
    pub agents: AgentProjection,
    pub conversation: ConversationProjection,
    pub composer: ComposerProjection,
    pub settings: SettingsProjection,
}
pub struct ApplicationSession {
    navigation: NavigationState,
    shell: ShellState,
    agents: AgentSession,
    conversation: ConversationSession,
    composer: ComposerSession,
    settings: SettingsSession,
    revision: u64,
    serialized: serde_json::Value,
}
impl Default for ApplicationSession {
    fn default() -> Self {
        Self::new(&HarnessState::default())
    }
}
impl ApplicationSession {
    pub fn new(harness: &HarnessState) -> Self {
        let mut session = Self {
            navigation: NavigationState::default(),
            shell: ShellState::default(),
            agents: AgentSession::new(harness),
            conversation: ConversationSession::new(harness),
            composer: ComposerSession::new(harness),
            settings: SettingsSession::new(harness),
            revision: 0,
            serialized: serde_json::Value::Null,
        };
        session.serialized = session.fingerprint();
        session
    }
    pub fn navigation(&self) -> &NavigationState {
        &self.navigation
    }
    /// Transitional access while existing native panels migrate their field
    /// writes. Hosts call `refresh` after edits before publishing projections.
    pub fn navigation_mut(&mut self) -> &mut NavigationState {
        &mut self.navigation
    }
    pub fn agent_state(&self) -> &AgentState {
        self.agents.state()
    }
    /// Native geometry never enters this state. Refresh after direct semantic
    /// field edits so the shared selection and projection contract is restored.
    pub fn compatibility_agent_state_mut(&mut self) -> &mut AgentState {
        self.agents.compatibility_state_mut()
    }
    pub fn application_view(&self) -> ApplicationView {
        ApplicationView::from_navigation(&self.navigation)
    }
    pub fn shell_projection(&self) -> ShellProjection {
        ShellProjection {
            ui_revision: self.revision,
            state: self.shell.clone(),
            view: self.shell.view(),
        }
    }
    pub fn agents_projection(&self) -> AgentProjection {
        self.agents.projection()
    }
    pub fn projection(&self) -> ApplicationProjection {
        ApplicationProjection {
            ui_revision: self.revision,
            navigation: self.navigation.clone(),
            state: self.shell.clone(),
            view: self.shell.view(),
            application: self.application_view(),
            agents: self.agents.projection(),
            conversation: self.conversation.projection(),
            composer: self.composer.projection(),
            settings: self.settings.projection(),
        }
    }
    pub fn settings_projection(&self) -> SettingsProjection {
        self.settings.projection()
    }
    pub fn settings_state(&self) -> &SettingsState {
        self.settings.state()
    }
    pub fn settings_environment(&self) -> &SettingsEnvironment {
        self.settings.environment()
    }
    pub fn configure_settings(
        &mut self,
        environment: SettingsEnvironment,
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        self.settings.configure(environment, harness)?;
        self.advance();
        Some(self.projection())
    }
    pub fn prepare_settings(
        &self,
        action: SettingsAction,
        harness: &HarnessState,
    ) -> PreparedSettingsAction {
        self.settings.prepare(action, harness)
    }
    pub fn commit_settings(
        &mut self,
        prepared: PreparedSettingsAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, SettingsEffect) {
        let (_, effect) = self.settings.commit(prepared, harness);
        self.advance();
        (self.projection(), effect)
    }
    pub fn composer_projection(&self) -> ComposerProjection {
        self.composer.projection()
    }
    pub fn composer_environment(&self) -> &ComposerEnvironment {
        self.composer.environment()
    }
    pub fn configure_composer(
        &mut self,
        environment: ComposerEnvironment,
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        self.composer.configure(environment, harness)?;
        self.advance();
        Some(self.projection())
    }
    pub fn prepare_composer(
        &self,
        action: ComposerAction,
        harness: &HarnessState,
    ) -> PreparedComposerAction {
        self.composer.prepare(action, harness)
    }
    pub fn commit_composer(
        &mut self,
        prepared: PreparedComposerAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, ComposerEffect) {
        let (_, effect) = self.composer.commit(prepared, harness);
        self.advance();
        (self.projection(), effect)
    }
    pub fn observe_composer_new_session(&mut self, harness: &HarnessState) {
        let mut environment = self.composer.environment().clone();
        environment.context.unsaved_generation =
            environment.context.unsaved_generation.saturating_add(1);
        environment.context.session_id = None;
        self.composer.configure(environment, harness);
        self.advance();
    }
    pub fn conversation_state(&self) -> &ConversationState {
        self.conversation.state()
    }
    pub fn conversation_projection(&self) -> ConversationProjection {
        self.conversation.projection()
    }
    pub fn prepare_conversation(
        &self,
        action: ConversationAction,
        harness: &HarnessState,
    ) -> PreparedConversationAction {
        self.conversation.prepare(action, harness)
    }
    pub fn commit_conversation(
        &mut self,
        prepared: PreparedConversationAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, ConversationEffect) {
        let (_, effect) = self.conversation.commit(prepared, harness);
        self.advance();
        (self.projection(), effect)
    }
    /// Slice variants support hosts that keep the runtime transcript separately.
    pub fn prepare_conversation_entries(
        &self,
        action: ConversationAction,
        entries: &[crate::model::ConversationEntry],
        harness: &HarnessState,
    ) -> PreparedConversationAction {
        self.conversation.prepare_entries(action, entries, harness)
    }
    pub fn commit_conversation_entries(
        &mut self,
        prepared: PreparedConversationAction,
        entries: &[crate::model::ConversationEntry],
        harness: &HarnessState,
    ) -> (ApplicationProjection, ConversationEffect) {
        let (_, effect) = self.conversation.commit_entries(prepared, entries, harness);
        self.advance();
        (self.projection(), effect)
    }
    pub fn refresh_conversation_entries(
        &mut self,
        entries: &[crate::model::ConversationEntry],
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        self.conversation.refresh_entries(entries, harness)?;
        self.advance();
        Some(self.projection())
    }
    pub fn prepare_agent(
        &self,
        action: AgentAction,
        harness: &HarnessState,
    ) -> PreparedAgentAction {
        self.agents.prepare(action, harness)
    }
    /// Commit only after the host delivered the generated Harness command.
    pub fn commit_agent(
        &mut self,
        prepared: PreparedAgentAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, AgentEffect) {
        let action = prepared.action.clone();
        let (_, effect) = self.agents.commit(prepared, harness);
        match action {
            AgentAction::Open => {
                self.shell.apply(ShellAction::OpenAgents);
            }
            AgentAction::Close => {
                self.shell.apply(ShellAction::CloseAgents);
                self.leave_agents();
            }
            _ => {}
        }
        self.advance();
        (self.projection(), effect)
    }
    /// All hosts use this transition, including its agent-sheet interaction.
    /// The original shell channel remains a projection of this controller.
    pub fn apply_shell(
        &mut self,
        action: ShellAction,
        harness: &HarnessState,
    ) -> ApplicationProjection {
        let dismissed = self.shell.apply(action);
        let agents = match action {
            ShellAction::OpenAgents => Some(AgentAction::Open),
            ShellAction::CloseAgents => Some(AgentAction::Close),
            ShellAction::Dismiss if dismissed == Some(Surface::Agents) => Some(AgentAction::Close),
            _ => None,
        };
        if let Some(action) = agents {
            let prepared = self.agents.prepare(action, harness);
            self.agents.commit(prepared, harness);
            if !self.shell.agents {
                self.leave_agents();
            }
        }
        self.advance();
        self.projection()
    }
    /// The resource adapter first performs its surface operation, then reports
    /// the resulting inventory here. Stale or rejected identities cannot change
    /// navigation; filesystem loading and native widgets remain outside UI.
    pub fn accepted_navigation(
        &mut self,
        tab: WorkbenchTab,
        resources: &WorkbenchResources,
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        let accepted = match tab {
            WorkbenchTab::File(id) => {
                resources.active_file == Some(id)
                    && resources
                        .files
                        .as_ref()
                        .is_some_and(|files| files.contains(&id))
            }
            WorkbenchTab::Diff(id) => {
                resources.active_diff == Some(id) && resources.diffs.contains(&id)
            }
            WorkbenchTab::Files => resources.files.is_some(),
            WorkbenchTab::NewDiff => resources.active_diff.is_some(),
            WorkbenchTab::CloseFile(id) => resources
                .files
                .as_ref()
                .is_none_or(|files| !files.contains(&id)),
            WorkbenchTab::CloseDiff(id) => !resources.diffs.contains(&id),
            _ => true,
        };
        if !accepted {
            return None;
        }
        match tab {
            WorkbenchTab::CloseDiff(_) => self.navigation.diff_closed(resources.diffs.len()),
            WorkbenchTab::Agents => {
                self.shell.apply(ShellAction::OpenAgents);
                let prepared = self.agents.prepare(AgentAction::Open, harness);
                self.agents.commit(prepared, harness);
                self.navigation.activated(tab);
            }
            _ => {
                if matches!(
                    tab,
                    WorkbenchTab::Home
                        | WorkbenchTab::Session
                        | WorkbenchTab::Files
                        | WorkbenchTab::File(_)
                        | WorkbenchTab::Diff(_)
                        | WorkbenchTab::NewDiff
                ) {
                    self.shell.apply(ShellAction::CloseAgents);
                }
                self.navigation.activated(tab);
            }
        }
        self.advance();
        Some(self.projection())
    }
    /// Reconcile runtime updates without choosing a new navigation target.
    pub fn refresh(&mut self, harness: &HarnessState) -> Option<ApplicationProjection> {
        if let Some(entries) = harness.conversation.as_deref() {
            return self.refresh_with_conversation_entries(entries, harness);
        }
        // Sparse transport states carry no replacement transcript. Hosts that
        // keep entries separately use the explicit slice variant below.
        self.agents.refresh(harness);
        self.composer.refresh(harness);
        self.settings.refresh(harness);
        if self.fingerprint() != self.serialized {
            self.advance();
            Some(self.projection())
        } else {
            None
        }
    }
    pub fn refresh_with_conversation_entries(
        &mut self,
        entries: &[crate::model::ConversationEntry],
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        self.agents.refresh(harness);
        self.composer.refresh(harness);
        self.settings.refresh(harness);
        self.conversation.refresh_entries(entries, harness);
        if self.fingerprint() != self.serialized {
            self.advance();
            Some(self.projection())
        } else {
            None
        }
    }
    fn leave_agents(&mut self) {
        if self.navigation.mode == super::navigation::Screen::Agents {
            self.navigation.activated(WorkbenchTab::Home);
        }
    }
    fn advance(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.serialized = self.fingerprint();
    }
    fn fingerprint(&self) -> serde_json::Value {
        serde_json::to_value((
            &self.navigation,
            &self.shell,
            self.agents.projection().view,
            self.conversation.projection().view,
            self.composer.projection().view,
            self.settings.projection().view,
        ))
        .expect("serialize application UI")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::{application::Content, surfaces::Tabs};
    #[test]
    fn shell_dismiss_and_runtime_refresh_share_agent_intent_without_navigating() {
        let mut harness = HarnessState::default();
        let mut session = ApplicationSession::new(&harness);
        session.apply_shell(ShellAction::OpenAgents, &harness);
        assert!(session.shell_projection().state.agents);
        assert!(session.agent_state().open);
        let prepared = session.prepare_agent(AgentAction::CreateGroup, &harness);
        session.commit_agent(prepared, &harness);
        let before = session.projection();
        let failed = session.prepare_agent(AgentAction::SubmitDraft("objective".into()), &harness);
        assert!(failed.effect.command.is_some());
        drop(failed);
        assert!(session.agent_state().creating_group);
        assert_eq!(session.projection().ui_revision, before.ui_revision);
        harness.agent_group.objective = Some("Background objective".into());
        let refreshed = session.refresh(&harness).unwrap();
        assert_eq!(refreshed.application.content, Content::Home);
        assert!(refreshed.agents.view.state.creating_group);
        assert!(session.refresh(&harness).is_none());
        session.apply_shell(ShellAction::OpenModels, &harness);
        session.apply_shell(ShellAction::Dismiss, &harness);
        assert!(
            session.agent_state().open,
            "dismissing Models must preserve Agents"
        );
        let dismissed = session.apply_shell(ShellAction::Dismiss, &harness);
        assert!(!dismissed.state.agents && !dismissed.agents.view.state.open);
        let explicit_open = session.prepare_agent(AgentAction::Open, &harness);
        let (opened, _) = session.commit_agent(explicit_open, &harness);
        assert!(opened.state.agents && opened.agents.view.state.open);
        let explicit_close = session.prepare_agent(AgentAction::Close, &harness);
        let (closed, _) = session.commit_agent(explicit_close, &harness);
        assert!(!closed.state.agents && !closed.agents.view.state.open);
    }
    #[test]
    fn resource_identity_and_legacy_edits_are_reconciled_by_one_controller() {
        let harness = HarnessState::default();
        let mut session = ApplicationSession::new(&harness);
        let mut tabs = Tabs::default();
        let first = tabs.open("same", ());
        let second = tabs.open("same", ());
        let mut resources = WorkbenchResources {
            diffs: vec![first, second],
            active_diff: Some(second),
            ..Default::default()
        };
        assert!(
            session
                .accepted_navigation(WorkbenchTab::Diff(first), &resources, &harness)
                .is_none()
        );
        let projection = session
            .accepted_navigation(WorkbenchTab::Diff(second), &resources, &harness)
            .unwrap();
        assert_eq!(projection.application.content, Content::Diff);
        tabs.close(first);
        resources.diffs = vec![second];
        session.accepted_navigation(WorkbenchTab::CloseDiff(first), &resources, &harness);
        assert_eq!(session.application_view().content, Content::Diff);
        tabs.close(second);
        resources.diffs.clear();
        resources.active_diff = None;
        session.accepted_navigation(WorkbenchTab::CloseDiff(second), &resources, &harness);
        assert_eq!(session.application_view().content, Content::Home);
        session.navigation_mut().activated(WorkbenchTab::Session);
        assert_eq!(
            session.refresh(&harness).unwrap().application.content,
            Content::Session
        );
        assert!(session.refresh(&harness).is_none());
    }
}
