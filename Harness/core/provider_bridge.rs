//! ProviderBridge: the Rust side of the Rust <-> Node provider transport.
//!
//! `BridgeClient` owns the Node sidecar process (`RuntimeSource/dist/bridge.js`)
//! that hosts provider adapters, authentication, Skills, MCP and Computer Use.
//! It speaks the versioned line protocol (`BRIDGE_PROTOCOL_VERSION`) and is a
//! protocol service, not an application-state owner: sessions, history and
//! tool policy stay in Rust. This is distinct from the harness/runtime state
//! transport (`BridgeEnvelope`/`BridgeState`, i.e. HarnessEvent/HarnessState)
//! that frontends and the background daemon exchange.

use super::*;

#[derive(Debug)]
struct BridgeInner {
    child: Mutex<TrackedChild>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    pending: Mutex<HashMap<String, mpsc::Sender<Value>>>,
    stderr_tail: Arc<Mutex<String>>,
    events: Mutex<mpsc::Receiver<Value>>,
    event_tx: mpsc::Sender<Value>,
    openai_flex: AtomicBool,
    shutting_down: AtomicBool,
    generation: AtomicU64,
}

#[derive(Debug, Clone)]
/// Synchronous client for the long-lived RuntimeSource bridge process and request protocol.
pub struct BridgeClient {
    inner: Arc<BridgeInner>,
    workspace: Option<PathBuf>,
}

impl BridgeClient {
    pub fn start() -> Result<Self> {
        Self::start_with_workspace(None)
    }

    pub fn start_for_workspace(workspace: &Path) -> Result<Self> {
        Self::start_with_workspace(Some(workspace))
    }

    fn start_with_workspace(workspace: Option<&Path>) -> Result<Self> {
        let node = node_executable()?;
        let script = bridge_script()?;
        let mut command = Command::new(node);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(workspace) = workspace {
            command.current_dir(workspace);
            apply_workspace_env(&mut command, workspace)?;
        }
        Self::spawn(command, workspace.map(Path::to_path_buf))
    }

    /// Starts a stand-in bridge script with the real protocol client, for
    /// harness tests that must observe provider-visible requests.
    #[cfg(test)]
    pub(crate) fn start_script(script: &Path, envs: &[(&str, &Path)]) -> Result<Self> {
        let mut command = Command::new(node_executable()?);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        Self::spawn(command, None)
    }

