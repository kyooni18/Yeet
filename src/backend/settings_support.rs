//! Pure configuration adapters used by backend settings commands.
//!
//! Authentication/provider refresh, Foundation readiness, and sandbox policy
//! conversions live here so the command-facing settings module stays small.

use super::*;

/// Loads runtime authentication status for every known provider.
pub(super) fn load_auth_providers(bridge: &BridgeClient) -> Result<Vec<AuthProviderItem>> {
    let mut providers = bridge.list_providers()?;
    providers.sort();
    Ok(providers
        .into_iter()
        .map(|provider| match bridge.auth_status(&provider) {
            Ok(status) => AuthProviderItem {
                provider,
                authenticated: status.authenticated,
                method: status.method,
                expires_at: status.expires_at,
                error: None,
            },
            Err(error) => AuthProviderItem {
                provider,
                authenticated: false,
                method: String::new(),
                expires_at: None,
                error: Some(error.to_string()),
            },
        })
        .collect())
}

/// Builds provider-specific browser-login options from environment variables.
pub(super) fn auth_login_options(provider: &str) -> Option<Value> {
    if provider != "gemini" {
        return None;
    }
    let client_id = std::env::var("GEMINI_OAUTH_CLIENT_ID")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLIENT_ID").ok());
    let client_secret = std::env::var("GEMINI_OAUTH_CLIENT_SECRET")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLIENT_SECRET").ok());
    let project_id = std::env::var("GEMINI_PROJECT_ID")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok());
    Some(json!({"clientId":client_id,"clientSecret":client_secret,"projectId":project_id}))
}

/// Completes an asynchronous auth mutation and refreshes UI state.
pub(super) fn finish_auth_action(
    bridge: &BridgeClient,
    shared: &Arc<Mutex<SharedSession>>,
    tx: &Sender<BackendEvent>,
    action_result: Result<String>,
) {
    let refresh = load_auth_providers(bridge);
    if let Ok(mut state) = shared.lock() {
        state.state.auth_working = false;
        if let Ok(items) = refresh.as_ref() {
            state.state.auth_providers = items.clone();
        }
        state.state.auth_notice = Some(match (action_result, refresh) {
            (Ok(message), Ok(_)) => message,
            (Err(error), _) => format!("Authentication error: {error}"),
            (Ok(message), Err(error)) => format!("{message} Refresh failed: {error}"),
        });
        let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
    }
}

