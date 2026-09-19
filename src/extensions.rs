use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    config::ConfigStore,
    model::{BridgeState, ConversationKind, ExtensionCommandItem, ToolCallStatus},
    platform::{configure_process_group, force_terminate_process_tree},
};

pub const EXTENSION_PROTOCOL: &str = "yeet.extension.v1";
pub const EXTENSION_SCHEMA_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "extension.json";

fn schema_version_one() -> u32 {
    EXTENSION_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    #[serde(default = "schema_version_one")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default = "default_events")]
    pub events: Vec<String>,
    #[serde(default)]
    pub commands: Vec<ExtensionCommand>,
    pub entry: ExtensionEntry,
}

fn default_events() -> Vec<String> {
    vec!["state".into()]
}

impl ExtensionManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EXTENSION_SCHEMA_VERSION {
            bail!(
                "unsupported extension schemaVersion {} (expected {})",
                self.schema_version,
                EXTENSION_SCHEMA_VERSION
            );
        }
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            bail!("extension name must not be empty");
        }
        if self.version.trim().is_empty() {
            bail!("extension version must not be empty");
        }
        if self.entry.command.trim().is_empty() {
            bail!("extension entry.command must not be empty");
        }
        let mut seen_commands = std::collections::HashSet::new();
        for command in &self.commands {
            validate_command_name(&command.name)?;
            if !seen_commands.insert(command.name.to_ascii_lowercase()) {
                bail!("duplicate extension command: {}", command.name);
            }
        }
        Ok(())
    }

    pub fn supports_current_platform(&self) -> bool {
        if self.platforms.is_empty() {
            return true;
        }
        let platform = current_platform();
        self.platforms
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(platform))
    }

    fn subscribes_to(&self, event: &str) -> bool {
        self.events.is_empty()
            || self
                .events
                .iter()
                .any(|candidate| candidate == "*" || candidate.eq_ignore_ascii_case(event))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredExtension {
    pub manifest: ExtensionManifest,
    pub directory: PathBuf,
    pub scope: ExtensionScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionScope {
    Global,
    Project,
}

impl ExtensionScope {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionActivity {
    pub phase: String,
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionConversationMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionState {
    pub is_streaming: bool,
    pub model: String,
    pub reasoning_level: String,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub pending_permission: Option<String>,
    pub error_message: Option<String>,
    pub activity: Option<ExtensionActivity>,
    pub active_tool: Option<String>,
    pub available_models: Vec<String>,
    pub messages: Vec<ExtensionConversationMessage>,
    pub assistant_text: String,
}

impl ExtensionState {
    pub fn from_bridge(state: &BridgeState) -> Self {
        let activity = active_activity(state);
        let active_tool = active_tool(state);
        let pending_permission = if state.pending_native_app_permission.is_some() {
            Some("native-app".into())
        } else if state.pending_shell_permission.is_some() {
            Some("shell".into())
        } else {
            None
        };
        let available_models = if state.model_catalog.is_empty() {
            state.available_models.clone()
        } else {
            state
                .model_catalog
                .iter()
                .map(|item| item.id.clone())
                .collect()
        };

        Self {
            is_streaming: state.is_streaming,
            model: state.active_model.clone(),
            reasoning_level: state.active_reasoning_level.clone(),
            session_id: state.current_session_id.clone(),
            run_id: state.active_run_id.clone(),
            pending_permission,
            error_message: state.error_message.clone(),
            activity,
            active_tool,
            available_models,
            messages: extension_messages(state),
            assistant_text: state.active_assistant_text.clone(),
        }
    }
}

fn extension_messages(state: &BridgeState) -> Vec<ExtensionConversationMessage> {
    let Some(conversation) = state.conversation.as_ref() else {
        return Vec::new();
    };
    let mut messages = conversation
        .iter()
        .filter_map(|entry| match &entry.kind {
            ConversationKind::User { content } if !content.trim().is_empty() => {
                Some(ExtensionConversationMessage {
                    role: "user".into(),
                    content: content.clone(),
                })
            }
            ConversationKind::Assistant { content, .. } if !content.trim().is_empty() => {
                Some(ExtensionConversationMessage {
                    role: "assistant".into(),
                    content: content.clone(),
                })
            }
            _ => None,
        })
        .rev()
        .take(24)
        .collect::<Vec<_>>();
    messages.reverse();
    messages
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionRequest {
    Submit { extension_id: String, text: String },
    SelectModel { extension_id: String, model: String },
    SelectReasoning { extension_id: String, level: String },
    Interrupt { extension_id: String },
    NewSession { extension_id: String },
    RequestModels { extension_id: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionRequestEnvelope {
    protocol: String,
    action: String,
    #[serde(default)]
    payload: Value,
}

fn active_activity(state: &BridgeState) -> Option<ExtensionActivity> {
    let id = state.active_activity_entry_id.as_deref()?;
    let conversation = state.conversation.as_ref()?;
    let entry = conversation.iter().rev().find(|entry| entry.id == id)?;
    let ConversationKind::Activity { activity } = &entry.kind else {
        return None;
    };
    Some(ExtensionActivity {
        phase: value_to_compact_string(&activity.phase),
        title: activity.title.clone(),
        detail: activity.detail.clone(),
    })
}

fn active_tool(state: &BridgeState) -> Option<String> {
    let conversation = state.conversation.as_ref()?;
    for entry in conversation.iter().rev() {
        match &entry.kind {
            ConversationKind::ToolCall { tool_call }
                if matches!(
                    tool_call.status,
                    ToolCallStatus::Preparing
                        | ToolCallStatus::AwaitingPermission
                        | ToolCallStatus::Running
                ) =>
            {
                return Some(tool_call.name.clone());
            }
            ConversationKind::Assistant { tool_calls, .. } => {
                if let Some(tool_call) = tool_calls.iter().rev().find(|tool_call| {
                    matches!(
                        tool_call.status,
                        ToolCallStatus::Preparing
                            | ToolCallStatus::AwaitingPermission
                            | ToolCallStatus::Running
                    )
                }) {
                    return Some(tool_call.name.clone());
                }
            }
            _ => {}
        }
    }
    None
}

fn value_to_compact_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionEnvelope<'a> {
    protocol: &'static str,
    sequence: u64,
    timestamp: String,
    event: &'a str,
    workspace: String,
    payload: Value,
}

struct RunningExtension {
    manifest: ExtensionManifest,
    child: Child,
    input: BufWriter<ChildStdin>,
}

impl RunningExtension {
    fn send(&mut self, event: &ExtensionEnvelope<'_>) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            bail!("extension process exited");
        }
        serde_json::to_writer(&mut self.input, event)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }

    fn stop(&mut self) {
        let _ = self.input.flush();
        if let Ok(None) = self.child.try_wait() {
            let pid = self.child.id();
            if force_terminate_process_tree(pid).is_err() {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }
}

pub struct ExtensionHost {
    workspace: PathBuf,
    discovered: BTreeMap<String, DiscoveredExtension>,
    running: Mutex<HashMap<String, RunningExtension>>,
    last_state: Mutex<Option<ExtensionState>>,
    request_tx: Sender<ExtensionRequest>,
    request_rx: Mutex<Receiver<ExtensionRequest>>,
    sequence: AtomicU64,
}

impl ExtensionHost {
    pub fn discover_and_start(config_dir: &Path, workspace: &Path) -> Self {
        let discovered = discover_extensions(config_dir, workspace).unwrap_or_else(|error| {
            eprintln!("yeet: extension discovery failed: {error}");
            BTreeMap::new()
        });
        let (request_tx, request_rx) = mpsc::channel();
        let host = Self {
            workspace: workspace.to_path_buf(),
            discovered,
            running: Mutex::new(HashMap::new()),
            last_state: Mutex::new(None),
            request_tx,
            request_rx: Mutex::new(request_rx),
            sequence: AtomicU64::new(1),
        };

        let auto_start = host
            .discovered
            .values()
            .filter(|extension| {
                extension.manifest.auto_start && extension.manifest.supports_current_platform()
            })
            .map(|extension| extension.manifest.id.clone())
            .collect::<Vec<_>>();
        for id in auto_start {
            if let Err(error) = host.start(&id) {
                eprintln!("yeet: failed to start extension {id}: {error}");
            }
        }
        host
    }

    pub fn start(&self, id: &str) -> Result<()> {
        let extension = self
            .discovered
            .get(id)
            .ok_or_else(|| anyhow!("unknown extension: {id}"))?;
        if !extension.manifest.supports_current_platform() {
            bail!("extension {id} does not support {}", current_platform());
        }

        let mut running = self.running.lock().unwrap();
        if running.contains_key(id) {
            return Ok(());
        }

        let mut process = spawn_extension(extension, &self.workspace, self.request_tx.clone())?;
        let hello = self.envelope(
            "hello",
            json!({
                "extensionId": extension.manifest.id,
                "name": extension.manifest.name,
                "version": extension.manifest.version,
                "scope": extension.scope.label(),
            }),
        );
        process.send(&hello)?;
        running.insert(id.to_owned(), process);
        Ok(())
    }

    pub fn command_items(&self) -> Vec<ExtensionCommandItem> {
        let mut items = self
            .discovered
            .values()
            .filter(|extension| extension.manifest.supports_current_platform())
            .flat_map(|extension| {
                extension
                    .manifest
                    .commands
                    .iter()
                    .map(move |command| ExtensionCommandItem {
                        extension_id: extension.manifest.id.clone(),
                        command: format!("/{}", command.name),
                        description: command.description.clone(),
                    })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.command.cmp(&right.command));
        items.dedup_by(|left, right| left.command.eq_ignore_ascii_case(&right.command));
        items
    }

    pub fn invoke_command(&self, command: &str, args: &[String]) -> Result<bool> {
        let normalized = command.trim().trim_start_matches('/').to_ascii_lowercase();
        let Some(extension_id) = self
            .discovered
            .values()
            .filter(|extension| extension.manifest.supports_current_platform())
            .find_map(|extension| {
                extension
                    .manifest
                    .commands
                    .iter()
                    .any(|candidate| candidate.name.eq_ignore_ascii_case(&normalized))
                    .then(|| extension.manifest.id.clone())
            })
        else {
            return Ok(false);
        };

        self.start(&extension_id)?;
        let envelope = self.envelope(
            "command",
            json!({
                "name": normalized,
                "args": args,
            }),
        );
        let mut running = self.running.lock().unwrap();
        let extension = running
            .get_mut(&extension_id)
            .ok_or_else(|| anyhow!("extension {extension_id} did not start"))?;
        extension.send(&envelope)?;
        Ok(true)
    }

    pub fn try_recv_request(&self) -> Option<ExtensionRequest> {
        self.request_rx.lock().unwrap().try_recv().ok()
    }

    pub fn publish_state(&self, state: &BridgeState) {
        let snapshot = ExtensionState::from_bridge(state);
        {
            let mut previous = self.last_state.lock().unwrap();
            if previous.as_ref() == Some(&snapshot) {
                return;
            }
            *previous = Some(snapshot.clone());
        }
        self.broadcast("state", json!(snapshot));
    }

    fn broadcast(&self, event_name: &str, payload: Value) {
        let envelope = self.envelope(event_name, payload);
        let mut running = self.running.lock().unwrap();
        let mut dead = Vec::new();
        for (id, extension) in running.iter_mut() {
            if !extension.manifest.subscribes_to(event_name) {
                continue;
            }
            if let Err(error) = extension.send(&envelope) {
                eprintln!("yeet: extension {id} disconnected: {error}");
                dead.push(id.clone());
            }
        }
        for id in dead {
            if let Some(mut extension) = running.remove(&id) {
                extension.stop();
            }
        }
    }

    fn envelope<'a>(&self, event: &'a str, payload: Value) -> ExtensionEnvelope<'a> {
        ExtensionEnvelope {
            protocol: EXTENSION_PROTOCOL,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            timestamp: Utc::now().to_rfc3339(),
            event,
            workspace: self.workspace.display().to_string(),
            payload,
        }
    }
}

impl Drop for ExtensionHost {
    fn drop(&mut self) {
        let running = self.running.get_mut().unwrap();
        for extension in running.values_mut() {
            extension.stop();
        }
        running.clear();
    }
}

fn spawn_extension(
    extension: &DiscoveredExtension,
    workspace: &Path,
    request_tx: Sender<ExtensionRequest>,
) -> Result<RunningExtension> {
    let entry = &extension.manifest.entry;
    let command = resolve_command(&entry.command, &extension.directory, workspace);
    let mut process = Command::new(&command);
    process
        .args(entry.args.iter().map(|arg| expand_value(arg, workspace)))
        .current_dir(&extension.directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("YEET_EXTENSION_PROTOCOL", EXTENSION_PROTOCOL)
        .env("YEET_EXTENSION_ID", &extension.manifest.id)
        .env("YEET_EXTENSION_DIR", &extension.directory)
        .env("YEET_WORKSPACE", workspace);
    for (key, value) in &entry.env {
        process.env(key, expand_value(value, workspace));
    }
    configure_process_group(&mut process);
    let mut child = process.spawn().with_context(|| {
        format!(
            "start extension {} with {}",
            extension.manifest.id,
            command.display()
        )
    })?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("extension stdin was not piped"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("extension stdout was not piped"))?;
    spawn_extension_request_reader(extension.manifest.id.clone(), output, request_tx);
    Ok(RunningExtension {
        manifest: extension.manifest.clone(),
        child,
        input: BufWriter::new(input),
    })
}

fn spawn_extension_request_reader(
    extension_id: String,
    output: impl std::io::Read + Send + 'static,
    request_tx: Sender<ExtensionRequest>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(output);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match parse_extension_request(&extension_id, &line) {
                Ok(request) => {
                    if request_tx.send(request).is_err() {
                        break;
                    }
                }
                Err(error) => eprintln!("yeet: extension {extension_id} output ignored: {error}"),
            }
        }
    });
}

fn parse_extension_request(extension_id: &str, line: &str) -> Result<ExtensionRequest> {
    let envelope: ExtensionRequestEnvelope = serde_json::from_str(line)?;
    if envelope.protocol != EXTENSION_PROTOCOL {
        bail!("unsupported protocol {}", envelope.protocol);
    }
    let string_field = |name: &str| -> Result<String> {
        envelope
            .payload
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("missing payload.{name}"))
    };
    let extension_id = extension_id.to_owned();
    match envelope.action.as_str() {
        "submit" => Ok(ExtensionRequest::Submit {
            extension_id,
            text: string_field("text")?,
        }),
        "selectModel" => Ok(ExtensionRequest::SelectModel {
            extension_id,
            model: string_field("model")?,
        }),
        "selectReasoning" => Ok(ExtensionRequest::SelectReasoning {
            extension_id,
            level: string_field("level")?,
        }),
        "interrupt" => Ok(ExtensionRequest::Interrupt { extension_id }),
        "newSession" => Ok(ExtensionRequest::NewSession { extension_id }),
        "requestModels" => Ok(ExtensionRequest::RequestModels { extension_id }),
        action => bail!("unknown extension action {action:?}"),
    }
}

fn resolve_command(value: &str, extension_dir: &Path, workspace: &Path) -> PathBuf {
    let expanded = expand_value(value, workspace);
    if let Some(rest) = expanded.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    let path = PathBuf::from(&expanded);
    if path.is_absolute() || expanded.contains('/') || expanded.contains('\\') {
        if path.is_absolute() {
            path
        } else {
            extension_dir.join(path)
        }
    } else {
        path
    }
}

fn expand_value(value: &str, workspace: &Path) -> String {
    let home = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    value
        .replace("${HOME}", &home)
        .replace("${workspace}", &workspace.display().to_string())
}

pub fn discover_extensions(
    config_dir: &Path,
    workspace: &Path,
) -> Result<BTreeMap<String, DiscoveredExtension>> {
    let mut extensions = BTreeMap::new();
    discover_root(
        &config_dir.join("extensions"),
        ExtensionScope::Global,
        &mut extensions,
    )?;
    discover_root(
        &workspace.join(".yeet/extensions"),
        ExtensionScope::Project,
        &mut extensions,
    )?;
    Ok(extensions)
}

fn discover_root(
    root: &Path,
    scope: ExtensionScope,
    extensions: &mut BTreeMap<String, DiscoveredExtension>,
) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
    };
    for entry in entries {
        let entry = entry?;
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let manifest_path = directory.join(MANIFEST_NAME);
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = load_manifest(&manifest_path)?;
        extensions.insert(
            manifest.id.clone(),
            DiscoveredExtension {
                manifest,
                directory,
                scope,
            },
        );
    }
    Ok(())
}

pub fn load_manifest(path: &Path) -> Result<ExtensionManifest> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: ExtensionManifest =
        serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?;
    manifest.validate()?;
    Ok(manifest)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("invalid extension id {id:?}; use letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn validate_command_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!(
            "invalid extension command {name:?}; use letters, digits, '_' or '-' without a leading slash"
        );
    }
    Ok(())
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

