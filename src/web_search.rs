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

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::config::ConfigStore;

pub const CAPABILITY_ID: &str = "web-search";
const DEFAULT_URL: &str = "http://127.0.0.1:8888";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
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
}

#[derive(Debug, Default)]
struct ProcessState {
    child: Option<Child>,
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
            agent,
        }
    }
}

impl WebSearchClient {
    pub fn read_url(&self, url: &str, max_chars: usize, cancel: &AtomicBool) -> Result<Value> {
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        validate_public_http_url(url)?;
        let max_chars = max_chars.clamp(2_000, 48_000);
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
            WebSearchBackend::Auto => match self.search_agent_reach(query, max_results, cancel) {
                Ok(result) => Ok(result),
                Err(agent_reach_error) => {
                    if cancel.load(Ordering::Acquire) {
                        bail!("cancelled");
                    }
                    let mut fallback = self.search_searxng(request, cancel)?;
                    if let Some(object) = fallback.as_object_mut() {
                        object.insert("fallbackFrom".into(), json!("agent-reach"));
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
        let stdout = self.invoke_agent_reach("exa.web_search_exa", arguments, cancel, "search")?;
        normalize_agent_reach_output(query, &stdout, max_results.clamp(1, 20))
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
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
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
        let status = loop {
            if cancel.load(Ordering::Acquire) {
                kill_child_group(&mut child);
                let _ = child.wait();
                bail!("cancelled");
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            thread::sleep(Duration::from_millis(20));
        };
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
        if let Some(url) = env::var_os("YEET_SEARXNG_URL").filter(|value| !value.is_empty()) {
            let url = url.to_string_lossy().trim_end_matches('/').to_owned();
            if self.probe(&url) {
                return Ok(url);
            }
            bail!("YEET_SEARXNG_URL is set but no SearXNG instance answered at {url}");
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
                "Web search is attached but local SearXNG is not installed. Run `yeet search install` once, or set YEET_SEARXNG_URL to an existing instance."
            );
        }

        fs::create_dir_all(&self.paths.root)?;
        rotate_search_log(&self.paths.log)?;
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log)?;
        let stderr = stdout.try_clone()?;
        let child = Command::new(self.paths.python())
            .arg("-m")
            .arg("searx.webapp")
            .current_dir(&self.paths.source)
            .env("SEARXNG_SETTINGS_PATH", &self.paths.settings)
            .env("SEARXNG_BIND_ADDRESS", "127.0.0.1")
            .env("SEARXNG_PORT", "8888")
            .env("SEARXNG_BASE_URL", format!("{DEFAULT_URL}/"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
                return Err(error).with_context(|| {
                    format!("start SearXNG with {}", self.paths.python().display())
                });
            }
        };

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

fn validate_public_http_url(url: &str) -> Result<()> {
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
        "install" => install(args.get(1).map(String::as_str).unwrap_or("all")),
        "status" => status(),
        "path" => Ok(SearchPaths::discover().root.display().to_string()),
        "remove" | "uninstall" => remove(args.get(1).map(String::as_str).unwrap_or("all")),
        _ => bail!(
            "Usage: yeet search [install [all|searxng|agent-reach]|status|path|remove [all|searxng|agent-reach]]"
        ),
    }
}

fn install(target: &str) -> Result<String> {
    match target {
        "all" => {
            let searxng = install_searxng()?;
            let agent_reach = install_agent_reach()?;
            Ok(format!("{searxng}\n{agent_reach}"))
        }
        "searxng" | "searx" => install_searxng(),
        "agent-reach" | "agent_reach" | "exa" => install_agent_reach(),
        value => bail!("unknown search install target '{value}'"),
    }
}

fn install_searxng() -> Result<String> {
    let paths = SearchPaths::discover();
    fs::create_dir_all(&paths.root).with_context(|| format!("create {}", paths.root.display()))?;

    if paths.source.join(".git").is_dir() {
        run(
            Command::new("git")
                .arg("-C")
                .arg(&paths.source)
                .arg("pull")
                .arg("--ff-only"),
            "update SearXNG source",
        )?;
    } else {
        if paths.source.exists() {
            fs::remove_dir_all(&paths.source)?;
        }
        run(
            Command::new("git")
                .arg("clone")
                .arg("--depth")
                .arg("1")
                .arg("https://github.com/searxng/searxng.git")
                .arg(&paths.source),
            "clone SearXNG",
        )?;
    }

    if !paths.python().is_file() {
        let system_python = env::var_os("YEET_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        run(
            Command::new(system_python)
                .arg("-m")
                .arg("venv")
                .arg(&paths.venv),
            "create SearXNG virtual environment",
        )?;
    }

    let python = paths.python();
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .args([
                "pip",
                "setuptools",
                "wheel",
                "pyyaml",
                "msgspec",
                "typing-extensions",
                "pybind11",
            ]),
        "prepare SearXNG Python environment",
    )?;
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--use-pep517")
            .arg("--no-build-isolation")
            .arg("-e")
            .arg(&paths.source),
        "install SearXNG",
    )?;

    let secret = Uuid::new_v4().simple().to_string();
    let settings = format!(
        "use_default_settings: true\n\ngeneral:\n  debug: false\n  instance_name: \"Yeet Search\"\n\nsearch:\n  safe_search: 0\n  formats:\n    - html\n    - json\n\nserver:\n  secret_key: \"{secret}\"\n  bind_address: \"127.0.0.1\"\n  port: 8888\n  limiter: false\n  public_instance: false\n  image_proxy: false\n"
    );
    fs::write(&paths.settings, settings)?;
    Ok(format!(
        "SearXNG installed at {}. Web search is attached by default in new projects and starts lazily on the first search.",
        paths.root.display()
    ))
}

fn install_agent_reach() -> Result<String> {
    let paths = SearchPaths::discover();
    fs::create_dir_all(&paths.root).with_context(|| format!("create {}", paths.root.display()))?;

    if !paths.agent_reach_python().is_file() {
        let system_python = env::var_os("YEET_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        run(
            Command::new(system_python)
                .arg("-m")
                .arg("venv")
                .arg(&paths.agent_reach_venv),
            "create Agent-Reach virtual environment",
        )?;
    }
    let python = paths.agent_reach_python();
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip"),
        "prepare Agent-Reach Python environment",
    )?;
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg(AGENT_REACH_URL),
        "install Agent-Reach",
    )?;

    fs::create_dir_all(&paths.agent_reach_node)?;
    let npm = env::var_os("YEET_NPM")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("npm"));
    run(
        Command::new(npm)
            .arg("install")
            .arg("--prefix")
            .arg(&paths.agent_reach_node)
            .arg("mcporter"),
        "install Agent-Reach mcporter backend",
    )?;

    let config = json!({
        "mcpServers": {
            "exa": {
                "baseUrl": EXA_MCP_URL
            }
        }
    });
    fs::write(
        &paths.agent_reach_mcporter_config,
        serde_json::to_vec_pretty(&config)?,
    )?;

    Ok(format!(
        "Agent-Reach + Exa installed at {}. web_search now prefers Agent-Reach automatically and falls back to SearXNG.",
        paths.agent_reach_venv.display()
    ))
}

