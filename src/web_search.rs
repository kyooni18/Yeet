use std::{
    env,
    fs::{self, OpenOptions},
    io::Read,
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::{
    config::ConfigStore,
    core::BridgeClient,
    platform::{TrackedChild, configure_hidden_process_group},
    project_settings::{ProjectSettingsStore, ServiceBackend},
};

pub const CAPABILITY_ID: &str = "web-search";
const DEFAULT_URL: &str = "http://127.0.0.1:8888";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const AGENT_REACH_OPERATION_TIMEOUT: Duration = Duration::from_secs(20);
const RESTART_BACKOFF: Duration = Duration::from_secs(3);
const MAX_SEARCH_LOG_BYTES: u64 = 8 * 1024 * 1024;
const AGENT_REACH_URL: &str = "https://github.com/Panniantong/agent-reach/archive/main.zip";
const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchBackend {
    Auto,
    AgentReach,
    Searxng,
}

#[derive(Debug, Clone, Copy)]
pub struct WebSearchRequest<'a> {
    pub query: &'a str,
    pub max_results: usize,
    pub language: Option<&'a str>,
    pub category: Option<&'a str>,
    pub time_range: Option<&'a str>,
    pub safe_search: Option<u64>,
    pub page: Option<u64>,
    pub backend: WebSearchBackend,
}

impl WebSearchBackend {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("auto") => Ok(Self::Auto),
            Some("agent-reach") | Some("agent_reach") | Some("exa") => Ok(Self::AgentReach),
            Some("searxng") | Some("searx") => Ok(Self::Searxng),
            Some(value) => bail!(
                "unknown web search backend '{value}'; expected auto, agent-reach, or searxng"
            ),
        }
    }
}

#[derive(Debug)]
struct SearchPaths {
    root: PathBuf,
    source: PathBuf,
    venv: PathBuf,
    settings: PathBuf,
    log: PathBuf,
    agent_reach_venv: PathBuf,
    agent_reach_node: PathBuf,
    agent_reach_mcporter_config: PathBuf,
}

impl SearchPaths {
    fn discover() -> Self {
        let root = ConfigStore::default().directory.join("search");
        Self {
            source: root.join("searxng-src"),
            venv: root.join("venv"),
            settings: root.join("settings.yml"),
            log: root.join("searxng.log"),
            agent_reach_venv: root.join("agent-reach-venv"),
            agent_reach_node: root.join("agent-reach-node"),
            agent_reach_mcporter_config: root.join("agent-reach-mcporter.json"),
            root,
        }
    }

    fn python(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.venv.join("Scripts/python.exe")
        }
        #[cfg(not(windows))]
        {
            self.venv.join("bin/python")
        }
    }

    fn installed(&self) -> bool {
        self.python().is_file()
            && self.source.join("searx/webapp.py").is_file()
            && self.settings.is_file()
    }

    fn agent_reach_python(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.agent_reach_venv.join("Scripts/python.exe")
        }
        #[cfg(not(windows))]
        {
            self.agent_reach_venv.join("bin/python")
        }
    }

    fn agent_reach_cli(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.agent_reach_venv.join("Scripts/agent-reach.exe")
        }
        #[cfg(not(windows))]
        {
            self.agent_reach_venv.join("bin/agent-reach")
        }
    }

    fn managed_mcporter(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.agent_reach_node.join("node_modules/.bin/mcporter.cmd")
        }
        #[cfg(not(windows))]
        {
            self.agent_reach_node.join("node_modules/.bin/mcporter")
        }
    }

    fn agent_reach_installed(&self) -> bool {
        self.agent_reach_python().is_file()
            && self.agent_reach_cli().is_file()
            && self.managed_mcporter().is_file()
            && self.agent_reach_mcporter_config.is_file()
    }

    fn native_installed(&self) -> bool {
        self.root.join("native-exa.enabled").is_file()
    }
}

#[derive(Debug, Default)]
struct ProcessState {
    child: Option<TrackedChild>,
    base_url: Option<String>,
    restart_not_before: Option<Instant>,
}

