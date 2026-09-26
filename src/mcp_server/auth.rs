use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    config::ConfigStore,
    platform::{replace_file, set_private_directory, set_private_file},
};

const AUTH_CODE_TTL: Duration = Duration::from_secs(5 * 60);
const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const REFRESH_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const OFFLINE_ACCESS_SCOPE: &str = "offline_access";
const MCP_SCOPE: &str = "mcp";
const GEMINI_SPARK_COMPAT_SCOPES: &[&str] = &[
    "ACCESS_VIEW_MANAGE_MCP_CONTENT",
    "SHARE_THROUGH_MCP_CONVERSATION_INFO",
    "TRIGGER_TOOLS_AND_FUNCTION",
];

fn oauth_scope_supported(scope: &str) -> bool {
    scope == MCP_SCOPE
        || scope == OFFLINE_ACCESS_SCOPE
        || GEMINI_SPARK_COMPAT_SCOPES.contains(&scope)
}

fn oauth_scope_allows_mcp(scope_set: &str) -> bool {
    let mut includes_mcp = false;
    for scope in scope_set.split_ascii_whitespace() {
        if !oauth_scope_supported(scope) {
            return false;
        }
        includes_mcp |= scope == MCP_SCOPE;
    }
    includes_mcp
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum AuthMode {
    None,
    Key,
    Oauth,
}

impl AuthMode {
    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "key" => Ok(Self::Key),
            "oauth" => Ok(Self::Oauth),
            _ => bail!("auth mode must be one of: none, key, oauth"),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Key => "key",
            Self::Oauth => "oauth",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AuthDocument {
    version: u32,
    mode: AuthMode,
    key_hash: Option<String>,
}

impl Default for AuthDocument {
    fn default() -> Self {
        Self {
            version: 1,
            mode: AuthMode::Key,
            key_hash: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AuthStatus {
    pub mode: AuthMode,
    pub key_enabled: bool,
    pub oauth_enabled: bool,
}

#[derive(Debug, Clone)]
pub(super) struct AuthStore {
    path: PathBuf,
    oauth_path: PathBuf,
}

impl AuthStore {
    pub(super) fn new(port: u16) -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let directory = config.directory.join("mcpserver");
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        Ok(Self {
            path: directory.join(format!("auth-{port}.json")),
            oauth_path: directory.join(format!("oauth-{port}.enabled")),
        })
    }

    fn load(&self) -> Result<AuthDocument> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let document: AuthDocument = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode MCP auth file {}", self.path.display()))?;
                if document.version != 1 {
                    bail!("unsupported MCP auth version: {}", document.version);
                }
                Ok(document)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AuthDocument::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, document: &AuthDocument) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("MCP auth path has no parent"))?;
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            self.path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("auth"),
            uuid::Uuid::new_v4().simple()
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(document)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        replace_file(&temporary, &self.path)?;
        set_private_file(&self.path)?;
        Ok(())
    }

    pub(super) fn status(&self) -> Result<AuthStatus> {
        let document = self.load()?;
        Ok(AuthStatus {
            mode: document.mode,
            key_enabled: document.key_hash.is_some(),
            oauth_enabled: document.mode == AuthMode::Oauth || self.oauth_marker_enabled()?,
        })
    }

    pub(super) fn set_mode(&self, mode: AuthMode) -> Result<()> {
        let mut document = self.load()?;
        if matches!(mode, AuthMode::Key | AuthMode::Oauth) && document.key_hash.is_none() {
            bail!("MCP auth mode {mode:?} requires an access key; generate or set one first");
        }
        document.mode = mode;
        self.save(&document)
    }

    pub(super) fn set_oauth_enabled(&self, enabled: bool) -> Result<()> {
        let document = self.load()?;
        if enabled && document.key_hash.is_none() {
            bail!("OAuth requires an MCP access key; generate or set one first");
        }
        if !enabled && document.mode == AuthMode::Oauth {
            bail!(
                "OAuth is the primary legacy auth mode; switch primary auth mode before disabling the additive OAuth path"
            );
        }
        if enabled {
            self.write_oauth_marker()
        } else {
            self.clear_oauth_marker()
        }
    }

    fn oauth_marker_enabled(&self) -> Result<bool> {
        match fs::metadata(&self.oauth_path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn write_oauth_marker(&self) -> Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&self.oauth_path)
            .with_context(|| format!("write OAuth marker {}", self.oauth_path.display()))?;
        file.write_all(b"enabled\n")?;
        file.sync_all()?;
        set_private_file(&self.oauth_path)?;
        Ok(())
    }

    fn clear_oauth_marker(&self) -> Result<()> {
        match fs::remove_file(&self.oauth_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn generate_key(&self) -> Result<String> {
        let key = random_token("yeet_mcp");
        self.set_key(&key)?;
        Ok(key)
    }

    pub(super) fn set_key(&self, key: &str) -> Result<()> {
        let key = key.trim();
        if key.len() < 16 {
            bail!("MCP access key must be at least 16 characters");
        }
        let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
            .map_err(|error| anyhow!("generate MCP access-key salt: {error}"))?;
        let hash = Argon2::default()
            .hash_password(key.as_bytes(), &salt)
            .map_err(|error| anyhow!("hash MCP access key: {error}"))?
            .to_string();
        let mut document = self.load()?;
        document.key_hash = Some(hash);
        self.save(&document)
    }

    pub(super) fn clear_key(&self) -> Result<()> {
        let mut document = self.load()?;
        document.key_hash = None;
        document.mode = AuthMode::None;
        self.save(&document)?;
        self.clear_oauth_marker()
    }

    pub(super) fn verify_key(&self, key: &str) -> Result<bool> {
        let document = self.load()?;
        let Some(hash) = document.key_hash else {
            return Ok(false);
        };
        let parsed = PasswordHash::new(&hash)
            .map_err(|error| anyhow!("decode MCP access-key hash: {error}"))?;
        Ok(Argon2::default()
            .verify_password(key.as_bytes(), &parsed)
            .is_ok())
    }
}

#[derive(Debug, Clone)]
pub(super) struct AuthorizationRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub resource: String,
    pub scope: String,
}

#[derive(Debug, Clone)]
struct AuthorizationCode {
    request: AuthorizationRequest,
    expires_at: u64,
}

#[derive(Debug, Clone)]
struct AccessToken {
    expires_at: u64,
    resource: String,
    scope: String,
}

#[derive(Debug, Clone)]
struct RefreshToken {
    expires_at: u64,
    client_id: String,
    resource: String,
    scope: String,
}

#[derive(Debug, Clone)]
struct RegisteredClient {
    redirect_uris: Vec<String>,
}

#[derive(Clone)]
pub(super) struct OAuthRuntime {
    store: AuthStore,
    issuer: Url,
    resource: Url,
    authorization_codes: Arc<Mutex<HashMap<String, AuthorizationCode>>>,
    access_tokens: Arc<Mutex<HashMap<String, AccessToken>>>,
    refresh_tokens: Arc<Mutex<HashMap<String, RefreshToken>>>,
    registered_clients: Arc<Mutex<HashMap<String, RegisteredClient>>>,
}

impl OAuthRuntime {
    pub(super) fn new(store: AuthStore, issuer: Url, resource: Url) -> Result<Self> {
        if issuer.path() != "/" || issuer.query().is_some() || issuer.fragment().is_some() {
            bail!("OAuth issuer must be an origin URL without a path, query, or fragment");
        }
        Ok(Self {
            store,
            issuer,
            resource,
            authorization_codes: Arc::new(Mutex::new(HashMap::new())),
            access_tokens: Arc::new(Mutex::new(HashMap::new())),
            refresh_tokens: Arc::new(Mutex::new(HashMap::new())),
            registered_clients: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub(super) fn auth_store(&self) -> &AuthStore {
        &self.store
    }

    pub(super) fn protected_resource_metadata(&self) -> Value {
        json!({
            "resource": self.resource,
            "authorization_servers": [self.issuer],
            "scopes_supported": [MCP_SCOPE],
            "bearer_methods_supported": ["header"]
        })
    }

    pub(super) fn authorization_server_metadata(&self) -> Value {
        let scopes_supported = std::iter::once(MCP_SCOPE)
            .chain(GEMINI_SPARK_COMPAT_SCOPES.iter().copied())
            .chain(std::iter::once(OFFLINE_ACCESS_SCOPE))
            .collect::<Vec<_>>();
        json!({
            "issuer": self.issuer,
            "authorization_endpoint": self.issuer.join("authorize").unwrap(),
            "token_endpoint": self.issuer.join("token").unwrap(),
            "registration_endpoint": self.issuer.join("register").unwrap(),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": scopes_supported,
            "client_id_metadata_document_supported": true,
            "authorization_response_iss_parameter_supported": true
        })
    }

    pub(super) fn parse_authorization_request(
        &self,
        params: &HashMap<String, String>,
    ) -> Result<AuthorizationRequest> {
        if params.get("response_type").map(String::as_str) != Some("code") {
            bail!("response_type must be code");
        }
        if params.get("code_challenge_method").map(String::as_str) != Some("S256") {
            bail!("code_challenge_method must be S256");
        }
        let request = AuthorizationRequest {
            client_id: required_param(params, "client_id")?.to_owned(),
            redirect_uri: required_param(params, "redirect_uri")?.to_owned(),
            state: params.get("state").cloned(),
            code_challenge: required_param(params, "code_challenge")?.to_owned(),
            resource: params
                .get("resource")
                .map(String::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(self.resource.as_str())
                .to_owned(),
            scope: params
                .get("scope")
                .map(String::as_str)
                .unwrap_or(MCP_SCOPE)
                .to_owned(),
        };
        if request.resource != self.resource.as_str() {
            bail!("OAuth resource does not match this MCP server");
        }
        if !oauth_scope_allows_mcp(&request.scope) {
            bail!("unsupported OAuth scope");
        }
        if request.code_challenge.len() < 32 {
            bail!("invalid PKCE code_challenge");
        }
        self.validate_client(&request.client_id, &request.redirect_uri)?;
        Ok(request)
    }

    pub(super) fn authorize_with_key(
        &self,
        request: AuthorizationRequest,
        key: &str,
    ) -> Result<Url> {
        if !self.store.status()?.oauth_enabled {
            bail!("OAuth authentication is not enabled for this MCP daemon");
        }
        if !self.store.verify_key(key)? {
            bail!("invalid MCP authorization key");
        }
        let code = random_token("code");
        self.authorization_codes.lock().unwrap().insert(
            code.clone(),
            AuthorizationCode {
                request: request.clone(),
                expires_at: unix_time() + AUTH_CODE_TTL.as_secs(),
            },
        );
        let mut redirect = Url::parse(&request.redirect_uri).context("invalid redirect_uri")?;
        {
            let mut pairs = redirect.query_pairs_mut();
            pairs.append_pair("code", &code);
            pairs.append_pair("iss", self.issuer.as_str());
            if let Some(state) = request.state.as_deref() {
                pairs.append_pair("state", state);
            }
        }
        Ok(redirect)
    }

    pub(super) fn exchange_token(&self, params: &HashMap<String, String>) -> Result<Value> {
        if !self.store.status()?.oauth_enabled {
            bail!("OAuth authentication is not enabled for this MCP daemon");
        }
        match required_param(params, "grant_type")? {
            "authorization_code" => self.exchange_authorization_code(params),
            "refresh_token" => self.exchange_refresh_token(params),
            _ => bail!("unsupported grant_type"),
        }
    }

    fn exchange_authorization_code(&self, params: &HashMap<String, String>) -> Result<Value> {
        let code = required_param(params, "code")?;
        let redirect_uri = required_param(params, "redirect_uri")?;
        let client_id = required_param(params, "client_id")?;
        let verifier = required_param(params, "code_verifier")?;
        let resource = params.get("resource").map(String::as_str);
        let stored = self
            .authorization_codes
            .lock()
            .unwrap()
            .remove(code)
            .ok_or_else(|| anyhow!("authorization code is invalid or already used"))?;
        if stored.expires_at <= unix_time() {
            bail!("authorization code expired");
        }
        if stored.request.redirect_uri != redirect_uri
            || stored.request.client_id != client_id
            || resource.is_some_and(|value| value != stored.request.resource)
        {
            bail!("authorization code binding mismatch");
        }
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        if challenge != stored.request.code_challenge {
            bail!("PKCE verification failed");
        }
        Ok(self.issue_token_pair(
            stored.request.client_id,
            stored.request.resource,
            stored.request.scope,
        ))
    }

    fn exchange_refresh_token(&self, params: &HashMap<String, String>) -> Result<Value> {
        let refresh_token = required_param(params, "refresh_token")?;
        let stored = self
            .refresh_tokens
            .lock()
            .unwrap()
            .remove(refresh_token)
            .ok_or_else(|| anyhow!("refresh token is invalid or already used"))?;
        if stored.expires_at <= unix_time() {
            bail!("refresh token expired");
        }
        if let Some(client_id) = params.get("client_id").filter(|value| !value.is_empty())
            && client_id != &stored.client_id
        {
            bail!("refresh token client binding mismatch");
        }
        if let Some(resource) = params.get("resource").filter(|value| !value.is_empty())
            && resource != &stored.resource
        {
            bail!("refresh token resource binding mismatch");
        }
        let scope = match params.get("scope").filter(|value| !value.is_empty()) {
            Some(requested) => {
                if !oauth_scope_allows_mcp(requested)
                    || requested.split_ascii_whitespace().any(|scope| {
                        !stored
                            .scope
                            .split_ascii_whitespace()
                            .any(|granted| granted == scope)
                    })
                {
                    bail!("refresh token scope escalation is not allowed");
                }
                requested.clone()
            }
            None => stored.scope,
        };
        Ok(self.issue_token_pair(stored.client_id, stored.resource, scope))
    }

    fn issue_token_pair(&self, client_id: String, resource: String, scope: String) -> Value {
        let access_token = random_token("mcp_oauth");
        self.access_tokens.lock().unwrap().insert(
            access_token.clone(),
            AccessToken {
                expires_at: unix_time() + ACCESS_TOKEN_TTL.as_secs(),
                resource: resource.clone(),
                scope: scope.clone(),
            },
        );
        let refresh_token = random_token("mcp_refresh");
        self.refresh_tokens.lock().unwrap().insert(
            refresh_token.clone(),
            RefreshToken {
                expires_at: unix_time() + REFRESH_TOKEN_TTL.as_secs(),
                client_id,
                resource,
                scope: scope.clone(),
            },
        );
        json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_TTL.as_secs(),
            "refresh_token": refresh_token,
            "scope": scope
        })
    }

    pub(super) fn verify_oauth_token(&self, token: &str) -> Result<bool> {
        if !self.store.status()?.oauth_enabled {
            return Ok(false);
        }
        let now = unix_time();
        let mut tokens = self.access_tokens.lock().unwrap();
        tokens.retain(|_, value| value.expires_at > now);
        Ok(tokens.get(token).is_some_and(|value| {
            value.resource == self.resource.as_str() && oauth_scope_allows_mcp(&value.scope)
        }))
    }

    pub(super) fn register_client(&self, body: &Value) -> Result<Value> {
        let object = body
            .as_object()
            .ok_or_else(|| anyhow!("OAuth registration body must be an object"))?;
        let redirect_uris = string_array(object, "redirect_uris")?;
        if redirect_uris.is_empty() {
            bail!("redirect_uris must contain at least one URI");
        }
        for uri in &redirect_uris {
            validate_redirect_uri(uri)?;
        }
        let client_id = random_token("dcr");
        self.registered_clients.lock().unwrap().insert(
            client_id.clone(),
            RegisteredClient {
                redirect_uris: redirect_uris.clone(),
            },
        );
        Ok(json!({
            "client_id": client_id,
            "client_id_issued_at": unix_time(),
            "redirect_uris": redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
    }

    fn validate_client(&self, client_id: &str, redirect_uri: &str) -> Result<()> {
        validate_redirect_uri(redirect_uri)?;
        if let Some(client) = self.registered_clients.lock().unwrap().get(client_id) {
            if !client
                .redirect_uris
                .iter()
                .any(|value| value == redirect_uri)
            {
                bail!("redirect_uri is not registered for this client");
            }
            return Ok(());
        }
        let url = Url::parse(client_id).context("client_id must be a registered ID or CIMD URL")?;
        if url.scheme() != "https" || url.path() == "/" || url.host_str().is_none() {
            bail!("CIMD client_id must be an HTTPS URL with a path");
        }
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            bail!("invalid CIMD client_id URL");
        }
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .redirects(0)
            .build();
        let response = agent
            .get(url.as_str())
            .timeout(Duration::from_secs(5))
            .call()
            .map_err(|error| anyhow!("fetch client metadata document: {error}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(128 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 128 * 1024 {
            bail!("client metadata document exceeds 128 KiB");
        }
        let metadata: Value = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow!("decode client metadata document: {error}"))?;
        let object = metadata
            .as_object()
            .ok_or_else(|| anyhow!("client metadata document must be an object"))?;
        if object.get("client_id").and_then(Value::as_str) != Some(client_id) {
            bail!("client metadata document client_id does not match its URL");
        }
        let redirects = string_array(object, "redirect_uris")?;
        if !redirects.iter().any(|value| value == redirect_uri) {
            bail!("redirect_uri is not listed by the client metadata document");
        }
        Ok(())
    }
}

pub(super) fn bearer_token(header: Option<&str>) -> Option<&str> {
    let value = header?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") || token.trim().is_empty() {
        return None;
    }
    Some(token.trim())
}

fn required_param<'a>(params: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("missing OAuth parameter: {key}"))
}

fn string_array(object: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{key} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("{key} entries must be strings"))
        })
        .collect()
}

fn validate_redirect_uri(value: &str) -> Result<()> {
    let url = Url::parse(value).context("invalid redirect_uri")?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        bail!("redirect_uri must not contain credentials or a fragment");
    }
    let host = url.host_str().unwrap_or_default();
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("redirect_uri must use HTTPS, except for localhost/loopback clients");
    }
    Ok(())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_token(prefix: &str) -> String {
    format!(
        "{prefix}_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth_runtime() -> (tempfile::TempDir, OAuthRuntime, String) {
        let directory = tempfile::tempdir().expect("create temporary auth directory");
        let store = AuthStore {
            path: directory.path().join("auth.json"),
            oauth_path: directory.path().join("oauth.enabled"),
        };
        store
            .set_key("0123456789abcdef")
            .expect("set test access key");
        store.set_mode(AuthMode::Oauth).expect("enable OAuth mode");

        let runtime = OAuthRuntime::new(
            store,
            Url::parse("https://auth.example/").unwrap(),
            Url::parse("https://mcp.example/mcp").unwrap(),
        )
        .unwrap();
        let registration = runtime
            .register_client(&json!({
                "redirect_uris": ["http://127.0.0.1:32123/callback"]
            }))
            .unwrap();
        let client_id = registration["client_id"].as_str().unwrap().to_owned();
        (directory, runtime, client_id)
    }

    fn authorization_params(client_id: &str) -> HashMap<String, String> {
        HashMap::from([
            ("response_type".to_owned(), "code".to_owned()),
            ("client_id".to_owned(), client_id.to_owned()),
            (
                "redirect_uri".to_owned(),
                "http://127.0.0.1:32123/callback".to_owned(),
            ),
            ("state".to_owned(), "state-123".to_owned()),
            (
                "code_challenge".to_owned(),
                "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG".to_owned(),
            ),
            ("code_challenge_method".to_owned(), "S256".to_owned()),
            ("scope".to_owned(), MCP_SCOPE.to_owned()),
        ])
    }

    #[test]
    fn authorization_defaults_missing_resource_and_returns_issuer() {
        let (_directory, runtime, client_id) = oauth_runtime();
        let request = runtime
            .parse_authorization_request(&authorization_params(&client_id))
            .expect("authorization without resource should use the MCP resource");
        assert_eq!(request.resource, "https://mcp.example/mcp");

        let metadata = runtime.authorization_server_metadata();
        assert_eq!(
            metadata["authorization_response_iss_parameter_supported"],
            true
        );

        let redirect = runtime
            .authorize_with_key(request, "0123456789abcdef")
            .expect("authorize request");
        let query = redirect.query_pairs().collect::<HashMap<_, _>>();
        assert!(query.contains_key("code"));
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some("state-123")
        );
        assert_eq!(
            query.get("iss").map(|value| value.as_ref()),
            Some("https://auth.example/")
        );
    }

    #[test]
    fn authorization_rejects_wrong_explicit_resource() {
        let (_directory, runtime, client_id) = oauth_runtime();
        let mut params = authorization_params(&client_id);
        params.insert(
            "resource".to_owned(),
            "https://different.example/mcp".to_owned(),
        );
        let error = runtime
            .parse_authorization_request(&params)
            .expect_err("wrong resource must remain rejected");
        assert!(
            error
                .to_string()
                .contains("OAuth resource does not match this MCP server")
        );
    }

    #[test]
    fn gemini_spark_scopes_are_accepted_without_granting_scope_only_access() {
        let (_directory, runtime, client_id) = oauth_runtime();
        let spark_scope = format!(
            "{} {} {} {}",
            GEMINI_SPARK_COMPAT_SCOPES[0],
            GEMINI_SPARK_COMPAT_SCOPES[1],
            GEMINI_SPARK_COMPAT_SCOPES[2],
            MCP_SCOPE
        );
        let verifier = "gemini-spark-pkce-verifier-0123456789abcdefghijklmnopqrstuvwxyz";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

        let mut params = authorization_params(&client_id);
        params.insert("scope".to_owned(), spark_scope.clone());
        params.insert("code_challenge".to_owned(), challenge);

        let request = runtime
            .parse_authorization_request(&params)
            .expect("Gemini Spark compatibility scopes should be accepted");
        assert_eq!(request.scope, spark_scope);

        let metadata = runtime.authorization_server_metadata();
        let supported = metadata["scopes_supported"]
            .as_array()
            .expect("scopes_supported array");
        for scope in std::iter::once(MCP_SCOPE).chain(GEMINI_SPARK_COMPAT_SCOPES.iter().copied()) {
            assert!(supported.iter().any(|value| value.as_str() == Some(scope)));
        }
        assert!(
            supported
                .iter()
                .any(|value| value.as_str() == Some(OFFLINE_ACCESS_SCOPE))
        );
        assert!(
            metadata["grant_types_supported"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("refresh_token"))
        );

        let redirect = runtime
            .authorize_with_key(request, "0123456789abcdef")
            .expect("authorize Spark request");
        let code = redirect
            .query_pairs()
            .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
            .expect("authorization code");

        let token = runtime
            .exchange_token(&HashMap::from([
                ("grant_type".to_owned(), "authorization_code".to_owned()),
                ("code".to_owned(), code),
                (
                    "redirect_uri".to_owned(),
                    "http://127.0.0.1:32123/callback".to_owned(),
                ),
                ("client_id".to_owned(), client_id.clone()),
                ("code_verifier".to_owned(), verifier.to_owned()),
            ]))
            .expect("exchange Spark authorization code");
        assert_eq!(token["scope"].as_str(), Some(spark_scope.as_str()));
        let access_token = token["access_token"].as_str().expect("access token");
        assert!(runtime.verify_oauth_token(access_token).unwrap());
        let refresh_token = token["refresh_token"]
            .as_str()
            .expect("refresh token")
            .to_owned();
        let refreshed = runtime
            .exchange_token(&HashMap::from([
                ("grant_type".to_owned(), "refresh_token".to_owned()),
                ("refresh_token".to_owned(), refresh_token.clone()),
                ("client_id".to_owned(), client_id.clone()),
            ]))
            .expect("rotate Spark refresh token");
        let refreshed_access = refreshed["access_token"]
            .as_str()
            .expect("refreshed access token");
        assert!(runtime.verify_oauth_token(refreshed_access).unwrap());
        assert_ne!(
            refreshed["refresh_token"].as_str(),
            Some(refresh_token.as_str())
        );
        let reuse_error = runtime
            .exchange_token(&HashMap::from([
                ("grant_type".to_owned(), "refresh_token".to_owned()),
                ("refresh_token".to_owned(), refresh_token),
                ("client_id".to_owned(), client_id),
            ]))
            .expect_err("rotated refresh token must be single-use");
        assert!(reuse_error.to_string().contains("invalid or already used"));

        let mut missing_mcp = authorization_params("unused");
        missing_mcp.insert("scope".to_owned(), GEMINI_SPARK_COMPAT_SCOPES.join(" "));
        assert!(!oauth_scope_allows_mcp(&missing_mcp["scope"]));
    }

    #[test]
    fn authorization_rejects_unknown_scope_even_with_mcp() {
        let (_directory, runtime, client_id) = oauth_runtime();
        let mut params = authorization_params(&client_id);
        params.insert("scope".to_owned(), "mcp UNKNOWN_SCOPE".to_owned());
        let error = runtime
            .parse_authorization_request(&params)
            .expect_err("unknown scope must remain rejected");
        assert!(error.to_string().contains("unsupported OAuth scope"));
    }
}
