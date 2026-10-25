//! Artifact tools: read-only views over bounded, externalized tool output.

use super::*;

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
                self.artifacts.read_page(
                    id,
                    usize_arg(object, "startLine"),
                    usize_arg(object, "endLine"),
                    usize_arg(object, "offset").unwrap_or(0),
                    usize_arg(object, "maxChars").unwrap_or(8192),
                )
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