#[derive(Debug)]
struct AgentReachInvocation {
    mcporter: PathBuf,
    config: Option<PathBuf>,
}

pub struct WebSearchClient {
    paths: SearchPaths,
    state: Mutex<ProcessState>,
    searxng_url: Mutex<Option<String>>,
    agent_failure: Mutex<Option<(Instant, String)>>,
    agent: ureq::Agent,
}

impl Default for WebSearchClient {
    fn default() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(20))
            .timeout_write(Duration::from_secs(5))
            .redirects(0)
            .build();
        Self {
            paths: SearchPaths::discover(),
            state: Mutex::new(ProcessState::default()),
            searxng_url: Mutex::new(None),
            agent_failure: Mutex::new(None),
            agent,
        }
    }
}

impl WebSearchClient {
    pub fn set_searxng_url(&self, url: Option<String>) {
        if let Ok(mut current) = self.searxng_url.lock() {
            *current = url;
        }
    }

    pub fn read_url(&self, url: &str, max_chars: usize, cancel: &AtomicBool) -> Result<Value> {
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        validate_public_http_url(url)?;
        let max_chars = max_chars.clamp(2_000, 48_000);
        if self.paths.native_installed() {
            return self.read_url_direct(url, max_chars, cancel);
        }
        match self.read_url_agent_reach(url, max_chars, cancel) {
            Ok(value) => Ok(value),
            Err(error) => {
                if cancel.load(Ordering::Acquire) {
                    bail!("cancelled");
                }
                let mut fallback = self.read_url_direct(url, max_chars, cancel)?;
                if let Some(object) = fallback.as_object_mut() {
                    object.insert("fallbackFrom".into(), json!("agent-reach/exa"));
                    object.insert(
                        "fallbackReason".into(),
                        json!(truncate_chars(&error.to_string(), 320)),
                    );
                }
                Ok(fallback)
            }
        }
    }

