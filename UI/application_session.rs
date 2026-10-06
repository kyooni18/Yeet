//! One application controller for native and Remote hosts.
//! Owns navigation, shell placement and agent interaction state. Harness event
//! cursors stay in Harness/transport; these revisions describe only UI intent.
use super::toolbar::*;
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
    home::{HomeAction, HomeState, HomeView, ResourceTarget, WorkspaceContent},
    navigation::{NavigationState, WorkbenchResources},
    settings::{SettingsAction, SettingsEffect, SettingsEnvironment, SettingsState},
    settings_session::{PreparedSettingsAction, SettingsProjection, SettingsSession},
    shell::{ShellAction, ShellState, ShellView, Surface},
    workbench::WorkbenchTab,
};
use crate::harness::HarnessState;
pub struct PreparedToolbarAction {
    pub effect: ToolbarEffect,
    route: ToolbarRoute,
}
/// Effects that a platform host performs after preparing a Home interaction.
/// Resource opening stays outside shared UI; session creation uses Harness.
#[derive(Debug, Clone, Default)]
pub struct HomeEffect {
    pub command: Option<crate::harness::HarnessCommand>,
    pub open: Option<ResourceTarget>,
}
pub struct PreparedHomeAction {
    state: HomeState,
    composer: Option<PreparedComposerAction>,
    pub effect: HomeEffect,
}
enum ToolbarRoute {
    None,
    Shell(ShellAction),
    Composer(PreparedComposerAction),
}
use serde::{Deserialize, Serialize};

