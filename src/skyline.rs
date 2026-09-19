use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::core::ToolDefinition;

mod setup;

use setup::initialize_project;

pub(crate) const CAPABILITY_ID: &str = "builtin:skyline";
const HANDLE_VERSION: u64 = 1;

fn bundled_runtime_digest() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("../skyline/skyline.py"))
    )
}
const WORKER_OPERATIONS: &[&str] = &[
    "next",
    "sync",
    "name",
    "send",
    "react",
    "inbox",
    "outcome",
    "test_run_start",
    "test_run_finish",
    "test_run_show",
    "deploy_agent",
    "status",
];

pub(crate) fn direct_mcp_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "activate_capability",
            "Attach one explicitly user-invoked lazy Yeet MCP capability by exact capability id. Optional capability-specific state and schemas remain cold until activation.",
            json!({
                "type":"object",
                "properties":{
                    "capability":{"type":"string","minLength":1},
                    "explicitUserInvocation":{"type":"boolean"},
                    "arguments":{"type":"object"}
                },
                "required":["capability","explicitUserInvocation"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            "invoke_capability",
            "Invoke an already activated lazy Yeet MCP capability through the opaque handle returned by activate_capability.",
            json!({
                "type":"object",
                "properties":{
                    "handle":{"type":"string","minLength":1},
                    "operation":{"type":"string","minLength":1},
                    "arguments":{"type":"object"}
                },
                "required":["handle","operation"],
                "additionalProperties":false
            }),
        ),
    ]
}

pub(crate) fn tui_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "skyline",
        "Use Skyline as the unified coordination and native agent-deployment capability for this session. Choose work autonomously from actual project state. Call next once to orient, sync after material changes or before collision-sensitive work, coordinate with send/react, publish evidence-backed outcomes, and serialize scarce live-test resources. Use deploy_agent{task} only when the user requests delegation; it starts a separate native session with normal permissions. Jobs and scopes are advisory; do not poll or stay active merely to satisfy a time target.",
        json!({
            "type":"object",
            "properties":{
                "operation":{"type":"string","enum":WORKER_OPERATIONS},
                "arguments":{"type":"object","description":"Required keys by operation: name{name}; send{to[],subject,body}; react{message}; outcome{summary}; test_run_start{resource,purpose}; test_run_finish{id,outcome}; deploy_agent{task}. next/sync/inbox/test_run_show/status accept optional filters/state fields.","additionalProperties":true}
            },
            "required":["operation"],
            "additionalProperties":false
        }),
    )
}

pub(crate) fn activate(workspace: &Path, arguments: Map<String, Value>) -> Result<String> {
    ensure_known_keys(
        &arguments,
        &["capability", "explicitUserInvocation", "arguments"],
        "activate_capability",
    )?;
    let capability = required_string(&arguments, "capability")?;
    if capability != CAPABILITY_ID {
        bail!("unsupported lazy capability: {capability}");
    }
    if arguments
        .get("explicitUserInvocation")
        .and_then(Value::as_bool)
        != Some(true)
    {
        bail!(
            "Skyline is explicit-only. activate_capability requires explicitUserInvocation=true only when the user explicitly invoked Skyline."
        );
    }
    let activation_arguments = match arguments.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(value)) => value.clone(),
        Some(_) => bail!("activate_capability.arguments must be an object"),
    };
    ensure_known_keys(
        &activation_arguments,
        &["resumeAgent", "full"],
        "builtin:skyline activation",
    )?;

    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve Skyline workspace {}", workspace.display()))?;
    let project = resolve_project(&workspace)?;
    let source_root = project_source_root(&project)?;

    let mut argv = vec![
        "next".to_owned(),
        "--root".to_owned(),
        project.display().to_string(),
    ];
    if let Some(agent) = optional_string(&activation_arguments, "resumeAgent")? {
        argv.push("--agent".into());
        argv.push(agent.to_owned());
    }
    if optional_bool(&activation_arguments, "full")?.unwrap_or(false) {
        argv.push("--full".into());
    }

    let bootstrap = run_cli(&project, &argv)?;
    let agent = bootstrap
        .get("agent_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Skyline activation did not return an agent_id"))?
        .to_owned();
    let handle = encode_handle(&source_root, &project, &agent)?;
    let mailbox = mailbox_summary(&project, &agent)?;
    let mut bootstrap = compact_runtime_result("next", &activation_arguments, bootstrap);
    add_bootstrap_project_context(&project, &source_root, &mut bootstrap)?;

    Ok(json!({
        "activated": CAPABILITY_ID,
        "handle": handle,
        "agent_id": agent,
        "operations": WORKER_OPERATIONS,
        "bootstrap": bootstrap,
        "mailbox": mailbox,
    })
    .to_string())
}

pub(crate) fn invoke(workspace: &Path, arguments: Map<String, Value>) -> Result<String> {
    ensure_known_keys(
        &arguments,
        &["handle", "operation", "arguments"],
        "invoke_capability",
    )?;
    let handle = required_string(&arguments, "handle")?;
    let operation = required_string(&arguments, "operation")?;
    if !WORKER_OPERATIONS.contains(&operation) {
        bail!("unsupported Skyline operation: {operation}");
    }
    let operation_arguments = match arguments.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(value)) => value.clone(),
        Some(_) => bail!("invoke_capability.arguments must be an object"),
    };

    let decoded = decode_handle(handle)?;
    validate_handle_for_workspace(&decoded, workspace)?;
    let result = run_operation(
        &decoded.project_root,
        &decoded.agent_id,
        operation,
        &operation_arguments,
    )?;
    let result = compact_runtime_result(operation, &operation_arguments, result);
    let mailbox = mailbox_summary(&decoded.project_root, &decoded.agent_id)?;
    Ok(json!({
        "operation": operation,
        "agent_id": decoded.agent_id,
        "result": result,
        "mailbox": mailbox,
    })
    .to_string())
}