/// Loads persisted OpenAI-compatible provider configurations for the UI.
pub(super) fn load_provider_configurations(
    bridge: &BridgeClient,
) -> Result<Vec<ProviderConfigurationItem>> {
    let mut items = bridge
        .list_provider_configurations()?
        .into_iter()
        .map(|provider| ProviderConfigurationItem {
            id: provider.id,
            base_url: provider.base_url,
            require_api_key: provider.require_api_key.unwrap_or(false),
            header_count: provider.headers.as_ref().map_or(0, HashMap::len),
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.id.to_ascii_lowercase());
    Ok(items)
}

/// Completes a provider mutation and refreshes provider configuration state.
pub(super) fn finish_provider_action(
    bridge: &BridgeClient,
    shared: &Arc<Mutex<SharedSession>>,
    tx: &Sender<BackendEvent>,
    action_result: Result<String>,
) {
    let refresh = load_provider_configurations(bridge);
    if let Ok(mut state) = shared.lock() {
        state.state.providers_working = false;
        if let Ok(items) = refresh.as_ref() {
            state.state.provider_configurations = items.clone();
        }
        state.state.providers_notice = Some(match (action_result, refresh) {
            (Ok(message), Ok(_)) => message,
            (Err(error), _) => format!("Provider error: {error}"),
            (Ok(message), Err(error)) => format!("{message} Refresh failed: {error}"),
        });
        let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
    }
}

/// Reports whether the configured Foundation memory store can be opened.
pub(super) fn foundation_server_ready(_bridge: &BridgeClient, _server: &str) -> bool {
    crate::memory::MemoryStore::default().status().is_ok()
}

/// Converts a sandbox policy into its bridge/UI representation.
pub(super) fn sandbox_settings_state(policy: &SandboxPolicy) -> SandboxSettingsState {
    let (workspace_mode, workspace_paths) = match &policy.workspace_read {
        WorkspaceRead::None => ("none".into(), Vec::new()),
        WorkspaceRead::All => ("all".into(), Vec::new()),
        WorkspaceRead::Paths(paths) => ("paths".into(), paths.iter().cloned().collect()),
    };
    SandboxSettingsState {
        preset: detect_sandbox_preset(policy).into(),
        execution_mode: match policy.mode {
            SandboxMode::Sandboxed => "sandboxed".into(),
            SandboxMode::Unlimited => "unlimited".into(),
        },
        auto_approve: policy.auto_approve,
        workspace_mode,
        workspace_paths,
        scratch_writable: policy.scratch_writable,
        network_allow: policy
            .network_allow
            .iter()
            .map(|endpoint| SandboxNetworkItem {
                host: endpoint.host.clone(),
                port: endpoint.port,
            })
            .collect(),
        environment: policy
            .environment
            .iter()
            .map(|(key, value)| SandboxEnvironmentItem {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
        secret_ids: policy.secret_ids.iter().cloned().collect(),
        limits: SandboxLimitsState {
            wall_time_seconds: policy.limits.wall_time_seconds,
            max_stdout_bytes: policy.limits.max_stdout_bytes,
            max_stderr_bytes: policy.limits.max_stderr_bytes,
            max_memory_bytes: policy.limits.max_memory_bytes,
            max_processes: policy.limits.max_processes,
        },
    }
}

/// Returns the complete policy associated with a named sandbox preset.
pub(super) fn sandbox_policy_for_preset(preset: &str) -> Result<SandboxPolicy> {
    let mut policy = SandboxPolicy::default();
    match preset {
        "safe" => {}
        "balanced" => policy.workspace_read = WorkspaceRead::All,
        "unlimited" => {
            policy.mode = SandboxMode::Unlimited;
            policy.auto_approve = true;
            policy.workspace_read = WorkspaceRead::All;
        }
        other => return Err(anyhow!("Unknown sandbox preset: {other}")),
    }
    Ok(policy)
}

/// Identifies a policy as one of the standard presets or custom.
pub(super) fn detect_sandbox_preset(policy: &SandboxPolicy) -> &'static str {
    let mut normalized = policy.clone();
    normalized.workspace_writable = false;
    for (name, candidate) in [
        ("safe", sandbox_policy_for_preset("safe")),
        ("balanced", sandbox_policy_for_preset("balanced")),
        ("unlimited", sandbox_policy_for_preset("unlimited")),
    ] {
        if candidate.is_ok_and(|candidate| candidate == normalized) {
            return name;
        }
    }
    "custom"
}

/// Applies one sandbox UI mutation, validates it, and persists the result.
pub(super) fn apply_sandbox_action(
    workspace: &PathBuf,
    action: SandboxAction,
) -> Result<SandboxPolicy> {
    let store = SandboxStore::new(workspace)?;
    if action == SandboxAction::Reset {
        store.reset()?;
        return Ok(SandboxPolicy::default());
    }
    let mut policy = store.load()?;
    match action {
        SandboxAction::ApplyPreset { preset } => policy = sandbox_policy_for_preset(&preset)?,
        SandboxAction::SetExecutionMode { mode } => {
            policy.mode = match mode.as_str() {
                "sandboxed" => SandboxMode::Sandboxed,
                "unlimited" => SandboxMode::Unlimited,
                other => return Err(anyhow!("Unsupported execution mode: {other}")),
            };
        }
        SandboxAction::SetAutoApprove { enabled } => policy.auto_approve = enabled,
        SandboxAction::SetWorkspaceMode { mode } => match mode.as_str() {
            "none" => policy.workspace_read = WorkspaceRead::None,
            "all" => policy.workspace_read = WorkspaceRead::All,
            other => return Err(anyhow!("Unsupported workspace mode: {other}")),
        },
        SandboxAction::AddWorkspacePath { path } => {
            let path = validate_relative_path(path.trim())?;
            let mut paths = match &policy.workspace_read {
                WorkspaceRead::Paths(paths) => paths.clone(),
                _ => Default::default(),
            };
            paths.insert(path);
            policy.workspace_read = WorkspaceRead::Paths(paths);
        }
        SandboxAction::RemoveWorkspacePath { path } => {
            let mut paths = match &policy.workspace_read {
                WorkspaceRead::Paths(paths) => paths.clone(),
                _ => Default::default(),
            };
            paths.remove(path.trim());
            policy.workspace_read = if paths.is_empty() {
                WorkspaceRead::None
            } else {
                WorkspaceRead::Paths(paths)
            };
        }
        SandboxAction::SetScratchWritable { enabled } => policy.scratch_writable = enabled,
        SandboxAction::AddNetwork { host, port } => {
            policy
                .network_allow
                .insert(NetworkEndpoint::new(host.trim(), port)?);
            policy.normalize_network();
        }
        SandboxAction::RemoveNetwork { host, port } => {
            policy
                .network_allow
                .remove(&NetworkEndpoint::new(host.trim(), port)?);
        }
        SandboxAction::SetEnvironment { key, value } => {
            let mut environment = policy.environment.clone();
            environment.insert(key.trim().into(), value);
            validate_environment(&environment)?;
            policy.environment = environment;
        }
        SandboxAction::RemoveEnvironment { key } => {
            policy.environment.remove(key.trim());
        }
        SandboxAction::AddSecret { id } => {
            let id = id.trim();
            validate_secret_id(id)?;
            policy.secret_ids.insert(id.into());
        }
        SandboxAction::RemoveSecret { id } => {
            policy.secret_ids.remove(id.trim());
        }
        SandboxAction::SetLimit { name, value } => {
            match name.as_str() {
                "wall_time_seconds" => policy.limits.wall_time_seconds = value,
                "max_stdout_bytes" => {
                    policy.limits.max_stdout_bytes =
                        usize::try_from(value).map_err(|_| anyhow!("stdout limit is too large"))?
                }
                "max_stderr_bytes" => {
                    policy.limits.max_stderr_bytes =
                        usize::try_from(value).map_err(|_| anyhow!("stderr limit is too large"))?
                }
                "max_memory_bytes" => policy.limits.max_memory_bytes = value,
                "max_processes" => {
                    policy.limits.max_processes =
                        usize::try_from(value).map_err(|_| anyhow!("process limit is too large"))?
                }
                other => return Err(anyhow!("Unknown sandbox limit: {other}")),
            }
            policy.limits.validate()?;
        }
        SandboxAction::Reset => unreachable!(),
    }
    store.save(&policy)?;
    Ok(policy)
}