/// Existing shell channel shape, retained during adapter migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellProjection {
    pub toolbar: ToolbarView,
    pub ui_revision: u64,
    pub state: ShellState,
    pub view: ShellView,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationProjection {
    pub toolbar: ToolbarView,
    pub ui_revision: u64,
    pub home_revision: u64,
    pub navigation: NavigationState,
    pub state: ShellState,
    pub view: ShellView,
    pub application: ApplicationView,
    pub home: HomeView,
    pub agents: AgentProjection,
    pub conversation: ConversationProjection,
    pub composer: ComposerProjection,
    pub settings: SettingsProjection,
}
pub struct ApplicationSession {
    toolbar: ToolbarRuntime,
    navigation: NavigationState,
    shell: ShellState,
    agents: AgentSession,
    conversation: ConversationSession,
    composer: ComposerSession,
    settings: SettingsSession,
    home: HomeState,
    revision: u64,
    home_revision: u64,
    home_serialized: HomeView,
    serialized: serde_json::Value,
}
impl Default for ApplicationSession {
    fn default() -> Self {
        Self::new(&HarnessState::default())
    }
}
impl ApplicationSession {
    pub fn new(harness: &HarnessState) -> Self {
        let mut toolbar = ToolbarRuntime::default();
        toolbar.update(harness);
        let mut session = Self {
            toolbar,
            navigation: NavigationState::default(),
            shell: ShellState::default(),
            agents: AgentSession::new(harness),
            conversation: ConversationSession::new(harness),
            composer: ComposerSession::new(harness),
            settings: SettingsSession::new(harness),
            home: HomeState::default(),
            revision: 0,
            home_revision: 0,
            home_serialized: HomeState::default().project(true),
            serialized: serde_json::Value::Null,
        };
        session.serialized = session.fingerprint();
        session
    }
    pub fn toolbar_view(&self) -> ToolbarView {
        self.toolbar.view(
            &self.shell,
            &self.composer.projection().view,
            self.composer.environment().available,
        )
    }
    pub fn toolbar_view_for(&self, harness: &HarnessState) -> ToolbarView {
        let mut runtime = ToolbarRuntime::default();
        runtime.update(harness);
        runtime.view(
            &self.shell,
            &self.composer.projection().view,
            self.composer.environment().available,
        )
    }
    pub fn prepare_toolbar(
        &self,
        action: ToolbarAction,
        harness: &HarnessState,
    ) -> PreparedToolbarAction {
        let mut runtime = ToolbarRuntime::default();
        runtime.update(harness);
        let view = runtime.view(
            &self.shell,
            &self.composer.projection().view,
            self.composer.environment().available,
        );
        let (id, choice) = match action {
            ToolbarAction::Activate(id) => (id, None),
            ToolbarAction::Choose { id, value } => (id, Some(value)),
            ToolbarAction::ChooseWorkspace {
                id,
                source_workspace,
            } => {
                let mut prepared = PreparedToolbarAction {
                    effect: ToolbarEffect::default(),
                    route: ToolbarRoute::None,
                };
                if source_workspace != view.workspace_source {
                    return prepared;
                }
                let Some(workspace) = view
                    .workspace_choices
                    .iter()
                    .find(|workspace| workspace.id == id)
                else {
                    return prepared;
                };
                if !workspace.selected {
                    prepared.effect.workspace_switch = Some(WorkspaceSwitch {
                        id: workspace.id.clone(),
                        path: workspace.path.clone(),
                        source_workspace,
                    });
                }
                return prepared;
            }
        };
        let mut prepared = PreparedToolbarAction {
            effect: ToolbarEffect::default(),
            route: ToolbarRoute::None,
        };
        let Some(control) = view
            .groups
            .iter()
            .flat_map(|g| &g.controls)
            .chain(view.sandbox_controls.iter())
            .find(|c| c.id == id && c.enabled)
        else {
            return prepared;
        };
        if let Some(value) = choice {
            if id == "sandbox_preset" && control.options.iter().any(|option| option.value == value)
            {
                prepared.effect.command = Some(crate::harness::HarnessCommand::UpdateSandbox {
                    action: crate::model::SandboxAction::ApplyPreset {
                        preset: value.clone(),
                    },
                });
            }
            if id == "auto_approve" && control.options.iter().any(|option| option.value == value) {
                prepared.effect.command = Some(crate::harness::HarnessCommand::UpdateSandbox {
                    action: crate::model::SandboxAction::SetAutoApprove {
                        enabled: value == "true",
                    },
                });
            }
            if id == "reasoning" && control.options.iter().any(|o| o.value == value) {
                prepared.effect.command = Some(crate::harness::HarnessCommand::SelectReasoning {
                    level: value.clone(),
                });
            }
            if id == "goal" && control.options.iter().any(|o| o.value == value) {
                prepared.effect.command = Some(crate::harness::HarnessCommand::SetGoal {
                    enabled: value == "true",
                });
            }
            return prepared;
        }
        use super::composer::ComposerDestination as Destination;
        match id.as_str() {
            "auto_approve" => {
                prepared.effect.command = Some(crate::harness::HarnessCommand::UpdateSandbox {
                    action: crate::model::SandboxAction::SetAutoApprove {
                        enabled: !harness
                            .sandbox_settings
                            .as_ref()
                            .is_some_and(|settings| settings.auto_approve),
                    },
                });
            }
            "new_session" | "interrupt" => {
                let action = if id == "new_session" {
                    ComposerAction::NewSession
                } else {
                    ComposerAction::Interrupt
                };
                let composer = self.prepare_composer(action, harness);
                prepared.effect.command = composer.effect.command.clone();
                prepared.route = ToolbarRoute::Composer(composer);
            }
            "navigation" => {
                prepared.route = ToolbarRoute::Shell(if self.shell.navigation {
                    ShellAction::CloseNavigation
                } else {
                    ShellAction::OpenNavigation
                })
            }
            "quick_controls" => prepared.route = ToolbarRoute::Shell(ShellAction::ToggleInspector),
            "models" => {
                prepared.route = ToolbarRoute::Shell(ShellAction::OpenModels);
                prepared.effect.destination = Some(Destination::Models);
            }
            "settings" => {
                prepared.route = ToolbarRoute::Shell(ShellAction::OpenSettings);
                prepared.effect.destination = Some(Destination::Settings);
            }
            "reasoning" => prepared.effect.destination = Some(Destination::Reasoning),
            "sessions" => prepared.effect.destination = Some(Destination::Sessions),
            "files" => prepared.effect.destination = Some(Destination::Files),
            "capabilities" => prepared.effect.destination = Some(Destination::Capabilities),
            "goal" => {
                prepared.effect.command = Some(crate::harness::HarnessCommand::SetGoal {
                    enabled: !harness.goal_mode,
                })
            }
            _ => {}
        }
        prepared
    }
    pub fn commit_toolbar(
        &mut self,
        prepared: PreparedToolbarAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, ToolbarUiEffect) {
        let new_session = matches!(
            prepared.effect.command,
            Some(crate::harness::HarnessCommand::NewSession)
        );
        match prepared.route {
            ToolbarRoute::None => {}
            ToolbarRoute::Shell(action) => {
                self.apply_shell(action, harness);
            }
            ToolbarRoute::Composer(composer) => {
                self.commit_composer(composer, harness);
            }
        }
        if new_session {
            self.observe_composer_new_session(harness);
        }
        self.toolbar.update(harness);
        self.advance();
        (
            self.projection(),
            ToolbarUiEffect {
                destination: prepared.effect.destination,
            },
        )
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
            toolbar: self.toolbar_view(),
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
            toolbar: self.toolbar_view(),
            ui_revision: self.revision,
            home_revision: self.home_revision,
            navigation: self.navigation.clone(),
            state: self.shell.clone(),
            view: self.shell.view(),
            application: self.application_view(),
            home: self.home.project(true),
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
        self.toolbar.update(harness);
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
    /// Prepare a validated Home action. Hosts deliver `effect.command` or
    /// perform `effect.open` before calling `commit_home`.
    pub fn prepare_home(&self, action: HomeAction, harness: &HarnessState) -> PreparedHomeAction {
        let mut state = self.home.clone();
        let mut composer = None;
        let mut effect = HomeEffect::default();
        if let Some(action) = self.home.validate_action(action) {
            match action {
                HomeAction::Select(target) => state.selected = Some(target),
                HomeAction::Open(target) => effect.open = Some(target),
                HomeAction::NewSession => {
                    let prepared = self.prepare_composer(ComposerAction::NewSession, harness);
                    effect.command = prepared.effect.command.clone();
                    composer = Some(prepared);
                }
            }
        }
        PreparedHomeAction {
            state,
            composer,
            effect,
        }
    }
    /// Commit only after a host has performed any returned effect. A failed
    /// Harness delivery or resource open leaves the prepared state unapplied.
    pub fn commit_home(
        &mut self,
        prepared: PreparedHomeAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, HomeEffect) {
        let home_changed = self.home.project(true) != prepared.state.project(true);
        let new_session = matches!(
            prepared.effect.command,
            Some(crate::harness::HarnessCommand::NewSession)
        );
        if let Some(composer) = prepared.composer {
            self.commit_composer(composer, harness);
        }
        self.home = prepared.state;
        if new_session {
            self.observe_composer_new_session(harness);
        } else if home_changed {
            self.advance();
        }
        (self.projection(), prepared.effect)
    }
    /// Replace Home's Harness-derived inventory without moving filesystem or
    /// Git collection into UI. Returns a new projection only when it changed.
    pub fn update_home_content(
        &mut self,
        content: WorkspaceContent,
    ) -> Option<ApplicationProjection> {
        self.home.replace_content(content);
        if self.fingerprint() != self.serialized {
            self.advance();
            Some(self.projection())
        } else {
            None
        }
    }
    pub fn commit_settings(
        &mut self,
        prepared: PreparedSettingsAction,
        harness: &HarnessState,
    ) -> (ApplicationProjection, SettingsEffect) {
        let (_, effect) = self.settings.commit(prepared, harness);
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
        self.advance();
        (self.projection(), effect)
    }
    pub fn observe_composer_new_session(&mut self, harness: &HarnessState) {
        let mut environment = self.composer.environment().clone();
        environment.context.unsaved_generation =
            environment.context.unsaved_generation.saturating_add(1);
        environment.context.session_id = None;
        self.composer.configure(environment, harness);
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
        self.advance();
        (self.projection(), effect)
    }
    pub fn refresh_conversation_entries(
        &mut self,
        entries: &[crate::model::ConversationEntry],
        harness: &HarnessState,
    ) -> Option<ApplicationProjection> {
        self.conversation.refresh_entries(entries, harness)?;
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
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
        self.toolbar.update(harness);
        self.agents.refresh(harness);
        self.composer.refresh(harness);
        self.settings.refresh(harness);
        if self.fingerprint() != self.serialized {
            self.toolbar.update(harness);
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
        self.toolbar.update(harness);
        self.agents.refresh(harness);
        self.composer.refresh(harness);
        self.settings.refresh(harness);
        self.conversation.refresh_entries(entries, harness);
        if self.fingerprint() != self.serialized {
            self.toolbar.update(harness);
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
        let home = self.home.project(true);
        if home != self.home_serialized {
            self.home_revision = self.home_revision.saturating_add(1);
            self.home_serialized = home;
        }
        self.revision = self.revision.saturating_add(1);
        self.serialized = self.fingerprint();
    }
    fn fingerprint(&self) -> serde_json::Value {
        serde_json::to_value((
            self.toolbar_view(),
            &self.navigation,
            &self.shell,
            self.agents.projection().view,
            self.conversation.projection().view,
            self.composer.projection().view,
            self.settings.projection().view,
            self.home.project(true),
        ))
        .expect("serialize application UI")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::{
        application::Content,
        home::{ResourceItem, ResourceKind},
        surfaces::Tabs,
    };
    #[test]
    fn home_content_projects_and_advances_only_when_the_view_changes() {
        let mut session = ApplicationSession::default();
        let mut content = WorkspaceContent {
            workspace: "/workspace/yeet".into(),
            branch: "main".into(),
            ..Default::default()
        };
        content.sessions.push(ResourceItem::new(
            ResourceTarget::Session("session-1".into()),
            ResourceKind::Session,
            "First session",
        ));

        let projection = session.update_home_content(content.clone()).unwrap();
        assert_eq!(projection.ui_revision, 1);
        assert_eq!(projection.home_revision, 1);
        assert_eq!(
            projection.home.summary,
            "yeet  ·  main  ·  working tree clean"
        );
        assert_eq!(projection.home.recent.len(), 1);
        assert_eq!(projection.home.recent[0].title, "First session");
        assert_eq!(
            projection.home.selected,
            projection
                .home
                .recent
                .first()
                .map(|item| item.target.clone())
        );
        assert!(session.update_home_content(content).is_none());
        let shell = session.apply_shell(ShellAction::OpenModels, &HarnessState::default());
        assert_eq!(shell.home_revision, 1);
    }

    #[test]
    fn home_actions_reject_stale_targets_and_return_valid_open_intent() {
        let mut session = ApplicationSession::default();
        let harness = HarnessState::default();
        let stale = ResourceTarget::Session("removed-session".into());
        let stale_action = session.prepare_home(HomeAction::Open(stale), &harness);
        assert!(stale_action.effect.open.is_none());
        let (projection, effect) = session.commit_home(stale_action, &harness);
        assert!(effect.open.is_none());
        assert_eq!(projection.ui_revision, 0);

        let target = ResourceTarget::File("/workspace/yeet/src/lib.rs".into());
        let mut content = WorkspaceContent::default();
        content.recent_views.push(ResourceItem::new(
            target.clone(),
            ResourceKind::File,
            "src/lib.rs",
        ));
        let projection = session.update_home_content(content).unwrap();
        assert_eq!(projection.ui_revision, 1);
        let selected = session.prepare_home(HomeAction::Select(target.clone()), &harness);
        let (projection, effect) = session.commit_home(selected, &harness);
        assert!(effect.open.is_none());
        assert_eq!(projection.home.selected, Some(target.clone()));
        assert_eq!(projection.ui_revision, 1);

        let opening = session.prepare_home(HomeAction::Open(target.clone()), &harness);
        assert_eq!(opening.effect.open, Some(target.clone()));
        let (projection, effect) = session.commit_home(opening, &harness);
        assert_eq!(effect.open, Some(target));
        assert_eq!(projection.ui_revision, 1);
    }

    #[test]
    fn home_new_session_exposes_composer_command_before_commit() {
        let mut session = ApplicationSession::default();
        let harness = HarnessState::default();
        let before = session.projection().ui_revision;

        let pending = session.prepare_home(HomeAction::NewSession, &harness);
        assert!(matches!(
            pending.effect.command,
            Some(crate::harness::HarnessCommand::NewSession)
        ));
        assert_eq!(session.projection().ui_revision, before);
        drop(pending); // A failed host delivery never commits the prepared UI.
        assert_eq!(session.projection().ui_revision, before);

        let prepared = session.prepare_home(HomeAction::NewSession, &harness);
        let (projection, effect) = session.commit_home(prepared, &harness);
        assert!(matches!(
            effect.command,
            Some(crate::harness::HarnessCommand::NewSession)
        ));
        assert!(projection.ui_revision > before);
    }
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