const COMPACT_PEER_LIMIT: usize = 8;
const COMPACT_OPPORTUNITY_LIMIT: usize = 6;
const COMPACT_OUTCOME_LIMIT: usize = 4;
const COMPACT_WHITEBOARD_LIMIT: usize = 6;
const COMPACT_TEST_RUN_LIMIT: usize = 6;
const COMPACT_MESSAGE_LIMIT: usize = 6;
const COMPACT_STATUS_AGENT_LIMIT: usize = 12;
const BOOTSTRAP_MISSION_CONTEXT_LIMIT: usize = 12_000;

fn add_bootstrap_project_context(
    project: &Path,
    source_root: &Path,
    bootstrap: &mut Value,
) -> Result<()> {
    let Some(object) = bootstrap.as_object_mut() else {
        return Ok(());
    };
    let mission_path = project.join("skyline").join("SKYLINE.md");
    let mission = fs::read_to_string(&mission_path)
        .with_context(|| format!("read Skyline mission {}", mission_path.display()))?;
    object.insert(
        "mission_context".into(),
        Value::String(truncate(&mission, BOOTSTRAP_MISSION_CONTEXT_LIMIT)),
    );
    object.insert(
        "source_root".into(),
        Value::String(source_root.display().to_string()),
    );
    Ok(())
}

fn compact_runtime_result(operation: &str, arguments: &Map<String, Value>, result: Value) -> Value {
    let full = arguments.get("full").and_then(Value::as_bool) == Some(true);
    if full {
        return result;
    }
    match operation {
        "next" | "sync" | "outcome" => compact_worker_payload(&result),
        "inbox" => compact_inbox_payload(&result),
        "status" => compact_status_payload(&result),
        "test_run_show" => compact_test_run_show_payload(&result),
        _ => result,
    }
}

fn copy_fields(source: &Value, fields: &[&str]) -> Value {
    let Some(source) = source.as_object() else {
        return source.clone();
    };
    let mut out = Map::new();
    for field in fields {
        if let Some(value) = source.get(*field)
            && !value.is_null()
        {
            out.insert((*field).to_owned(), value.clone());
        }
    }
    Value::Object(out)
}

fn compact_list(value: Option<&Value>, fields: &[&str], limit: usize) -> (Vec<Value>, usize) {
    let Some(items) = value.and_then(Value::as_array) else {
        return (Vec::new(), 0);
    };
    let compact = items
        .iter()
        .take(limit)
        .map(|item| copy_fields(item, fields))
        .collect::<Vec<_>>();
    (compact, items.len().saturating_sub(limit))
}

fn message_value(item: &Value) -> &Value {
    item.get("message").unwrap_or(item)
}

fn message_needs_attention(item: &Value) -> bool {
    let message = message_value(item);
    matches!(
        message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase()
            .as_str(),
        "ALERT" | "BLOCKER" | "DECISION_PROPOSAL" | "REVIEW_REQUEST"
    ) || message.get("requires_response").and_then(Value::as_bool) == Some(true)
        || message.get("user_relayed").and_then(Value::as_bool) == Some(true)
}

fn compact_message(item: &Value) -> Value {
    let message = message_value(item);
    let mut compact = copy_fields(
        message,
        &[
            "id",
            "from",
            "from_name",
            "to",
            "type",
            "subject",
            "body",
            "related",
            "requires_response",
            "user_relayed",
            "created_at",
        ],
    );
    if let Some(body) = compact.get_mut("body")
        && let Some(text) = body.as_str()
        && text.chars().count() > 1200
    {
        *body = Value::String(truncate(text, 1200));
    }
    compact
}

fn compact_coordination(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let mut out = Map::new();
    for field in [
        "unread_count",
        "attention_count",
        "deferred_routine_count",
        "attention_required",
        "cursor",
        "delta_mode",
    ] {
        if let Some(value) = source.get(field) {
            out.insert(field.to_owned(), value.clone());
        }
    }
    if let Some(messages) = source.get("unread_messages").and_then(Value::as_array) {
        let attention = messages
            .iter()
            .filter(|item| message_needs_attention(item))
            .take(COMPACT_MESSAGE_LIMIT)
            .map(compact_message)
            .collect::<Vec<_>>();
        if !attention.is_empty() {
            out.insert("unread_messages".into(), Value::Array(attention));
        }
        let deferred = messages
            .iter()
            .filter(|item| !message_needs_attention(item))
            .count();
        if deferred > 0 {
            let existing = source
                .get("deferred_routine_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            out.insert(
                "deferred_routine_count".into(),
                json!(existing.max(deferred as u64)),
            );
        }
    }
    Value::Object(out)
}