    fn read_url_direct(&self, url: &str, max_chars: usize, cancel: &AtomicBool) -> Result<Value> {
        let max_bytes = (max_chars.saturating_mul(4)).clamp(16_000, 256_000);
        let original = Url::parse(url).with_context(|| format!("invalid web source URL {url}"))?;
        let mut current = original.clone();
        let mut redirects = 0usize;
        let response = loop {
            ensure_public_url(&current)?;
            let result = self
                .agent
                .get(current.as_str())
                .set(
                    "User-Agent",
                    "Mozilla/5.0 (compatible; YeetResearch/0.1; +https://github.com/)",
                )
                .call();
            let response = match result {
                Ok(response) => response,
                Err(ureq::Error::Status(status, response)) if (300..400).contains(&status) => {
                    response
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to read web source {current}"));
                }
            };
            if !(300..400).contains(&response.status()) {
                break response;
            }
            if redirects >= 4 {
                bail!("web source redirected too many times");
            }
            let location = response
                .header("location")
                .ok_or_else(|| anyhow!("web source redirect omitted Location"))?;
            current = current
                .join(location)
                .with_context(|| format!("invalid redirect from {current}"))?;
            redirects += 1;
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
        };
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        let status = response.status();
        let content_type = response
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_ascii_lowercase();
        if !(content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("html"))
        {
            bail!(
                "web source has unsupported content type {content_type}; only textual pages can be read directly"
            );
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take((max_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .context("failed reading web source body")?;
        let byte_truncated = bytes.len() > max_bytes;
        bytes.truncate(max_bytes);
        let raw = String::from_utf8_lossy(&bytes);
        let text = if content_type.contains("html") {
            html_to_readable_text(&raw)
        } else {
            raw.into_owned()
        };
        let char_truncated = text.chars().count() > max_chars;
        let content = if char_truncated {
            text.chars().take(max_chars).collect::<String>()
        } else {
            text
        };
        Ok(json!({
            "url": original.as_str(),
            "finalUrl": current.as_str(),
            "source": "direct-http",
            "status": status,
            "contentType": content_type,
            "content": content,
            "truncated": byte_truncated || char_truncated,
        }))
    }

    pub fn search(&self, request: WebSearchRequest<'_>) -> Result<Value> {
        let cancel = AtomicBool::new(false);
        self.search_cancellable(request, &cancel)
    }

    pub fn search_cancellable(
        &self,
        request: WebSearchRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let WebSearchRequest {
            query,
            max_results,
            language,
            category,
            time_range,
            safe_search,
            page,
            backend,
        } = request;
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        let query = query.trim();
        if query.is_empty() {
            bail!("web_search requires a non-empty query");
        }

        let has_searxng_only_filters = language.is_some()
            || category.is_some()
            || time_range.is_some()
            || safe_search.is_some()
            || page.is_some_and(|page| page > 1);

        match backend {
            WebSearchBackend::AgentReach => {
                if has_searxng_only_filters {
                    bail!(
                        "agent-reach web search does not support language/category/timeRange/safeSearch/page filters; use backend=searxng or backend=auto"
                    );
                }
                self.search_agent_reach(query, max_results, cancel)
            }
            WebSearchBackend::Searxng => self.search_searxng(request, cancel),
            WebSearchBackend::Auto if has_searxng_only_filters => {
                self.search_searxng(request, cancel)
            }
            WebSearchBackend::Auto => match if self.paths.native_installed() {
                self.search_native_at(EXA_MCP_URL, query, max_results, cancel)
            } else {
                self.search_agent_reach(query, max_results, cancel)
            } {
                Ok(result) => Ok(result),
                Err(agent_reach_error) => {
                    if cancel.load(Ordering::Acquire) {
                        bail!("cancelled");
                    }
                    let mut fallback = match self.search_searxng(request, cancel) {
                        Ok(value) => value,
                        Err(_) if cancel.load(Ordering::Acquire) => bail!("cancelled"),
                        Err(error) => bail!(
                            "Web search failed: {agent_reach_error:#}; SearXNG fallback failed: {error:#}"
                        ),
                    };
                    if let Some(object) = fallback.as_object_mut() {
                        object.insert(
                            "fallbackFrom".into(),
                            json!(if self.paths.native_installed() {
                                "exa"
                            } else {
                                "agent-reach"
                            }),
                        );
                        object.insert(
                            "fallbackReason".into(),
                            json!(truncate_chars(&agent_reach_error.to_string(), 320)),
                        );
                    }
                    Ok(fallback)
                }
            },
        }
    }

    fn search_native_at(
        &self,
        endpoint: &str,
        query: &str,
        max_results: usize,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let agent = self.agent.clone();
        let endpoint = endpoint.to_owned();
        let query = query.to_owned();
        let max_results = max_results.clamp(1, 20);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> Result<Value> {
                let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"web_search_exa","arguments":{"query":query,"numResults":max_results}}});
                let response = agent
                    .post(&endpoint)
                    .set("Content-Type", "application/json")
                    .set("Accept", "application/json, text/event-stream")
                    .set("MCP-Protocol-Version", "2025-03-26")
                    .send_string(&request.to_string())
                    .map_err(|error| anyhow!("Exa search request failed: {error}"))?;
                let mut body = String::new();
                response
                    .into_reader()
                    .take(2_000_001)
                    .read_to_string(&mut body)
                    .context("read Exa search response")?;
                if body.len() > 2_000_000 {
                    bail!("Exa search response exceeded the size limit");
                }
                let payload = if body.trim_start().starts_with('{') {
                    body.trim().to_owned()
                } else {
                    body.lines()
                        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let value: Value =
                    serde_json::from_str(&payload).context("decode Exa MCP response")?;
                if let Some(error) = value.get("error") {
                    bail!("Exa search failed: {error}");
                }
                let result = value
                    .get("result")
                    .context("Exa MCP response missing result")?;
                if result.get("isError").and_then(Value::as_bool) == Some(true) {
                    bail!("Exa search failed: {result}");
                }
                let mut normalized =
                    normalize_agent_reach_output(&query, &result.to_string(), max_results)?;
                if let Some(object) = normalized.as_object_mut() {
                    object.insert("source".into(), json!("exa"));
                    if let Some(results) = object.get_mut("results").and_then(Value::as_array_mut) {
                        for result in results {
                            if let Some(entry) = result.as_object_mut() {
                                entry.insert("engines".into(), json!(["exa"]));
                            }
                        }
                    }
                }
                Ok(normalized)
            })();
            let _ = tx.send(result);
        });
        loop {
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("Exa search worker stopped unexpectedly")
                }
            }
        }
    }

    fn search_searxng(&self, request: WebSearchRequest<'_>, cancel: &AtomicBool) -> Result<Value> {
        let WebSearchRequest {
            query,
            max_results,
            language,
            category,
            time_range,
            safe_search,
            page,
            ..
        } = request;
        let base_url = self.ensure_ready(cancel)?;
        let endpoint = format!("{}/search", base_url.trim_end_matches('/'));
        let agent = self.agent.clone();
        let query_owned = query.to_owned();
        let language = language.map(str::to_owned);
        let category = category.map(str::to_owned);
        let time_range = time_range.map(str::to_owned);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut request = agent
                .get(&endpoint)
                .query("q", &query_owned)
                .query("format", "json");
            if let Some(language) = language.as_deref().filter(|value| !value.trim().is_empty()) {
                request = request.query("language", language);
            }
            if let Some(category) = category.as_deref().filter(|value| !value.trim().is_empty()) {
                request = request.query("categories", category);
            }
            if let Some(time_range) = time_range.as_deref() {
                request = request.query("time_range", time_range);
            }
            let safe_search_text = safe_search.map(|value| value.to_string());
            if let Some(value) = safe_search_text.as_deref() {
                request = request.query("safesearch", value);
            }
            let page_text = page.map(|value| value.to_string());
            if let Some(value) = page_text.as_deref() {
                request = request.query("pageno", value);
            }
            let result = (|| {
                let response = request
                    .call()
                    .map_err(|error| anyhow!("SearXNG search failed: {error}"))?;
                let body = response.into_string().context("read SearXNG response")?;
                let value: Value =
                    serde_json::from_str(&body).context("decode SearXNG JSON response")?;
                if !value.get("results").is_some_and(Value::is_array) {
                    bail!("SearXNG returned a response without a results array");
                }
                Ok(normalize_results(
                    &query_owned,
                    value,
                    max_results.clamp(1, 20),
                ))
            })();
            let _ = tx.send(result);
        });
        loop {
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("SearXNG search worker stopped unexpectedly")
                }
            }
        }
    }

    fn search_agent_reach(
        &self,
        query: &str,
        max_results: usize,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let arguments = json!({
            "query": query,
            "numResults": max_results.clamp(1, 20),
        });
        let mut failure = loop {
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
            match self.agent_failure.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    bail!("Agent-Reach state lock poisoned")
                }
            }
        };
        if let Some((until, error)) = failure.as_ref()
            && Instant::now() < *until
        {
            bail!("Agent-Reach is cooling down after a failed search: {error}");
        }
        let result = self
            .invoke_agent_reach("exa.web_search_exa", arguments, cancel, "search")
            .and_then(|stdout| {
                normalize_agent_reach_output(query, &stdout, max_results.clamp(1, 20))
            });
        match &result {
            Ok(_) => *failure = None,
            Err(error) if !cancel.load(Ordering::Acquire) => {
                *failure = Some((Instant::now() + RESTART_BACKOFF, error.to_string()))
            }
            Err(_) => {}
        }
        result
    }

    fn read_url_agent_reach(
        &self,
        url: &str,
        max_chars: usize,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let arguments = json!({
            "urls": [url],
            "maxCharacters": max_chars,
        });
        let stdout =
            self.invoke_agent_reach("exa.web_fetch_exa", arguments, cancel, "source fetch")?;
        let content = stdout.trim();
        if content.is_empty() {
            bail!("Agent-Reach Exa source fetch returned no content");
        }
        Ok(json!({
            "url": url,
            "source": "agent-reach/exa",
            "status": 200,
            "contentType": "text/markdown",
            "content": truncate_chars(content, max_chars),
            "truncated": content.chars().count() > max_chars,
        }))
    }

    fn invoke_agent_reach(
        &self,
        tool: &str,
        arguments: Value,
        cancel: &AtomicBool,
        operation: &str,
    ) -> Result<String> {
        let invocation = self.agent_reach_invocation().ok_or_else(|| {
            anyhow!(
                "Agent-Reach is not installed for Yeet. Run `yeet search install agent-reach`, or install agent-reach + mcporter on PATH."
            )
        })?;
        let mut command = Command::new(&invocation.mcporter);
        command
            .arg("call")
            .arg(tool)
            .arg("--args")
            .arg(arguments.to_string())
            .arg("--output")
            .arg("text")
            .arg("--no-oauth")
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(config) = invocation.config.as_ref() {
            command.env("MCPORTER_CONFIG", config);
        }
        configure_hidden_process_group(&mut command);
        let mut child = command
            .spawn()
            .with_context(|| format!("run Agent-Reach Exa {operation} through mcporter"))?;
        let mut stdout = child
            .stdout
            .take()
            .context("Agent-Reach stdout unavailable")?;
        let mut stderr = child
            .stderr
            .take()
            .context("Agent-Reach stderr unavailable")?;
        let mut child = TrackedChild::new(child, "agent-reach");
        let stdout_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let stderr_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        let status = wait_for_agent_reach_child(
            &mut child,
            cancel,
            AGENT_REACH_OPERATION_TIMEOUT,
            operation,
        )?;
        let stdout = stdout_thread.join().unwrap_or_default();
        let stderr = stderr_thread.join().unwrap_or_default();
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
            bail!(
                "Agent-Reach Exa {operation} failed with {}{}",
                status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {}", truncate_chars(&stderr, 500))
                }
            );
        }
        String::from_utf8(stdout).with_context(|| format!("decode Agent-Reach {operation} output"))
    }

    fn agent_reach_invocation(&self) -> Option<AgentReachInvocation> {
        if self.paths.agent_reach_installed() {
            return Some(AgentReachInvocation {
                mcporter: self.paths.managed_mcporter(),
                config: Some(self.paths.agent_reach_mcporter_config.clone()),
            });
        }
        if command_available("agent-reach") && command_available("mcporter") {
            return Some(AgentReachInvocation {
                mcporter: PathBuf::from("mcporter"),
                config: None,
            });
        }
        None
    }

    pub fn shutdown(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.base_url = None;
    }

    fn ensure_ready(&self, cancel: &AtomicBool) -> Result<String> {
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        let configured = self
            .searxng_url
            .lock()
            .ok()
            .and_then(|url| url.clone())
            .or_else(|| {
                env::var("YEET_SEARXNG_URL")
                    .ok()
                    .filter(|url| !url.trim().is_empty())
            });
        if let Some(url) = configured {
            let url = url.trim_end_matches('/').to_owned();
            let parsed = Url::parse(&url).context("Invalid SearXNG endpoint")?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                bail!("SearXNG endpoint must be an HTTP or HTTPS URL without credentials");
            }
            if self.probe(&url) {
                return Ok(url);
            }
            bail!("No SearXNG instance answered at {url}");
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("web search process lock poisoned"))?;
        if let Some(url) = state.base_url.clone() {
            if self.probe(&url) {
                return Ok(url);
            }
            if let Some(mut child) = state.child.take()
                && child.try_wait()?.is_none()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            state.base_url = None;
        }

        if self.probe(DEFAULT_URL) {
            state.base_url = Some(DEFAULT_URL.into());
            state.restart_not_before = None;
            return Ok(DEFAULT_URL.into());
        }
        if let Some(not_before) = state.restart_not_before
            && Instant::now() < not_before
        {
            bail!("managed SearXNG restart is cooling down after a failed startup");
        }
        if !self.paths.installed() {
            bail!(
                "SearXNG is not installed. Run `yeet search install searxng`, or set YEET_SEARXNG_URL to an existing instance."
            );
        }

        fs::create_dir_all(&self.paths.root)?;
        rotate_search_log(&self.paths.log)?;
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log)?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(self.paths.python());
        command
            .arg("-m")
            .arg("searx.webapp")
            .current_dir(&self.paths.source)
            .env("SEARXNG_SETTINGS_PATH", &self.paths.settings)
            .env("SEARXNG_BIND_ADDRESS", "127.0.0.1")
            .env("SEARXNG_PORT", "8888")
            .env("SEARXNG_BASE_URL", format!("{DEFAULT_URL}/"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        configure_hidden_process_group(&mut command);
        let child = command.spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
                return Err(error).with_context(|| {
                    format!("start SearXNG with {}", self.paths.python().display())
                });
            }
        };
        let mut child = TrackedChild::new(child, "searxng");

        let started = Instant::now();
        while started.elapsed() < STARTUP_TIMEOUT {
            if cancel.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cancelled");
            }
            if self.probe(DEFAULT_URL) {
                state.base_url = Some(DEFAULT_URL.into());
                state.child = Some(child);
                state.restart_not_before = None;
                return Ok(DEFAULT_URL.into());
            }
            if let Some(status) = child.try_wait()? {
                state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
                bail!(
                    "SearXNG exited during startup with {status}. See {}",
                    self.paths.log.display()
                );
            }
            thread::sleep(Duration::from_millis(125));
        }
        let _ = child.kill();
        let _ = child.wait();
        state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
        bail!(
            "SearXNG did not become ready within {} seconds. See {}",
            STARTUP_TIMEOUT.as_secs(),
            self.paths.log.display()
        )
    }

    fn probe(&self, base_url: &str) -> bool {
        let url = format!("{}/config", base_url.trim_end_matches('/'));
        let Ok(response) = self.agent.get(&url).call() else {
            return false;
        };
        let Ok(body) = response.into_string() else {
            return false;
        };
        serde_json::from_str::<Value>(&body)
            .ok()
            .is_some_and(|value| value.get("engines").and_then(Value::as_array).is_some())
    }
}

