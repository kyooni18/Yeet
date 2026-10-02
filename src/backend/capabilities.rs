//! Harness capability discovery, toggling, and project-level persistence.
use super::*;

impl BackendService {
    pub(super) fn request_capabilities(&self) {
        if let Err(error) = self.reload_project_capabilities() {
            self.append_error(format!("Unable to load project settings: {error}"));
            return;
        }
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.is_loading_capabilities = true;
            shared.state.error_message = None;
        }
        self.publish_state();

        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<Vec<CapabilityToggleItem>> {
                let harness = bridge.list_harness_capabilities()?;
                let skills = bridge.list_skills().unwrap_or_default();
                let mcp = bridge.list_mcp_servers().unwrap_or_default();

                let state = shared
                    .lock()
                    .map_err(|_| anyhow!("session lock poisoned"))?;
                let effective_harness = state
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&harness));
                let disabled = state.meta.disabled_capabilities.clone();
                drop(state);

                let mut items = harness
                    .into_iter()
                    .map(|capability| CapabilityToggleItem {
                        id: capability.id.clone(),
                        kind: "capability".into(),
                        name: capability.name,
                        description: capability.description,
                        enabled: effective_harness.contains(&capability.id),
                        source: Some("harness".into()),
                    })
                    .collect::<Vec<_>>();
                items.push(CapabilityToggleItem {
                    id: "web-search".into(),
                    kind: "capability".into(),
                    name: "Web Search".into(),
                    description:
                        "Default-attached live web search through Agent-Reach/Exa with managed SearXNG fallback."
                            .into(),
                    enabled: effective_harness.iter().any(|value| value == "web-search"),
                    source: Some("harness".into()),
                });
                items.push(CapabilityToggleItem {
                    id: SKYLINE_CAPABILITY_ID.into(),
                    kind: "builtin".into(),
                    name: "Skyline".into(),
                    description: "Explicit session-attached autonomy-first shared coordination through ~/.yeet/Skyline. Models choose work themselves; Skyline provides compact deltas, peer intent, evidence exchange, and scarce live-test serialization.".into(),
                    enabled: effective_harness
                        .iter()
                        .any(|value| value == SKYLINE_CAPABILITY_ID),
                    source: Some("session".into()),
                });
                items.extend(
                    crate::tools::builtin_capabilities()
                        .iter()
                        .map(|capability| CapabilityToggleItem {
                            id: capability.id.into(),
                            kind: "builtin".into(),
                            name: capability.name.into(),
                            description: capability.description.into(),
                            enabled: !disabled.iter().any(|value| value == capability.id),
                            source: Some("builtin".into()),
                        }),
                );
                items.extend(skills.into_iter().map(|skill| {
                    let id = format!("skill:{}", skill.name);
                    CapabilityToggleItem {
                        enabled: effective_harness.contains(&id),
                        id,
                        kind: "skill".into(),
                        name: skill.name,
                        description: skill.description,
                        source: skill.source,
                    }
                }));
                items.extend(mcp.into_iter().map(|server| {
                    let id = format!("mcp:{}", server.name);
                    CapabilityToggleItem {
                        enabled: !disabled.contains(&id),
                        id,
                        kind: "mcp".into(),
                        name: server.name,
                        description: format!(
                            "{} MCP server{}",
                            server.transport,
                            if server.connected {
                                " · connected"
                            } else {
                                ""
                            }
                        ),
                        source: Some("configured".into()),
                    }
                }));
                items.sort_by(|a, b| {
                    a.kind.cmp(&b.kind).then_with(|| {
                        a.name
                            .to_ascii_lowercase()
                            .cmp(&b.name.to_ascii_lowercase())
                    })
                });
                Ok(items)
            })();

            if let Ok(mut state) = shared.lock() {
                state.state.is_loading_capabilities = false;
                match result {
                    Ok(items) => state.state.available_capabilities = items,
                    Err(error) => {
                        state.state.error_message =
                            Some(format!("Unable to load capabilities: {error}"))
                    }
                }
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    pub(super) fn toggle_capability(&self, id: &str) -> Result<bool> {
        if id == "vision" {
            // Legacy clients may still request this toggle. Image support is automatic.
            return Ok(true);
        }
        self.reload_project_capabilities()?;
        let harness = self.bridge.list_harness_capabilities()?;
        let is_harness = id == "web-search" || harness.iter().any(|capability| capability.id == id);
        let is_skill = id.strip_prefix("skill:").is_some_and(|name| {
            self.bridge
                .list_skills()
                .is_ok_and(|skills| skills.iter().any(|skill| skill.name == name))
        });
        let is_mcp = id.starts_with("mcp:");
        let is_skyline = id == SKYLINE_CAPABILITY_ID;
        let is_builtin = crate::tools::builtin_capabilities()
            .iter()
            .any(|capability| capability.id == id);
        if !is_harness && !is_skill && !is_mcp && !is_builtin && !is_skyline {
            return Err(anyhow!("Unknown capability: {id}"));
        }

        let mut skill_transition: Option<(String, bool)> = None;
        let mut skyline_transition: Option<bool> = None;
        {
            let mut shared = self.shared.lock_or_recover();
            if shared.state.is_streaming {
                shared.state.error_message = Some(CAPABILITY_STREAMING_LOCK_ERROR.into());
                drop(shared);
                self.publish_state();
                return Ok(false);
            }
            if is_harness || is_skill || is_skyline {
                let mut values = shared
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&harness));
                let was_attached = values.iter().any(|value| value == id);
                if was_attached {
                    values.retain(|value| value != id);
                } else {
                    values.push(id.to_owned());
                    values.sort();
                    values.dedup();
                }
                shared.meta.attached_capabilities = Some(values);
                if let Some(name) = id.strip_prefix("skill:") {
                    shared
                        .meta
                        .disabled_capabilities
                        .retain(|value| value != id);
                    skill_transition = Some((name.to_owned(), !was_attached));
                }
                if is_skyline {
                    shared
                        .meta
                        .disabled_capabilities
                        .retain(|value| value != id);
                    skyline_transition = Some(!was_attached);
                }
            } else if shared
                .meta
                .disabled_capabilities
                .iter()
                .any(|value| value == id)
            {
                shared
                    .meta
                    .disabled_capabilities
                    .retain(|value| value != id);
            } else {
                shared.meta.disabled_capabilities.push(id.to_owned());
                shared.meta.disabled_capabilities.sort();
                shared.meta.disabled_capabilities.dedup();
            }
        }
        if skill_transition.is_some() || skyline_transition.is_some() {
            let mut coordinator = self
                .coordinator
                .lock()
                .map_err(|_| anyhow!("agent coordinator lock poisoned"))?;
            if let Some((name, attached)) = skill_transition.as_ref() {
                if *attached {
                    coordinator.attach_skill_to_session(name)?;
                } else {
                    coordinator.detach_skill_from_session(name);
                }
            }
            if let Some(attached) = skyline_transition {
                coordinator.set_skyline_attachment(attached)?;
            }
        }
        self.save_project_capabilities()?;
        if skill_transition.is_some() || skyline_transition.is_some() {
            let history = self
                .coordinator
                .lock()
                .map_err(|_| anyhow!("agent coordinator lock poisoned"))?
                .model_history();
            let mut shared = self.shared.lock_or_recover();
            if shared.state.current_session_id.is_some() {
                persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
            }
        }
        self.request_capabilities();
        Ok(true)
    }

    pub(super) fn save_project_capabilities(&self) -> Result<()> {
        let shared = self.shared.lock_or_recover();
        let attached = shared.meta.attached_capabilities.clone().map(|values| {
            values
                .into_iter()
                .filter(|value| !session_only_capability(value))
                .collect::<Vec<_>>()
        });
        let disabled = shared
            .meta
            .disabled_capabilities
            .iter()
            .filter(|value| !session_only_capability(value))
            .cloned()
            .collect::<Vec<_>>();
        self.project_settings.save_capabilities(attached, disabled)
    }

    pub(super) fn reload_project_capabilities(&self) -> Result<()> {
        let project = self.project_settings.load()?;
        let harness = self.bridge.list_harness_capabilities()?;
        let mut shared = self.shared.lock_or_recover();
        let session_attachments = shared
            .meta
            .attached_capabilities
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|value| session_only_capability(value))
            .cloned()
            .collect::<Vec<_>>();
        let mut attached = project.capabilities.attached.map(|values| {
            values
                .into_iter()
                .filter(|value| !session_only_capability(value))
                .collect::<Vec<_>>()
        });
        if !session_attachments.is_empty() {
            let values = attached.get_or_insert_with(|| default_attached_harness(&harness));
            values.extend(session_attachments);
            values.sort();
            values.dedup();
        }
        shared.meta.attached_capabilities = attached;
        shared.meta.disabled_capabilities = project
            .capabilities
            .disabled
            .into_iter()
            .filter(|value| !session_only_capability(value))
            .collect();
        Ok(())
    }
}