fn compact_delta(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let mut out = Map::new();
    for field in ["mode", "cursor", "changed_count", "removed"] {
        if let Some(value) = source.get(field) {
            out.insert(field.to_owned(), value.clone());
        }
    }
    let Some(changes) = source.get("changes").and_then(Value::as_object) else {
        return Value::Object(out);
    };
    let specs: [(&str, &[&str], usize); 6] = [
        (
            "peer_intents",
            &[
                "agent_id",
                "agent_name",
                "display_name",
                "state",
                "status",
                "intent",
                "intent_scope",
                "associated_job",
                "next_action",
                "updated_at",
            ],
            COMPACT_PEER_LIMIT,
        ),
        (
            "opportunities",
            &[
                "id",
                "title",
                "kind",
                "status",
                "associated_agent",
                "assigned_agent",
                "ownership",
                "updated_at",
                "latest_progress",
            ],
            COMPACT_OPPORTUNITY_LIMIT,
        ),
        (
            "outcomes",
            &[
                "id",
                "agent_id",
                "agent_name",
                "summary",
                "related_job",
                "created_at",
            ],
            COMPACT_OUTCOME_LIMIT,
        ),
        (
            "whiteboard_cards",
            &[
                "id",
                "section",
                "text",
                "author",
                "author_name",
                "related",
                "created_at",
                "updated_at",
            ],
            COMPACT_WHITEBOARD_LIMIT,
        ),
        (
            "test_runs",
            &[
                "id",
                "resource",
                "status",
                "agent_id",
                "agent_name",
                "purpose",
                "revision",
                "config_hash",
                "checkpoint",
                "outcome",
                "started_at",
                "finished_at",
            ],
            COMPACT_TEST_RUN_LIMIT,
        ),
        (
            "meta",
            &[
                "path",
                "digest",
                "schema_version",
                "coordination_mode",
                "claim_mode",
                "whiteboard_enabled",
                "source_root",
            ],
            4,
        ),
    ];
    let mut compact_changes = Map::new();
    let mut omitted = Map::new();
    for (name, fields, limit) in specs {
        let (items, omitted_count) = compact_list(changes.get(name), fields, limit);
        if !items.is_empty() {
            compact_changes.insert(name.to_owned(), Value::Array(items));
        }
        if omitted_count > 0 {
            omitted.insert(name.to_owned(), json!(omitted_count));
        }
    }
    if !compact_changes.is_empty() {
        out.insert("changes".into(), Value::Object(compact_changes));
    }
    if !omitted.is_empty() {
        out.insert("omitted_counts".into(), Value::Object(omitted));
    }
    Value::Object(out)
}

fn compact_team_summary(value: &Value) -> Value {
    let mut out = copy_fields(
        value,
        &[
            "live_workers",
            "stale_workers",
            "independent_declared_scopes",
            "scope_collision_count",
            "opportunity_counts",
            "active_test_runs",
            "stale_after_seconds",
        ],
    );
    if let Some(collisions) = value.get("scope_collisions").and_then(Value::as_array) {
        let limited = collisions.iter().take(4).cloned().collect::<Vec<_>>();
        if !limited.is_empty() {
            out["scope_collisions"] = Value::Array(limited);
        }
        if collisions.len() > 4 {
            out["omitted_scope_collisions"] = json!(collisions.len() - 4);
        }
    }
    out
}

fn compact_worker_payload(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let mut out = Map::new();
    for field in [
        "status",
        "agent_id",
        "display_name",
        "mission_digest",
        "instruction",
    ] {
        if let Some(value) = source.get(field) {
            out.insert(field.to_owned(), value.clone());
        }
    }
    if let Some(agent) = source.get("agent") {
        out.insert(
            "agent".into(),
            copy_fields(
                agent,
                &[
                    "agent_id",
                    "display_name",
                    "status",
                    "assessment",
                    "intent",
                    "intent_scope",
                    "associated_job",
                    "next_action",
                    "last_heartbeat",
                ],
            ),
        );
    }
    if let Some(naming) = source.get("naming")
        && naming.get("required").and_then(Value::as_bool) == Some(true)
    {
        out.insert("naming".into(), naming.clone());
    }
    if let Some(value) = source.get("team_summary")
        && !value.is_null()
    {
        out.insert("team_summary".into(), compact_team_summary(value));
    }
    if let Some(value) = source.get("coordination_delta") {
        out.insert("coordination_delta".into(), compact_delta(value));
    }
    if let Some(value) = source.get("coordination") {
        out.insert("coordination".into(), compact_coordination(value));
    }
    if let Some(value) = source.get("advisory_claim")
        && !value.is_null()
    {
        out.insert("advisory_claim".into(), value.clone());
    }
    if let Some(value) = source.get("outcome") {
        out.insert(
            "outcome".into(),
            copy_fields(value, &["id", "summary", "related_job", "created_at"]),
        );
    }
    if let Some(value) = source.get("messages").and_then(Value::as_array) {
        let attention = value
            .iter()
            .filter(|item| message_needs_attention(item))
            .take(COMPACT_MESSAGE_LIMIT)
            .map(compact_message)
            .collect::<Vec<_>>();
        if !attention.is_empty() {
            out.insert("messages".into(), Value::Array(attention));
        }
    }
    Value::Object(out)
}

fn compact_inbox_payload(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let messages = source
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let compact = messages
        .iter()
        .take(COMPACT_MESSAGE_LIMIT)
        .map(compact_message)
        .collect::<Vec<_>>();
    json!({
        "agent_id": source.get("agent_id").cloned().unwrap_or(Value::Null),
        "messages": compact,
        "total_count": messages.len(),
        "omitted_count": messages.len().saturating_sub(COMPACT_MESSAGE_LIMIT),
    })
}