    fn spawn(mut command: Command, workspace: Option<PathBuf>) -> Result<Self> {
        // Keep the provider bridge and every MCP subprocess it starts in one
        // owned process group. Linux additionally arms parent-death cleanup.
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .context("failed to start Node provider bridge")?;
        let stdin = child
            .stdin
            .take()
            .context("provider bridge stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("provider bridge stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("provider bridge stderr unavailable")?;
        let child = TrackedChild::new(child, "provider-bridge");
        let pending: Mutex<HashMap<String, mpsc::Sender<Value>>> = Mutex::new(HashMap::new());
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let (event_tx, event_rx) = mpsc::channel();
        let inner = Arc::new(BridgeInner {
            child: Mutex::new(child),
            stdin: Mutex::new(BufWriter::new(stdin)),
            pending,
            stderr_tail: stderr_tail.clone(),
            events: Mutex::new(event_rx),
            event_tx: event_tx.clone(),
            openai_flex: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            generation: AtomicU64::new(1),
        });

        let reader_inner = inner.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    let sender = reader_inner
                        .pending
                        .lock()
                        .ok()
                        .and_then(|pending| pending.get(id).cloned());
                    if let Some(sender) = sender {
                        let _ = sender.send(value);
                    }
                } else {
                    let _ = reader_inner.event_tx.send(value);
                }
            }
            if reader_inner.generation.load(Ordering::Acquire) == 1 {
                if let Ok(mut pending) = reader_inner.pending.lock() {
                    pending.clear();
                }
                let _ = reader_inner
                    .event_tx
                    .send(json!({ "type": "bridge_closed" }));
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut tail) = stderr_tail.lock() {
                    tail.push_str(&line);
                    tail.push('\n');
                    trim_bridge_stderr_tail(&mut tail);
                }
            }
        });

        let client = Self { inner, workspace };
        let pong =
            match client.request_with_timeout("ping", Map::new(), BRIDGE_STARTUP_PING_TIMEOUT) {
                Ok(pong) => pong,
                Err(error) => {
                    if let Ok(mut child) = client.inner.child.lock() {
                        kill_bridge_process_group(&mut child);
                        let _ = child.wait();
                    }
                    return Err(error);
                }
            };
        if pong.get("type").and_then(Value::as_str) != Some("pong") {
            if let Ok(mut child) = client.inner.child.lock() {
                kill_bridge_process_group(&mut child);
                let _ = child.wait();
            }
            bail!("provider bridge did not answer ping");
        }
        Ok(client)
    }

    pub fn restart(&self) -> Result<()> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            bail!("provider bridge is shutting down");
        }

        self.interrupt_active_requests();
        let generation = self.inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut pending) = self.inner.pending.lock() {
            pending.clear();
        }

        {
            let mut child = self
                .inner
                .child
                .lock()
                .map_err(|_| anyhow!("bridge child lock poisoned"))?;
            if child.try_wait()?.is_none() {
                kill_bridge_process_group(&mut child);
                let _ = child.wait();
            }
        }

        let node = node_executable()?;
        let script = bridge_script()?;
        let mut command = Command::new(node);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(workspace) = &self.workspace {
            command.current_dir(workspace);
            apply_workspace_env(&mut command, workspace)?;
        }
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .context("failed to restart Node provider bridge")?;
        let stdin = child
            .stdin
            .take()
            .context("provider bridge stdin unavailable after restart")?;
        let stdout = child
            .stdout
            .take()
            .context("provider bridge stdout unavailable after restart")?;
        let stderr = child
            .stderr
            .take()
            .context("provider bridge stderr unavailable after restart")?;
        let child = TrackedChild::new(child, "provider-bridge");

        {
            let mut bridge_stdin = self
                .inner
                .stdin
                .lock()
                .map_err(|_| anyhow!("bridge stdin lock poisoned"))?;
            *bridge_stdin = BufWriter::new(stdin);
        }
        {
            let mut bridge_child = self
                .inner
                .child
                .lock()
                .map_err(|_| anyhow!("bridge child lock poisoned"))?;
            *bridge_child = child;
        }
        if let Ok(mut tail) = self.inner.stderr_tail.lock() {
            tail.clear();
        }

        let reader_inner = self.inner.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    let sender = reader_inner
                        .pending
                        .lock()
                        .ok()
                        .and_then(|pending| pending.get(id).cloned());
                    if let Some(sender) = sender {
                        let _ = sender.send(value);
                    }
                } else {
                    let _ = reader_inner.event_tx.send(value);
                }
            }
            if reader_inner.generation.load(Ordering::Acquire) == generation {
                if let Ok(mut pending) = reader_inner.pending.lock() {
                    pending.clear();
                }
                let _ = reader_inner
                    .event_tx
                    .send(json!({ "type": "bridge_closed" }));
            }
        });
        let stderr_tail = self.inner.stderr_tail.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut tail) = stderr_tail.lock() {
                    tail.push_str(&line);
                    tail.push('\n');
                    trim_bridge_stderr_tail(&mut tail);
                }
            }
        });

        let pong = match self.request_with_timeout("ping", Map::new(), BRIDGE_STARTUP_PING_TIMEOUT)
        {
            Ok(pong) => pong,
            Err(error) => {
                if let Ok(mut child) = self.inner.child.lock() {
                    kill_bridge_process_group(&mut child);
                    let _ = child.wait();
                }
                return Err(error);
            }
        };
        if pong.get("type").and_then(Value::as_str) != Some("pong") {
            if let Ok(mut child) = self.inner.child.lock() {
                kill_bridge_process_group(&mut child);
                let _ = child.wait();
            }
            bail!("restarted provider bridge did not answer ping");
        }
        Ok(())
    }

    pub fn request(&self, op: &str, mut fields: Map<String, Value>) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .map_err(|_| anyhow!("bridge pending lock poisoned"))?
            .insert(id.clone(), tx);
        detached_requests::note_request(&id);
        fields.insert("v".into(), json!(BRIDGE_PROTOCOL_VERSION));
        fields.insert("id".into(), json!(id));
        fields.insert("op".into(), json!(op));
        if let Err(error) = self.write_value(&Value::Object(fields)) {
            self.inner.pending.lock().ok().map(|mut p| p.remove(&id));
            return Err(error);
        }
        let frame = rx
            .recv()
            .context("provider bridge closed before response")?;
        self.inner.pending.lock().ok().map(|mut p| p.remove(&id));
        ensure_success(frame)
    }

    fn request_with_timeout(
        &self,
        op: &str,
        mut fields: Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .map_err(|_| anyhow!("bridge pending lock poisoned"))?
            .insert(id.clone(), tx);
        detached_requests::note_request(&id);
        fields.insert("v".into(), json!(BRIDGE_PROTOCOL_VERSION));
        fields.insert("id".into(), json!(id));
        fields.insert("op".into(), json!(op));
        if let Err(error) = self.write_value(&Value::Object(fields)) {
            self.remove_pending(&id);
            return Err(error);
        }
        let frame = match rx.recv_timeout(timeout) {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.remove_pending(&id);
                let tail = self.stderr_tail();
                if tail.trim().is_empty() {
                    bail!("provider bridge {op} timed out after {timeout:?}");
                }
                bail!(
                    "provider bridge {op} timed out after {timeout:?}; stderr: {}",
                    tail.trim()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.remove_pending(&id);
                bail!("provider bridge closed before {op} response");
            }
        };
        self.remove_pending(&id);
        ensure_success(frame)
    }

    pub fn request_cancellable(
        &self,
        op: &str,
        mut fields: Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .map_err(|_| anyhow!("bridge pending lock poisoned"))?
            .insert(id.clone(), tx);
        detached_requests::note_request(&id);
        fields.insert("v".into(), json!(BRIDGE_PROTOCOL_VERSION));
        fields.insert("id".into(), json!(id));
        fields.insert("op".into(), json!(op));
        if let Err(error) = self.write_value(&Value::Object(fields)) {
            self.remove_pending(&id);
            return Err(error);
        }

        loop {
            if cancel.load(Ordering::Acquire) {
                self.cancel(&id);
                self.remove_pending(&id);
                bail!("cancelled");
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(frame) => {
                    self.remove_pending(&id);
                    return ensure_success(frame);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.remove_pending(&id);
                    let tail = self.stderr_tail();
                    if tail.trim().is_empty() {
                        bail!("provider bridge closed before response")
                    }
                    bail!(
                        "provider bridge closed before response; stderr: {}",
                        tail.trim()
                    );
                }
            }
        }
    }

    pub fn request_typed<T: DeserializeOwned>(
        &self,
        op: &str,
        fields: Map<String, Value>,
        key: &str,
    ) -> Result<T> {
        let frame = self.request(op, fields)?;
        serde_json::from_value(
            frame
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow!("bridge response missing {key}"))?,
        )
        .with_context(|| format!("invalid bridge {key} payload"))
    }

    pub fn stream(&self, request: &CallRequest) -> Result<BridgeStream> {
        let request = apply_openai_flex(request, self.inner.openai_flex.load(Ordering::Acquire));
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .map_err(|_| anyhow!("bridge pending lock poisoned"))?
            .insert(id.clone(), tx);
        detached_requests::note_request(&id);
        if let Err(error) = self.write_value(
            &json!({ "v": BRIDGE_PROTOCOL_VERSION, "id": id, "op": "stream", "request": request }),
        ) {
            self.remove_pending(&id);
            return Err(error);
        }
        Ok(BridgeStream {
            client: self.clone(),
            id,
            rx,
            finished: false,
        })
    }

    pub fn jev_evaluate_cancellable(
        &self,
        provider: Option<&str>,
        model: Option<&str>,
        state: Value,
        questions: Value,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let mut fields = Map::new();
        if let Some(provider) = provider {
            fields.insert("provider".into(), json!(provider));
        }
        if let Some(model) = model {
            fields.insert("model".into(), json!(model));
        }
        fields.insert("state".into(), state);
        fields.insert("questions".into(), questions);
        let frame = self.request_cancellable("jev-evaluate", fields, cancel)?;
        frame
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("bridge response missing Jev result"))
    }

    pub fn complete(&self, request: &CallRequest) -> Result<CallResult> {
        let request = apply_openai_flex(request, self.inner.openai_flex.load(Ordering::Acquire));
        let mut fields = Map::new();
        fields.insert("request".into(), serde_json::to_value(&request)?);
        self.request_typed("complete", fields, "result")
    }

    pub fn complete_cancellable(
        &self,
        request: &CallRequest,
        cancel: &AtomicBool,
    ) -> Result<CallResult> {
        let request = apply_openai_flex(request, self.inner.openai_flex.load(Ordering::Acquire));
        let mut fields = Map::new();
        fields.insert("request".into(), serde_json::to_value(&request)?);
        let frame = self.request_cancellable("complete", fields, cancel)?;
        serde_json::from_value(
            frame
                .get("result")
                .cloned()
                .ok_or_else(|| anyhow!("bridge response missing result"))?,
        )
        .context("invalid bridge result payload")
    }

    pub fn set_openai_flex(&self, enabled: bool) {
        self.inner.openai_flex.store(enabled, Ordering::Release);
    }

    pub fn embedding_models(&self, cancel: &AtomicBool) -> Result<Vec<String>> {
        let frame = self.request_cancellable("embedding-models", Map::new(), cancel)?;
        serde_json::from_value(frame.get("models").cloned().unwrap_or(Value::Null))
            .context("Invalid embedding model list")
    }
    pub fn embeddings(
        &self,
        model: &str,
        input: &[String],
        cancel: &AtomicBool,
    ) -> Result<crate::memory::EmbeddingResult> {
        let mut fields = field("model", model);
        fields.insert("input".into(), json!(input));
        let frame = self.request_cancellable("embed", fields, cancel)?;
        serde_json::from_value(frame.get("result").cloned().unwrap_or(Value::Null))
            .context("Invalid embedding result")
    }
    pub fn list_providers(&self) -> Result<Vec<String>> {
        self.request_typed("list-providers", Map::new(), "providers")
    }
    pub fn list_model_info(&self, provider: &str) -> Result<Vec<ModelInfo>> {
        self.request_typed("list-model-info", field("provider", provider), "modelInfo")
    }
    pub fn context_length(&self, model: &str) -> Result<Option<u64>> {
        let frame = self.request("context-length", field("model", model))?;
        Ok(frame.get("contextLength").and_then(Value::as_u64))
    }
    pub fn list_harness_capabilities(&self) -> Result<Vec<HarnessCapabilityDescriptor>> {
        self.request_typed("list-harness-capabilities", Map::new(), "capabilities")
    }
    pub fn auth_status(&self, provider: &str) -> Result<AuthStatus> {
        self.request_typed("auth-status", field("provider", provider), "status")
    }
    pub fn provider_usage(&self, provider: &str) -> Result<ProviderUsageStatus> {
        self.request_typed("provider-usage", field("provider", provider), "usage")
    }
    pub fn set_api_key(&self, provider: &str, key: &str) -> Result<AuthStatus> {
        let mut fields = field("provider", provider);
        fields.insert("apiKey".into(), json!(key));
        self.request_typed("auth-set-api-key", fields, "status")
    }
    pub fn login_browser(&self, provider: &str, options: Option<Value>) -> Result<AuthStatus> {
        let mut fields = field("provider", provider);
        if let Some(options) = options {
            fields.insert("options".into(), options);
        }
        self.request_typed("auth-login-browser", fields, "status")
    }
    pub fn logout(&self, provider: &str) -> Result<AuthStatus> {
        self.request_typed("auth-logout", field("provider", provider), "status")
    }
    pub fn list_provider_configurations(&self) -> Result<Vec<OpenAiCompatibleProvider>> {
        self.request_typed(
            "list-provider-configurations",
            Map::new(),
            "providerConfigurations",
        )
    }
    pub fn save_provider_configuration(
        &self,
        provider: &OpenAiCompatibleProvider,
    ) -> Result<OpenAiCompatibleProvider> {
        let mut fields = Map::new();
        fields.insert("provider".into(), serde_json::to_value(provider)?);
        self.request_typed(
            "save-provider-configuration",
            fields,
            "providerConfiguration",
        )
    }
    pub fn remove_provider_configuration(&self, provider: &str) -> Result<bool> {
        let frame = self.request("remove-provider-configuration", field("provider", provider))?;
        Ok(frame
            .get("removed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
    pub fn list_skills(&self) -> Result<Vec<SkillSummary>> {
        self.request_typed("skill-list", Map::new(), "skills")
    }
    pub fn load_skill(&self, skill: &str) -> Result<Skill> {
        self.request_typed("skill-load", field("skill", skill), "skill")
    }
    pub fn read_skill_file(&self, skill: &str, path: &str) -> Result<String> {
        let mut fields = field("skill", skill);
        fields.insert("path".into(), json!(path));
        self.request_typed("skill-read", fields, "content")
    }
    pub fn validate_skills(&self, source: &str) -> Result<Vec<SkillSummary>> {
        self.request_typed("skill-validate", field("source", source), "skills")
    }
    pub fn install_skills(&self, source: &str) -> Result<Vec<String>> {
        self.request_typed("skill-install", field("source", source), "installed")
    }
    pub fn remove_skill(&self, skill: &str) -> Result<bool> {
        self.request_typed("skill-remove", field("skill", skill), "removed")
    }
    pub fn list_mcp_servers(&self) -> Result<Vec<McpServerStatus>> {
        self.request_typed("mcp-list-servers", Map::new(), "servers")
    }
    pub fn set_mcp_server(
        &self,
        server: &McpServerConfiguration,
    ) -> Result<McpServerConfiguration> {
        let mut fields = Map::new();
        fields.insert("server".into(), serde_json::to_value(server)?);
        self.request_typed("mcp-set-server", fields, "server")
    }
    pub fn set_runtime_mcp_server(
        &self,
        server: &McpServerConfiguration,
    ) -> Result<McpServerConfiguration> {
        let mut fields = Map::new();
        fields.insert("server".into(), serde_json::to_value(server)?);
        self.request_typed("mcp-set-runtime-server", fields, "server")
    }
    pub fn remove_mcp_server(&self, server: &str) -> Result<bool> {
        let frame = self.request("mcp-remove-server", field("server", server))?;
        Ok(frame
            .get("removed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
    pub fn list_mcp_tools(&self, server: Option<&str>) -> Result<Vec<McpTool>> {
        let mut fields = Map::new();
        if let Some(server) = server {
            fields.insert("server".into(), json!(server));
        }
        self.request_typed("mcp-list-tools", fields, "tools")
    }
    pub fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: &Map<String, Value>,
    ) -> Result<Value> {
        let mut fields = field("server", server);
        fields.insert("tool".into(), json!(tool));
        fields.insert("arguments".into(), Value::Object(arguments.clone()));
        let frame = self.request("mcp-call-tool", fields)?;
        Ok(frame.get("toolResult").cloned().unwrap_or(Value::Null))
    }
    pub fn call_mcp_tool_cancellable(
        &self,
        server: &str,
        tool: &str,
        arguments: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let mut fields = field("server", server);
        fields.insert("tool".into(), json!(tool));
        fields.insert("arguments".into(), Value::Object(arguments.clone()));
        let frame = self.request_cancellable("mcp-call-tool", fields, cancel)?;
        Ok(frame.get("toolResult").cloned().unwrap_or(Value::Null))
    }
    pub fn call_computer_use_cancellable(
        &self,
        tool: &str,
        arguments: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let mut fields = field("tool", tool);
        fields.insert("arguments".into(), Value::Object(arguments.clone()));
        let frame = self.request_cancellable("computer-use-call", fields, cancel)?;
        Ok(frame.get("toolResult").cloned().unwrap_or(Value::Null))
    }
    pub fn list_mcp_resources(&self, server: Option<&str>) -> Result<Vec<McpResource>> {
        let mut fields = Map::new();
        if let Some(server) = server {
            fields.insert("server".into(), json!(server));
        }
        self.request_typed("mcp-list-resources", fields, "resources")
    }
    pub fn read_mcp_resource(&self, server: &str, uri: &str) -> Result<Value> {
        let mut fields = field("server", server);
        fields.insert("uri".into(), json!(uri));
        let frame = self.request("mcp-read-resource", fields)?;
        Ok(frame.get("resourceResult").cloned().unwrap_or(Value::Null))
    }
    pub fn list_mcp_prompts(&self, server: Option<&str>) -> Result<Vec<McpPrompt>> {
        let mut fields = Map::new();
        if let Some(server) = server {
            fields.insert("server".into(), json!(server));
        }
        self.request_typed("mcp-list-prompts", fields, "prompts")
    }
    pub fn get_mcp_prompt(
        &self,
        server: &str,
        prompt: &str,
        arguments: &Map<String, Value>,
    ) -> Result<Value> {
        let mut fields = field("server", server);
        fields.insert("prompt".into(), json!(prompt));
        fields.insert("arguments".into(), Value::Object(arguments.clone()));
        let frame = self.request("mcp-get-prompt", fields)?;
        Ok(frame.get("promptResult").cloned().unwrap_or(Value::Null))
    }

    pub fn shutdown(&self) {
        if self.inner.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }

        self.interrupt_active_requests();

        // Do not use request("shutdown") here. A wedged MCP close inside the
        // bridge can otherwise make Yeet's daemon shutdown wait forever. Send
        // the graceful request, then enforce a finite grace period and kill the
        // bridge as a last resort.
        let _ = self.write_value(&json!({
            "v": BRIDGE_PROTOCOL_VERSION,
            "id": Uuid::new_v4().to_string(),
            "op": "shutdown",
        }));

        let started = Instant::now();
        while started.elapsed() < BRIDGE_SHUTDOWN_GRACE {
            let exited = self
                .inner
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok())
                .flatten()
                .is_some();
            if exited {
                return;
            }
            thread::sleep(BRIDGE_SHUTDOWN_POLL);
        }

        if let Ok(mut child) = self.inner.child.lock() {
            kill_bridge_process_group(&mut child);
            let _ = child.wait();
        }
    }

    pub fn stderr_tail(&self) -> String {
        self.inner
            .stderr_tail
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn try_recv_event(&self) -> Option<BridgeEvent> {
        let value = self.inner.events.lock().ok()?.try_recv().ok()?;
        BridgeEvent::from_value(value).ok()
    }

    pub fn interrupt_active_requests(&self) {
        let targets = self
            .inner
            .pending
            .lock()
            .map(|pending| pending.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let detached = detached_requests::retain_live(&targets);
        for target in targets.iter().filter(|id| !detached.contains(*id)) {
            self.cancel(target);
        }
    }

    pub fn send_native_app_approval_decision(
        &self,
        request_id: &str,
        approved: bool,
    ) -> Result<()> {
        self.write_value(&json!({
            "v": BRIDGE_PROTOCOL_VERSION,
            "type": "native_app_approval_decision",
            "requestId": request_id,
            "decision": if approved { "accept" } else { "decline" },
            "scope": "session",
        }))
    }

    fn cancel(&self, target: &str) {
        let id = Uuid::new_v4().to_string();
        let _ = self.write_value(&json!({
            "v": BRIDGE_PROTOCOL_VERSION,
            "id": id,
            "op": "cancel",
            "target": target,
        }));
    }

    fn remove_pending(&self, id: &str) {
        detached_requests::forget(id);
        if let Ok(mut pending) = self.inner.pending.lock() {
            pending.remove(id);
        }
    }

    fn write_value(&self, value: &Value) -> Result<()> {
        let mut stdin = self
            .inner
            .stdin
            .lock()
            .map_err(|_| anyhow!("bridge stdin lock poisoned"))?;
        serde_json::to_writer(&mut *stdin, value)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
}

fn apply_workspace_env(command: &mut Command, workspace: &Path) -> Result<()> {
    let policy = SandboxStore::new(workspace)
        .context("resolve workspace sandbox settings for provider bridge")?
        .load()
        .context("load workspace sandbox environment for provider bridge")?;
    command.envs(policy.environment);
    Ok(())
}

fn kill_bridge_process_group(child: &mut Child) {
    let pid = child.id();
    if force_terminate_process_tree(pid).is_ok() {
        return;
    }
    let _ = child.kill();
}

fn trim_bridge_stderr_tail(tail: &mut String) {
    if tail.len() <= BRIDGE_STDERR_TAIL_BYTES {
        return;
    }
    let mut split = tail.len() - BRIDGE_STDERR_TAIL_BYTES;
    while !tail.is_char_boundary(split) {
        split += 1;
    }
    tail.drain(..split);
}

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    NativeAppApprovalRequest(NativeAppApprovalRequest),
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAppApprovalRequest {
    pub request_id: String,
    pub server: String,
    pub tool: String,
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub operation: String,
    pub message: String,
}

impl BridgeEvent {
    fn from_value(value: Value) -> Result<Self> {
        match value.get("type").and_then(Value::as_str) {
            Some("native_app_approval_request") => Ok(Self::NativeAppApprovalRequest(
                serde_json::from_value(value)?,
            )),
            Some("bridge_closed") => Ok(Self::Closed),
            _ => bail!("unknown bridge event"),
        }
    }
}

pub struct BridgeStream {
    client: BridgeClient,
    id: String,
    rx: mpsc::Receiver<Value>,
    finished: bool,
}

#[allow(clippy::large_enum_variant)]
pub enum StreamPoll {
    Event(StreamEvent),
    Timeout,
    Done,
}

impl BridgeStream {
    pub fn recv(&mut self) -> Result<Option<StreamEvent>> {
        loop {
            let frame = self
                .rx
                .recv()
                .context("provider bridge stream ended unexpectedly")?;
            if let Some(result) = self.decode_frame(frame)? {
                return Ok(result);
            }
        }
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<StreamPoll> {
        loop {
            let frame = match self.rx.recv_timeout(timeout) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(StreamPoll::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let tail = self.client.stderr_tail();
                    if tail.trim().is_empty() {
                        bail!("provider bridge stream ended unexpectedly")
                    } else {
                        bail!(
                            "provider bridge stream ended unexpectedly; stderr: {}",
                            tail.trim()
                        )
                    }
                }
            };
            match self.decode_frame(frame)? {
                Some(Some(event)) => return Ok(StreamPoll::Event(event)),
                Some(None) => return Ok(StreamPoll::Done),
                None => continue,
            }
        }
    }

    fn decode_frame(&mut self, frame: Value) -> Result<Option<Option<StreamEvent>>> {
        match frame
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "event" => {
                let event = frame
                    .get("event")
                    .cloned()
                    .ok_or_else(|| anyhow!("stream frame missing event"))?;
                Ok(Some(Some(StreamEvent::from_value(event)?)))
            }
            "done" => {
                self.finished = true;
                self.client.remove_pending(&self.id);
                Ok(Some(None))
            }
            "error" => {
                self.finished = true;
                self.client.remove_pending(&self.id);
                Err(remote_error(&frame))
            }
            _ => Ok(None),
        }
    }

    pub fn cancel(&mut self) {
        if !self.finished {
            self.client.cancel(&self.id);
            self.client.remove_pending(&self.id);
            self.finished = true;
        }
    }
}

impl Drop for BridgeStream {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) fn field(name: &str, value: impl Serialize) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert(
        name.into(),
        serde_json::to_value(value).unwrap_or(Value::Null),
    );
    fields
}

fn ensure_success(frame: Value) -> Result<Value> {
    if frame.get("type").and_then(Value::as_str) == Some("error") {
        return Err(remote_error(&frame));
    }
    Ok(frame)
}

fn remote_error(frame: &Value) -> anyhow::Error {
    let error = frame.get("error").cloned().unwrap_or(Value::Null);
    let name = error
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("BridgeError");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown bridge error");
    let provider = error.get("provider").and_then(Value::as_str);
    let server = error.get("server").and_then(Value::as_str);
    let body = error
        .get("responseBody")
        .and_then(Value::as_str)
        .unwrap_or("");
    let prefix = if let Some(provider) = provider {
        format!("{name} [{provider}]: {message}")
    } else if let Some(server) = server {
        format!("{name} [MCP {server}]: {message}")
    } else {
        format!("{name}: {message}")
    };
    if body.trim().is_empty() {
        anyhow!(prefix)
    } else {
        let bounded: String = body.chars().take(800).collect();
        anyhow!("{prefix} — response: {bounded}")
    }
}
