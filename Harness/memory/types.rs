//! Serializable memory contracts, validation and deliberately narrow authority.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    Explicit,
    WholeSession,
}

impl MemoryKind {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::WholeSession => "whole-session",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Privacy {
    Shared,
    LocalOnly,
}

impl Privacy {
    pub(super) fn schema(self) -> &'static str {
        match self {
            Self::Shared => "main",
            Self::LocalOnly => "protected",
        }
    }
}

/// Scope is assigned/intersected by the host, never trusted from a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "workspaceId", rename_all = "kebab-case")]
pub enum MemoryScope {
    Workspace(String),
    UserGlobal,
}

impl MemoryScope {
    pub(super) fn key(&self) -> &str {
        match self {
            Self::Workspace(id) => id,
            Self::UserGlobal => "",
        }
    }
}

/// Only a locally authorized user-management host may construct management
/// authority. Do not derive this from a remote request or chat-provider label.
#[derive(Debug, Clone)]
pub struct MemoryAccess {
    pub(super) workspace_id: String,
    pub(super) management: bool,
}

impl MemoryAccess {
    pub fn local_management(workspace_id: String) -> Result<Self> {
        ensure!(!workspace_id.is_empty(), "workspace identity is required");
        Ok(Self {
            workspace_id,
            management: true,
        })
    }
    /// Protected model recall is unavailable until all retained-context and
    /// background egress guards are implemented; unknown providers fail closed.
    pub fn shared_consumer(workspace_id: String) -> Result<Self> {
        let mut access = Self::local_management(workspace_id)?;
        access.management = false;
        Ok(access)
    }
    pub(super) fn allows(&self, scope: &MemoryScope, privacy: Privacy) -> bool {
        (privacy == Privacy::Shared || self.management)
            && (scope == &MemoryScope::UserGlobal
                || scope == &MemoryScope::Workspace(self.workspace_id.clone()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KindSettings {
    pub enabled: bool,
    pub local_only: bool,
    pub shared_profile_id: Option<String>,
    pub local_profile_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemorySettings {
    pub revision: u64,
    pub explicit: KindSettings,
    pub whole_session: KindSettings,
}

impl MemorySettings {
    pub fn kind(&self, kind: MemoryKind) -> &KindSettings {
        match kind {
            MemoryKind::Explicit => &self.explicit,
            MemoryKind::WholeSession => &self.whole_session,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemorySource {
    pub session_id: String,
    pub window_id: String,
    pub source_revision: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub id: String,
    pub revision: u64,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub privacy: Privacy,
    pub text: String,
    pub tags: Vec<String>,
    pub source: Option<MemorySource>,
    pub edited: bool,
    pub created_at: String,
    pub updated_at: String,
    pub indexing_status: String,
}

#[derive(Debug, Clone)]
pub struct SaveMemory {
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub privacy: Privacy,
    pub text: String,
    pub tags: Vec<String>,
    pub source: Option<MemorySource>,
}

impl SaveMemory {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            !self.text.trim().is_empty() && self.text.len() <= 128 * 1024,
            "memory text must contain 1..131072 bytes"
        );
        ensure!(
            self.tags.len() <= 32 && self.tags.iter().all(|s| s.len() <= 128),
            "invalid memory tags"
        );
        if let MemoryScope::Workspace(id) = &self.scope {
            ensure!(
                !id.is_empty() && id.len() <= 256,
                "invalid workspace identity"
            );
        }
        ensure!(
            self.kind != MemoryKind::WholeSession || self.source.is_some(),
            "session memory requires provenance"
        );
        if let Some(source) = &self.source {
            ensure!(
                !source.session_id.is_empty()
                    && !source.source_revision.is_empty()
                    && source.start <= source.end,
                "invalid source provenance"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub kind: Option<MemoryKind>,
    pub privacy: Option<Privacy>,
    pub query: Option<String>,
    pub after: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPage {
    pub records: Vec<MemoryRecord>,
    pub next_cursor: Option<String>,
}
