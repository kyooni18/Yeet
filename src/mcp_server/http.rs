use std::{
    collections::HashMap,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use socket2::{Domain, Protocol, Socket, Type};
use url::{Url, form_urlencoded};

use super::{
    MODERN_PROTOCOL_VERSION,
    auth::{AuthMode, AuthStore, AuthorizationRequest, OAuthRuntime, bearer_token},
};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_WORKER_STACK_BYTES: usize = 4 * 1024 * 1024;
const MAX_HTTP_WORKERS: usize = 32;

#[derive(Debug, Clone)]
pub(super) struct HttpOptions {
    pub bind_host: String,
    pub port: u16,
    pub default_workspace: PathBuf,
    pub public_url: Option<Url>,
}

pub(super) struct HttpServer {
    listener: TcpListener,
    address: SocketAddr,
    public_url: Url,
    issuer: Url,
    mcp: Arc<Mutex<McpProcess>>,
    oauth: OAuthRuntime,
}

impl HttpServer {
    pub(super) fn bind(options: HttpOptions, auth_store: AuthStore) -> Result<Self> {
        let listener = bind_reusable(&options.bind_host, options.port)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let public_url = match options.public_url {
            Some(url) => normalize_public_url(url)?,
            None => inferred_public_url(&options.bind_host, address)?,
        };
        let issuer = issuer_for_resource(&public_url)?;
        let oauth = OAuthRuntime::new(auth_store, issuer.clone(), public_url.clone())?;
        Ok(Self {
            listener,
            address,
            public_url,
            issuer,
            mcp: Arc::new(Mutex::new(McpProcess::new(options.default_workspace)?)),
            oauth,
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn public_url(&self) -> &Url {
        &self.public_url
    }

    pub(super) fn serve(self, stop: Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
        let active_workers = Arc::new(AtomicUsize::new(0));
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let active = active_workers.fetch_add(1, Ordering::AcqRel);
                    if active >= MAX_HTTP_WORKERS {
                        active_workers.fetch_sub(1, Ordering::AcqRel);
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        let _ = write_response(
                            &mut stream,
                            HttpResponse::json(
                                503,
                                json!({
                                    "error":"server_busy",
                                    "error_description":"Yeet MCP has reached its concurrent HTTP connection limit; retry shortly"
                                }),
                            ),
                        );
                        continue;
                    }
                    let mcp = Arc::clone(&self.mcp);
                    let oauth = self.oauth.clone();
                    let public_url = self.public_url.clone();
                    let issuer = self.issuer.clone();
                    let active_workers_for_thread = Arc::clone(&active_workers);
                    let spawn = thread::Builder::new()
                        .name("yeet-mcp-http".into())
                        .stack_size(HTTP_WORKER_STACK_BYTES)
                        .spawn(move || {
                            let _slot = HttpWorkerSlot(active_workers_for_thread);
                            handle_connection(stream, mcp, oauth, public_url, issuer);
                        });
                    if let Err(error) = spawn {
                        active_workers.fetch_sub(1, Ordering::AcqRel);
                        return Err(error.into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

struct HttpWorkerSlot(Arc<AtomicUsize>);

impl Drop for HttpWorkerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn bind_reusable(host: &str, port: u16) -> Result<TcpListener> {
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolve Yeet MCP bind address {host}:{port}"))?;
    let mut last_error = None;
    for address in addresses {
        let socket = Socket::new(
            Domain::for_address(address),
            Type::STREAM,
            Some(Protocol::TCP),
        )?;
        #[cfg(not(windows))]
        socket.set_reuse_address(true)?;
        if let Err(error) = socket.bind(&address.into()) {
            last_error = Some(error);
            continue;
        }
        socket.listen(128)?;
        return Ok(socket.into());
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow!("no usable bind address for {host}:{port}")))
}

fn handle_connection(
    mut stream: TcpStream,
    mcp: Arc<Mutex<McpProcess>>,
    oauth: OAuthRuntime,
    public_url: Url,
    issuer: Url,
) {
    // The listening socket is nonblocking so the daemon can poll its stop flag.
    // On macOS an accepted socket can retain that mode, which makes a normal
    // request read fail immediately with EAGAIN (os error 35). Restore blocking
    // mode before applying the actual per-connection read/write timeouts.
    if let Err(error) = stream.set_nonblocking(false) {
        eprintln!("yeet mcpserver: set accepted socket blocking mode: {error}");
        return;
    }
    if let Err(error) = stream.set_read_timeout(Some(CONNECTION_TIMEOUT)) {
        eprintln!("yeet mcpserver: set read timeout: {error}");
        return;
    }
    if let Err(error) = stream.set_write_timeout(Some(CONNECTION_TIMEOUT)) {
        eprintln!("yeet mcpserver: set write timeout: {error}");
        return;
    }
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let response = HttpResponse::json(
                request_error_status(&error),
                json!({"error":"invalid_request","error_description":error.to_string()}),
            );
            if let Err(write_error) = write_response(&mut stream, response) {
                eprintln!("yeet mcpserver: {error}; failed to write error response: {write_error}");
            } else {
                eprintln!("yeet mcpserver: {error}");
            }
            return;
        }
    };
    let target = request.target.clone();
    let response = route_request(request, mcp, oauth, public_url, issuer).unwrap_or_else(|error| {
        let status = if target.starts_with("/authorize")
            || target.starts_with("/token")
            || target.starts_with("/register")
            || target.starts_with("/mcp")
        {
            400
        } else {
            500
        };
        HttpResponse::json(
            status,
            json!({"error":"invalid_request","error_description":error.to_string()}),
        )
    });
    if let Err(error) = write_response(&mut stream, response) {
        eprintln!("yeet mcpserver: write response: {error}");
    }
}

fn request_error_status(error: &anyhow::Error) -> u16 {
    if error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        })
    }) {
        408
    } else {
        400
    }
}

fn route_request(
    request: HttpRequest,
    mcp: Arc<Mutex<McpProcess>>,
    oauth: OAuthRuntime,
    public_url: Url,
    issuer: Url,
) -> Result<HttpResponse> {
    let (path, query) = split_target(&request.target);
    match (request.method.as_str(), path) {
        ("GET", "/health") => Ok(HttpResponse::json(
            200,
            json!({"ok":true,"service":"yeet-mcpserver","mcp":public_url}),
        )),
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            Ok(HttpResponse::json(200, oauth.protected_resource_metadata()))
        }
        ("GET", "/.well-known/oauth-authorization-server") => Ok(HttpResponse::json(
            200,
            oauth.authorization_server_metadata(),
        )),
        ("POST", "/register") => {
            ensure_oauth_mode(&oauth)?;
            let body: Value = serde_json::from_slice(&request.body)
                .context("decode OAuth client registration")?;
            Ok(HttpResponse::json(201, oauth.register_client(&body)?))
        }
        ("GET", "/authorize") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(query.as_bytes());
            let authorization = oauth.parse_authorization_request(&params)?;
            Ok(HttpResponse::html(200, authorization_page(&authorization)))
        }
        ("POST", "/authorize") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(&request.body);
            let key = params
                .get("access_key")
                .ok_or_else(|| anyhow!("missing MCP authorization key"))?;
            let authorization = oauth.parse_authorization_request(&params)?;
            let redirect = oauth.authorize_with_key(authorization, key)?;
            Ok(HttpResponse::redirect(redirect.as_str()))
        }
        ("POST", "/token") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(&request.body);
            match oauth.exchange_token(&params) {
                Ok(token) => Ok(HttpResponse::json(200, token)),
                Err(error) => Ok(HttpResponse::json(
                    400,
                    json!({"error":"invalid_grant","error_description":error.to_string()}),
                )),
            }
        }
        ("POST", "/mcp") => {
            let auth_mode = oauth.auth_store().status()?.mode;
            let header = request.headers.get("authorization").map(String::as_str);
            let authorized = match auth_mode {
                AuthMode::None => true,
                AuthMode::Key => bearer_token(header)
                    .map(|token| oauth.auth_store().verify_key(token))
                    .transpose()?
                    .unwrap_or(false),
                AuthMode::Oauth => bearer_token(header)
                    .map(|token| oauth.verify_oauth_token(token))
                    .transpose()?
                    .unwrap_or(false),
            };
            if !authorized {
                return Ok(unauthorized_response(auth_mode, &issuer));
            }
            let mut payload: Value =
                serde_json::from_slice(&request.body).context("decode MCP JSON-RPC request")?;
            if let Some(version) = request.headers.get("mcp-protocol-version") {
                attach_protocol_version(&mut payload, version);
            }
            let response = match mcp
                .lock()
                .map_err(|_| anyhow!("MCP server state lock poisoned"))?
                .handle(payload)
            {
                Ok(response) => response,
                Err(error) => {
                    return Ok(HttpResponse::json(
                        503,
                        json!({
                            "error":"runtime_unavailable",
                            "error_description":error.to_string(),
                        }),
                    ));
                }
            };
            match response {
                Some(value) => {
                    let mut response = HttpResponse::json(200, value);
                    response.headers.push((
                        "MCP-Protocol-Version".into(),
                        request
                            .headers
                            .get("mcp-protocol-version")
                            .cloned()
                            .unwrap_or_else(|| MODERN_PROTOCOL_VERSION.into()),
                    ));
                    Ok(response)
                }
                None => Ok(HttpResponse::empty(202)),
            }
        }
        ("GET", "/mcp") => Ok(HttpResponse::text(
            405,
            "MCP uses HTTP POST. The legacy HTTP+SSE GET transport is not served.",
        )),
        _ => Ok(HttpResponse::text(404, "Not found")),
    }
}

