//! Per-application settings projection and delivery-safe interaction checkpoints.
use super::settings::*;
use crate::harness::HarnessState;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsProjection {
    pub settings_revision: u64,
    pub view: SettingsView,
}
pub struct PreparedSettingsAction {
    state: SettingsState,
    pub effect: SettingsEffect,
}
pub struct SettingsSession {
    state: SettingsState,
    environment: SettingsEnvironment,
    projection: SettingsProjection,
    serialized: serde_json::Value,
}
impl SettingsSession {
    pub fn new(harness: &HarnessState) -> Self {
        let state = SettingsState::default();
        let environment = SettingsEnvironment::default();
        let view = state.view(&environment, harness);
        let serialized = serde_json::to_value(&view).expect("serialize settings UI");
        Self {
            state,
            environment,
            projection: SettingsProjection {
                settings_revision: 0,
                view,
            },
            serialized,
        }
    }
    pub fn environment(&self) -> &SettingsEnvironment {
        &self.environment
    }
    pub fn projection(&self) -> SettingsProjection {
        self.projection.clone()
    }
    pub fn configure(
        &mut self,
        environment: SettingsEnvironment,
        harness: &HarnessState,
    ) -> Option<SettingsProjection> {
        self.environment = environment;
        self.state.reconcile(&self.environment, harness);
        self.refresh(harness)
    }
    pub fn state(&self) -> &SettingsState {
        &self.state
    }
    pub fn prepare(
        &self,
        action: SettingsAction,
        harness: &HarnessState,
    ) -> PreparedSettingsAction {
        let mut state = self.state.clone();
        let effect = state.apply(action, &self.environment, harness);
        PreparedSettingsAction { state, effect }
    }
    /// The host must deliver effect.command before accepting this checkpoint.
    pub fn commit(
        &mut self,
        prepared: PreparedSettingsAction,
        harness: &HarnessState,
    ) -> (SettingsProjection, SettingsEffect) {
        self.state = prepared.state;
        self.publish(harness, true);
        (self.projection(), prepared.effect)
    }
    pub fn refresh(&mut self, harness: &HarnessState) -> Option<SettingsProjection> {
        self.publish(harness, false).then(|| self.projection())
    }
    fn publish(&mut self, harness: &HarnessState, force: bool) -> bool {
        self.state.reconcile(&self.environment, harness);
        let view = self.state.view(&self.environment, harness);
        let serialized = serde_json::to_value(&view).expect("serialize settings UI");
        if !force && self.serialized == serialized {
            return false;
        }
        self.serialized = serialized;
        self.projection.view = view;
        self.projection.settings_revision = self.projection.settings_revision.saturating_add(1);
        true
    }
}
