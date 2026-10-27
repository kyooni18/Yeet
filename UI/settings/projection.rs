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
    let mut services = Vec::new();
    if env
        .supported_features
        .contains(&SettingsFeature::ServiceBackends)
    {
        if let Some(web) = harness
            .available_capabilities
            .iter()
            .find(|item| item.id == "web-search")
        {
            services.push(toggle(
                "capability:web-search",
                "Web Search",
                web.description.clone(),
                SettingsIcon::Web,
                web.enabled,
                enabled,
            ));
        }
        services.push(choice(
            "web_backend",
            "Web Backend",
            &harness.web_server,
            SettingsIcon::Web,
            &harness.web_backend,
            &[("builtin", "Built-in"), ("mcp", "MCP")],
            enabled,
        ));
    }

    if !services.is_empty() {
        sections.push(section(
            "web_search",
            "Web Search",
            SettingsIcon::Web,
            services,
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
