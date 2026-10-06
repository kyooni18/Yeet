//! Provider-neutral runtime types and the persistent provider bridge protocol.
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::platform::{TrackedChild, configure_process_group, force_terminate_process_tree};
use crate::sandbox::SandboxStore;
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use uuid::Uuid;

mod detached_requests;
mod mcp_runtime;
mod provider_bridge;
mod runtime;
pub(crate) use detached_requests::run_detached;
pub use provider_bridge::{
    BridgeClient, BridgeEvent, BridgeStream, NativeAppApprovalRequest, StreamPoll,
};
use runtime::bridge_script;
pub use runtime::{
    edit_daemon_script, node_executable, runtime_directory, set_host_runtime_directory,
};

pub const BRIDGE_PROTOCOL_VERSION: u64 = 1;
const BRIDGE_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const BRIDGE_SHUTDOWN_POLL: Duration = Duration::from_millis(20);
const BRIDGE_STARTUP_PING_TIMEOUT: Duration = Duration::from_secs(3);
const BRIDGE_STDERR_TAIL_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    /// Input tokens for logical model calls where the provider explicitly reported cache-read telemetry.
    pub cache_measured_input_tokens: Option<u64>,
    /// Input tokens for logical model calls where cache-read telemetry was absent.
    pub cache_unreported_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub cost_equivalent_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub transport_attempts: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub provider_cache_diagnostic_type: Option<String>,
    pub provider_cache_miss_reason: Option<String>,
    pub provider_cache_missed_tokens: Option<u64>,
    pub provider_comparison_reusable_tokens: Option<u64>,
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
        let cache_measured_input_tokens = other.cache_measured_input_tokens.or_else(|| {
            other
                .input_tokens
                .filter(|_| other.cached_input_tokens.is_some())
        });
        let cache_unreported_input_tokens = other.cache_unreported_input_tokens.or_else(|| {
            other
                .input_tokens
                .filter(|_| other.cached_input_tokens.is_none())
        });
        self.cache_measured_input_tokens = add(
            self.cache_measured_input_tokens,
            cache_measured_input_tokens,
        );
        self.cache_unreported_input_tokens = add(
            self.cache_unreported_input_tokens,
            cache_unreported_input_tokens,
        );
        self.cache_write_input_tokens = add(
            self.cache_write_input_tokens,
            other.cache_write_input_tokens,
        );
        self.cost_equivalent_input_tokens = add(
            self.cost_equivalent_input_tokens,
            other.cost_equivalent_input_tokens,
        );
        self.reasoning_tokens = add(self.reasoning_tokens, other.reasoning_tokens);
        self.model_calls = add(self.model_calls, other.model_calls);
        self.transport_attempts = add(self.transport_attempts, other.transport_attempts);
        self.estimated_cost_usd = match (self.estimated_cost_usd, other.estimated_cost_usd) {
            (None, None) => None,
            (lhs, rhs) => Some(lhs.unwrap_or(0.0) + rhs.unwrap_or(0.0)),
        };
    }

    /// Returns a cache-rate view whose denominator includes only calls where
    /// cache-read telemetry was explicitly reported. Older persisted sessions
    /// without coverage fields are marked unclassified rather than guessed.
    pub fn cache_measurement(&self) -> CacheUsageMeasurement {
        let input_tokens = self.input_tokens.unwrap_or(0);
        let cached_input_tokens = self.cached_input_tokens.unwrap_or(0).min(input_tokens);
        let cache_write_input_tokens = self.cache_write_input_tokens.unwrap_or(0);
        let has_coverage = self.cache_measured_input_tokens.is_some()
            || self.cache_unreported_input_tokens.is_some();
        let measured_input_tokens = if has_coverage {
            self.cache_measured_input_tokens
                .unwrap_or(0)
                .min(input_tokens)
        } else {
            0
        };
        let unreported_input_tokens = if has_coverage {
            self.cache_unreported_input_tokens
                .unwrap_or(0)
                .min(input_tokens.saturating_sub(measured_input_tokens))
        } else {
            0
        };
        let unclassified_input_tokens = input_tokens
            .saturating_sub(measured_input_tokens)
            .saturating_sub(unreported_input_tokens);
        let hit_rate = (measured_input_tokens > 0 && unclassified_input_tokens == 0).then(|| {
            cached_input_tokens.min(measured_input_tokens) as f64 / measured_input_tokens as f64
        });
        let measurement_coverage_rate =
            (input_tokens > 0).then(|| measured_input_tokens as f64 / input_tokens as f64);
        CacheUsageMeasurement {
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            measured_input_tokens,
            unreported_input_tokens,
            unclassified_input_tokens,
            hit_rate,
            measurement_coverage_rate,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CacheUsageMeasurement {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub measured_input_tokens: u64,
    pub unreported_input_tokens: u64,
    pub unclassified_input_tokens: u64,
    pub hit_rate: Option<f64>,
    pub measurement_coverage_rate: Option<f64>,
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
    pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

    pub fn from_bytes(
        media_type: impl Into<String>,
        bytes: &[u8],
        name: Option<String>,
    ) -> Result<Self> {
        if bytes.len() > Self::MAX_IMAGE_BYTES {
            bail!("Image is larger than 20 MiB");
        }
        let media_type = media_type.into();
        if !matches!(
            media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            bail!("Unsupported image media type: {media_type}");
        }
        Ok(Self {
            media_type,
            data: BASE64.encode(bytes),
            name,
        })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("read image metadata {}", path.display()))?;
        if !metadata.is_file() {
            bail!("Image path is not a file: {}", path.display());
        }
        if metadata.len() > Self::MAX_IMAGE_BYTES as u64 {
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
        Self::from_bytes(
            media_type,
            &bytes,
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned),
        )
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
pub struct ProviderState {
    pub provider: String,
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub data: Value,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<ProviderState>,
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
    pub fn with_provider_state(mut self, provider_state: Option<ProviderState>) -> Self {
        self.provider_state = provider_state;
        self
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
            provider_state: None,
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
    /// Optional tools known to the harness but omitted from the ordinary
    /// schema surface. Provider adapters may expose these through a native
    /// deferred-tool mechanism; unsupported providers ignore this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<HashMap<String, String>>,
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
            deferred_tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
            provider_metadata: None,
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
    pub provider_state: Option<ProviderState>,
    pub finish_reason: String,
    pub usage: Option<Usage>,

    pub raw: Option<Value>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Start,
    ReasoningStart,
    Activity {
        title: String,
        detail: Option<String>,
    },
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
        provider_state: Option<ProviderState>,
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
            "reasoning-start" => Self::ReasoningStart,
            "activity" => Self::Activity {
                title: string_field(&value, "title")?,
                detail: value
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
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
                provider_state: value
                    .get("providerState")
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindow {
    pub id: String,
    pub label: String,
    pub used_percent: u8,
    pub remaining_percent: u8,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageStatus {
    pub provider: String,
    pub available: bool,
    pub source: String,
    pub fetched_at: String,
    pub plan: Option<String>,
    pub windows: Vec<ProviderUsageWindow>,
    pub message: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

fn string_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("missing string field {key}"))
}

#[cfg(test)]
mod provider_state_tests {
    use super::*;

    #[test]
    fn provider_state_round_trips_through_message_json() {
        let message = Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "call-1".into(),
                name: "lookup".into(),
                arguments: json!({"q":"x"}),
            }]),
        )
        .with_provider_state(Some(ProviderState {
            provider: "gemini".into(),
            protocol: "gemini-generate-content".into(),
            model: Some("gemini-test".into()),
            data: json!({"functionMetadata":{"call-1":{"thoughtSignature":"sig"}}}),
        }));

        let encoded = serde_json::to_value(&message).unwrap();
        assert_eq!(
            encoded["providerState"]["protocol"],
            "gemini-generate-content"
        );
        assert!(encoded.get("provider_state").is_none());
        let decoded: Message = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn request_keeps_internal_and_provider_metadata_distinct() {
        let mut request = CallRequest::simple("openai/gpt-test", vec![Message::user("hello")]);
        request.metadata = Some(HashMap::from([("lane".into(), "lead".into())]));
        request.provider_metadata = Some(HashMap::from([("trace".into(), "wire".into())]));
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["metadata"]["lane"], "lead");
        assert_eq!(encoded["providerMetadata"]["trace"], "wire");
        assert!(encoded.get("provider_metadata").is_none());
    }
}