fn rotate_search_log(path: &Path) -> Result<()> {
    if fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        <= MAX_SEARCH_LOG_BYTES
    {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&rotated);
    fs::rename(path, rotated)?;
    Ok(())
}

pub(crate) fn validate_public_http_url(url: &str) -> Result<()> {
    let parsed = Url::parse(url.trim()).context("web_read received an invalid URL")?;
    ensure_public_url(&parsed)
}

fn ensure_public_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("web_read only accepts http:// or https:// URLs");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("web_read refuses URLs containing user credentials");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("web_read URL has no host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        bail!("web_read refuses local or private network URLs");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip_is_non_public(ip) {
            bail!("web_read refuses local or private network URLs");
        }
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("web_read URL has no usable port"))?;
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("could not resolve web source host {host}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|addr| ip_is_non_public(addr.ip())) {
        bail!("web_read refuses hosts that resolve to local or private network addresses");
    }
    Ok(())
}

fn ip_is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 240
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| ip_is_non_public(IpAddr::V4(mapped)))
        }
    }
}

fn html_to_readable_text(html: &str) -> String {
    let mut output = String::with_capacity(html.len().min(64 * 1024));
    let mut in_tag = false;
    let mut tag = String::new();
    let mut hidden_depth = 0usize;
    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                let lowered = tag.trim().to_ascii_lowercase();
                let name = lowered
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/');
                if matches!(name, "script" | "style" | "noscript") {
                    if lowered.starts_with('/') {
                        hidden_depth = hidden_depth.saturating_sub(1);
                    } else if !lowered.ends_with('/') {
                        hidden_depth = hidden_depth.saturating_add(1);
                    }
                }
                if hidden_depth == 0
                    && matches!(
                        name,
                        "p" | "div"
                            | "br"
                            | "li"
                            | "section"
                            | "article"
                            | "header"
                            | "footer"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                            | "tr"
                    )
                {
                    output.push('\n');
                }
                tag.clear();
                in_tag = false;
            } else {
                tag.push(ch);
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if hidden_depth == 0 {
            output.push(ch);
        }
    }
    let decoded = output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let mut compact = String::new();
    let mut previous_blank = false;
    for line in decoded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !previous_blank && !compact.is_empty() {
                compact.push('\n');
            }
            previous_blank = true;
            continue;
        }
        if !compact.is_empty() && !compact.ends_with('\n') {
            compact.push('\n');
        }
        compact.push_str(&line);
        previous_blank = false;
    }
    compact.trim().to_owned()
}

