use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub const BRIDGE_PROTOCOL_VERSION: u64 = 1;
const BRIDGE_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const BRIDGE_SHUTDOWN_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

impl Usage {
    pub fn accumulate(&mut self, other: &Usage) {
        fn add(lhs: Option<u64>, rhs: Option<u64>) -> Option<u64> {
            match (lhs, rhs) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
            }
        }
        self.input_tokens = add(self.input_tokens, other.input_tokens);
        self.output_tokens = add(self.output_tokens, other.output_tokens);
        self.total_tokens = add(self.total_tokens, other.total_tokens);
        self.cached_input_tokens = add(self.cached_input_tokens, other.cached_input_tokens);
        self.cache_write_input_tokens = add(
            self.cache_write_input_tokens,
            other.cache_write_input_tokens,
        );
        self.reasoning_tokens = add(self.reasoning_tokens, other.reasoning_tokens);
        self.model_calls = add(self.model_calls, other.model_calls);
        self.estimated_cost_usd = match (self.estimated_cost_usd, other.estimated_cost_usd) {
            (None, None) => None,
            (lhs, rhs) => Some(lhs.unwrap_or(0.0) + rhs.unwrap_or(0.0)),
        };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub media_type: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ImageAttachment {
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
        let metadata = fs::metadata(path)
            .with_context(|| format!("read image metadata {}", path.display()))?;
        if !metadata.is_file() {
            bail!("Image path is not a file: {}", path.display());
        }
        if metadata.len() > MAX_IMAGE_BYTES {
            bail!("Image is larger than 20 MiB: {}", path.display());
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let media_type = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => bail!(
                "Unsupported image type for {}. Use PNG, JPEG, WebP, or GIF.",
                path.display()
            ),
        };
        let bytes = fs::read(path).with_context(|| format!("read image {}", path.display()))?;
        Ok(Self {
            media_type: media_type.into(),
            data: BASE64.encode(bytes),
            name: path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Request-local guidance that must not become durable conversation
    /// history or part of a reusable prompt-cache prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_only: Option<bool>,
    /// Explicit end of a reusable provider prompt-cache prefix. This is set
    /// only on request clones, never on durable conversation history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_breakpoint: Option<bool>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(MessageRole::System, Some(content.into()))
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, Some(content.into()))
    }
    pub fn user_with_images(content: impl Into<String>, images: Vec<ImageAttachment>) -> Self {
        let mut value = Self::new(MessageRole::User, Some(content.into()));
        if !images.is_empty() {
            value.images = Some(images);
        }
        value
    }
    pub fn assistant(content: impl Into<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        let mut value = Self::new(MessageRole::Assistant, Some(content.into()));
        value.tool_calls = tool_calls;
        value
    }
    pub fn tool(
        content: impl Into<String>,
        tool_call_id: impl Into<String>,
        name: Option<String>,
    ) -> Self {
        let mut value = Self::new(MessageRole::Tool, Some(content.into()));
        value.tool_call_id = Some(tool_call_id.into());
        value.name = name;
        value
    }
    pub fn request_only(mut self) -> Self {
        self.request_only = Some(true);
        self
    }
    pub fn cache_breakpoint(mut self) -> Self {
        self.cache_breakpoint = Some(true);
        self
    }
    fn new(role: MessageRole, content: Option<String>) -> Self {
        Self {
            role,
            content,
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            request_only: None,
            cache_breakpoint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Map<String, Value>,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, schema: Value) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_capabilities: Option<Vec<String>>,
    /// Opt in to provider prompt caching for a replayable conversation lane.
    /// The runtime chooses an explicit cache boundary after request
    /// capabilities and compaction have finished transforming the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<bool>,
}

impl CallRequest {
    pub fn simple(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            context_key: None,
            system: None,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
            timeout_ms: None,
            retry: None,
            provider_options: None,
            attached_capabilities: None,
            prompt_cache: None,
        }
    }
}

fn apply_openai_flex(request: &CallRequest, enabled: bool) -> CallRequest {
    if !enabled
        || request
            .model
            .split_once('/')
            .map(|(provider, _)| provider != "openai")
            .unwrap_or(true)
    {
        return request.clone();
    }

    let mut request = request.clone();
    request
        .provider_options
        .get_or_insert_with(Map::new)
        .insert("service_tier".into(), json!("flex"));
    request
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallResult {
    pub provider: String,
    pub model: String,
    pub id: Option<String>,
    pub text: String,
    pub reasoning: Option<String>,
    pub reasoning_summary: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Option<Usage>,
    pub raw: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Start,
    ReasoningDelta(String),
    ReasoningSummaryDelta(String),
    TextDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    ToolCall {
        index: usize,
        tool_call: ToolCall,
    },
    Finish {
        finish_reason: String,
        usage: Option<Usage>,
    },
}

impl StreamEvent {
    fn from_value(value: Value) -> Result<Self> {
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(match kind {
            "start" => Self::Start,
            "reasoning-delta" => Self::ReasoningDelta(string_field(&value, "delta")?),
            "reasoning-summary-delta" => {
                Self::ReasoningSummaryDelta(string_field(&value, "delta")?)
            }
            "text-delta" => Self::TextDelta(string_field(&value, "delta")?),
            "tool-call-delta" => Self::ToolCallDelta {
                index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                id: value.get("id").and_then(Value::as_str).map(str::to_owned),
                name: value.get("name").and_then(Value::as_str).map(str::to_owned),
                arguments_delta: value
                    .get("argumentsDelta")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "tool-call" => Self::ToolCall {
                index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                tool_call: serde_json::from_value(
                    value
                        .get("toolCall")
                        .cloned()
                        .ok_or_else(|| anyhow!("tool-call event missing toolCall"))?,
                )?,
            },
            "finish" => Self::Finish {
                finish_reason: string_field(&value, "finishReason")?,
                usage: value
                    .get("usage")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?,
            },
            other => bail!("unknown stream event type: {other}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilityDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_attached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub provider: String,
    pub authenticated: bool,
    pub method: String,
    pub expires_at: Option<String>,
    pub config_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompatibleProvider {
    #[serde(default = "openai_compatible_kind")]
    pub kind: String,
    pub id: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub require_api_key: Option<bool>,
}

fn openai_compatible_kind() -> String {
    "openai-compatible".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub root: String,
    pub entrypoint: String,
    #[serde(default)]
    pub short_description: Option<String>,
    #[serde(default)]
    pub allow_implicit_invocation: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub root: String,
    pub entrypoint: String,
    pub instructions: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub short_description: Option<String>,
    #[serde(default)]
    pub allow_implicit_invocation: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub connected: bool,
    #[serde(rename = "protocol")]
    pub protocol_version: Option<String>,
    pub era: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub qualified_name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Map<String, Value>,
    pub output_schema: Option<Map<String, Value>>,
    pub annotations: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub server: String,
    pub uri: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPrompt {
    pub server: String,
    pub name: String,
    pub qualified_name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub arguments: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfiguration {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug)]
struct BridgeInner {
    child: Mutex<Child>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    pending: Mutex<HashMap<String, mpsc::Sender<Value>>>,
    stderr_tail: Arc<Mutex<String>>,
    events: Mutex<mpsc::Receiver<Value>>,
    event_tx: mpsc::Sender<Value>,
    openai_flex: AtomicBool,
    shutting_down: AtomicBool,
}

#[derive(Debug, Clone)]
/// Synchronous client for the long-lived RuntimeSource bridge process and request protocol.
pub struct BridgeClient {
    inner: Arc<BridgeInner>,
}

impl BridgeClient {
    pub fn start() -> Result<Self> {
        let node = node_executable()?;
        let script = bridge_script()?;
        let mut command = Command::new(node);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        unsafe {
            // Keep the provider bridge and every MCP subprocess it starts in
            // one killable process group. This prevents a forced bridge
            // shutdown from leaving MCP servers orphaned behind it.
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
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
            if let Ok(mut pending) = reader_inner.pending.lock() {
                pending.clear();
            }
            let _ = reader_inner
                .event_tx
                .send(json!({ "type": "bridge_closed" }));
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut tail) = stderr_tail.lock() {
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.len() > 16 * 1024 {
                        let keep = tail.len() - 16 * 1024;
                        tail.drain(..keep);
                    }
                }
            }
        });

        let client = Self { inner };
        let pong = client.request("ping", Map::new())?;
        if pong.get("type").and_then(Value::as_str) != Some("pong") {
            bail!("provider bridge did not answer ping");
        }
        Ok(client)
    }

    pub fn request(&self, op: &str, mut fields: Map<String, Value>) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .map_err(|_| anyhow!("bridge pending lock poisoned"))?
            .insert(id.clone(), tx);
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
        self.write_value(
            &json!({ "v": BRIDGE_PROTOCOL_VERSION, "id": id, "op": "stream", "request": request }),
        )?;
        Ok(BridgeStream {
            client: self.clone(),
            id,
            rx,
            finished: false,
        })
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
        for target in targets {
            self.cancel(&target);
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

fn kill_bridge_process_group(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let pid = child.id() as i32;
        if pid > 0 && libc::kill(-pid, libc::SIGKILL) == 0 {
            return;
        }
    }
    let _ = child.kill();
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

pub fn runtime_directory() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_RUNTIME_DIR") {
        let path = PathBuf::from(value);
        if path.join("dist/bridge.js").is_file() {
            return Ok(path);
        }
        bail!(
            "YEET_RUNTIME_DIR does not contain dist/bridge.js: {}",
            path.display()
        );
    }
    if let Ok(exe) = env::current_exe()
        && let Some(bin) = exe.parent()
    {
        if let Some(prefix) = bin.parent() {
            let path = prefix.join("share/yeet/runtime");
            if path.join("dist/bridge.js").is_file() {
                return Ok(path);
            }
        }
        let beside = bin.join("runtime");
        if beside.join("dist/bridge.js").is_file() {
            return Ok(beside);
        }
    }
    let cwd = PathBuf::from("RuntimeSource");
    if cwd.join("dist/bridge.js").is_file() {
        return fs::canonicalize(cwd).context("canonicalize RuntimeSource");
    }
    bail!("Yeet runtime not found; set YEET_RUNTIME_DIR or install RuntimeSource beside the binary")
}

pub fn node_executable() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_NODE") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Ok(path); // allow PATH-style command names supplied explicitly
    }
    Ok(PathBuf::from("node"))
}

fn bridge_script() -> Result<PathBuf> {
    Ok(runtime_directory()?.join("dist/bridge.js"))
}

pub fn edit_daemon_script() -> Result<PathBuf> {
    Ok(runtime_directory()?.join("dist/edit-backend/daemon.js"))
}

fn field(name: &str, value: impl Serialize) -> Map<String, Value> {
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

fn string_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("missing string field {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_request_uses_typescript_field_names() {
        let mut request = CallRequest::simple("openai/test", vec![Message::user("hello")]);
        request.context_key = Some("ctx".into());
        request.tool_choice = Some(json!("auto"));
        request.prompt_cache = Some(true);
        request
            .messages
            .push(Message::system("volatile").request_only());
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["contextKey"], "ctx");
        assert_eq!(value["toolChoice"], "auto");
        assert_eq!(value["promptCache"], true);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][1]["requestOnly"], true);
    }

    #[test]
    fn openai_flex_preserves_existing_provider_options() {
        let mut request = CallRequest::simple("openai/gpt-5.6-luna", vec![Message::user("hello")]);
        request.provider_options = Some(Map::from_iter([(
            "reasoning".into(),
            json!({ "effort": "high" }),
        )]));

        let prepared = apply_openai_flex(&request, true);

        let options = prepared.provider_options.unwrap();
        assert_eq!(options["service_tier"], "flex");
        assert_eq!(options["reasoning"]["effort"], "high");
    }

    #[test]
    fn openai_flex_never_leaks_to_other_providers() {
        for model in [
            "openrouter/openai/gpt-5.6-luna",
            "anthropic/claude-sonnet",
            "local/gpt-5.6-luna",
        ] {
            let request = CallRequest::simple(model, vec![Message::user("hello")]);
            assert!(apply_openai_flex(&request, true).provider_options.is_none());
        }
    }

    #[test]
    fn disabled_openai_flex_leaves_openai_request_untouched() {
        let request = CallRequest::simple("openai/gpt-5.6-luna", vec![Message::user("hello")]);
        assert!(
            apply_openai_flex(&request, false)
                .provider_options
                .is_none()
        );
    }

    #[test]
    fn unsolicited_native_approval_event_decodes_without_response_id() {
        let event = BridgeEvent::from_value(json!({
            "v": 1,
            "type": "native_app_approval_request",
            "requestId": "9001",
            "server": "computer-use",
            "tool": "open",
            "bundleId": "org.blenderfoundation.blender",
            "appName": "Blender",
            "operation": "Open Blender",
            "message": "Allow Blender?",
        }))
        .unwrap();
        match event {
            BridgeEvent::NativeAppApprovalRequest(request) => {
                assert_eq!(request.request_id, "9001");
                assert_eq!(
                    request.bundle_id.as_deref(),
                    Some("org.blenderfoundation.blender")
                );
            }
            BridgeEvent::Closed => panic!("wrong bridge event"),
        }
    }
}
