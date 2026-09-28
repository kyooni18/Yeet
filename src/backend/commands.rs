//! Backend command routing, session-environment reporting, status output, and catalog requests.

use super::*;

impl BackendService {
    pub(super) fn run_command(&mut self, input: &str) -> Result<()> {
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or_default();
        let arguments: Vec<_> = parts.collect();
        match command {
            "/debate" => return self.start_debate(arguments.join(" "), None),
            "/help" => self.append_system("/new  /model [ID]  /reasoning [auto|low|medium|high]  /login  /provider  /providers  /settings  /permissions  /permission [allow|deny]  /allow  /deny  /sessions  /workspace [cd|add|remove|reset] PATH  /cd PATH  /capabilities  /skyline [on|off]  /image PATH|clear  /compact  /context [LENGTH|auto]  /status  /goal [on|off|toggle|status]  /attach ID  /detach ID  /clear"),
            "/new" => self.new_session(),
            "/model" => {
                if arguments.is_empty() {
                    self.request_models();
                    let current = self.shared.lock_or_recover().state.active_model.clone();
                    self.append_system(&format!(
                        "Current model: {}. Use /model MODEL_ID to change it.",
                        if current.is_empty() { "none" } else { &current }
                    ));
                } else {
                    self.select_model(arguments.join(" "))?;
                }
            }
            "/reasoning" => {
                if let Some(level) = arguments.first().copied() {
                    self.select_reasoning(level.to_owned())?;
                    self.append_system(&format!("Reasoning: {level}"));
                } else {
                    let current = self.shared.lock_or_recover().state.active_reasoning_level.clone();
                    self.append_system(&format!(
                        "Current reasoning: {}. Use /reasoning [auto|low|medium|high].",
                        if current.is_empty() { "auto" } else { &current }
                    ));
                }
            }
            "/sessions" => {
                self.request_sessions();
                let count = self.shared.lock_or_recover().state.saved_sessions.len();
                self.append_system(&format!("Sessions refreshed: {count} saved."));
            }
            "/providers" => {
                self.request_providers();
                self.append_system("Provider list refresh requested.");
            }
            "/allow" => {
                self.permission.resolve(true);
                self.publish_state();
                self.append_system("Pending permission allowed once.");
            }
            "/deny" => {
                self.permission.resolve(false);
                self.publish_state();
                self.append_system("Pending permission denied.");
            }
            "/clear" => {
                let mut shared = self.shared.lock_or_recover();
                if let Some(entries) = shared.state.conversation.as_mut()
                    && let Some(index) = entries
                        .iter()
                        .rposition(|entry| matches!(entry.kind, ConversationKind::System { .. }))
                {
                    entries.remove(index);
                    shared.state.conversation_revision =
                        shared.state.conversation_revision.wrapping_add(1);
                }
                drop(shared);
                self.publish_state();
            }
            "/permission" => match arguments.first().copied() {
                Some("allow") => { self.permission.resolve(true); self.publish_state(); }
                Some("deny") => { self.permission.resolve(false); self.publish_state(); }
                _ => self.append_system("Usage: /permission [allow|deny]"),
            },
            "/compact" => {
                let streaming = self.shared.lock_or_recover().state.is_streaming;
                if streaming {
                    self.shared.lock_or_recover().meta.pending_compaction = true;
                    self.append_system("Context compaction queued for the end of the current response.");
                } else {
                    self.compact_context()?;
                }
            }
            "/context" => {
                let model = self.shared.lock_or_recover().state.active_model.clone();
                if model.is_empty() {
                    self.append_error("No model is selected.".into());
                    return Ok(());
                }
                match arguments.first().copied() {
                    None => {
                        let override_length = self.config.context_length_override(&model)?;
                        let effective = self.shared.lock_or_recover().state.active_model_context_length;
                        let message = match (override_length, effective) {
                            (Some(value), _) => format!("Context length for {model}: {value} tokens (manual override)."),
                            (None, Some(value)) => format!("Context length for {model}: {value} tokens (automatic)."),
                            (None, None) => format!("Context length for {model}: unknown."),
                        };
                        self.append_system(&message);
                    }
                    Some("auto") | Some("reset") => {
                        self.config.set_context_length_override(&model, None)?;
                        self.refresh_context_length();
                        self.append_system(&format!("Context length override cleared for {model}."));
                    }
                    Some(value) => {
                        let length = parse_context_length(value)?;
                        self.config.set_context_length_override(&model, Some(length))?;
                        self.shared.lock_or_recover().state.active_model_context_length = Some(length);
                        self.append_system(&format!("Context length for {model} set to {length} tokens."));
                    }
                }
            }

            "/workspace" => {
                let action = arguments.first().copied();
                let result: Result<String> = match action {
                    None | Some("list") | Some("show") => self.session_environment_report(),
                    Some("cd") | Some("set") => {
                        let path = arguments.iter().skip(1).copied().collect::<Vec<_>>().join(" ");
                        if path.is_empty() {
                            Err(anyhow!("Usage: /workspace cd PATH"))
                        } else {
                            if !self.guard_session_environment_mutation() {
                                return Ok(());
                            }
                            let resolved = {
                                let mut coordinator = self
                                    .coordinator
                                    .lock()
                                    .map_err(|_| anyhow!("coordinator lock poisoned"))?;
                                coordinator.set_working_directory(Path::new(&path))?
                            };
                            self.sync_session_environment()?;
                            Ok(format!("Session cwd: {}", resolved.display()))
                        }
                    }
                    Some("add") => {
                        let path = arguments.iter().skip(1).copied().collect::<Vec<_>>().join(" ");
                        if path.is_empty() {
                            Err(anyhow!("Usage: /workspace add PATH"))
                        } else {
                            if !self.guard_session_environment_mutation() {
                                return Ok(());
                            }
                            let resolved = {
                                let mut coordinator = self
                                    .coordinator
                                    .lock()
                                    .map_err(|_| anyhow!("coordinator lock poisoned"))?;
                                coordinator.add_context_root(Path::new(&path))?
                            };
                            self.sync_session_environment()?;
                            Ok(format!("Added context root: {}", resolved.display()))
                        }
                    }
                    Some("remove") | Some("rm") => {
                        let path = arguments.iter().skip(1).copied().collect::<Vec<_>>().join(" ");
                        if path.is_empty() {
                            Err(anyhow!("Usage: /workspace remove PATH"))
                        } else {
                            if !self.guard_session_environment_mutation() {
                                return Ok(());
                            }
                            let removed = {
                                let mut coordinator = self
                                    .coordinator
                                    .lock()
                                    .map_err(|_| anyhow!("coordinator lock poisoned"))?;
                                coordinator.remove_context_root(Path::new(&path))?
                            };
                            self.sync_session_environment()?;
                            if removed {
                                Ok(format!("Removed context root: {path}"))
                            } else {
                                Ok(format!("Context root was not registered: {path}"))
                            }
                        }
                    }
                    Some("reset") => {
                        if !self.guard_session_environment_mutation() {
                            return Ok(());
                        }
                        {
                            let mut coordinator = self
                                .coordinator
                                .lock()
                                .map_err(|_| anyhow!("coordinator lock poisoned"))?;
                            coordinator.restore_session_environment(None, &[])?;
                        }
                        self.sync_session_environment()?;
                        Ok(format!("Session environment reset to {}", self.workspace_root.display()))
                    }
                    Some(other) => Err(anyhow!(
                        "Unknown workspace action '{other}'. Use /workspace [cd|add|remove|reset] PATH"
                    )),
                };
                match result {
                    Ok(message) => self.append_system(&message),
                    Err(error) => self.append_error(error.to_string()),
                }
            }
            "/cd" => {
                let path = arguments.join(" ");
                if path.is_empty() {
                    match self.session_environment_report() {
                        Ok(message) => self.append_system(&message),
                        Err(error) => self.append_error(error.to_string()),
                    }
                } else {
                    if !self.guard_session_environment_mutation() {
                        return Ok(());
                    }
                    let result = (|| -> Result<PathBuf> {
                        let resolved = {
                            let mut coordinator = self
                                .coordinator
                                .lock()
                                .map_err(|_| anyhow!("coordinator lock poisoned"))?;
                            coordinator.set_working_directory(Path::new(&path))?
                        };
                        self.sync_session_environment()?;
                        Ok(resolved)
                    })();
                    match result {
                        Ok(path) => self.append_system(&format!("Session cwd: {}", path.display())),
                        Err(error) => self.append_error(error.to_string()),
                    }
                }
            }
            "/status" => self.append_system(&self.status_report()),
            "/goal" => {
                let current = self.shared.lock_or_recover().state.goal_mode;
                match arguments.first().copied() {
                    None | Some("toggle") => {
                        let enabled = !current;
                        self.set_goal_enabled(enabled)?;
                        self.append_system(&format!("Goal: {}", if enabled { "ON" } else { "OFF" }));
                    }
                    Some("on") | Some("enable") => {
                        self.set_goal_enabled(true)?;
                        self.append_system("Goal: ON");
                    }
                    Some("off") | Some("disable") => {
                        self.set_goal_enabled(false)?;
                        self.append_system("Goal: OFF");
                    }
                    Some("status") => {
                        self.append_system(&format!("Goal: {}", if current { "ON" } else { "OFF" }));
                    }
                    Some(_) => self.append_system("Usage: /goal [on|off|toggle|status]"),
                }
            }
            "/image" => {
                let Some(argument) = arguments.first().copied() else {
                    let count = self.shared.lock_or_recover().meta.pending_images.len();
                    self.append_system(&format!("{count} image{} queued for the next turn. Use /image PATH to add one or /image clear to remove them.", if count == 1 { "" } else { "s" }));
                    return Ok(());
                };
                if argument == "clear" {
                    self.shared.lock_or_recover().meta.pending_images.clear();
                    self.append_system("Cleared queued images.");
                    return Ok(());
                }
                let path_text = arguments.join(" ");
                let path = PathBuf::from(&path_text);
                let path = if path.is_absolute() {
                    path
                } else {
                    let (cwd, _) = self
                        .coordinator
                        .lock()
                        .map_err(|_| anyhow!("coordinator lock poisoned"))?
                        .session_environment();
                    PathBuf::from(cwd).join(path)
                };
                let image = ImageAttachment::from_file(&path)?;
                let name = image.name.clone().unwrap_or_else(|| path.display().to_string());
                let mut shared = self.shared.lock_or_recover();
                shared.meta.pending_images.push(image);
                let count = shared.meta.pending_images.len();
                drop(shared);
                self.append_system(&format!("Queued {name} for the next turn ({count} total)."));
            }
            "/skyline" => {
                self.reload_project_capabilities()?;
                let desired = match arguments.first().copied() {
                    None | Some("on") | Some("attach") | Some("enable") => true,
                    Some("off") | Some("detach") | Some("disable") => false,
                    Some(_) => {
                        self.append_system("Usage: /skyline [on|off]");
                        return Ok(());
                    }
                };
                let attached = self
                    .shared
                    .lock_or_recover()
                    .meta
                    .attached_capabilities
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|value| value == SKYLINE_CAPABILITY_ID);
                if attached != desired && !self.toggle_capability(SKYLINE_CAPABILITY_ID)? {
                    return Ok(());
                }
                self.append_system(if desired {
                    "Skyline attached to this TUI session. The agent now has one compact `skyline` coordination tool."
                } else {
                    "Skyline detached from this TUI session."
                });
            }
            "/capabilities" => {
                self.reload_project_capabilities()?;
                let capabilities = self.bridge.list_harness_capabilities()?;
                let attached = self.shared.lock_or_recover().meta.attached_capabilities.clone();
                let effective: Vec<String> =
                    attached.unwrap_or_else(|| default_attached_harness(&capabilities));
                let mut lines = capabilities.into_iter().map(|capability| format!("{} [{}] — {}", capability.id, if effective.contains(&capability.id) { "attached" } else { "detached" }, capability.description)).collect::<Vec<_>>();
                lines.push(format!("web-search [{}] — Default-attached live web search through Agent-Reach/Exa with managed SearXNG fallback.", if effective.iter().any(|value| value == "web-search") { "attached" } else { "detached" }));
                lines.push(format!("{SKYLINE_CAPABILITY_ID} [{}] — Explicit session-attached Skyline coordination through ~/.yeet/Skyline.", if effective.iter().any(|value| value == SKYLINE_CAPABILITY_ID) { "attached" } else { "detached" }));
                let text = lines.join("\n");
                self.append_system(if text.is_empty() { "No harness capabilities are available." } else { &text });
            }
            "/attach" => {
                let Some(id) = arguments.first() else { self.append_system("Usage: /attach capability-id"); return Ok(()); };
                self.reload_project_capabilities()?;
                if *id == SKYLINE_CAPABILITY_ID {
                    let attached = self
                        .shared
                        .lock_or_recover()
                        .meta
                        .attached_capabilities
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|value| value == SKYLINE_CAPABILITY_ID);
                    if !attached && !self.toggle_capability(SKYLINE_CAPABILITY_ID)? {
                        return Ok(());
                    }
                    self.append_system("Attached builtin:skyline for this TUI session.");
                    return Ok(());
                }
                let capabilities = self.bridge.list_harness_capabilities()?;
                if *id != "web-search" && !capabilities.iter().any(|capability| capability.id == *id) { self.append_error(format!("Unknown capability: {id}")); return Ok(()); }
                let attached = self
                    .shared
                    .lock_or_recover()
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&capabilities))
                    .iter()
                    .any(|value| value == id);
                if !attached && !self.toggle_capability(id)? {
                    return Ok(());
                }
                self.append_system(&format!("Attached {id}."));
            }
            "/detach" => {
                let Some(id) = arguments.first() else { self.append_system("Usage: /detach capability-id"); return Ok(()); };
                self.reload_project_capabilities()?;
                if *id == SKYLINE_CAPABILITY_ID {
                    let attached = self
                        .shared
                        .lock_or_recover()
                        .meta
                        .attached_capabilities
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|value| value == SKYLINE_CAPABILITY_ID);
                    if attached && !self.toggle_capability(SKYLINE_CAPABILITY_ID)? {
                        return Ok(());
                    }
                    self.append_system("Detached builtin:skyline from this TUI session.");
                    return Ok(());
                }
                let capabilities = self.bridge.list_harness_capabilities()?;
                let attached = self
                    .shared
                    .lock_or_recover()
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&capabilities))
                    .iter()
                    .any(|value| value == id);
                if attached && !self.toggle_capability(id)? {
                    return Ok(());
                }
                self.append_system(&format!("Detached {id}."));
            }
            "/provider" => self.append_system("Use `yeet provider ...` for custom API endpoints."),
            "/login" => self.append_system("Use `yeet auth ...` for provider authentication."),
            "/settings" => self.append_system(
                "Open /settings in the interactive TUI to manage runtime and application settings. Use /permissions for sandbox and permission settings.",
            ),
            "/permissions" => self.append_system(
                "Open /permissions in the interactive TUI to manage sandbox and permission settings.",
            ),
            _ => self.append_error(format!("Unknown command: {command}. Type /help for commands.")),
        }
        Ok(())
    }

    pub(super) fn guard_session_environment_mutation(&self) -> bool {
        let allowed =
            session_environment_mutation_allowed(self.shared.lock_or_recover().state.is_streaming);
        if !allowed {
            self.append_error(SESSION_ENVIRONMENT_STREAMING_LOCK_ERROR.into());
        }
        allowed
    }

    pub(super) fn session_environment_report(&self) -> Result<String> {
        let (cwd, roots) = self
            .coordinator
            .lock()
            .map_err(|_| anyhow!("coordinator lock poisoned"))?
            .session_environment();
        let roots = if roots.is_empty() {
            "  (none)".to_owned()
        } else {
            roots
                .iter()
                .map(|root| format!("  {root}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(format!(
            "Primary workspace: {}\nSession cwd: {cwd}\nContext roots:\n{roots}",
            self.workspace_root.display()
        ))
    }

    pub(super) fn sync_session_environment(&self) -> Result<()> {
        let (cwd, roots, history) = {
            let coordinator = self
                .coordinator
                .lock()
                .map_err(|_| anyhow!("coordinator lock poisoned"))?;
            let (cwd, roots) = coordinator.session_environment();
            (cwd, roots, coordinator.model_history())
        };
        let mut shared = self.shared.lock_or_recover();
        shared.meta.working_directory = Some(cwd);
        shared.meta.context_roots = roots;
        persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
        Ok(())
    }

    pub(super) fn status_report(&self) -> String {
        let state = self.shared.lock_or_recover().state.without_conversation();
        let current_context = state.current_context_tokens.unwrap_or(0);
        let context = match state.active_model_context_length {
            Some(total) if total > 0 => format!(
                "Context: {current_context}/{total} tokens ({:.1}% used)",
                current_context as f64 * 100.0 / total as f64
            ),
            Some(total) => format!("Context: {current_context}/{total} tokens"),
            None => format!("Context: {current_context} tokens / unknown limit"),
        };
        let usage = &state.token_usage;
        let cache = usage.cache_measurement();
        let input = cache.input_tokens;
        let output = usage.output_tokens.unwrap_or(0);
        let reasoning_tokens = usage.reasoning_tokens.unwrap_or(0);
        let reasoning_mode = if state.active_reasoning_level.is_empty() {
            "auto"
        } else {
            state.active_reasoning_level.as_str()
        };
        let agent_mode = match state.agent_mode {
            AgentMode::Single => "single",
            AgentMode::Adaptive => "adaptive",
        };
        let autonomy_mode = match state.autonomy_mode {
            AutonomyMode::Manual => "manual",
            AutonomyMode::Goal => "goal",
            AutonomyMode::Autonomous => "autonomous",
        };
        let active_workers = state
            .agent_tasks
            .iter()
            .filter(|task| matches!(task.status.as_str(), "pending" | "running"))
            .count();
        let (permission, sandbox_detail) = state
            .sandbox_settings
            .as_ref()
            .map(|settings| {
                (
                    settings.permission_mode(),
                    format!(
                        "preset {} · execution {} · auto-approve {}",
                        settings.preset, settings.execution_mode, settings.auto_approve
                    ),
                )
            })
            .unwrap_or(("unknown", "sandbox state unavailable".to_owned()));
        let cache_line = cache_status_line(usage);
        let mut lines = vec![
            format!(
                "Model: {}",
                if state.active_model.is_empty() {
                    "not selected"
                } else {
                    &state.active_model
                }
            ),
            context,
            format!("Tokens: input {input} · output {output} · reasoning {reasoning_tokens}"),
            cache_line,
            format!("Reasoning mode: {reasoning_mode}"),
            format!(
                "Agents: {agent_mode} · {active_workers} active / {} tracked",
                state.agent_tasks.len()
            ),
            format!("Autonomy: {autonomy_mode}"),
            format!("Goal: {}", if state.goal_mode { "ON" } else { "OFF" }),
            format!("Permission: {permission} · {sandbox_detail}"),
            self.session_environment_report()
                .unwrap_or_else(|error| format!("Session environment unavailable: {error}")),
            format!(
                "Runtime: {}",
                if state.is_streaming {
                    "executing"
                } else {
                    "idle"
                }
            ),
        ];
        if let Some(cost) = usage.estimated_cost_usd {
            lines.push(format!("Estimated cost: ${cost:.4}"));
        }
        if state.credit_usage > 0 {
            lines.push(format!("Provider calls / credits: {}", state.credit_usage));
        }
        if let Some(session_id) = state.current_session_id.as_deref() {
            lines.push(format!("Session: {session_id}"));
        }
        if let Some(run_id) = state.active_run_id.as_deref() {
            lines.push(format!("Run: {run_id}"));
        }
        if let Some(error) = state.error_message.as_deref() {
            lines.push(format!("Last error: {error}"));
        }
        if let Some((provider, _)) = state.active_model.split_once('/')
            && let Ok(provider_usage) = self.bridge.provider_usage(provider)
            && provider_usage.source != "none"
        {
            if provider_usage.windows.is_empty() {
                lines.push(format!(
                    "Quota: {} · {}",
                    provider_usage.source,
                    provider_usage.message.as_deref().unwrap_or("unavailable")
                ));
            } else {
                let windows = provider_usage
                    .windows
                    .iter()
                    .map(|window| {
                        let reset = window
                            .resets_at
                            .as_deref()
                            .map(|value| format!(" · resets {value}"))
                            .unwrap_or_default();
                        format!("{} {}% left{reset}", window.label, window.remaining_percent)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                lines.push(format!("Quota: {} · {windows}", provider_usage.source));
            }
        }
        lines.join("\n")
    }

    pub(super) fn request_models(&self) {
        {
            let mut shared = self.shared.lock_or_recover();
            if shared.state.is_loading_models {
                return;
            }
            shared.state.is_loading_models = true;
            shared.state.error_message = None;
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let mut providers = match bridge.list_providers() {
                Ok(providers) => providers,
                Err(error) => {
                    if let Ok(mut state) = shared.lock() {
                        state.state.is_loading_models = false;
                        state.state.error_message = Some(format!("Unable to load models: {error}"));
                        let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                    }
                    return;
                }
            };
            providers.sort();
            providers.dedup();

            // The provider list itself is authoritative. Prune cached slices for
            // providers that no longer exist before refreshing the remaining ones.
            // A failure to obtain the provider list is handled above and keeps the
            // cache intact because that is not an authoritative removal signal.
            let mut pruned_cache = None;
            if let Ok(mut state) = shared.lock() {
                let before = state.state.model_catalog.len();
                state
                    .state
                    .model_catalog
                    .retain(|item| providers.binary_search(&item.provider).is_ok());
                if state.state.model_catalog.len() != before {
                    state.state.available_models = state
                        .state
                        .model_catalog
                        .iter()
                        .map(|item| item.id.clone())
                        .collect();
                    pruned_cache = Some(state.state.model_catalog.clone());
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                }
            }
            if let Some(catalog) = pruned_cache {
                let _ = config.set_model_catalog_cache(&catalog);
            }

            if providers.is_empty() {
                if let Ok(mut state) = shared.lock() {
                    state.state.is_loading_models = false;
                    state.state.error_message = Some(
                        "No available models could be loaded. Check provider credentials.".into(),
                    );
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                }
                return;
            }

            // Provider discovery is independent and some catalogs (notably Gemini)
            // can take many seconds. Fetch them concurrently and publish each provider
            // as soon as it finishes so a slow endpoint cannot make the TUI look stale.
            let provider_count = providers.len();
            let (provider_tx, provider_rx) = mpsc::channel();
            for provider in providers {
                let bridge = bridge.clone();
                let provider_tx = provider_tx.clone();
                thread::spawn(move || {
                    let result = bridge.list_model_info(&provider).map(|info| {
                        let mut catalog = Vec::with_capacity(info.len());
                        let mut lengths = HashMap::new();
                        for model in info {
                            let full = format!("{provider}/{}", model.id);
                            if let Some(length) = model.context_length {
                                lengths.insert(full.clone(), length);
                            }
                            catalog.push(ModelCatalogItem {
                                id: full,
                                provider: provider.clone(),
                                model: model.id,
                                context_length: model.context_length,
                            });
                        }
                        (catalog, lengths)
                    });
                    let _ = provider_tx.send((provider, result));
                });
            }
            drop(provider_tx);

            let mut completed = 0usize;
            for (provider, result) in provider_rx {
                completed += 1;
                let mut cache_snapshot = None;
                match result {
                    Ok((mut provider_catalog, lengths)) => {
                        for (model, length) in lengths {
                            let _ = config.set_context_length(&model, Some(length));
                        }
                        provider_catalog.sort_by_key(|value| value.id.to_ascii_lowercase());
                        provider_catalog.dedup_by(|lhs, rhs| lhs.id == rhs.id);

                        if let Ok(mut state) = shared.lock() {
                            let current_provider_catalog = state
                                .state
                                .model_catalog
                                .iter()
                                .filter(|item| item.provider == provider)
                                .cloned()
                                .collect::<Vec<_>>();
                            if current_provider_catalog != provider_catalog {
                                state
                                    .state
                                    .model_catalog
                                    .retain(|item| item.provider != provider);
                                state.state.model_catalog.extend(provider_catalog);
                                state
                                    .state
                                    .model_catalog
                                    .sort_by_key(|value| value.id.to_ascii_lowercase());
                                state
                                    .state
                                    .model_catalog
                                    .dedup_by(|lhs, rhs| lhs.id == rhs.id);
                                state.state.available_models = state
                                    .state
                                    .model_catalog
                                    .iter()
                                    .map(|value| value.id.clone())
                                    .collect();
                                cache_snapshot = Some(state.state.model_catalog.clone());
                            }
                            state.state.is_loading_models = completed < provider_count;
                            if state.state.available_models.is_empty()
                                && !state.state.is_loading_models
                            {
                                state.state.error_message = Some(
                                    "No available models could be loaded. Check provider credentials.".into(),
                                );
                            } else if !state.state.available_models.is_empty() {
                                state.state.error_message = None;
                            }
                            let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                        }
                    }
                    Err(_) => {
                        // A transient provider failure is not an authoritative empty catalog.
                        // Keep that provider's cached models visible and only finish its refresh.
                        if let Ok(mut state) = shared.lock() {
                            state.state.is_loading_models = completed < provider_count;
                            if state.state.available_models.is_empty()
                                && !state.state.is_loading_models
                            {
                                state.state.error_message = Some(
                                    "No available models could be loaded. Check provider credentials.".into(),
                                );
                            } else if !state.state.available_models.is_empty() {
                                state.state.error_message = None;
                            }
                            let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                        }
                    }
                }
                if let Some(catalog) = cache_snapshot {
                    let _ = config.set_model_catalog_cache(&catalog);
                }
            }

            // Defensive cleanup if a provider worker ever exits without reporting.
            if let Ok(mut state) = shared.lock()
                && state.state.is_loading_models
            {
                state.state.is_loading_models = false;
                if state.state.available_models.is_empty() {
                    state.state.error_message = Some(
                        "No available models could be loaded. Check provider credentials.".into(),
                    );
                }
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }
}
