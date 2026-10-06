//! Session-scoped store tools: the artifact store and the session catalog.
//!
//! Artifact tools are read-only views over bounded, externalized tool output.
//! Session tools read the session store through the execution context;
//! exports pass the file-scope check before writing outside the store.

use super::*;

impl ToolRegistry {
    pub(super) fn list_sessions_tool(&self, object: &Map<String, Value>) -> Result<String> {
        let store = self
            .context
            .session_store
            .as_ref()
            .ok_or_else(|| anyhow!("Session runtime is not attached"))?;
        let debate_only = object
            .get("debateOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include_current = object
            .get("includeCurrent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let limit = usize_arg(object, "limit").unwrap_or(20).clamp(1, 100);
        let mut values = Vec::new();
        for session in store.list(&self.context.workspace_root)? {
            let is_current = self.context.active_session_id.as_deref() == Some(session.id.as_str());
            if is_current && !include_current {
                continue;
            }
            let is_debate = store.is_debate_session(&session.id);
            if debate_only && !is_debate {
                continue;
            }
            values.push(json!({
                "id": session.id,
                "title": session.title,
                "updatedAt": session.updated_at,
                "model": session.model,
                "messageCount": session.message_count,
                "isDebate": is_debate,
                "isCurrent": is_current,
            }));
            if values.len() >= limit {
                break;
            }
        }
        Ok(json!({
            "sessions": values,
            "currentSessionId": self.context.active_session_id,
            "currentExcludedByDefault": !include_current,
            "predicate": if debate_only { "debates/state.json exists and topic is non-empty" } else { "workspace session" },
        }).to_string())
    }

    pub(super) fn export_session_tool(&self, object: &Map<String, Value>) -> Result<String> {
        let session_id = string_arg(object, "sessionId")?.to_owned();
        let destination = object
            .get("destination")
            .and_then(Value::as_str)
            .map(|value| {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    self.context.workspace_root.join(path)
                }
            });
        if let Some(destination) = destination.as_ref() {
            self.context
                .ensure_file_scope(&destination.to_string_lossy(), true)?;
        }
        let delete_source = object
            .get("deleteSource")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let store = self
            .context
            .session_store
            .as_ref()
            .ok_or_else(|| anyhow!("Session runtime is not attached"))?;
        let result = store.export_session(
            &session_id,
            destination.as_deref(),
            delete_source,
            self.context.active_session_id.as_deref(),
        )?;
        Ok(serde_json::to_string(&result)?)
    }
}

impl ToolRegistry {
    /// Executes read-only artifact tools against the session artifact store.
    pub(super) fn execute_artifact_tool(
        &self,
        name: &str,
        object: &Map<String, Value>,
    ) -> Result<String> {
        match name {
            "artifact_info" => {
                let id = string_arg(object, "id")?;
                Ok(self.artifacts.info(id)?.to_string())
            }
            "read_artifact" => {
                let id = string_arg(object, "id")?;
                let text = self.artifacts.read(
                    id,
                    usize_arg(object, "startLine"),
                    usize_arg(object, "endLine"),
                )?;
                artifact_output::page(&text, object)
            }
            "search_artifact" => {
                let id = string_arg(object, "id")?;
                let query = string_arg(object, "query")?;
                Ok(self
                    .artifacts
                    .search(
                        id,
                        query,
                        usize_arg(object, "maxResults").unwrap_or(20).clamp(1, 50),
                    )?
                    .to_string())
            }
            other => bail!("{other} is not an artifact tool"),
        }
    }
}
