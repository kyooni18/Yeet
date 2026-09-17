//! HTTP endpoint URL normalization plus bounded HTTP/1 request/response wire handling.

use super::*;

pub(super) fn inferred_public_url(bind_host: &str, address: SocketAddr) -> Result<Url> {
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

pub(super) fn normalize_public_url(mut url: Url) -> Result<Url> {
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

pub(super) fn issuer_for_resource(resource: &Url) -> Result<Url> {
    let mut issuer = resource.clone();
    issuer.set_path("/");
    issuer.set_query(None);
    issuer.set_fragment(None);
    Ok(issuer)
}

#[derive(Debug)]
pub(super) struct HttpRequest {
    pub(super) method: String,
    pub(super) target: String,
    pub(super) headers: HashMap<String, String>,
    pub(super) body: Vec<u8>,
}

pub(super) fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
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

pub(super) fn split_target(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

pub(super) fn parse_form(bytes: &[u8]) -> HashMap<String, String> {
    form_urlencoded::parse(bytes)
        .into_owned()
        .collect::<HashMap<_, _>>()
}

pub(super) struct HttpResponse {
    pub(super) status: u16,
    pub(super) content_type: &'static str,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl HttpResponse {
    pub(super) fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    pub(super) fn text(status: u16, value: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: value.as_bytes().to_vec(),
        }
    }

    pub(super) fn html(status: u16, value: String) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: value.into_bytes(),
        }
    }

    pub(super) fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            content_type: "text/plain; charset=utf-8",
            headers: vec![("Location".into(), location.into())],
            body: b"Redirecting".to_vec(),
        }
    }

    pub(super) fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

pub(super) fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
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
        429 => "Too Many Requests",
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