pub fn run_cli(args: &[String]) -> Result<String> {
    let config = ConfigStore::default();
    config.ensure()?;
    let workspace = std::env::current_dir()?
        .canonicalize()
        .unwrap_or(std::env::current_dir()?);
    let command = args.first().map(String::as_str).unwrap_or("list");
    match command {
        "list" => list_cli(&config.directory, &workspace),
        "path" => Ok(config.directory.join("extensions").display().to_string()),
        "validate" => {
            let source = args
                .get(1)
                .ok_or_else(|| anyhow!("Usage: yeet extension validate PATH"))?;
            let (manifest, manifest_path) = manifest_from_source(Path::new(source))?;
            Ok(format!(
                "{} {} ({})\n{}",
                manifest.name,
                manifest.version,
                manifest.id,
                manifest_path.display()
            ))
        }
        "install" => install_cli(&config.directory, &workspace, &args[1..]),
        "remove" => remove_cli(&config.directory, &workspace, &args[1..]),
        _ => bail!(
            "Usage: yeet extension [list|path|validate PATH|install PATH [--project] [--force]|remove ID [--project]]"
        ),
    }
}

fn list_cli(config_dir: &Path, workspace: &Path) -> Result<String> {
    let extensions = discover_extensions(config_dir, workspace)?;
    if extensions.is_empty() {
        return Ok("No Yeet extensions installed.".into());
    }
    Ok(extensions
        .values()
        .map(|extension| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                extension.manifest.id,
                extension.manifest.version,
                extension.scope.label(),
                if extension.manifest.auto_start {
                    "auto"
                } else {
                    "manual"
                },
                if extension.manifest.supports_current_platform() {
                    "compatible"
                } else {
                    "incompatible"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn install_cli(config_dir: &Path, workspace: &Path, args: &[String]) -> Result<String> {
    let source = args
        .first()
        .ok_or_else(|| anyhow!("Usage: yeet extension install PATH [--project] [--force]"))?;
    let project = args.iter().any(|value| value == "--project");
    let force = args.iter().any(|value| value == "--force");
    for value in args.iter().skip(1) {
        if value != "--project" && value != "--force" {
            bail!("unknown extension install option: {value}");
        }
    }
    let source = Path::new(source);
    let (manifest, manifest_path) = manifest_from_source(source)?;
    let source_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("extension manifest has no parent directory"))?;
    let root = if project {
        workspace.join(".yeet/extensions")
    } else {
        config_dir.join("extensions")
    };
    fs::create_dir_all(&root)?;
    let destination = root.join(&manifest.id);
    if destination.exists() {
        if !force {
            bail!(
                "extension {} is already installed at {}; use --force to replace it",
                manifest.id,
                destination.display()
            );
        }
        fs::remove_dir_all(&destination)?;
    }
    copy_directory(source_dir, &destination)?;
    Ok(format!(
        "Installed {} {} to {}",
        manifest.name,
        manifest.version,
        destination.display()
    ))
}

fn remove_cli(config_dir: &Path, workspace: &Path, args: &[String]) -> Result<String> {
    let id = args
        .first()
        .ok_or_else(|| anyhow!("Usage: yeet extension remove ID [--project]"))?;
    validate_id(id)?;
    let project = args.iter().any(|value| value == "--project");
    for value in args.iter().skip(1) {
        if value != "--project" {
            bail!("unknown extension remove option: {value}");
        }
    }
    let root = if project {
        workspace.join(".yeet/extensions")
    } else {
        config_dir.join("extensions")
    };
    let destination = root.join(id);
    if !destination.exists() {
        bail!(
            "extension {id} is not installed in {} scope",
            if project { "project" } else { "global" }
        );
    }
    fs::remove_dir_all(&destination)?;
    Ok(format!("Removed extension {id}"))
}

fn manifest_from_source(source: &Path) -> Result<(ExtensionManifest, PathBuf)> {
    let manifest_path = if source.is_dir() {
        source.join(MANIFEST_NAME)
    } else {
        source.to_path_buf()
    };
    if manifest_path.file_name().and_then(|name| name.to_str()) != Some(MANIFEST_NAME) {
        bail!(
            "extension source must be a directory containing {MANIFEST_NAME} or the manifest itself"
        );
    }
    Ok((load_manifest(&manifest_path)?, manifest_path))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}