fn compact_test_run_show_payload(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let fields = [
        "id",
        "resource",
        "status",
        "agent_id",
        "agent_name",
        "purpose",
        "revision",
        "config_hash",
        "checkpoint",
        "outcome",
        "started_at",
        "finished_at",
    ];
    let mut out = Map::new();
    for key in ["active", "recent"] {
        let limit = if key == "active" {
            COMPACT_TEST_RUN_LIMIT
        } else {
            4
        };
        let (items, omitted) = compact_list(source.get(key), &fields, limit);
        if !items.is_empty() {
            out.insert(key.to_owned(), Value::Array(items));
        }
        if omitted > 0 {
            out.insert(format!("{key}_omitted"), json!(omitted));
        }
    }
    Value::Object(out)
}

fn compact_status_payload(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return value.clone();
    };
    let mut out = Map::new();
    if let Some(summary) = source.get("team_summary") {
        out.insert("team_summary".into(), compact_team_summary(summary));
    }
    if let Some(whiteboard) = source.get("whiteboard") {
        out.insert(
            "whiteboard".into(),
            copy_fields(whiteboard, &["enabled", "open_count", "counts"]),
        );
    }
    if let Some(test_runs) = source.get("test_runs") {
        out.insert("test_runs".into(), compact_test_run_show_payload(test_runs));
    }
    if let Some(divisions) = source.get("divisions") {
        out.insert("divisions".into(), divisions.clone());
    }
    let agent_fields = [
        "agent_id",
        "display_name",
        "status",
        "intent",
        "intent_scope",
        "associated_job",
        "next_action",
        "last_heartbeat",
    ];
    let (agents, omitted_agents) = compact_list(
        source.get("agents"),
        &agent_fields,
        COMPACT_STATUS_AGENT_LIMIT,
    );
    if !agents.is_empty() {
        out.insert("agents".into(), Value::Array(agents));
    }
    if omitted_agents > 0 {
        out.insert("agents_omitted".into(), json!(omitted_agents));
    }
    let (outcomes, omitted_outcomes) = compact_list(
        source.get("recent_outcomes"),
        &[
            "id",
            "agent_id",
            "agent_name",
            "summary",
            "related_job",
            "created_at",
        ],
        COMPACT_OUTCOME_LIMIT,
    );
    if !outcomes.is_empty() {
        out.insert("recent_outcomes".into(), Value::Array(outcomes));
    }
    if omitted_outcomes > 0 {
        out.insert("recent_outcomes_omitted".into(), json!(omitted_outcomes));
    }
    if let Some(jobs) = source
        .get("jobs_as_opportunities")
        .and_then(Value::as_object)
    {
        let mut compact_jobs = Map::new();
        for (state, value) in jobs {
            let (items, omitted) = compact_list(
                Some(value),
                &[
                    "id",
                    "title",
                    "kind",
                    "status",
                    "assigned_agent",
                    "ownership",
                    "updated_at",
                ],
                COMPACT_OPPORTUNITY_LIMIT,
            );
            if !items.is_empty() {
                compact_jobs.insert(state.clone(), Value::Array(items));
            }
            if omitted > 0 {
                compact_jobs.insert(format!("{state}_omitted"), json!(omitted));
            }
        }
        if !compact_jobs.is_empty() {
            out.insert("jobs_as_opportunities".into(), Value::Object(compact_jobs));
        }
    }
    Value::Object(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapabilityHandle {
    source_root: PathBuf,
    project_root: PathBuf,
    agent_id: String,
}

fn encode_handle(source_root: &Path, project_root: &Path, agent_id: &str) -> Result<String> {
    let secret = capability_secret()?;
    encode_handle_with_secret(source_root, project_root, agent_id, &secret)
}

fn encode_handle_with_secret(
    source_root: &Path,
    project_root: &Path,
    agent_id: &str,
    secret: &[u8],
) -> Result<String> {
    let payload = json!({
        "v": HANDLE_VERSION,
        "capability": CAPABILITY_ID,
        "sourceRoot": source_root,
        "projectRoot": project_root,
        "agent": agent_id,
    });
    let payload = serde_json::to_vec(&payload)?;
    let tag = hmac_sha256(secret, &payload);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(tag)
    ))
}

fn decode_handle(encoded: &str) -> Result<CapabilityHandle> {
    let secret = capability_secret()?;
    decode_handle_with_secret(encoded, &secret)
}

fn decode_handle_with_secret(encoded: &str, secret: &[u8]) -> Result<CapabilityHandle> {
    let (payload, tag) = encoded
        .split_once('.')
        .ok_or_else(|| anyhow!("invalid lazy capability handle"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decode lazy capability handle")?;
    let tag = URL_SAFE_NO_PAD
        .decode(tag)
        .context("decode lazy capability handle signature")?;
    let expected = hmac_sha256(secret, &bytes);
    if !constant_time_eq(&tag, &expected) {
        bail!("invalid lazy capability handle signature");
    }

    let value: Value = serde_json::from_slice(&bytes).context("parse lazy capability handle")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("lazy capability handle payload must be an object"))?;
    if object.get("v").and_then(Value::as_u64) != Some(HANDLE_VERSION)
        || object.get("capability").and_then(Value::as_str) != Some(CAPABILITY_ID)
    {
        bail!("invalid or unsupported lazy capability handle");
    }
    let source_root = PathBuf::from(
        object
            .get("sourceRoot")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("lazy capability handle is missing sourceRoot"))?,
    );
    let project_root = PathBuf::from(
        object
            .get("projectRoot")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("lazy capability handle is missing projectRoot"))?,
    );
    let agent_id = object
        .get("agent")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("lazy capability handle is missing agent"))?
        .to_owned();
    Ok(CapabilityHandle {
        source_root,
        project_root,
        agent_id,
    })
}