struct McpProcess {
    default_workspace: PathBuf,
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl McpProcess {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        let mut process = Self::spawn(default_workspace)?;
        // Prove the child is ready before advertising the HTTP server. This also
        // makes daemon startup fail fast if the stdio runtime cannot initialize.
        let response = process.call(
            json!({"jsonrpc":"2.0","id":"http-runtime-health","method":"initialize","params":{}}),
        )?;
        if response.get("error").is_some() {
            bail!("Yeet MCP stdio runtime failed initialization: {response}");
        }
        Ok(process)
    }

    fn spawn(default_workspace: PathBuf) -> Result<Self> {
        let executable = std::env::current_exe().context("locate Yeet executable")?;
        let mut child = Command::new(executable)
            .arg("mcpserver")
            .arg("stdio")
            .arg("--workspace")
            .arg(&default_workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start isolated Yeet MCP stdio runtime")?;
        let stdin = child
            .stdin
            .take()
            .context("MCP runtime stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("MCP runtime stdout unavailable")?;
        Ok(Self {
            default_workspace,
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    fn handle(&mut self, payload: Value) -> Result<Option<Value>> {
        let expects_response = !payload
            .as_object()
            .is_some_and(|object| !object.contains_key("id"));
        if !expects_response {
            return match self.send(payload) {
                Ok(()) => Ok(None),
                Err(error) => {
                    let restart = self.restart();
                    match restart {
                        Ok(()) => Err(anyhow!(
                            "isolated MCP runtime failed while sending a notification and was restarted: {error}"
                        )),
                        Err(restart_error) => Err(anyhow!(
                            "isolated MCP runtime failed while sending a notification: {error}; restart also failed: {restart_error}"
                        )),
                    }
                }
            };
        }
        match self.call(payload) {
            Ok(response) => Ok(Some(response)),
            Err(error) => {
                // A Rust stack overflow aborts the runtime process and cannot be
                // caught in-process. Recreate it for the next request, but never
                // replay the failed request because tool calls may be mutating.
                let restart = self.restart();
                match restart {
                    Ok(()) => Err(anyhow!(
                        "isolated MCP tool runtime failed and was restarted; the failed request was not replayed: {error}"
                    )),
                    Err(restart_error) => Err(anyhow!(
                        "isolated MCP tool runtime failed: {error}; restart also failed: {restart_error}"
                    )),
                }
            }
        }
    }

    fn call(&mut self, payload: Value) -> Result<Value> {
        self.send(payload)?;
        let mut line = String::new();
        loop {
            line.clear();
            let count = self.stdout.read_line(&mut line)?;
            if count == 0 {
                let status = self
                    .child
                    .try_wait()?
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "unknown".into());
                bail!("MCP stdio runtime closed stdout (status {status})");
            }
            if line.trim().is_empty() {
                continue;
            }
            return serde_json::from_str(&line).context("decode MCP stdio runtime response");
        }
    }

    fn send(&mut self, payload: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &payload)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn restart(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let replacement = Self::new(self.default_workspace.clone())?;
        *self = replacement;
        Ok(())
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ensure_oauth_mode(oauth: &OAuthRuntime) -> Result<()> {
    if oauth.auth_store().status()?.mode != AuthMode::Oauth {
        bail!("OAuth authentication is not enabled for this MCP daemon");
    }
    Ok(())
}

fn unauthorized_response(mode: AuthMode, issuer: &Url) -> HttpResponse {
    let mut response = HttpResponse::json(401, json!({"error":"unauthorized"}));
    let challenge = if mode == AuthMode::Oauth {
        let metadata = issuer
            .join(".well-known/oauth-protected-resource")
            .expect("metadata URL");
        format!("Bearer resource_metadata=\"{}\", scope=\"mcp\"", metadata)
    } else {
        "Bearer".into()
    };
    response
        .headers
        .push(("WWW-Authenticate".into(), challenge));
    response
}

fn attach_protocol_version(payload: &mut Value, version: &str) {
    if let Some(items) = payload.as_array_mut() {
        for item in items {
            attach_protocol_version(item, version);
        }
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let params = object.entry("params").or_insert_with(|| json!({}));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let meta = params.entry("_meta").or_insert_with(|| json!({}));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    meta.entry("io.modelcontextprotocol/protocolVersion")
        .or_insert_with(|| json!(version));
}

fn authorization_page(request: &AuthorizationRequest) -> String {
    let state = request.state.as_deref().unwrap_or("");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Authorize Yeet MCP</title>\
<style>body{{font:16px system-ui;max-width:620px;margin:10vh auto;padding:24px}}input,button{{font:inherit;padding:10px;width:100%;box-sizing:border-box}}code{{overflow-wrap:anywhere}}</style>\
<h1>Authorize Yeet MCP</h1><p>Client: <code>{}</code></p><p>Resource: <code>{}</code></p>\
<form method=\"post\" action=\"/authorize\">\
<input type=\"hidden\" name=\"response_type\" value=\"code\">\
<input type=\"hidden\" name=\"client_id\" value=\"{}\">\
<input type=\"hidden\" name=\"redirect_uri\" value=\"{}\">\
<input type=\"hidden\" name=\"state\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge_method\" value=\"S256\">\
<input type=\"hidden\" name=\"resource\" value=\"{}\">\
<input type=\"hidden\" name=\"scope\" value=\"{}\">\
<label>Authorization key<br><input autofocus type=\"password\" name=\"access_key\" autocomplete=\"current-password\" required></label><p><button type=\"submit\">Authorize</button></p></form>",
        html_escape(&request.client_id),
        html_escape(&request.resource),
        html_escape(&request.client_id),
        html_escape(&request.redirect_uri),
        html_escape(state),
        html_escape(&request.code_challenge),
        html_escape(&request.resource),
        html_escape(&request.scope),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn inferred_public_url(bind_host: &str, address: SocketAddr) -> Result<Url> {
    if matches!(bind_host, "0.0.0.0" | "::") {
        bail!(
            "--public-url is required for wildcard MCP binds; use the externally reachable /mcp URL"
        );
    }
    let host = if bind_host.is_empty() {
        address.ip().to_string()
    } else {
        bind_host.to_owned()
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    Url::parse(&format!("http://{host}:{}/mcp", address.port())).map_err(Into::into)
}

fn normalize_public_url(mut url: Url) -> Result<Url> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("--public-url must be an HTTP(S) MCP endpoint without credentials/query/fragment");
    }
    if url.path() == "/" || url.path().is_empty() {
        url.set_path("/mcp");
    }
    Ok(url)
}

fn issuer_for_resource(resource: &Url) -> Result<Url> {
    let mut issuer = resource.clone();
    issuer.set_path("/");
    issuer.set_query(None);
    issuer.set_fragment(None);
    Ok(issuer)
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            bail!("HTTP connection closed before request headers");
        }
        buffer.extend_from_slice(&chunk[..count]);
        if buffer.len() > MAX_HEADER_BYTES {
            bail!("HTTP headers exceed {MAX_HEADER_BYTES} bytes");
        }
        if let Some(index) = find_bytes(&buffer, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header_text = std::str::from_utf8(&buffer[..header_end - 4])?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP method"))?
        .to_owned();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP target"))?
        .to_owned();
    let version = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP version"))?;
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        bail!("unsupported HTTP version: {version}");
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed HTTP header"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("invalid Content-Length")?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("HTTP body exceeds {MAX_BODY_BYTES} bytes");
    }
    while buffer.len() - header_end < content_length {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            bail!("HTTP connection closed before request body completed");
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
    Ok(HttpRequest {
        method,
        target,
        headers,
        body: buffer[header_end..header_end + content_length].to_vec(),
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn split_target(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

fn parse_form(bytes: &[u8]) -> HashMap<String, String> {
    form_urlencoded::parse(bytes)
        .into_owned()
        .collect::<HashMap<_, _>>()
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    fn text(status: u16, value: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: value.as_bytes().to_vec(),
        }
    }

    fn html(status: u16, value: String) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: value.into_bytes(),
        }
    }

    fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            content_type: "text/plain; charset=utf-8",
            headers: vec![("Location".into(), location.into())],
            body: b"Redirecting".to_vec(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )?;
    for (name, value) in response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_header_is_added_to_request_metadata_without_overwriting_body() {
        let mut request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        attach_protocol_version(&mut request, MODERN_PROTOCOL_VERSION);
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            MODERN_PROTOCOL_VERSION
        );
        attach_protocol_version(&mut request, "old");
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            MODERN_PROTOCOL_VERSION
        );
    }

    #[test]
    fn public_url_defaults_to_mcp_path() {
        let url = normalize_public_url(Url::parse("https://example.com/").unwrap()).unwrap();
        assert_eq!(url.as_str(), "https://example.com/mcp");
        assert_eq!(
            issuer_for_resource(&url).unwrap().as_str(),
            "https://example.com/"
        );
    }
}
