use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::core::{edit_daemon_script, node_executable};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub path: String,
    pub snapshot: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub content: String,
    pub numbered: String,
    pub anchored: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub files_scanned: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesResult {
    pub entries: Vec<WorkspaceEntry>,
    pub truncated: bool,
    pub result_limit_reached: bool,
    pub depth_limited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub path: String,
    pub severity: String,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileApplyResult {
    pub path: String,
    pub destination: Option<String>,
    pub operation: String,
    pub snapshot: Option<String>,
    pub anchors: Option<String>,
    pub diff: Option<Value>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub files: Vec<FileApplyResult>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct EditClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl EditClient {
    pub fn start(root: &Path) -> Result<Self> {
        let node = node_executable()?;
        let script = edit_daemon_script()?;
        let mut child = Command::new(node)
            .arg(script)
            .arg("--root")
            .arg(root)
            .arg("--allow-outside")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to start Yeet edit daemon")?;
        let stdin = child
            .stdin
            .take()
            .context("edit daemon stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("edit daemon stdout unavailable")?;
        let mut client = Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        };
        let _: Value = client.call("health", json!({}))?;
        Ok(client)
    }

    pub fn read(
        &mut self,
        path: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
        unsafe_access: bool,
    ) -> Result<ReadResult> {
        let mut params = serde_json::Map::new();
        params.insert("path".into(), json!(path));
        if let Some(value) = start_line {
            params.insert("startLine".into(), json!(value));
        }
        if let Some(value) = end_line {
            params.insert("endLine".into(), json!(value));
        }
        if unsafe_access {
            params.insert("unsafe".into(), json!(true));
        }
        self.call("read", Value::Object(params))
    }

    pub fn search(
        &mut self,
        query: &str,
        path: Option<&str>,
        max_results: usize,
        case_sensitive: bool,
        regex: bool,
    ) -> Result<SearchResult> {
        let mut params = serde_json::Map::new();
        params.insert("query".into(), json!(query));
        if let Some(path) = path {
            params.insert("path".into(), json!(path));
        }
        params.insert("maxResults".into(), json!(max_results));
        params.insert("caseSensitive".into(), json!(case_sensitive));
        params.insert("regex".into(), json!(regex));
        self.call("search", Value::Object(params))
    }

    pub fn list_files(
        &mut self,
        path: Option<&str>,
        max_results: usize,
        max_depth: usize,
    ) -> Result<ListFilesResult> {
        let mut params = serde_json::Map::new();
        if let Some(path) = path {
            params.insert("path".into(), json!(path));
        }
        params.insert("maxResults".into(), json!(max_results));
        params.insert("maxDepth".into(), json!(max_depth));
        self.call("listFiles", Value::Object(params))
    }

    pub fn apply(&mut self, request: &Value, unsafe_access: bool) -> Result<ApplyResult> {
        let mut request = request.clone();
        if unsafe_access {
            request
                .as_object_mut()
                .ok_or_else(|| anyhow!("apply request must be an object"))?
                .insert("unsafe".into(), json!(true));
        }
        self.call("apply", request)
    }

    pub fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        let id = Uuid::new_v4().to_string();
        serde_json::to_writer(
            &mut self.stdin,
            &json!({ "id": id, "method": method, "params": params }),
        )?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = String::new();
        loop {
            line.clear();
            let count = self.stdout.read_line(&mut line)?;
            if count == 0 {
                let status = self
                    .child
                    .try_wait()?
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown".into());
                bail!("edit daemon closed stdout (status {status})");
            }
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line).context("invalid edit daemon JSON")?;
            if value.get("id").and_then(Value::as_str) != Some(&id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(anyhow!(
                    "{}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("edit daemon error")
                ));
            }
            return serde_json::from_value(value.get("result").cloned().unwrap_or(Value::Null))
                .context("invalid edit daemon result");
        }
    }
}

impl Drop for EditClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
