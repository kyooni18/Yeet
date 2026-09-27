//! Owns lazy provider-bridge startup, reuse, restart, and shutdown for tools.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::core::{
    AuthStatus, BridgeClient, BridgeEvent, CallRequest, CallResult, HarnessCapabilityDescriptor,
    McpServerStatus, ModelInfo, OpenAiCompatibleProvider, ProviderUsageStatus, SkillSummary,
};

#[derive(Clone)]
pub(crate) struct BridgeHandle {
    inner: Arc<Mutex<BridgeHandleState>>,
}

struct BridgeHandleState {
    client: Option<BridgeClient>,
    workspace_root: Option<PathBuf>,
    openai_flex: bool,
}

impl BridgeHandle {
    pub(super) fn eager(bridge: BridgeClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeHandleState {
                client: Some(bridge),
                workspace_root: None,
                openai_flex: false,
            })),
        }
    }

    pub(crate) fn lazy() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeHandleState {
                client: None,
                workspace_root: None,
                openai_flex: false,
            })),
        }
    }

    pub(crate) fn lazy_for_workspace(workspace_root: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeHandleState {
                client: None,
                workspace_root: Some(workspace_root),
                openai_flex: false,
            })),
        }
    }

    pub(crate) fn client(&self) -> Result<BridgeClient> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow!("provider bridge slot poisoned"))?;
        if state.client.is_none() {
            let bridge = if let Some(workspace_root) = state.workspace_root.as_deref() {
                BridgeClient::start_for_workspace(workspace_root)?
            } else {
                BridgeClient::start()?
            };
            bridge.set_openai_flex(state.openai_flex);
            state.client = Some(bridge);
        }
        Ok(state
            .client
            .as_ref()
            .expect("provider bridge initialized")
            .clone())
    }

    pub(crate) fn existing_client(&self) -> Option<BridgeClient> {
        self.inner
            .lock()
            .ok()
            .and_then(|state| state.client.as_ref().cloned())
    }

    pub(crate) fn set_openai_flex(&self, enabled: bool) {
        let client = self.inner.lock().ok().and_then(|mut state| {
            state.openai_flex = enabled;
            state.client.as_ref().cloned()
        });
        if let Some(client) = client {
            client.set_openai_flex(enabled);
        }
    }

    pub(crate) fn interrupt_active_requests(&self) {
        if let Some(client) = self.existing_client() {
            client.interrupt_active_requests();
        }
    }

    pub(crate) fn context_length(&self, model: &str) -> Result<Option<u64>> {
        self.client()?.context_length(model)
    }

    pub(crate) fn auth_status(&self, provider: &str) -> Result<AuthStatus> {
        self.client()?.auth_status(provider)
    }

    pub(crate) fn complete_cancellable(
        &self,
        request: &CallRequest,
        cancel: &AtomicBool,
    ) -> Result<CallResult> {
        self.client()?.complete_cancellable(request, cancel)
    }

    pub(crate) fn restart(&self) -> Result<()> {
        self.client()?.restart()
    }

    pub(crate) fn list_harness_capabilities(&self) -> Result<Vec<HarnessCapabilityDescriptor>> {
        self.client()?.list_harness_capabilities()
    }

    pub(crate) fn list_skills(&self) -> Result<Vec<SkillSummary>> {
        self.client()?.list_skills()
    }

    pub(crate) fn list_mcp_servers(&self) -> Result<Vec<McpServerStatus>> {
        self.client()?.list_mcp_servers()
    }

    pub(crate) fn provider_usage(&self, provider: &str) -> Result<ProviderUsageStatus> {
        self.client()?.provider_usage(provider)
    }

    pub(crate) fn list_providers(&self) -> Result<Vec<String>> {
        self.client()?.list_providers()
    }

    pub(crate) fn list_model_info(&self, provider: &str) -> Result<Vec<ModelInfo>> {
        self.client()?.list_model_info(provider)
    }

    pub(crate) fn login_browser(
        &self,
        provider: &str,
        options: Option<Value>,
    ) -> Result<AuthStatus> {
        self.client()?.login_browser(provider, options)
    }

    pub(crate) fn logout(&self, provider: &str) -> Result<AuthStatus> {
        self.client()?.logout(provider)
    }

    pub(crate) fn set_api_key(&self, provider: &str, key: &str) -> Result<AuthStatus> {
        self.client()?.set_api_key(provider, key)
    }

    pub(crate) fn list_provider_configurations(&self) -> Result<Vec<OpenAiCompatibleProvider>> {
        self.client()?.list_provider_configurations()
    }

    pub(crate) fn save_provider_configuration(
        &self,
        provider: &OpenAiCompatibleProvider,
    ) -> Result<OpenAiCompatibleProvider> {
        self.client()?.save_provider_configuration(provider)
    }

    pub(crate) fn remove_provider_configuration(&self, provider: &str) -> Result<bool> {
        self.client()?.remove_provider_configuration(provider)
    }

    pub(crate) fn try_recv_event(&self) -> Option<BridgeEvent> {
        self.existing_client()?.try_recv_event()
    }

    pub(crate) fn send_native_app_approval_decision(
        &self,
        request_id: &str,
        approved: bool,
    ) -> Result<()> {
        self.client()?
            .send_native_app_approval_decision(request_id, approved)
    }

    pub(crate) fn shutdown(&self) {
        let bridge = self
            .inner
            .lock()
            .ok()
            .and_then(|mut state| state.client.take());
        if let Some(bridge) = bridge {
            bridge.shutdown();
        }
    }

    #[cfg(test)]
    pub(crate) fn is_started(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.client.is_some())
            .unwrap_or(false)
    }
}