fn status() -> Result<String> {
    let paths = SearchPaths::discover();
    let client = WebSearchClient::default();
    let external = env::var("YEET_SEARXNG_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let url = external.as_deref().unwrap_or(DEFAULT_URL);
    let running = client.probe(url);
    let managed_agent_reach = paths.agent_reach_installed();
    let external_agent_reach = command_available("agent-reach") && command_available("mcporter");
    Ok(format!(
        "path: {}\nsearxng.installed: {}\nsearxng.running: {}\nsearxng.url: {}\nagent-reach.managed: {}\nagent-reach.external: {}\nagent-reach.preferred: {}",
        paths.root.display(),
        if paths.installed() { "yes" } else { "no" },
        if running { "yes" } else { "no" },
        url,
        if managed_agent_reach { "yes" } else { "no" },
        if external_agent_reach { "yes" } else { "no" },
        if managed_agent_reach || external_agent_reach {
            "yes"
        } else {
            "no"
        },
    ))
}

fn remove(target: &str) -> Result<String> {
    let paths = SearchPaths::discover();
    let client = WebSearchClient::default();
    match target {
        "all" => {
            ensure_searxng_stopped(&client)?;
            if paths.root.exists() {
                fs::remove_dir_all(&paths.root)?;
            }
            Ok(format!(
                "Removed managed web search files from {}.",
                paths.root.display()
            ))
        }
        "searxng" | "searx" => {
            ensure_searxng_stopped(&client)?;
            for path in [&paths.source, &paths.venv] {
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            for path in [&paths.settings, &paths.log] {
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
            Ok("Removed managed SearXNG backend.".into())
        }
        "agent-reach" | "agent_reach" | "exa" => {
            for path in [&paths.agent_reach_venv, &paths.agent_reach_node] {
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            if paths.agent_reach_mcporter_config.exists() {
                fs::remove_file(&paths.agent_reach_mcporter_config)?;
            }
            Ok("Removed managed Agent-Reach backend.".into())
        }
        value => bail!("unknown search remove target '{value}'"),
    }
}

fn ensure_searxng_stopped(client: &WebSearchClient) -> Result<()> {
    if env::var_os("YEET_SEARXNG_URL").is_none() && client.probe(DEFAULT_URL) {
        bail!(
            "A SearXNG instance is still running at {DEFAULT_URL}. Close the Yeet process that owns it before removing the managed installation."
        );
    }
    Ok(())
}

fn run(command: &mut Command, purpose: &str) -> Result<()> {
    let status = command.status().with_context(|| purpose.to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{purpose} failed with {status}"))
    }
}

fn command_available(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(search_path) = env::var_os("PATH") else {
        return false;
    };
    for directory in env::split_paths(&search_path) {
        if directory.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        for extension in ["exe", "cmd", "bat"] {
            if directory.join(format!("{name}.{extension}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn normalize_agent_reach_output(query: &str, output: &str, max_results: usize) -> Result<Value> {
    let output = output.trim();
    if output.is_empty() {
        bail!("Agent-Reach Exa search returned no output");
    }

    let mut results = Vec::new();
    let mut text_blocks = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        collect_agent_reach_results(&value, &mut results, max_results);
        collect_agent_reach_text(&value, &mut text_blocks);
    } else {
        text_blocks.push(output.to_owned());
    }

    for text in &text_blocks {
        if results.len() >= max_results {
            break;
        }
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            collect_agent_reach_results(&value, &mut results, max_results);
        }
        collect_agent_reach_text_results(text, &mut results, max_results);
    }

    results.retain(|entry| {
        entry
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
    });
    let mut seen = std::collections::HashSet::new();
    results.retain(|entry| {
        entry
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| seen.insert(url.to_owned()))
    });
    results.truncate(max_results);

    let answers = if results.is_empty() {
        let context = text_blocks
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n\n");
        if context.trim().is_empty() {
            Vec::new()
        } else {
            vec![json!(truncate_chars(&context, 12_000))]
        }
    } else {
        Vec::new()
    };

    if results.is_empty() && answers.is_empty() {
        bail!("Agent-Reach Exa search returned no usable search evidence");
    }
    Ok(json!({
        "query": query,
        "source": "agent-reach/exa",
        "numberOfResults": results.len(),
        "results": results,
        "answers": answers,
        "suggestions": [],
        "corrections": [],
    }))
}

fn collect_agent_reach_results(value: &Value, results: &mut Vec<Value>, max_results: usize) {
    if results.len() >= max_results {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_agent_reach_results(value, results, max_results);
                if results.len() >= max_results {
                    break;
                }
            }
        }
        Value::Object(object) => {
            if let Some(url) = object.get("url").and_then(Value::as_str)
                && (url.starts_with("http://") || url.starts_with("https://"))
            {
                let snippet = object
                    .get("snippet")
                    .or_else(|| object.get("summary"))
                    .or_else(|| object.get("text"))
                    .and_then(Value::as_str)
                    .map(|value| truncate_chars(&strip_markup(value), 420))
                    .or_else(|| {
                        object
                            .get("highlights")
                            .and_then(Value::as_array)
                            .and_then(|values| values.iter().find_map(Value::as_str))
                            .map(|value| truncate_chars(&strip_markup(value), 420))
                    })
                    .unwrap_or_default();
                results.push(json!({
                    "title": object.get("title").and_then(Value::as_str).unwrap_or(url),
                    "url": url,
                    "snippet": snippet,
                    "publishedAt": object.get("publishedDate").or_else(|| object.get("publishedAt")).cloned().unwrap_or(Value::Null),
                    "category": Value::Null,
                    "engines": ["agent-reach", "exa"],
                    "score": object.get("score").cloned().unwrap_or(Value::Null),
                }));
                if results.len() >= max_results {
                    return;
                }
            }
            for value in object.values() {
                collect_agent_reach_results(value, results, max_results);
                if results.len() >= max_results {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn collect_agent_reach_text(value: &Value, text_blocks: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_agent_reach_text(value, text_blocks);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = object.get("text").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                text_blocks.push(text.to_owned());
            }
            for value in object.values() {
                collect_agent_reach_text(value, text_blocks);
            }
        }
        _ => {}
    }
}

fn collect_agent_reach_text_results(text: &str, results: &mut Vec<Value>, max_results: usize) {
    for block in text.split("\n---\n") {
        if results.len() >= max_results {
            return;
        }
        let mut title: Option<String> = None;
        let mut url: Option<String> = None;
        let mut published_at: Option<String> = None;
        let mut snippet_lines = Vec::new();
        let mut in_content = false;

        for raw_line in block.lines() {
            let line = raw_line.trim().trim_start_matches(['-', '*', '#', ' ']);
            if title.is_none()
                && let Some(value) = strip_field(line, "Title:")
            {
                title = Some(value.to_owned());
                continue;
            }
            if url.is_none()
                && let Some(value) = strip_field(line, "URL:").or_else(|| strip_field(line, "Url:"))
            {
                url = Some(value.trim_matches(['<', '>']).to_owned());
                continue;
            }
            if published_at.is_none()
                && let Some(value) =
                    strip_field(line, "Published Date:").or_else(|| strip_field(line, "Published:"))
            {
                if value != "N/A" {
                    published_at = Some(value.to_owned());
                }
                continue;
            }
            if line == "Highlights:" || line == "Text:" || line == "Summary:" {
                in_content = true;
                continue;
            }
            if let Some(value) =
                strip_field(line, "Text:").or_else(|| strip_field(line, "Summary:"))
            {
                in_content = true;
                snippet_lines.push(value.to_owned());
                continue;
            }
            if in_content && !line.is_empty() && line != "..." {
                snippet_lines.push(line.to_owned());
            }
        }

        let Some(url) = url else {
            continue;
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            continue;
        }
        let title = title.unwrap_or_else(|| url.clone());
        results.push(json!({
            "title": title,
            "url": url,
            "snippet": truncate_chars(&strip_markup(&snippet_lines.join(" ")), 420),
            "publishedAt": published_at.map(Value::String).unwrap_or(Value::Null),
            "category": Value::Null,
            "engines": ["agent-reach", "exa"],
            "score": Value::Null,
        }));
    }
}

fn strip_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(field)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalize_results(query: &str, value: Value, max_results: usize) -> Value {
    let results = value.get("results").and_then(Value::as_array).map(|values| {
        values.iter().take(max_results).map(|entry| {
            let engines = entry.get("engines").cloned().or_else(|| entry.get("engine").and_then(Value::as_str).map(|engine| json!([engine]))).unwrap_or_else(|| json!([]));
            json!({
                "title": entry.get("title").and_then(Value::as_str).unwrap_or_default(),
                "url": entry.get("url").and_then(Value::as_str).unwrap_or_default(),
                "snippet": truncate_chars(&strip_markup(entry.get("content").and_then(Value::as_str).unwrap_or_default()), 420),
                "publishedAt": entry.get("publishedDate").or_else(|| entry.get("published_at")).cloned().unwrap_or(Value::Null),
                "category": entry.get("category").cloned().unwrap_or(Value::Null),
                "engines": engines,
                "score": entry.get("score").cloned().unwrap_or(Value::Null),
            })
        }).collect::<Vec<_>>()
    }).unwrap_or_default();
    json!({
        "query": query,
        "source": "searxng",
        "numberOfResults": value.get("number_of_results").cloned().unwrap_or_else(|| json!(results.len())),
        "results": results,
        "answers": value.get("answers").cloned().unwrap_or_else(|| json!([])),
        "suggestions": value.get("suggestions").cloned().unwrap_or_else(|| json!([])),
        "corrections": value.get("corrections").cloned().unwrap_or_else(|| json!([])),
    })
}

fn truncate_chars(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_owned();
    }
    input
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn strip_markup(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_read_rejects_private_and_credentialed_urls() {
        assert!(validate_public_http_url("http://127.0.0.1/secret").is_err());
        assert!(validate_public_http_url("http://10.1.2.3/").is_err());
        assert!(validate_public_http_url("http://[::1]/").is_err());
        assert!(validate_public_http_url("https://user:pass@example.com/").is_err());
    }

    #[test]
    fn html_source_reader_removes_markup_and_hidden_blocks() {
        let text = html_to_readable_text(
            "<html><style>.x{display:none}</style><body><h1>Title</h1><script>alert(1)</script><p>Hello &amp; bye</p></body></html>",
        );
        assert!(text.contains("Title"));
        assert!(text.contains("Hello & bye"));
        assert!(!text.contains("display:none"));
        assert!(!text.contains("alert(1)"));
    }

    #[test]
    fn normalizes_searxng_results() {
        let value = json!({
            "number_of_results": 42,
            "results": [{"title":"Example","url":"https://example.com","content":"hello <b>world</b> &amp; friends","engine":"duckduckgo","score":1.5}],
            "suggestions": ["example search"]
        });
        let result = normalize_results("example", value, 5);
        assert_eq!(result["results"][0]["snippet"], "hello world & friends");
        assert_eq!(result["results"][0]["engines"][0], "duckduckgo");
        assert_eq!(result["numberOfResults"], 42);
    }

    #[test]
    fn search_snippets_are_bounded() {
        let value = json!({
            "results": [{"title":"Long","url":"https://example.com","content":"x".repeat(2_000)}]
        });
        let result = normalize_results("long", value, 5);
        assert!(
            result["results"][0]["snippet"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 420
        );
    }

    #[test]
    fn parses_web_search_backend_names() {
        assert_eq!(
            WebSearchBackend::parse(None).unwrap(),
            WebSearchBackend::Auto
        );
        assert_eq!(
            WebSearchBackend::parse(Some("agent-reach")).unwrap(),
            WebSearchBackend::AgentReach
        );
        assert_eq!(
            WebSearchBackend::parse(Some("exa")).unwrap(),
            WebSearchBackend::AgentReach
        );
        assert_eq!(
            WebSearchBackend::parse(Some("searxng")).unwrap(),
            WebSearchBackend::Searxng
        );
        assert!(WebSearchBackend::parse(Some("mystery")).is_err());
    }

    #[test]
    fn normalizes_agent_reach_structured_results() {
        let raw = json!({
            "content": [{
                "type": "text",
                "text": "search context"
            }],
            "structuredContent": {
                "results": [{
                    "title": "Agent Reach",
                    "url": "https://example.com/agent-reach",
                    "highlights": ["semantic web search"],
                    "publishedDate": "2026-09-01T00:00:00Z",
                    "score": 0.9
                }]
            }
        })
        .to_string();
        let result = normalize_agent_reach_output("agent reach", &raw, 5).unwrap();
        assert_eq!(result["source"], "agent-reach/exa");
        assert_eq!(result["results"][0]["title"], "Agent Reach");
        assert_eq!(result["results"][0]["snippet"], "semantic web search");
        assert_eq!(result["results"][0]["engines"][1], "exa");
    }

    #[test]
    fn normalizes_agent_reach_text_results() {
        let raw = json!({
            "content": [{
                "type": "text",
                "text": "Title: First result\nURL: https://example.com/one\nPublished: 2026-09-02\nAuthor: N/A\nHighlights:\nuseful snippet\n\n---\n\nTitle: Second result\nURL: https://example.com/two\nPublished: N/A\nAuthor: N/A\nHighlights:\nanother snippet"
            }]
        })
        .to_string();
        let result = normalize_agent_reach_output("example", &raw, 5).unwrap();
        assert_eq!(result["results"].as_array().unwrap().len(), 2);
        assert_eq!(result["results"][0]["url"], "https://example.com/one");
        assert_eq!(result["results"][1]["snippet"], "another snippet");
    }

    #[test]
    fn preserves_agent_reach_context_when_no_urls_can_be_parsed() {
        let raw = json!({
            "content": [{"type":"text","text":"A useful answer without structured URLs"}]
        })
        .to_string();
        let result = normalize_agent_reach_output("answer", &raw, 5).unwrap();
        assert!(result["results"].as_array().unwrap().is_empty());
        assert_eq!(
            result["answers"][0],
            "A useful answer without structured URLs"
        );
    }
}