fn capability_secret() -> Result<Vec<u8>> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow!("cannot determine the user home directory"))?;
    let runtime = home.join(".yeet").join("runtime");
    let path = runtime.join("skyline-capability.key");
    if let Ok(secret) = fs::read(&path)
        && secret.len() >= 32
    {
        return Ok(secret);
    }
    fs::create_dir_all(&runtime)
        .with_context(|| format!("create Yeet runtime directory {}", runtime.display()))?;
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()).into_bytes();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(&secret)
                .with_context(|| format!("write Skyline capability key {}", path.display()))?;
            file.sync_all()
                .with_context(|| format!("sync Skyline capability key {}", path.display()))?;
            Ok(secret)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&path)
                .with_context(|| format!("read Skyline capability key {}", path.display()))?;
            if existing.len() < 32 {
                bail!("Skyline capability key is invalid: {}", path.display());
            }
            Ok(existing)
        }
        Err(error) => {
            Err(error).with_context(|| format!("create Skyline capability key {}", path.display()))
        }
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(data);
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    let digest = outer.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= *left ^ *right;
    }
    difference == 0
}

fn validate_handle_for_workspace(handle: &CapabilityHandle, workspace: &Path) -> Result<()> {
    let skyline_root = skyline_root()?;
    let skyline_root = skyline_root
        .canonicalize()
        .with_context(|| format!("resolve Skyline root {}", skyline_root.display()))?;
    let project_root = handle
        .project_root
        .canonicalize()
        .with_context(|| format!("resolve Skyline project {}", handle.project_root.display()))?;
    if project_root == skyline_root || !project_root.starts_with(&skyline_root) {
        bail!("Skyline capability handle points outside ~/.yeet/Skyline");
    }
    let source_root = project_source_root(&project_root)?;
    let handle_source = handle.source_root.canonicalize().with_context(|| {
        format!(
            "resolve Skyline handle source {}",
            handle.source_root.display()
        )
    })?;
    if source_root != handle_source {
        bail!("Skyline capability handle no longer matches the project's source_root");
    }
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve Skyline workspace {}", workspace.display()))?;
    if workspace != source_root && !workspace.starts_with(&source_root) {
        bail!(
            "Skyline capability handle is scoped to {}; requested workspace is {}",
            source_root.display(),
            workspace.display()
        );
    }
    Ok(())
}

fn skyline_root() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow!("cannot determine the user home directory"))?;
    Ok(home.join(".yeet").join("Skyline"))
}

pub(crate) fn setup_installation() -> Result<PathBuf> {
    let root = skyline_root()?;
    let runtime_dir = root.join(".runtime");
    fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("create Skyline runtime directory {}", runtime_dir.display()))?;
    let runtime_path = runtime_dir.join("skyline.py");
    let runtime_digest_path = runtime_dir.join("runtime.sha256");
    let bundled = include_bytes!("../skyline/skyline.py");
    let bundled_digest = bundled_runtime_digest();
    let installed_digest = fs::read_to_string(&runtime_digest_path).ok();
    if !runtime_path.is_file()
        || installed_digest.as_deref().map(str::trim) != Some(bundled_digest.as_str())
    {
        fs::write(&runtime_path, bundled)
            .with_context(|| format!("install Skyline runtime {}", runtime_path.display()))?;
        fs::write(&runtime_digest_path, format!("{bundled_digest}\n")).with_context(|| {
            format!(
                "write Skyline runtime digest {}",
                runtime_digest_path.display()
            )
        })?;
    }
    #[cfg(unix)]
    {
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

pub(crate) fn setup_workspace(workspace: &Path) -> Result<PathBuf> {
    resolve_project(workspace)
}

fn resolve_project(workspace: &Path) -> Result<PathBuf> {
    let root = setup_installation()?;
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve Skyline workspace {}", workspace.display()))?;
    if let Some(project) = resolve_project_from_root(&root, &workspace)? {
        initialize_project(&root, &project, &workspace)?;
        return Ok(project);
    }
    provision_project(&root, &workspace)
}

fn provision_project(root: &Path, workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve Skyline workspace {}", workspace.display()))?;
    let runtime_path = root.join(".runtime").join("skyline.py");
    if !runtime_path.is_file() {
        bail!(
            "Skyline runtime is not installed at {}",
            runtime_path.display()
        );
    }

    let base = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    let slug = base
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || matches!(value, '-' | '_') {
                value
            } else {
                '-'
            }
        })
        .collect::<String>();
    let digest = format!(
        "{:x}",
        Sha256::digest(workspace.to_string_lossy().as_bytes())
    );
    let project = root.join(format!("{}-{}", slug.trim_matches('-'), &digest[..12]));
    fs::create_dir_all(&project)
        .with_context(|| format!("create Skyline project {}", project.display()))?;
    initialize_project(root, &project, &workspace)?;
    Ok(project)
}

