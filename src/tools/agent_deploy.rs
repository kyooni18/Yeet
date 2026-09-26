//! Native child-agent deployment.
//!
//! A deployment gets its own background runtime and persisted session. The
//! caller only waits until the child session has an addressable id; the child
//! response continues independently after this tool returns.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{background::BackgroundConnection, model::FrontendCommand};

const DEPLOYMENT_START_TIMEOUT: Duration = Duration::from_secs(20);
const DEPLOYMENT_POLL_INTERVAL: Duration = Duration::from_millis(20);

impl super::ToolRegistry {
    pub(super) fn deploy_agent(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let _ = (object, cancel);
        bail!(
            "Agent deployment is exclusively available through builtin:skyline: use skyline with operation=deploy_agent after attaching Skyline"
        )
    }
}

/// Starts a detached native Yeet agent in a separate session runtime.
pub(crate) fn deploy_agent_for_workspace(
    workspace: &Path,
    task: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    let task = task.trim();
    if task.is_empty() {
        bail!("deploy_agent task must not be empty");
    }
    if task.chars().count() > 32_000 {
        bail!("deploy_agent task exceeds the 32,000 character limit");
    }
    if cancel.load(Ordering::Acquire) {
        bail!("agent deployment was cancelled before launch");
    }

    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve deployment workspace {}", workspace.display()))?;
    let scope = std::env::var("YEET_BACKGROUND_SCOPE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let mut connection = BackgroundConnection::connect_scoped(&workspace, scope.as_deref())
        .context("connect to Yeet background service for agent deployment")?;

    // A newly connected client gets a separate runtime when the caller's
    // runtime is attached or streaming. NewSession makes the child identity
    // unambiguous even when a detached idle runtime is available for reuse.
    connection
        .send(&FrontendCommand::NewSession)
        .context("create child Yeet session")?;
    if cancel.load(Ordering::Acquire) {
        let _ = connection.send(&FrontendCommand::Interrupt);
        bail!("agent deployment was cancelled before task submission");
    }
    connection
        .send(&FrontendCommand::Submit {
            text: task.to_owned(),
            images: Vec::new(),
            attachment_ids: Vec::new(),
        })
        .context("submit task to child Yeet agent")?;

    let deadline = Instant::now() + DEPLOYMENT_START_TIMEOUT;
    loop {
        if let Some(envelope) = connection.try_recv() {
            if envelope.kind == "error" {
                let message = envelope
                    .message
                    .unwrap_or_else(|| "child Yeet runtime returned an error".into());
                let _ = connection.send(&FrontendCommand::Interrupt);
                return Err(anyhow!(message));
            }
            if let Some(state) = envelope.state
                && let Some(session_id) = state.current_session_id
            {
                return Ok(json!({
                    "deployed": true,
                    "sessionId": session_id,
                    "workspace": workspace.display().to_string(),
                    "status": "running",
                    "detached": true,
                    "message": "Child Yeet agent is running in a separate session runtime. Open the returned sessionId in TUI or WebUI to follow it."
                })
                .to_string());
            }
        }

        if cancel.load(Ordering::Acquire) {
            let _ = connection.send(&FrontendCommand::Interrupt);
            bail!("agent deployment was cancelled before the child session became addressable");
        }
        if Instant::now() >= deadline {
            let _ = connection.send(&FrontendCommand::Interrupt);
            bail!(
                "child Yeet agent did not publish a session id within {} seconds",
                DEPLOYMENT_START_TIMEOUT.as_secs()
            );
        }
        thread::sleep(DEPLOYMENT_POLL_INTERVAL);
    }
}
