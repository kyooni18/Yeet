//! Per-application composer projection and delivery-safe interaction checkpoints.
use super::composer::*;
use crate::harness::HarnessState;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerProjection {
    pub composer_revision: u64,
    pub view: ComposerView,
}
pub struct PreparedComposerAction {
    state: ComposerState,
    pub effect: ComposerEffect,
}
pub struct ComposerSession {
    state: ComposerState,
    environment: ComposerEnvironment,
    projection: ComposerProjection,
    serialized: serde_json::Value,
}
impl ComposerSession {
    pub fn new(harness: &HarnessState) -> Self {
        let state = ComposerState::default();
        let environment = ComposerEnvironment::default();
        let view = state.view(&environment, harness);
        let serialized = serde_json::to_value(&view).expect("serialize composer UI");
        Self {
            state,
            environment,
            projection: ComposerProjection {
                composer_revision: 0,
                view,
            },
            serialized,
        }
    }
    pub fn environment(&self) -> &ComposerEnvironment {
        &self.environment
    }
    pub fn projection(&self) -> ComposerProjection {
        self.projection.clone()
    }
    pub fn configure(
        &mut self,
        environment: ComposerEnvironment,
        harness: &HarnessState,
    ) -> Option<ComposerProjection> {
        self.environment = environment;
        self.state.reconcile(&self.environment);
        self.refresh(harness)
    }
    pub fn prepare(
        &self,
        action: ComposerAction,
        harness: &HarnessState,
    ) -> PreparedComposerAction {
        let mut state = self.state.clone();
        let effect = state.apply(action, &self.environment, harness);
        PreparedComposerAction { state, effect }
    }
    /// The host must deliver effect.command before accepting this checkpoint.
    pub fn commit(
        &mut self,
        prepared: PreparedComposerAction,
        harness: &HarnessState,
    ) -> (ComposerProjection, ComposerEffect) {
        if prepared.state.editor.context == self.environment.context
            && prepared.state.editor.revision >= self.state.editor.revision
        {
            self.state = prepared.state;
        }
        self.publish(harness, true);
        (self.projection(), prepared.effect)
    }
    pub fn refresh(&mut self, harness: &HarnessState) -> Option<ComposerProjection> {
        self.publish(harness, false).then(|| self.projection())
    }
    fn publish(&mut self, harness: &HarnessState, force: bool) -> bool {
        let view = self.state.view(&self.environment, harness);
        let serialized = serde_json::to_value(&view).expect("serialize composer UI");
        if !force && self.serialized == serialized {
            return false;
        }
        self.serialized = serialized;
        self.projection.view = view;
        self.projection.composer_revision = self.projection.composer_revision.saturating_add(1);
        true
    }
}