fn resolve_project_from_root(root: &Path, workspace: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve Skyline workspace {}", workspace.display()))?;
    let mut candidates = Vec::new();
    let direct = root.join("skyline").join("CONFIG.json");
    if direct.is_file() {
        candidates.push(root.to_path_buf());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("skyline").join("CONFIG.json").is_file() {
            candidates.push(path);
        }
    }

    let mut selected: Option<(usize, PathBuf)> = None;
    for project in candidates {
        let source = match project_source_root(&project) {
            Ok(source) => source,
            Err(_) => continue,
        };
        if workspace != source && !workspace.starts_with(&source) {
            continue;
        }
        let depth = source.components().count();
        if selected
            .as_ref()
            .is_none_or(|(selected_depth, _)| depth > *selected_depth)
        {
            selected = Some((depth, project));
        }
    }
    Ok(selected.map(|(_, project)| project))
}

fn project_source_root(project_root: &Path) -> Result<PathBuf> {
    let config_path = project_root.join("skyline").join("CONFIG.json");
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("read Skyline config {}", config_path.display()))?;
    let config: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse Skyline config {}", config_path.display()))?;
    let source = config
        .get("source_root")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "Skyline config {} has no source_root",
                config_path.display()
            )
        })?;
    PathBuf::from(source)
        .canonicalize()
        .with_context(|| format!("resolve Skyline source_root {source}"))
}

