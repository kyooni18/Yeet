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
const MCP_SCOPE: &str = "mcp";

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
}

#[derive(Debug, Clone)]
pub(super) struct AuthStore {
    path: PathBuf,
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
        self.save(&document)
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
        json!({
            "issuer": self.issuer,
            "authorization_endpoint": self.issuer.join("authorize").unwrap(),
            "token_endpoint": self.issuer.join("token").unwrap(),
            "registration_endpoint": self.issuer.join("register").unwrap(),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": [MCP_SCOPE],
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
            resource: required_param(params, "resource")?.to_owned(),
            scope: params
                .get("scope")
                .map(String::as_str)
                .unwrap_or(MCP_SCOPE)
                .to_owned(),
        };
        if request.resource != self.resource.as_str() {
            bail!("OAuth resource does not match this MCP server");
        }
        if request
            .scope
            .split_ascii_whitespace()
            .any(|value| value != MCP_SCOPE)
        {
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
        if self.store.status()?.mode != AuthMode::Oauth {
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
            if let Some(state) = request.state.as_deref() {
                pairs.append_pair("state", state);
            }
            // RFC 9207 authorization response issuer identification.
            pairs.append_pair("iss", self.issuer.as_str());
        }
        Ok(redirect)
    }

    pub(super) fn exchange_token(&self, params: &HashMap<String, String>) -> Result<Value> {
        if self.store.status()?.mode != AuthMode::Oauth {
            bail!("OAuth authentication is not enabled for this MCP daemon");
        }
        if params.get("grant_type").map(String::as_str) != Some("authorization_code") {
            bail!("grant_type must be authorization_code");
        }
        let code = required_param(params, "code")?;
        let redirect_uri = required_param(params, "redirect_uri")?;
        let client_id = required_param(params, "client_id")?;
        let verifier = required_param(params, "code_verifier")?;
        let resource = required_param(params, "resource")?;
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
            || stored.request.resource != resource
        {
            bail!("authorization code binding mismatch");
        }
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        if challenge != stored.request.code_challenge {
            bail!("PKCE verification failed");
        }
        let token = random_token("mcp_oauth");
        self.access_tokens.lock().unwrap().insert(
            token.clone(),
            AccessToken {
                expires_at: unix_time() + ACCESS_TOKEN_TTL.as_secs(),
                resource: stored.request.resource.clone(),
                scope: stored.request.scope.clone(),
            },
        );
        Ok(json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_TTL.as_secs(),
            "scope": stored.request.scope
        }))
    }

    pub(super) fn verify_oauth_token(&self, token: &str) -> Result<bool> {
        if self.store.status()?.mode != AuthMode::Oauth {
            return Ok(false);
        }
        let now = unix_time();
        let mut tokens = self.access_tokens.lock().unwrap();
        tokens.retain(|_, value| value.expires_at > now);
        Ok(tokens.get(token).is_some_and(|value| {
            value.resource == self.resource.as_str()
                && value
                    .scope
                    .split_ascii_whitespace()
                    .all(|scope| scope == MCP_SCOPE)
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
            "grant_types": ["authorization_code"],
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
    use argon2::{Algorithm, Params, Version};

    fn test_store(path: PathBuf, key: &str, mode: AuthMode) -> AuthStore {
        let store = AuthStore { path };
        let salt = SaltString::encode_b64(b"mcp-test-salt-01").unwrap();
        let argon = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(32, 1, 1, None).unwrap(),
        );
        let key_hash = argon
            .hash_password(key.as_bytes(), &salt)
            .unwrap()
            .to_string();
        store
            .save(&AuthDocument {
                version: 1,
                mode,
                key_hash: Some(key_hash),
            })
            .unwrap();
        store
    }

    #[test]
    fn key_hash_round_trip_and_clear() {
        let root = tempfile::tempdir().unwrap();
        let store = test_store(
            root.path().join("auth.json"),
            "0123456789abcdef",
            AuthMode::Key,
        );
        assert!(store.verify_key("0123456789abcdef").unwrap());
        assert!(!store.verify_key("wrong-wrong-wrong").unwrap());
        store.clear_key().unwrap();
        assert_eq!(store.status().unwrap().mode, AuthMode::None);
    }

    #[test]
    fn oauth_exchange_requires_matching_pkce_resource_and_binding() {
        let root = tempfile::tempdir().unwrap();
        let store = test_store(
            root.path().join("auth.json"),
            "0123456789abcdef",
            AuthMode::Oauth,
        );
        let issuer = Url::parse("https://server.example/").unwrap();
        let resource = Url::parse("https://server.example/mcp").unwrap();
        let oauth = OAuthRuntime::new(store, issuer, resource).unwrap();
        let client_id = "dcr_test".to_owned();
        oauth.registered_clients.lock().unwrap().insert(
            client_id.clone(),
            RegisteredClient {
                redirect_uris: vec!["http://127.0.0.1:3210/callback".into()],
            },
        );
        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let request = AuthorizationRequest {
            client_id: client_id.clone(),
            redirect_uri: "http://127.0.0.1:3210/callback".into(),
            state: Some("state-1".into()),
            code_challenge: challenge,
            resource: "https://server.example/mcp".into(),
            scope: MCP_SCOPE.into(),
        };
        let redirect = oauth
            .authorize_with_key(request, "0123456789abcdef")
            .unwrap();
        let code = redirect
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let token = oauth
            .exchange_token(&HashMap::from([
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), code),
                (
                    "redirect_uri".into(),
                    "http://127.0.0.1:3210/callback".into(),
                ),
                ("client_id".into(), client_id),
                ("code_verifier".into(), verifier.into()),
                ("resource".into(), "https://server.example/mcp".into()),
            ]))
            .unwrap();
        assert!(
            oauth
                .verify_oauth_token(token["access_token"].as_str().unwrap())
                .unwrap()
        );
    }
}
