//! Reusable Yeet runtime attachment surface.
//!
//! Frontends depend on this module for agent and session execution.
//! A harness can either attach to the workspace's shared background runtime or
//! own an in-process runtime directly. The latter is intended for native apps,
//! tests, and other hosts that embed Yeet as a library and therefore cannot
//! rely on Yeet's CLI executable to own the daemon lifecycle.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub mod agent;
pub mod agents;
pub mod context_cache;
pub mod core;
pub mod model;
pub mod permission;
pub mod resources;
pub mod sandbox;
mod service;
pub mod session_store;
pub mod theme_resources;
pub mod tools;
pub mod workers;

pub use service::{HarnessClient, ServiceEvent, forward_cli};
pub(crate) use service::{HarnessService, SessionCatalog, tool_detail};

use crate::background::BackgroundConnection;

pub use model::{FrontendCommand as HarnessCommand, HarnessEvent, HarnessState};

/// Selects how a [`Harness`] is hosted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessMode {
    /// Attach to the workspace-wide daemon and share its session runtimes with
    /// other Yeet frontends.
    Shared,
    /// Own the full Yeet session runtime inside the current process.
    Embedded,
}

/// Construction options for a reusable Yeet harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessOptions {
    pub mode: HarnessMode,
    /// Optional background-daemon namespace. Scopes are only meaningful for
    /// shared harnesses.
    pub scope: Option<String>,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            mode: HarnessMode::Shared,
            scope: None,
        }
    }
}

enum HarnessTransport {
    Shared(BackgroundConnection),
    Embedded(HarnessService),
}

/// Frontend-neutral handle to Yeet's agent/session runtime.
///
/// `Harness` contains no terminal, Ratatui, WebSocket, or stdin/stdout logic.
/// Hosts submit [`HarnessCommand`] values and consume [`HarnessEvent`] values,
/// so the same runtime can be attached to a TUI, native application, service,
/// test harness, or another presentation layer.
pub struct Harness {
    workspace: PathBuf,
    mode: HarnessMode,
    transport: HarnessTransport,
    latest_state: Option<HarnessState>,
}

impl Harness {
    /// Attaches to the shared runtime for `workspace`.
    pub fn attach(workspace: impl AsRef<Path>) -> Result<Self> {
        Self::with_options(workspace, HarnessOptions::default())
    }

    /// Attaches to the shared runtime for the process current directory.
    pub fn attach_current() -> Result<Self> {
        let workspace = std::env::current_dir().context("resolve current workspace")?;
        Self::attach(workspace)
    }

    /// Starts an in-process runtime that is independent of the background
    /// daemon and of the executable hosting this library.
    pub fn embedded(workspace: impl AsRef<Path>) -> Result<Self> {
        Self::with_options(
            workspace,
            HarnessOptions {
                mode: HarnessMode::Embedded,
                scope: None,
            },
        )
    }

    /// Creates a harness with explicit hosting options.
    pub fn with_options(workspace: impl AsRef<Path>, options: HarnessOptions) -> Result<Self> {
        let workspace = resolve_workspace(workspace.as_ref())?;
        if options.mode == HarnessMode::Embedded && options.scope.is_some() {
            bail!("embedded harnesses do not use background scopes");
        }

        let (transport, latest_state) = match options.mode {
            HarnessMode::Shared => (
                HarnessTransport::Shared(BackgroundConnection::connect_scoped(
                    &workspace,
                    options.scope.as_deref(),
                )?),
                None,
            ),
            HarnessMode::Embedded => {
                let service = HarnessService::spawn(workspace.clone(), None)?;
                let state = service.state_snapshot();
                (HarnessTransport::Embedded(service), Some(state))
            }
        };

        Ok(Self {
            workspace,
            mode: options.mode,
            transport,
            latest_state,
        })
    }

    /// Canonical workspace root owned or attached by this harness.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn mode(&self) -> HarnessMode {
        self.mode
    }

    /// Most recent state observed by this host. Embedded harnesses have an
    /// initial snapshot immediately; shared harnesses populate this after the
    /// daemon's initial state event is consumed.
    pub fn latest_state(&self) -> Option<&HarnessState> {
        self.latest_state.as_ref()
    }

    /// Sends a frontend-neutral runtime command.
    pub fn send(&mut self, command: HarnessCommand) -> Result<()> {
        match &mut self.transport {
            HarnessTransport::Shared(connection) => connection.send(&command),
            HarnessTransport::Embedded(service) => service.send(command),
        }
    }

    /// Returns the next runtime event without blocking.
    pub fn try_recv(&mut self) -> Option<HarnessEvent> {
        let event = match &mut self.transport {
            HarnessTransport::Shared(connection) => connection.try_recv(),
            HarnessTransport::Embedded(service) => match service.try_recv()? {
                ServiceEvent::Envelope(envelope) => Some(envelope),
            },
        }?;

        if let Some(state) = event.state.as_ref() {
            self.latest_state = Some(state.clone());
        }
        Some(event)
    }
}

fn resolve_workspace(workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve harness workspace {}", workspace.display()))?;
    if !workspace.is_dir() {
        bail!(
            "harness workspace is not a directory: {}",
            workspace.display()
        );
    }
    Ok(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_workspace_independently_of_frontend_state() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();

        assert_eq!(
            resolve_workspace(&nested).unwrap(),
            nested.canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_missing_workspace_before_starting_any_runtime() {
        let missing =
            std::env::temp_dir().join(format!("yeet-harness-missing-{}", uuid::Uuid::new_v4()));
        let error = resolve_workspace(&missing).unwrap_err().to_string();
        assert!(error.contains("resolve harness workspace"));
    }
}