fn wait_for_agent_reach_child(
    child: &mut Child,
    cancel: &AtomicBool,
    timeout: Duration,
    operation: &str,
) -> Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            kill_child_group(child);
            let _ = child.wait();
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            kill_child_group(child);
            let _ = child.wait();
            bail!("Agent-Reach Exa {operation} timed out after {timeout:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_child_group(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let pid = child.id() as i32;
        if pid > 0 && libc::kill(-pid, libc::SIGKILL) == 0 {
            return;
        }
    }
    let _ = child.kill();
}

impl Drop for WebSearchClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn run_cli(args: &[String]) -> Result<String> {
    match args.first().map(String::as_str).unwrap_or("status") {
        "backend" => {
            let workspace = env::current_dir()?.canonicalize()?;
            let settings = ProjectSettingsStore::new(&workspace)?;
            let project = settings.load()?;
            match args.get(1).map(String::as_str) {
                None => Ok(format!(
                    "{}{}",
                    project.web.backend.as_str(),
                    if project.web.backend == ServiceBackend::Mcp {
                        format!(" {}", project.web.server)
                    } else {
                        String::new()
                    }
                )),
                Some(value) => {
                    if args.len() > 3 {
                        bail!("Usage: yeet search backend [builtin|mcp [SERVER]]");
                    }
                    let backend = ServiceBackend::parse(value)?;
                    settings.save_web_backend(backend, args.get(2).map(String::as_str))?;
                    let saved = settings.load()?;
                    Ok(format!(
                        "web backend={}{}",
                        saved.web.backend.as_str(),
                        if saved.web.backend == ServiceBackend::Mcp {
                            format!(" server={}", saved.web.server)
                        } else {
                            String::new()
                        }
                    ))
                }
            }
        }
        "install" => {
            let workspace = env::current_dir()?.canonicalize()?;
            let settings = ProjectSettingsStore::new(&workspace)?;
            if settings.load()?.web.backend == ServiceBackend::Mcp {
                bail!("yeet search install is only available with backend=builtin");
            }
            install(args.get(1).map(String::as_str).unwrap_or("all"))
        }
        "status" => {
            let workspace = env::current_dir()?.canonicalize()?;
            let settings = ProjectSettingsStore::new(&workspace)?;
            let project = settings.load()?;
            let value = web_search_status_for_workspace(&project.web, Some(&workspace));
            Ok(serde_json::to_string_pretty(&value)?)
        }
        "test" => {
            let workspace = env::current_dir()?.canonicalize()?;
            let project = ProjectSettingsStore::new(&workspace)?.load()?;
            test_web_search(&project.web, Some(&workspace))
        }
        "path" => Ok(SearchPaths::discover().root.display().to_string()),
        "remove" | "uninstall" => {
            let workspace = env::current_dir()?.canonicalize()?;
            let settings = ProjectSettingsStore::new(&workspace)?;
            if settings.load()?.web.backend == ServiceBackend::Mcp {
                bail!("yeet search remove is only available with backend=builtin");
            }
            remove(args.get(1).map(String::as_str).unwrap_or("all"))
        }
        _ => bail!(
            "Usage: yeet search [backend [builtin|mcp [SERVER]]|install [all|searxng|agent-reach]|status|test|path|remove [all|searxng|agent-reach]]"
        ),
    }
}