fn run_operation(
    project_root: &Path,
    agent_id: &str,
    operation: &str,
    arguments: &Map<String, Value>,
) -> Result<Value> {
    if operation == "deploy_agent" {
        ensure_known_keys(arguments, &["task"], operation)?;
        let task = required_string(arguments, "task")?;
        let source_root = project_source_root(project_root)?;
        let result =
            crate::tools::deploy_agent_for_workspace(&source_root, task, &AtomicBool::new(false))?;
        return serde_json::from_str(&result)
            .with_context(|| "decode native agent deployment result".to_owned());
    }
    let root = project_root.display().to_string();
    let mut argv = match operation {
        "next" => vec![
            "next".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "sync" => vec![
            "sync".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "name" => vec![
            "name".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "send" => vec![
            "send".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "react" => vec![
            "react".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "inbox" => vec![
            "inbox".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "outcome" => vec![
            "outcome".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "test_run_start" => vec![
            "test-run".into(),
            "start".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "test_run_finish" => vec![
            "test-run".into(),
            "finish".into(),
            "--root".into(),
            root,
            "--agent".into(),
            agent_id.into(),
        ],
        "test_run_show" => vec!["test-run".into(), "show".into(), "--root".into(), root],
        "status" => vec!["status".into(), "--root".into(), root],
        _ => bail!("unsupported Skyline operation: {operation}"),
    };

    match operation {
        "next" => {
            ensure_known_keys(arguments, &["job", "full"], operation)?;
            push_optional_string(arguments, "job", "--job", &mut argv)?;
            push_bool_flag(arguments, "full", "--full", &mut argv)?;
        }
        "sync" => {
            ensure_known_keys(
                arguments,
                &[
                    "claim",
                    "state",
                    "assessment",
                    "intent",
                    "scope",
                    "decisionBasis",
                    "job",
                    "clearJob",
                    "nextAction",
                    "note",
                    "full",
                ],
                operation,
            )?;
            push_optional_string(arguments, "claim", "--claim", &mut argv)?;
            push_optional_string(arguments, "state", "--state", &mut argv)?;
            push_optional_string(arguments, "assessment", "--assessment", &mut argv)?;
            push_optional_string(arguments, "intent", "--intent", &mut argv)?;
            push_repeated_strings(arguments, "scope", "--scope", &mut argv)?;
            push_optional_string(arguments, "decisionBasis", "--decision-basis", &mut argv)?;
            push_optional_string(arguments, "job", "--job", &mut argv)?;
            push_bool_flag(arguments, "clearJob", "--clear-job", &mut argv)?;
            push_optional_string(arguments, "nextAction", "--next-action", &mut argv)?;
            push_optional_string(arguments, "note", "--note", &mut argv)?;
            push_bool_flag(arguments, "full", "--full", &mut argv)?;
        }
        "name" => {
            ensure_known_keys(arguments, &["name"], operation)?;
            push_required_string(arguments, "name", "--name", &mut argv)?;
        }
        "send" => {
            ensure_known_keys(
                arguments,
                &[
                    "to",
                    "type",
                    "subject",
                    "body",
                    "related",
                    "requiresResponse",
                    "userRelayed",
                ],
                operation,
            )?;
            push_required_repeated_strings(arguments, "to", "--to", &mut argv)?;
            push_optional_string(arguments, "type", "--type", &mut argv)?;
            push_required_string(arguments, "subject", "--subject", &mut argv)?;
            push_required_string(arguments, "body", "--body", &mut argv)?;
            push_repeated_strings(arguments, "related", "--related", &mut argv)?;
            push_bool_flag(
                arguments,
                "requiresResponse",
                "--requires-response",
                &mut argv,
            )?;
            push_bool_flag(arguments, "userRelayed", "--user-relayed", &mut argv)?;
        }
        "react" => {
            ensure_known_keys(arguments, &["message", "reaction", "remove"], operation)?;
            push_required_string(arguments, "message", "--message", &mut argv)?;
            push_optional_string(arguments, "reaction", "--reaction", &mut argv)?;
            push_bool_flag(arguments, "remove", "--remove", &mut argv)?;
        }
        "inbox" => {
            ensure_known_keys(arguments, &["unread", "markRead", "full"], operation)?;
            push_bool_flag(arguments, "unread", "--unread", &mut argv)?;
            push_bool_flag(arguments, "markRead", "--mark-read", &mut argv)?;
            push_bool_flag(arguments, "full", "--full", &mut argv)?;
        }
        "outcome" => {
            ensure_known_keys(arguments, &["summary", "job"], operation)?;
            push_required_string(arguments, "summary", "--summary", &mut argv)?;
            push_optional_string(arguments, "job", "--job", &mut argv)?;
        }
        "test_run_start" => {
            ensure_known_keys(
                arguments,
                &[
                    "resource",
                    "purpose",
                    "revision",
                    "configHash",
                    "checkpoint",
                    "telemetry",
                ],
                operation,
            )?;
            push_required_string(arguments, "resource", "--resource", &mut argv)?;
            push_required_string(arguments, "purpose", "--purpose", &mut argv)?;
            push_optional_string(arguments, "revision", "--revision", &mut argv)?;
            push_optional_string(arguments, "configHash", "--config-hash", &mut argv)?;
            push_optional_string(arguments, "checkpoint", "--checkpoint", &mut argv)?;
            push_optional_string(arguments, "telemetry", "--telemetry", &mut argv)?;
        }
        "test_run_finish" => {
            ensure_known_keys(
                arguments,
                &["id", "outcome", "status", "anomaly", "telemetry"],
                operation,
            )?;
            push_required_string(arguments, "id", "--id", &mut argv)?;
            push_required_string(arguments, "outcome", "--outcome", &mut argv)?;
            push_optional_string(arguments, "status", "--status", &mut argv)?;
            push_repeated_strings(arguments, "anomaly", "--anomaly", &mut argv)?;
            push_optional_string(arguments, "telemetry", "--telemetry", &mut argv)?;
        }
        "test_run_show" => {
            ensure_known_keys(arguments, &["resource", "full"], operation)?;
            push_optional_string(arguments, "resource", "--resource", &mut argv)?;
            push_bool_flag(arguments, "full", "--full", &mut argv)?;
        }
        "status" => {
            ensure_known_keys(arguments, &["includeArchive", "full"], operation)?;
            push_bool_flag(arguments, "includeArchive", "--include-archive", &mut argv)?;
            push_bool_flag(arguments, "full", "--full", &mut argv)?;
        }
        _ => unreachable!(),
    }

    run_cli(project_root, &argv)
}

fn ensure_project_runtime_current(project_root: &Path) -> Result<PathBuf> {
    let skyline_dir = project_root.join("skyline");
    let script = skyline_dir.join("skyline.py");
    let config_path = skyline_dir.join("CONFIG.json");
    let raw_config = fs::read_to_string(&config_path)
        .with_context(|| format!("read Skyline config {}", config_path.display()))?;
    let mut config: Value = serde_json::from_str(&raw_config)
        .with_context(|| format!("parse Skyline config {}", config_path.display()))?;

    let bundled = include_bytes!("../skyline/skyline.py");
    let bundled_digest = bundled_runtime_digest();
    let runtime_current = script.is_file()
        && config.get("runtime_digest").and_then(Value::as_str) == Some(bundled_digest.as_str());
    if !runtime_current {
        fs::create_dir_all(&skyline_dir)
            .with_context(|| format!("create Skyline directory {}", skyline_dir.display()))?;
        fs::write(&script, bundled)
            .with_context(|| format!("refresh Skyline runtime {}", script.display()))?;
        #[cfg(unix)]
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
        config["runtime_digest"] = Value::String(bundled_digest);
        fs::write(&config_path, serde_json::to_vec_pretty(&config)?)
            .with_context(|| format!("write Skyline config {}", config_path.display()))?;
    }

    let needs_migration = config
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        < 7
        || config.get("coordination_mode").and_then(Value::as_str) != Some("autonomous")
        || config.get("snapshot_mode").and_then(Value::as_str) != Some("live_delta");
    if needs_migration {
        let output = Command::new("python3")
            .arg(&script)
            .arg("migrate")
            .arg("--root")
            .arg(project_root)
            .current_dir(project_root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .with_context(|| format!("migrate Skyline project {}", project_root.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            bail!("Skyline migration failed ({}): {detail}", output.status);
        }
    }
    Ok(script)
}

fn run_cli(project_root: &Path, argv: &[String]) -> Result<Value> {
    let script = ensure_project_runtime_current(project_root)?;
    let output = Command::new("python3")
        .arg(&script)
        .args(argv)
        .current_dir(project_root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .with_context(|| format!("launch Skyline runtime {}", script.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        bail!("Skyline runtime failed ({}): {detail}", output.status);
    }
    if stdout.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&stdout)
        .with_context(|| format!("parse Skyline runtime output: {}", truncate(&stdout, 1000)))
}

fn read_json_file(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn extend_json_files(directory: &Path, paths: &mut HashSet<PathBuf>) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read Skyline directory {}", directory.display()))?
    {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.insert(path);
        }
    }
    Ok(())
}

fn parse_skyline_timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?.as_str()?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn current_job_record(sky: &Path, job_id: &str) -> Option<Value> {
    for state in ["pending", "active", "review", "done"] {
        let path = sky.join("jobs").join(state).join(format!("{job_id}.json"));
        if path.is_file()
            && let Ok(value) = read_json_file(&path)
        {
            return Some(value);
        }
    }
    None
}

fn mailbox_summary(project_root: &Path, agent_id: &str) -> Result<Value> {
    let sky = project_root.join("skyline");
    let agent_dir = sky.join("agents").join(agent_id);
    let agent = read_json_file(&agent_dir.join("state.json"))?;
    let now = Utc::now();
    let since = parse_skyline_timestamp(agent.get("last_inbox_check"));
    let broadcast_dir = sky.join("messages").join("all");
    let mut paths = HashSet::new();
    extend_json_files(&agent_dir.join("inbox"), &mut paths)?;
    extend_json_files(&broadcast_dir, &mut paths)?;

    let current_job = agent
        .get("associated_job")
        .or_else(|| agent.get("current_job"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let mut divisions = HashSet::new();
    if let Some(division) = agent
        .get("primary_division")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        divisions.insert(division.to_owned());
    }
    if let Some(job_id) = current_job {
        extend_json_files(&sky.join("messages").join("jobs").join(job_id), &mut paths)?;
        if let Some(job) = current_job_record(&sky, job_id)
            && let Some(related) = job.get("related_divisions").and_then(Value::as_array)
        {
            for division in related.iter().filter_map(Value::as_str) {
                if !division.trim().is_empty() {
                    divisions.insert(division.to_owned());
                }
            }
        }
    }
    for division in divisions {
        extend_json_files(
            &sky.join("divisions").join(division).join("inbox"),
            &mut paths,
        )?;
    }

    let bootstrap_history_seconds = if since.is_none() {
        read_json_file(&sky.join("CONFIG.json"))
            .ok()
            .and_then(|config| {
                config
                    .get("bootstrap_broadcast_history_seconds")
                    .and_then(Value::as_i64)
            })
            .unwrap_or(6 * 60 * 60)
            .max(0)
    } else {
        0
    };
    let bootstrap_cutoff = now.timestamp() - bootstrap_history_seconds;
    let mut mail = 0_u64;
    let mut announcements = 0_u64;
    for path in paths {
        let Ok(record) = read_json_file(&path) else {
            continue;
        };
        let created_text = record.get("created_at").and_then(Value::as_str);
        let created = parse_skyline_timestamp(record.get("created_at"));
        if created_text.is_some() && created.is_none() {
            continue;
        }
        if let Some(created) = created {
            if created > now {
                continue;
            }
            if let Some(since) = since {
                if created <= since {
                    continue;
                }
            } else if path.parent() == Some(broadcast_dir.as_path())
                && created.timestamp() < bootstrap_cutoff
            {
                continue;
            }
        }
        if path.parent() == Some(broadcast_dir.as_path()) {
            announcements += 1;
        } else {
            mail += 1;
        }
    }
    Ok(json!({"mail": mail, "announcements": announcements}))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn ensure_known_keys(
    arguments: &Map<String, Value>,
    allowed: &[&str],
    operation: &str,
) -> Result<()> {
    if let Some(key) = arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        bail!("unknown {operation} argument: {key}");
    }
    Ok(())
}

fn required_string<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{key} must be a non-empty string"))
}

fn optional_string<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value)),
        Some(Value::String(_)) => bail!("{key} must not be empty"),
        Some(_) => bail!("{key} must be a string"),
    }
}

fn optional_bool(arguments: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => bail!("{key} must be a boolean"),
    }
}

fn push_required_string(
    arguments: &Map<String, Value>,
    key: &str,
    flag: &str,
    argv: &mut Vec<String>,
) -> Result<()> {
    let value = required_string(arguments, key)?;
    argv.push(flag.into());
    argv.push(value.into());
    Ok(())
}

fn push_optional_string(
    arguments: &Map<String, Value>,
    key: &str,
    flag: &str,
    argv: &mut Vec<String>,
) -> Result<()> {
    if let Some(value) = optional_string(arguments, key)? {
        argv.push(flag.into());
        argv.push(value.into());
    }
    Ok(())
}

fn push_bool_flag(
    arguments: &Map<String, Value>,
    key: &str,
    flag: &str,
    argv: &mut Vec<String>,
) -> Result<()> {
    if optional_bool(arguments, key)?.unwrap_or(false) {
        argv.push(flag.into());
    }
    Ok(())
}

fn string_values(arguments: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(vec![value.clone()]),
        Some(Value::String(_)) => bail!("{key} must not be empty"),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| anyhow!("{key} entries must be non-empty strings"))
            })
            .collect(),
        Some(_) => bail!("{key} must be a string or array of strings"),
    }
}

fn push_repeated_strings(
    arguments: &Map<String, Value>,
    key: &str,
    flag: &str,
    argv: &mut Vec<String>,
) -> Result<()> {
    for value in string_values(arguments, key)? {
        argv.push(flag.into());
        argv.push(value);
    }
    Ok(())
}

fn push_required_repeated_strings(
    arguments: &Map<String, Value>,
    key: &str,
    flag: &str,
    argv: &mut Vec<String>,
) -> Result<()> {
    let values = string_values(arguments, key)?;
    if values.is_empty() {
        bail!("{key} requires at least one value");
    }
    for value in values {
        argv.push(flag.into());
        argv.push(value);
    }
    Ok(())
}
