use super::*;
use crate::harness::HarnessState;

fn control(
    id: &str,
    label: &str,
    detail: String,
    icon: SettingsIcon,
    kind: SettingsControlKind,
    value: String,
    enabled: bool,
) -> SettingsControl {
    SettingsControl {
        id: id.into(),
        label: label.into(),
        detail,
        icon,
        kind,
        value,
        checked: None,
        enabled,
        options: Vec::new(),
        action: SettingsAction::Activate(id.into()),
        provider: None,
        action_label: "Open".into(),
    }
}
fn choice(
    id: &str,
    label: &str,
    detail: &str,
    icon: SettingsIcon,
    value: &str,
    values: &[(&str, &str)],
    enabled: bool,
) -> SettingsControl {
    let mut control = control(
        id,
        label,
        detail.into(),
        icon,
        SettingsControlKind::Choice,
        value.into(),
        enabled,
    );
    control.options = values
        .iter()
        .map(|(value, label)| SettingsOption {
            value: (*value).into(),
            label: (*label).into(),
            selected: *value == control.value,
            action: SettingsAction::SetChoice {
                id: id.into(),
                value: (*value).into(),
            },
        })
        .collect();
    control
}
fn toggle(
    id: &str,
    label: &str,
    detail: String,
    icon: SettingsIcon,
    checked: bool,
    enabled: bool,
) -> SettingsControl {
    let mut control = control(
        id,
        label,
        detail,
        icon,
        SettingsControlKind::Toggle,
        if checked { "on" } else { "off" }.into(),
        enabled,
    );
    control.checked = Some(checked);
    control.action = SettingsAction::SetToggle {
        id: id.into(),
        enabled: !checked,
    };
    control
}
fn section(
    id: &str,
    label: &str,
    icon: SettingsIcon,
    controls: Vec<SettingsControl>,
) -> SettingsSection {
    SettingsSection {
        id: id.into(),
        label: label.into(),
        icon,
        controls,
    }
}
pub(super) fn project(
    state: &SettingsState,
    env: &SettingsEnvironment,
    harness: &HarnessState,
) -> SettingsView {
    let enabled = env.available && !harness.settings_working;
    let runtime = &harness.runtime_settings;
    let flex = harness
        .active_model
        .split_once('/')
        .is_some_and(|(provider, _)| provider == "openai")
        && harness.auth_providers.iter().any(|provider| {
            provider.provider == "openai"
                && provider.authenticated
                && matches!(provider.method.as_str(), "api-key" | "environment")
        });
    let mut appearance = vec![choice(
        "appearance",
        "Appearance",
        "Choose application appearance",
        SettingsIcon::Appearance,
        &runtime.appearance,
        &[("auto", "Auto"), ("light", "Light"), ("dark", "Dark")],
        enabled,
    )];
    for (kind, id, label, value, warning) in [
        (
            SettingsEditorKind::DarkTheme,
            "theme_dark",
            "Dark theme",
            &runtime.theme_dark,
            &runtime.theme_dark_warning,
        ),
        (
            SettingsEditorKind::LightTheme,
            "theme_light",
            "Light theme",
            &runtime.theme_light,
            &runtime.theme_light_warning,
        ),
    ] {
        if env.supported_editors.contains(&kind) {
            appearance.push(control(
                id,
                label,
                warning
                    .clone()
                    .unwrap_or_else(|| "Built-in name or JSON/Lua theme path".into()),
                SettingsIcon::Theme,
                SettingsControlKind::Editor,
                value.clone(),
                enabled,
            ));
        }
    }
    let mut sections = vec![
        section(
            "appearance",
            "Appearance",
            SettingsIcon::Appearance,
            appearance,
        ),
        section(
            "runtime",
            "Runtime",
            SettingsIcon::Settings,
            vec![toggle(
                "openai_flex",
                "OpenAI Flex",
                if flex {
                    "Use flex processing when available"
                } else {
                    "Requires OpenAI API billing authentication"
                }
                .into(),
                SettingsIcon::Settings,
                harness.openai_flex,
                enabled && flex,
            )],
        ),
    ];
    // Advanced destination support describes available host capabilities, not a
    // platform switch. UI still authors the complete applicable control tree.
    if env
        .supported_features
        .contains(&SettingsFeature::ServiceBackends)
    {
        let enabled = enabled && !harness.web_search_working;
        let detail = harness
            .web_test_result
            .as_ref()
            .map(|result| result.message.clone())
            .unwrap_or_else(|| {
                harness
                    .web_status
                    .as_ref()
                    .map(|status| {
                        let provider = status
                            .selected_provider
                            .as_deref()
                            .unwrap_or("none detected");
                        let error =
                            if status.backend == crate::project_settings::ServiceBackend::Mcp {
                                status.mcp.as_ref().and_then(|p| p.error.as_deref())
                            } else if status.selected_provider.is_none() {
                                status.searxng.error.as_deref()
                            } else {
                                None
                            };
                        format!(
                            "Preferred: {provider}. {}",
                            error.unwrap_or("Run a search test to verify operation.")
                        )
                    })
                    .unwrap_or_else(|| {
                        "Checking status; run a search test to verify operation.".into()
                    })
            });
        let mut controls = vec![
            control(
                "web_status",
                "Status",
                detail,
                SettingsIcon::Web,
                SettingsControlKind::Navigation,
                "refresh".into(),
                enabled,
            ),
            control(
                "web_setup",
                "Set up or repair",
                "Check installed providers and prerequisites".into(),
                SettingsIcon::Web,
                SettingsControlKind::Navigation,
                "setup".into(),
                enabled,
            ),
            control(
                "web_test",
                "Test connection",
                "Run a real search".into(),
                SettingsIcon::Web,
                SettingsControlKind::Navigation,
                "test".into(),
                enabled,
            ),
        ];
        controls[0].action_label = "Refresh".into();
        controls[1].action_label = "Set Up".into();
        controls[2].action_label = "Test".into();
        if let Some(web) = harness
            .available_capabilities
            .iter()
            .find(|item| item.id == "web-search")
        {
            controls.insert(
                0,
                toggle(
                    "capability:web-search",
                    "Web Search",
                    web.description.clone(),
                    SettingsIcon::Web,
                    web.enabled,
                    enabled,
                ),
            );
        }
        if let Some(plan) = &harness.web_setup_plan
            && let Some(target) = &plan.install
        {
            let detail = if plan.missing_dependencies.is_empty() {
                plan.message.clone()
            } else {
                format!(
                    "Install prerequisites first: {}",
                    plan.missing_dependencies.join(", ")
                )
            };
            let mut install = control(
                "web_install",
                "Approve installation",
                detail,
                SettingsIcon::Web,
                SettingsControlKind::Navigation,
                target.clone(),
                enabled && plan.missing_dependencies.is_empty(),
            );
            install.action_label = "Install".into();
            controls.push(install);
        }
        let mut advanced = control(
            "web_advanced",
            "Advanced",
            "Provider and connection settings".into(),
            SettingsIcon::Settings,
            SettingsControlKind::Navigation,
            "open".into(),
            enabled,
        );
        advanced.action_label = if state.web_advanced { "Hide" } else { "Open" }.into();
        controls.push(advanced);
        if state.web_advanced {
            controls.push(choice(
                "web_backend",
                "Provider",
                "Automatic is recommended",
                SettingsIcon::Web,
                &harness.web_backend,
                &[
                    ("auto", "Automatic"),
                    ("agent-reach", "Agent-Reach/Exa"),
                    ("searxng", "SearXNG"),
                    ("mcp", "MCP"),
                ],
                enabled,
            ));
            {
                let options = harness
                    .web_mcp_servers
                    .iter()
                    .map(|s| (s.as_str(), s.as_str()))
                    .collect::<Vec<_>>();
                controls.push(choice(
                    "web_server",
                    "MCP server",
                    harness
                        .web_mcp_error
                        .as_deref()
                        .unwrap_or("Only servers with web_search and web_read are shown"),
                    SettingsIcon::Web,
                    &harness.web_server,
                    &options,
                    enabled,
                ));
            }
            controls.push(control(
                "web_endpoint",
                "SearXNG endpoint",
                "HTTP or HTTPS URL; blank uses the default".into(),
                SettingsIcon::Web,
                SettingsControlKind::Editor,
                harness.web_searxng_url.clone().unwrap_or_default(),
                enabled,
            ));
        }
        sections.push(section(
            "web_search",
            "Web Search",
            SettingsIcon::Web,
            controls,
        ));
    }
    let mut model = Vec::new();
    for (destination, id, label, value, icon) in [
        (
            SettingsDestination::Models,
            "model",
            "Model",
            harness.active_model.clone(),
            SettingsIcon::Model,
        ),
        (
            SettingsDestination::Reasoning,
            "reasoning",
            "Reasoning",
            harness.active_reasoning_level.clone(),
            SettingsIcon::Reasoning,
        ),
    ] {
        if env.supported_destinations.contains(&destination) {
            model.push(control(
                id,
                label,
                "Persistent default".into(),
                icon,
                SettingsControlKind::Navigation,
                value,
                enabled,
            ));
        }
    }
    if env
        .supported_editors
        .contains(&SettingsEditorKind::ContextLength)
    {
        model.push(control(
            "context_length",
            "Context window",
            "Current-model override; auto uses the model default".into(),
            SettingsIcon::Context,
            SettingsControlKind::Editor,
            runtime
                .context_length_override
                .map(|value| value.to_string())
                .unwrap_or_else(|| "auto".into()),
            enabled,
        ));
    }
    if !model.is_empty() {
        sections.push(section("model", "Model", SettingsIcon::Model, model));
    }
    for (kind, id, label) in [
        ("builtin", "optional_features", "Optional built-in features"),
        ("skill", "skills", "Skills"),
        ("mcp", "mcp_servers", "MCP servers"),
        ("capability", "extensions", "Other extensions"),
    ] {
        let capabilities = harness
            .available_capabilities
            .iter()
            .filter(|capability| capability.kind == kind && capability.id != "web-search")
            .map(|capability| {
                toggle(
                    &format!("capability:{}", capability.id),
                    &capability.name,
                    capability.description.clone(),
                    SettingsIcon::Capabilities,
                    capability.enabled,
                    enabled,
                )
            })
            .collect::<Vec<_>>();
        if !capabilities.is_empty() {
            sections.push(section(id, label, SettingsIcon::Capabilities, capabilities));
        }
    }
    let providers = harness
        .auth_providers
        .iter()
        .map(|provider| {
            let display = crate::shared_ui::presentation::provider_name(&provider.provider);
            let mut control = control(
                &format!("provider:{}", provider.provider),
                display,
                if provider.authenticated {
                    provider.method.clone()
                } else {
                    provider
                        .error
                        .clone()
                        .unwrap_or_else(|| "Not signed in".into())
                },
                SettingsIcon::Provider,
                SettingsControlKind::Navigation,
                String::new(),
                enabled,
            );
            control.provider = Some(provider.provider.clone());
            control.action_label = if provider.authenticated {
                "Log out"
            } else {
                "Log in"
            }
            .into();
            control
        })
        .collect::<Vec<_>>();
    if !providers.is_empty() {
        sections.push(section(
            "providers",
            "Providers",
            SettingsIcon::Provider,
            providers,
        ));
    }
    let mut advanced = Vec::new();
    for (destination, id, label, icon) in [
        (
            SettingsDestination::Agents,
            "agents",
            "Agent Group",
            SettingsIcon::Agents,
        ),
        (
            SettingsDestination::Permissions,
            "permissions",
            "Sandbox & permissions",
            SettingsIcon::Permissions,
        ),
        (
            SettingsDestination::Auth,
            "auth",
            "Authentication",
            SettingsIcon::Provider,
        ),
        (
            SettingsDestination::Providers,
            "providers",
            "Custom providers",
            SettingsIcon::Provider,
        ),
        (
            SettingsDestination::Capabilities,
            "capabilities",
            "Manage capabilities",
            SettingsIcon::Capabilities,
        ),
    ] {
        if env.supported_destinations.contains(&destination) {
            advanced.push(control(
                id,
                label,
                "Open detailed controls".into(),
                icon,
                SettingsControlKind::Navigation,
                "open".into(),
                enabled,
            ));
        }
    }
    if !advanced.is_empty() {
        sections.push(section(
            "advanced",
            "Advanced",
            SettingsIcon::Settings,
            advanced,
        ));
    }
    SettingsView {
        title: "Settings".into(),
        subtitle: "Runtime controls".into(),
        sections,
        selected: state.selected.clone(),
        editor: state.editor.clone(),
        notice: state
            .error
            .clone()
            .or_else(|| harness.settings_notice.clone()),
        working: harness.settings_working,
    }
}