mod setup;
pub use setup::{
    Connectivity, ProviderHealth, SearchTestResult, SetupPlan, WebSearchStatus, install_web_search,
    missing_web_tools, repair_plan, setup_plan, test_web_search, web_search_status,
    web_search_status_for_workspace,
};

mod install;
mod normalize;
use install::*;
use normalize::*;

#[cfg(test)]
mod health_tests {
    use super::*;

    #[test]
    fn mcp_requires_both_semantic_tools() {
        assert_eq!(missing_web_tools(["web_search"]), vec!["web_read"]);
        assert!(missing_web_tools(["web_read", "web_search"]).is_empty());
    }

    #[test]
    fn health_serialization_keeps_installation_and_connectivity_separate() {
        let health = ProviderHealth {
            installed: Some(true),
            connectivity: Connectivity::Unknown,
            error: None,
        };
        let value = serde_json::to_value(&health).unwrap();
        assert_eq!(value["installed"], true);
        assert_eq!(value["connectivity"], "unknown");
        assert_eq!(
            serde_json::from_value::<ProviderHealth>(value).unwrap(),
            health
        );
    }
}

#[cfg(test)]
mod provider_integration_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn native_search_decodes_mcp_event_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with("POST /mcp "));
            let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"Title: Example\\nURL: https://example.com/\\nHighlights: Example evidence\"}]}}\n\n";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let result = WebSearchClient::default()
            .search_native_at(&endpoint, "test", 1, &AtomicBool::new(false))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(result["source"], "exa");
        assert_eq!(result["results"][0]["url"], "https://example.com/");
    }

    #[test]
    fn configured_searxng_endpoint_is_used_and_results_normalized() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            for body in [
                r#"{"engines":[]}"#,
                r#"{"results":[{"title":"Example","url":"https://example.com/","content":"<b>hello</b>"}]}"#,
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let client = WebSearchClient::default();
        client.set_searxng_url(Some(endpoint));
        let result = client
            .search(WebSearchRequest {
                query: "test",
                max_results: 1,
                language: None,
                category: None,
                time_range: None,
                safe_search: None,
                page: None,
                backend: WebSearchBackend::Searxng,
            })
            .unwrap();
        worker.join().unwrap();
        assert_eq!(result["source"], "searxng");
        assert_eq!(result["results"][0]["snippet"], "hello");
    }
    #[cfg(unix)]
    #[test]
    fn automatic_fallback_reports_failed_provider() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut client = WebSearchClient::default();
        client.paths.agent_reach_venv = root.path().join("venv");
        client.paths.agent_reach_node = root.path().join("node");
        client.paths.agent_reach_mcporter_config = root.path().join("config.json");
        for path in [
            client.paths.agent_reach_python(),
            client.paths.agent_reach_cli(),
            client.paths.managed_mcporter(),
            client.paths.agent_reach_mcporter_config.clone(),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "#!/bin/sh\necho provider-failed >&2\nexit 1\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        client.set_searxng_url(Some(format!("http://{}", listener.local_addr().unwrap())));
        let worker = thread::spawn(move || {
            for body in [r#"{"engines":[]}"#, r#"{"results":[]}"#] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let result = client
            .search(WebSearchRequest {
                query: "test",
                max_results: 1,
                language: None,
                category: None,
                time_range: None,
                safe_search: None,
                page: None,
                backend: WebSearchBackend::Auto,
            })
            .unwrap();
        worker.join().unwrap();
        assert_eq!(result["fallbackFrom"], "agent-reach");
        assert!(
            result["fallbackReason"]
                .as_str()
                .unwrap()
                .contains("provider-failed")
        );
    }

    #[test]
    fn missing_installer_command_reports_the_failed_step() {
        let error = run(
            &mut Command::new("/missing/yeet-test-command"),
            "install dependency",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("install dependency"));
    }
}
