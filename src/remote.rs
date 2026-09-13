mod assets;
pub mod protocol;
mod render;
mod server;
mod websocket;
use render::render_buffer;

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use webauthn_rs::prelude::{
    CredentialID, Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid as WebauthnUuid, Webauthn, WebauthnBuilder,
};

use crate::{
    config::ConfigStore,
    platform::{
        bind_local, configure_detached, connect_local, replace_file, set_private_directory,
        set_private_file,
    },
};

const DEFAULT_BIND: &str = "0.0.0.0:7331";
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 12;
const MAX_COLS: u16 = 300;
const MAX_ROWS: u16 = 120;
const DAEMON_START_RETRIES: usize = 80;
const DAEMON_START_DELAY: Duration = Duration::from_millis(50);
const DAEMON_CONTROL_TIMEOUT: Duration = Duration::from_millis(600);
const AUTH_SESSION_SECONDS: u64 = 12 * 60 * 60;
const PASSKEY_ENROLL_SECONDS: u64 = 10 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Configures the remote HTTP/WebSocket frontend server and authentication boundary.
pub struct RemoteOptions {
    pub bind: String,
    pub cols: u16,
    pub rows: u16,
    pub origin: Option<String>,
    pub workspace: Option<PathBuf>,
    pub legacy_tui: bool,
}

impl Default for RemoteOptions {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.into(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            origin: None,
            workspace: None,
            legacy_tui: false,
        }
    }
}

impl RemoteOptions {
    pub fn parse(arguments: &[String]) -> Result<Option<Self>> {
        let Some(first) = arguments.first().map(String::as_str) else {
            return Ok(None);
        };
        if !matches!(first, "remote" | "--remote") {
            return Ok(None);
        }

        let mut options = Self::default();
        let mut index = 1;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--bind" | "--remote-bind" => {
                    index += 1;
                    options.bind = arguments
                        .get(index)
                        .cloned()
                        .ok_or_else(|| anyhow!("remote mode requires an address after --bind"))?;
                }
                "--size" | "--remote-size" => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| anyhow!("remote mode requires COLSxROWS after --size"))?;
                    let (cols, rows) = parse_size(value)?;
                    options.cols = cols;
                    options.rows = rows;
                }
                "--origin" => {
                    index += 1;
                    options.origin = Some(
                        arguments
                            .get(index)
                            .cloned()
                            .ok_or_else(|| anyhow!("remote mode requires a URL after --origin"))?,
                    );
                }
                "--legacy-tui" => {
                    options.legacy_tui = true;
                }
                "--workspace" => {
                    index += 1;
                    let path =
                        PathBuf::from(arguments.get(index).ok_or_else(|| {
                            anyhow!("remote mode requires a path after --workspace")
                        })?);
                    if options.workspace.replace(path).is_some() {
                        bail!("remote workspace was specified more than once");
                    }
                }
                "-h" | "--help" => bail!(remote_help()),
                value if !value.starts_with('-') && options.workspace.is_none() => {
                    options.workspace = Some(PathBuf::from(value));
                }
                value => bail!("unknown remote option: {value}\n\n{}", remote_help()),
            }
            index += 1;
        }
        Ok(Some(options))
    }
}

pub fn remote_help() -> &'static str {
    "Yeet remote mode\n\n  yeet remote [WORKSPACE] [--workspace PATH] [--bind ADDRESS] [--origin URL] [--legacy-tui]\n  yeet remote status [WORKSPACE|--workspace PATH]\n  yeet remote stop [WORKSPACE|--workspace PATH]\n  yeet remote auth status [WORKSPACE|--workspace PATH]\n  yeet remote auth key generate|set|clear [WORKSPACE|--workspace PATH]\n  yeet remote auth passkey add|clear [WORKSPACE|--workspace PATH]\n  yeet --remote [WORKSPACE] [--bind ADDRESS] [--origin URL]\n\nDefaults:\n  initial workspace: current directory (semantic WebUI clients may switch it)\n  --bind 0.0.0.0:7331\n  frontend: semantic Vue WebUI\n\nOptions:\n  --legacy-tui       serve the previous browser-rendered Ratatui frontend\n  --size COLSxROWS   terminal size for --legacy-tui (default 120x40)\n\nRemote mode runs as a detached background daemon. The production WebUI is served directly by Yeet and does not require a Node development server. Authentication is Remote-wide rather than workspace-bound; WebAuthn credentials remain scoped to the configured relying-party origin as required by WebAuthn. Network tunneling and port forwarding are intentionally outside Yeet."
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDaemonStatus {
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLaunchResult {
    pub status: RemoteDaemonStatus,
    pub already_running: bool,
}

struct RemoteDaemonPaths {
    socket: PathBuf,
    log: PathBuf,
    legacy_auth: PathBuf,
}

impl RemoteDaemonPaths {
    fn new(workspace: &Path) -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let directory = config.directory.join("remote");
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        let key = remote_workspace_key(workspace);
        Ok(Self {
            socket: directory.join(format!("{key}.sock")),
            log: directory.join(format!("{key}.log")),
            legacy_auth: directory.join(format!("{key}.auth.json")),
        })
    }
}

fn remote_auth_path() -> Result<PathBuf> {
    let config = ConfigStore::default();
    config.ensure()?;
    let directory = config.directory.join("remote");
    fs::create_dir_all(&directory)?;
    set_private_directory(&directory)?;
    Ok(directory.join("auth.json"))
}

fn remote_workspace_key(workspace: &Path) -> String {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = DefaultHasher::new();
    workspace.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RemoteAuthDocument {
    version: u32,
    key_hash: Option<String>,
    passkey_user_id: Option<WebauthnUuid>,
    passkeys: Vec<Passkey>,
}

impl Default for RemoteAuthDocument {
    fn default() -> Self {
        Self {
            version: 1,
            key_hash: None,
            passkey_user_id: None,
            passkeys: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAuthStatus {
    pub key_enabled: bool,
    pub passkey_count: usize,
}

fn load_remote_auth_document(workspace: &Path) -> Result<RemoteAuthDocument> {
    let auth_path = remote_auth_path()?;
    match fs::read_to_string(&auth_path) {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("decode remote authentication file {}", auth_path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Remote authentication used to be scoped to the daemon's startup
            // workspace. Migrate that document once so existing access keys and
            // passkeys keep working while authentication becomes Remote-wide.
            let legacy_path = RemoteDaemonPaths::new(workspace)?.legacy_auth;
            match fs::read_to_string(&legacy_path) {
                Ok(contents) => {
                    let document: RemoteAuthDocument = serde_json::from_str(&contents)
                        .with_context(|| {
                            format!(
                                "decode legacy remote authentication file {}",
                                legacy_path.display()
                            )
                        })?;
                    save_remote_auth_document(workspace, &document)?;
                    Ok(document)
                }
                Err(legacy_error) if legacy_error.kind() == io::ErrorKind::NotFound => {
                    Ok(RemoteAuthDocument::default())
                }
                Err(legacy_error) => Err(legacy_error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn save_remote_auth_document(_workspace: &Path, document: &RemoteAuthDocument) -> Result<()> {
    let auth_path = remote_auth_path()?;
    let parent = auth_path
        .parent()
        .ok_or_else(|| anyhow!("remote authentication path has no parent"))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        auth_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("auth"),
        uuid::Uuid::new_v4().simple()
    ));
    let bytes = serde_json::to_vec_pretty(document)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    replace_file(&temporary, &auth_path)?;
    set_private_file(&auth_path)?;
    Ok(())
}

pub fn remote_auth_status(workspace: &Path) -> Result<RemoteAuthStatus> {
    let document = load_remote_auth_document(workspace)?;
    Ok(RemoteAuthStatus {
        key_enabled: document.key_hash.is_some(),
        passkey_count: document.passkeys.len(),
    })
}

pub fn generate_remote_access_key(workspace: &Path) -> Result<String> {
    let key = format!(
        "yeet_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    set_remote_access_key(workspace, &key)?;
    Ok(key)
}

pub fn set_remote_access_key(workspace: &Path, key: &str) -> Result<()> {
    let key = key.trim();
    if key.len() < 12 {
        bail!("remote access key must be at least 12 characters");
    }
    let mut document = load_remote_auth_document(workspace)?;
    document.key_hash = Some(hash_remote_access_key(key)?);
    save_remote_auth_document(workspace, &document)?;
    notify_remote_auth_reload(workspace);
    Ok(())
}

fn hash_remote_access_key(key: &str) -> Result<String> {
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
        .map_err(|error| anyhow!("generate access-key salt: {error}"))?;
    Ok(Argon2::default()
        .hash_password(key.as_bytes(), &salt)
        .map_err(|error| anyhow!("hash remote access key: {error}"))?
        .to_string())
}

pub fn clear_remote_access_key(workspace: &Path) -> Result<()> {
    let mut document = load_remote_auth_document(workspace)?;
    document.key_hash = None;
    save_remote_auth_document(workspace, &document)?;
    notify_remote_auth_reload(workspace);
    Ok(())
}

pub fn clear_remote_passkeys(workspace: &Path) -> Result<()> {
    let mut document = load_remote_auth_document(workspace)?;
    document.passkeys.clear();
    document.passkey_user_id = None;
    save_remote_auth_document(workspace, &document)?;
    notify_remote_auth_reload(workspace);
    Ok(())
}

#[derive(Debug)]
struct PendingPasskeyRegistration {
    enrollment_token: String,
    state: PasskeyRegistration,
    expires_at: u64,
}

#[derive(Debug)]
struct PendingPasskeyAuthentication {
    state: PasskeyAuthentication,
    expires_at: u64,
}

#[derive(Debug)]
struct RemoteAuthState {
    document: RemoteAuthDocument,
    sessions: HashMap<String, u64>,
    enrollments: HashMap<String, u64>,
    registrations: HashMap<String, PendingPasskeyRegistration>,
    authentications: HashMap<String, PendingPasskeyAuthentication>,
}

#[derive(Clone)]
pub struct RemoteAuthRuntime {
    workspace: PathBuf,
    cookie_name: String,
    webauthn: Option<Arc<Webauthn>>,
    public_origin: Option<Url>,
    state: Arc<Mutex<RemoteAuthState>>,
}

impl RemoteAuthRuntime {
    fn for_workspace(
        workspace: &Path,
        options: &RemoteOptions,
        address: SocketAddr,
    ) -> Result<Self> {
        let document = load_remote_auth_document(workspace)?;
        Self::from_document(workspace.to_path_buf(), document, options, address)
    }

    fn from_document(
        workspace: PathBuf,
        document: RemoteAuthDocument,
        options: &RemoteOptions,
        address: SocketAddr,
    ) -> Result<Self> {
        let public_origin = remote_public_origin(options, address)?;
        let webauthn = if let Some(origin) = &public_origin {
            let rp_id = origin
                .domain()
                .ok_or_else(|| anyhow!("WebAuthn origin must use a hostname, not an IP address"))?;
            Some(Arc::new(
                WebauthnBuilder::new(rp_id, origin)
                    .map_err(|error| anyhow!("configure remote WebAuthn: {error}"))?
                    .rp_name("Yeet Remote")
                    .allow_any_port(rp_id == "localhost")
                    .build()
                    .map_err(|error| anyhow!("build remote WebAuthn configuration: {error}"))?,
            ))
        } else {
            None
        };
        if !document.passkeys.is_empty() && webauthn.is_none() {
            bail!(
                "Remote has passkeys configured; restart remote mode with --origin https://host when not using localhost"
            );
        }
        Ok(Self {
            // Authentication belongs to the Remote service, not to whichever
            // workspace a client selects after it connects.
            cookie_name: "yeet_remote_session".into(),
            workspace: if workspace.as_os_str().is_empty() {
                workspace
            } else {
                workspace.canonicalize().unwrap_or(workspace)
            },
            webauthn,
            public_origin,
            state: Arc::new(Mutex::new(RemoteAuthState {
                document,
                sessions: HashMap::new(),
                enrollments: HashMap::new(),
                registrations: HashMap::new(),
                authentications: HashMap::new(),
            })),
        })
    }

    fn disabled() -> Self {
        Self {
            workspace: PathBuf::new(),
            cookie_name: "yeet_remote_session".into(),
            webauthn: None,
            public_origin: None,
            state: Arc::new(Mutex::new(RemoteAuthState {
                document: RemoteAuthDocument::default(),
                sessions: HashMap::new(),
                enrollments: HashMap::new(),
                registrations: HashMap::new(),
                authentications: HashMap::new(),
            })),
        }
    }

    fn reload_document(&self) -> Result<()> {
        self.reload_document_inner(false)
    }

    fn refresh_document(&self) -> Result<()> {
        self.reload_document_inner(true)
    }

    fn reload_document_inner(&self, preserve_sessions: bool) -> Result<()> {
        if self.workspace.as_os_str().is_empty() {
            return Ok(());
        }
        let document = load_remote_auth_document(&self.workspace)?;
        let mut state = self.state.lock().unwrap();
        state.document = document;
        if !preserve_sessions {
            state.sessions.clear();
        }
        state.authentications.clear();
        Ok(())
    }

    fn required(&self) -> bool {
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state.enrollments.retain(|_, expires_at| *expires_at > now);
        state.document.key_hash.is_some()
            || !state.document.passkeys.is_empty()
            || !state.enrollments.is_empty()
    }

    fn methods(&self) -> (bool, bool) {
        let state = self.state.lock().unwrap();
        (
            state.document.key_hash.is_some(),
            !state.document.passkeys.is_empty() && self.webauthn.is_some(),
        )
    }

    fn is_authorized_headers(&self, headers: &axum::http::HeaderMap) -> bool {
        self.is_authorized_cookie_header(
            headers
                .get(axum::http::header::COOKIE)
                .and_then(|value| value.to_str().ok()),
        )
    }

    fn is_authorized_cookie_header(&self, cookie_header: Option<&str>) -> bool {
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state.enrollments.retain(|_, expires_at| *expires_at > now);
        if state.document.key_hash.is_none()
            && state.document.passkeys.is_empty()
            && state.enrollments.is_empty()
        {
            return true;
        }
        state.sessions.retain(|_, expires_at| *expires_at > now);
        let Some(token) = cookie_header.and_then(|header| {
            header.split(';').find_map(|pair| {
                let (candidate, value) = pair.trim().split_once('=')?;
                (candidate == self.cookie_name).then_some(value)
            })
        }) else {
            return false;
        };
        state
            .sessions
            .get(token)
            .is_some_and(|expires_at| *expires_at > now)
    }

    fn websocket_origin_allowed(&self, headers: &axum::http::HeaderMap) -> bool {
        self.browser_origin_allowed(headers, true)
    }

    fn http_auth_origin_allowed(&self, headers: &axum::http::HeaderMap) -> bool {
        self.browser_origin_allowed(headers, false)
    }

    fn browser_origin_allowed(
        &self,
        headers: &axum::http::HeaderMap,
        require_origin: bool,
    ) -> bool {
        let Some(origin) = headers
            .get(axum::http::header::ORIGIN)
            .and_then(|value| value.to_str().ok())
        else {
            return !require_origin;
        };
        let Ok(origin) = Url::parse(origin) else {
            return false;
        };
        if !matches!(origin.scheme(), "http" | "https")
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return false;
        }
        if let Some(expected) = self.public_origin.as_ref() {
            return origin == *expected;
        }
        let Some(host) = headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let Some(origin_host) = origin.host_str() else {
            return false;
        };
        let authority = match origin.port() {
            Some(port) => format!("{origin_host}:{port}"),
            None => origin_host.to_owned(),
        };
        authority.eq_ignore_ascii_case(host)
    }

    fn verify_access_key(&self, key: &str) -> Result<Option<String>> {
        let hash = self.state.lock().unwrap().document.key_hash.clone();
        let Some(hash) = hash else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&hash)
            .map_err(|error| anyhow!("decode remote access-key hash: {error}"))?;
        if Argon2::default()
            .verify_password(key.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        Ok(Some(self.issue_session()))
    }

    fn issue_session(&self) -> String {
        let token = random_token("session");
        self.state
            .lock()
            .unwrap()
            .sessions
            .insert(token.clone(), unix_time() + AUTH_SESSION_SECONDS);
        token
    }

    fn session_cookie(&self, token: &str) -> String {
        let secure = self
            .public_origin
            .as_ref()
            .is_some_and(|origin| origin.scheme() == "https");
        format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={AUTH_SESSION_SECONDS}{}",
            self.cookie_name,
            if secure { "; Secure" } else { "" }
        )
    }

    fn issue_enrollment(&self) -> Result<String> {
        let origin = self.public_origin.as_ref().ok_or_else(|| {
            anyhow!(
                "passkeys require a browser origin; restart remote mode with --origin https://host when not using localhost"
            )
        })?;
        if self.webauthn.is_none() {
            bail!("passkeys are unavailable for the configured remote origin");
        }
        let token = random_token("enroll");
        self.state
            .lock()
            .unwrap()
            .enrollments
            .insert(token.clone(), unix_time() + PASSKEY_ENROLL_SECONDS);
        Ok(format!(
            "{}/enroll?token={token}",
            origin.as_str().trim_end_matches('/')
        ))
    }

    fn enrollment_valid(&self, token: &str) -> bool {
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state.enrollments.retain(|_, expires_at| *expires_at > now);
        state
            .enrollments
            .get(token)
            .is_some_and(|expires_at| *expires_at > now)
    }

    fn begin_passkey_registration(&self, enrollment_token: &str) -> Result<serde_json::Value> {
        if !self.enrollment_valid(enrollment_token) {
            bail!("passkey enrollment token is invalid or expired");
        }
        let webauthn = self
            .webauthn
            .as_ref()
            .ok_or_else(|| anyhow!("passkeys are unavailable for this remote origin"))?;
        let (user_id, exclude_credentials) = {
            let mut state = self.state.lock().unwrap();
            let user_id = *state
                .document
                .passkey_user_id
                .get_or_insert_with(WebauthnUuid::new_v4);
            let exclude = (!state.document.passkeys.is_empty()).then(|| {
                state
                    .document
                    .passkeys
                    .iter()
                    .map(|passkey| passkey.cred_id().clone())
                    .collect::<Vec<CredentialID>>()
            });
            (user_id, exclude)
        };
        let (challenge, registration) = webauthn
            .start_passkey_registration(user_id, "yeet-remote", "Yeet Remote", exclude_credentials)
            .map_err(|error| anyhow!("start passkey registration: {error}"))?;
        let transaction = random_token("register");
        self.state.lock().unwrap().registrations.insert(
            transaction.clone(),
            PendingPasskeyRegistration {
                enrollment_token: enrollment_token.to_owned(),
                state: registration,
                expires_at: unix_time() + PASSKEY_ENROLL_SECONDS,
            },
        );
        Ok(json!({"transaction": transaction, "options": challenge}))
    }

    fn finish_passkey_registration(
        &self,
        transaction: &str,
        credential: &RegisterPublicKeyCredential,
    ) -> Result<String> {
        let pending = self
            .state
            .lock()
            .unwrap()
            .registrations
            .remove(transaction)
            .ok_or_else(|| anyhow!("passkey registration transaction is invalid or expired"))?;
        if pending.expires_at <= unix_time() || !self.enrollment_valid(&pending.enrollment_token) {
            bail!("passkey registration transaction is invalid or expired");
        }
        let webauthn = self
            .webauthn
            .as_ref()
            .ok_or_else(|| anyhow!("passkeys are unavailable for this remote origin"))?;
        let passkey = webauthn
            .finish_passkey_registration(credential, &pending.state)
            .map_err(|error| anyhow!("finish passkey registration: {error}"))?;
        {
            let mut state = self.state.lock().unwrap();
            if state
                .document
                .passkeys
                .iter()
                .any(|existing| existing.cred_id() == passkey.cred_id())
            {
                bail!("this passkey is already registered");
            }
            state.document.passkeys.push(passkey);
            state.enrollments.remove(&pending.enrollment_token);
            save_remote_auth_document(&self.workspace, &state.document)?;
        }
        notify_other_remote_auth_refresh(&self.workspace);
        Ok(self.issue_session())
    }

    fn begin_passkey_authentication(&self) -> Result<serde_json::Value> {
        let passkeys = self.state.lock().unwrap().document.passkeys.clone();
        if passkeys.is_empty() {
            bail!("no passkeys are registered");
        }
        let webauthn = self
            .webauthn
            .as_ref()
            .ok_or_else(|| anyhow!("passkeys are unavailable for this remote origin"))?;
        let (challenge, authentication) = webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(|error| anyhow!("start passkey authentication: {error}"))?;
        let transaction = random_token("auth");
        self.state.lock().unwrap().authentications.insert(
            transaction.clone(),
            PendingPasskeyAuthentication {
                state: authentication,
                expires_at: unix_time() + PASSKEY_ENROLL_SECONDS,
            },
        );
        Ok(json!({"transaction": transaction, "options": challenge}))
    }

    fn finish_passkey_authentication(
        &self,
        transaction: &str,
        credential: &PublicKeyCredential,
        enrollment_token: Option<&str>,
    ) -> Result<String> {
        let pending = self
            .state
            .lock()
            .unwrap()
            .authentications
            .remove(transaction)
            .ok_or_else(|| anyhow!("passkey authentication transaction is invalid or expired"))?;
        if pending.expires_at <= unix_time() {
            bail!("passkey authentication transaction is invalid or expired");
        }
        let webauthn = self
            .webauthn
            .as_ref()
            .ok_or_else(|| anyhow!("passkeys are unavailable for this remote origin"))?;
        let result = webauthn
            .finish_passkey_authentication(credential, &pending.state)
            .map_err(|error| anyhow!("finish passkey authentication: {error}"))?;
        let changed = {
            let mut state = self.state.lock().unwrap();
            if let Some(token) = enrollment_token {
                let now = unix_time();
                state.enrollments.retain(|_, expires_at| *expires_at > now);
                if !state.enrollments.contains_key(token) {
                    bail!("passkey enrollment token is invalid or expired");
                }
            }
            let mut matched = false;
            let mut changed = false;
            for passkey in &mut state.document.passkeys {
                if let Some(updated) = passkey.update_credential(&result) {
                    matched = true;
                    changed |= updated;
                }
            }
            if !matched {
                bail!("authenticated passkey is not registered for this Remote service");
            }
            if let Some(token) = enrollment_token {
                state.enrollments.remove(token);
            }
            if changed {
                save_remote_auth_document(&self.workspace, &state.document)?;
            }
            changed
        };
        if changed {
            notify_other_remote_auth_refresh(&self.workspace);
        }
        Ok(self.issue_session())
    }
}

fn remote_public_origin(options: &RemoteOptions, address: SocketAddr) -> Result<Option<Url>> {
    let value = if let Some(origin) = options.origin.as_deref() {
        origin.to_owned()
    } else if address.ip().is_loopback() {
        format!("http://localhost:{}", address.port())
    } else {
        return Ok(None);
    };
    Ok(Some(parse_remote_origin(&value)?))
}

fn normalize_remote_origin(value: &str) -> Result<String> {
    Ok(parse_remote_origin(value)?.to_string())
}

fn parse_remote_origin(value: &str) -> Result<Url> {
    let origin = Url::parse(value).with_context(|| format!("invalid remote origin: {value}"))?;
    if !matches!(origin.scheme(), "http" | "https") {
        bail!("remote WebAuthn origin must use http or https");
    }
    let Some(domain) = origin.domain() else {
        bail!("remote WebAuthn origin must use a hostname such as localhost or example.com");
    };
    if origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
    {
        bail!("remote WebAuthn --origin must contain only scheme, hostname, and optional port");
    }
    if origin.scheme() != "https" && domain != "localhost" {
        bail!("remote WebAuthn origins must use https, except for localhost");
    }
    Ok(origin)
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

pub fn launch_remote_daemon(
    workspace: &Path,
    options: &RemoteOptions,
) -> Result<RemoteLaunchResult> {
    if let Some(status) = remote_daemon_status(workspace)? {
        if let Some(running_legacy_tui) = remote_daemon_legacy_tui(workspace)?
            && running_legacy_tui != options.legacy_tui
        {
            bail!(
                "Yeet remote is already running with the {} frontend; stop it before switching to {}",
                if running_legacy_tui {
                    "legacy TUI"
                } else {
                    "WebUI"
                },
                if options.legacy_tui {
                    "legacy TUI"
                } else {
                    "WebUI"
                }
            );
        }
        let requested = normalize_requested_address(&options.bind);
        if requested
            .as_deref()
            .is_some_and(|value| !status.address.ends_with(value))
        {
            bail!(
                "Yeet remote daemon is already running at {}; stop it before changing --bind",
                status.address
            );
        }
        if let Some(requested_origin) = options.origin.as_deref() {
            let requested_origin = normalize_remote_origin(requested_origin)?;
            let running_origin = remote_daemon_origin(workspace)?.ok_or_else(|| {
                anyhow!(
                    "Yeet remote daemon is already running but its WebAuthn origin cannot be inspected; stop it before changing --origin"
                )
            })?;
            if running_origin != requested_origin {
                bail!(
                    "Yeet remote daemon is already using WebAuthn origin {running_origin}; stop it before changing --origin to {requested_origin}"
                );
            }
        }
        return Ok(RemoteLaunchResult {
            status,
            already_running: true,
        });
    }

    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let paths = RemoteDaemonPaths::new(&workspace)?;
    let _ = fs::remove_file(&paths.socket);
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("open remote daemon log {}", paths.log.display()))?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(executable);
    command
        .arg("__remote-daemon")
        .arg(&workspace)
        .arg("--bind")
        .arg(&options.bind)
        .arg("--size")
        .arg(format!("{}x{}", options.cols, options.rows));
    if options.legacy_tui {
        command.arg("--legacy-tui");
    }
    if let Some(origin) = &options.origin {
        command.arg("--origin").arg(origin);
    }
    command
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    configure_detached(&mut command);
    let mut child = command.spawn().context("start Yeet remote daemon")?;

    for _ in 0..DAEMON_START_RETRIES {
        if let Some(status) = remote_daemon_status(&workspace)? {
            return Ok(RemoteLaunchResult {
                status,
                already_running: false,
            });
        }
        if let Some(exit) = child.try_wait()? {
            bail!(
                "Yeet remote daemon exited during startup ({exit}); see {}",
                paths.log.display()
            );
        }
        thread::sleep(DAEMON_START_DELAY);
    }
    bail!(
        "Yeet remote daemon did not become ready; see {}",
        paths.log.display()
    )
}

pub fn remote_daemon_status(workspace: &Path) -> Result<Option<RemoteDaemonStatus>> {
    let Some(response) = remote_control_request(workspace, "status")? else {
        return Ok(None);
    };
    let address = response.trim();
    if address.is_empty() || address == "unknown" {
        return Ok(None);
    }
    Ok(Some(RemoteDaemonStatus {
        address: address.to_owned(),
    }))
}

pub fn remote_daemon_browser_url(workspace: &Path, status: &RemoteDaemonStatus) -> Result<String> {
    Ok(remote_daemon_origin(workspace)?.unwrap_or_else(|| status.address.clone()))
}

fn remote_control_request(workspace: &Path, command: &str) -> Result<Option<String>> {
    let paths = RemoteDaemonPaths::new(workspace)?;
    let mut stream = match connect_local(&paths.socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(DAEMON_CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(DAEMON_CONTROL_TIMEOUT))?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(Some(response.trim().to_owned()))
}

fn remote_daemon_origin(workspace: &Path) -> Result<Option<String>> {
    let Some(response) = remote_control_request(workspace, "origin")? else {
        return Ok(None);
    };
    let value = response.trim();
    if value.is_empty() || matches!(value, "none" | "unknown") {
        return Ok(None);
    }
    Ok(Some(value.to_owned()))
}

fn remote_daemon_legacy_tui(workspace: &Path) -> Result<Option<bool>> {
    let Some(response) = remote_control_request(workspace, "frontend")? else {
        return Ok(None);
    };
    match response.trim() {
        "webui" => Ok(Some(false)),
        "legacy-tui" => Ok(Some(true)),
        _ => Ok(None),
    }
}

fn notify_remote_auth_reload(workspace: &Path) {
    let _ = remote_control_request(workspace, "auth-reload");
    let exclude = RemoteDaemonPaths::new(workspace)
        .ok()
        .map(|paths| paths.socket);
    notify_remote_auth_command("auth-reload", exclude.as_deref());
}

fn notify_other_remote_auth_refresh(workspace: &Path) {
    let exclude = RemoteDaemonPaths::new(workspace)
        .ok()
        .map(|paths| paths.socket);
    notify_remote_auth_command("auth-refresh", exclude.as_deref());
}

fn notify_remote_auth_command(command: &str, exclude: Option<&Path>) {
    let Ok(config) = (|| -> Result<ConfigStore> {
        let config = ConfigStore::default();
        config.ensure()?;
        Ok(config)
    })() else {
        return;
    };
    let directory = config.directory.join("remote");
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sock") {
            continue;
        }
        if exclude.is_some_and(|excluded| excluded == path) {
            continue;
        }
        let Ok(mut stream) = connect_local(&path) else {
            continue;
        };
        let _ = stream.set_write_timeout(Some(DAEMON_CONTROL_TIMEOUT));
        let _ = stream.write_all(command.as_bytes());
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

pub fn request_remote_passkey_enrollment(workspace: &Path) -> Result<String> {
    let response = remote_control_request(workspace, "passkey-add")?
        .ok_or_else(|| anyhow!("Yeet remote UI is not running for this workspace"))?;
    if let Some(error) = response.strip_prefix("error:") {
        bail!(error.trim().to_owned());
    }
    if !response.starts_with("http://") && !response.starts_with("https://") {
        bail!("remote daemon returned an invalid passkey enrollment URL");
    }
    Ok(response)
}

pub fn stop_remote_daemon(workspace: &Path) -> Result<bool> {
    let paths = RemoteDaemonPaths::new(workspace)?;
    let mut stream = match connect_local(&paths.socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_write_timeout(Some(DAEMON_CONTROL_TIMEOUT))?;
    stream.write_all(b"stop\n")?;
    stream.flush()?;
    drop(stream);
    for _ in 0..DAEMON_START_RETRIES {
        if remote_daemon_status(workspace)?.is_none() {
            return Ok(true);
        }
        thread::sleep(DAEMON_START_DELAY);
    }
    bail!(
        "Yeet remote daemon did not stop; see {}",
        paths.log.display()
    );
}

fn normalize_requested_address(value: &str) -> Option<String> {
    let address = value.parse::<SocketAddr>().ok()?;
    (address.port() != 0).then(|| address.to_string())
}

pub struct RemoteDaemonControl {
    stop: Arc<AtomicBool>,
    socket: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl RemoteDaemonControl {
    pub fn start(
        workspace: &Path,
        address: SocketAddr,
        auth: Arc<RemoteAuthRuntime>,
        legacy_tui: bool,
    ) -> Result<Self> {
        let paths = RemoteDaemonPaths::new(workspace)?;
        if paths.socket.exists() {
            let _ = fs::remove_file(&paths.socket);
        }
        let listener = bind_local(&paths.socket)
            .with_context(|| format!("bind remote control socket {}", paths.socket.display()))?;
        set_private_file(&paths.socket)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("yeet-remote-control".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(DAEMON_CONTROL_TIMEOUT));
                            let request = stream
                                .try_clone()
                                .ok()
                                .and_then(|reader| {
                                    let mut reader = BufReader::new(reader);
                                    let mut line = String::new();
                                    reader.read_line(&mut line).ok().map(|_| line)
                                })
                                .unwrap_or_default();
                            match request.trim() {
                                "status" => {
                                    let _ = writeln!(stream, "http://{address}");
                                    let _ = stream.flush();
                                }
                                "origin" => {
                                    let value = auth
                                        .public_origin
                                        .as_ref()
                                        .map(Url::as_str)
                                        .unwrap_or("none");
                                    let _ = writeln!(stream, "{value}");
                                    let _ = stream.flush();
                                }
                                "frontend" => {
                                    let _ = writeln!(
                                        stream,
                                        "{}",
                                        if legacy_tui { "legacy-tui" } else { "webui" }
                                    );
                                    let _ = stream.flush();
                                }
                                "stop" => {
                                    thread_stop.store(true, Ordering::Release);
                                    let _ = stream.write_all(b"stopping\n");
                                    let _ = stream.flush();
                                }
                                "auth-reload" => {
                                    match auth.reload_document() {
                                        Ok(()) => {
                                            let _ = stream.write_all(b"ok\n");
                                        }
                                        Err(error) => {
                                            let _ = writeln!(stream, "error:{error}");
                                        }
                                    }
                                    let _ = stream.flush();
                                }
                                "auth-refresh" => {
                                    match auth.refresh_document() {
                                        Ok(()) => {
                                            let _ = stream.write_all(b"ok\n");
                                        }
                                        Err(error) => {
                                            let _ = writeln!(stream, "error:{error}");
                                        }
                                    }
                                    let _ = stream.flush();
                                }
                                "passkey-add" => {
                                    match auth.issue_enrollment() {
                                        Ok(url) => {
                                            let _ = writeln!(stream, "{url}");
                                        }
                                        Err(error) => {
                                            let _ = writeln!(stream, "error:{error}");
                                        }
                                    }
                                    let _ = stream.flush();
                                }
                                _ => {
                                    let _ = stream.write_all(b"unknown\n");
                                    let _ = stream.flush();
                                }
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(20)),
                    }
                }
            })
            .context("start Yeet remote control listener")?;
        Ok(Self {
            stop,
            socket: paths.socket,
            thread: Some(thread),
        })
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

impl Drop for RemoteDaemonControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

fn parse_size(value: &str) -> Result<(u16, u16)> {
    let (cols, rows) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow!("remote size must use COLSxROWS, for example 120x40"))?;
    let cols = cols
        .parse::<u16>()
        .with_context(|| format!("invalid remote column count: {cols}"))?;
    let rows = rows
        .parse::<u16>()
        .with_context(|| format!("invalid remote row count: {rows}"))?;
    Ok(clamp_size(cols, rows))
}

fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.clamp(MIN_COLS, MAX_COLS),
        rows.clamp(MIN_ROWS, MAX_ROWS),
    )
}

#[derive(Debug)]
pub enum RemoteInput {
    Key(KeyEvent),
    Text(String),
    Mouse(MouseEvent),
    Resize { cols: u16, rows: u16 },
}

impl RemoteInput {
    pub fn normalized(self) -> Self {
        match self {
            Self::Resize { cols, rows } => {
                let (cols, rows) = clamp_size(cols, rows);
                Self::Resize { cols, rows }
            }
            other => other,
        }
    }
}

#[derive(Clone)]
struct FrameSnapshot {
    sequence: u64,
    width: u16,
    height: u16,
    html: String,
}

impl FrameSnapshot {
    fn new(width: u16, height: u16) -> Self {
        Self {
            sequence: 1,
            width,
            height,
            html: "Starting Yeet…".into(),
        }
    }
}

pub struct RemoteServer {
    address: SocketAddr,
    input_rx: Receiver<RemoteInput>,
    frame: Arc<Mutex<FrameSnapshot>>,
    auth: Arc<RemoteAuthRuntime>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RemoteServer {
    pub fn start(options: &RemoteOptions) -> Result<Self> {
        Self::start_inner(options, None, None)
    }

    pub fn start_for_workspace(options: &RemoteOptions, workspace: &Path) -> Result<Self> {
        Self::start_inner(options, Some(workspace), None)
    }

    #[cfg(test)]
    fn start_with_auth_document(
        options: &RemoteOptions,
        document: RemoteAuthDocument,
    ) -> Result<Self> {
        Self::start_inner(options, None, Some(document))
    }

    fn start_inner(
        options: &RemoteOptions,
        workspace: Option<&Path>,
        auth_document: Option<RemoteAuthDocument>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(&options.bind)
            .with_context(|| format!("failed to bind Yeet remote UI to {}", options.bind))?;
        listener
            .set_nonblocking(true)
            .context("failed to configure Yeet remote listener")?;
        let address = listener.local_addr()?;
        let auth = Arc::new(match (workspace, auth_document) {
            (Some(workspace), None) => {
                RemoteAuthRuntime::for_workspace(workspace, options, address)?
            }
            (None, Some(document)) => {
                RemoteAuthRuntime::from_document(PathBuf::new(), document, options, address)?
            }
            (None, None) => RemoteAuthRuntime::disabled(),
            (Some(_), Some(_)) => unreachable!("workspace and test auth document are exclusive"),
        });
        let semantic_workspace = workspace
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let (input_tx, input_rx) = mpsc::channel();
        let frame = Arc::new(Mutex::new(FrameSnapshot::new(options.cols, options.rows)));
        let running = Arc::new(AtomicBool::new(true));
        let server_frame = Arc::clone(&frame);
        let server_running = Arc::clone(&running);
        let server_auth = Arc::clone(&auth);
        let legacy_tui = options.legacy_tui;
        let thread = thread::Builder::new()
            .name("yeet-remote-http".into())
            .spawn(move || {
                server::serve(
                    listener,
                    input_tx,
                    server_frame,
                    server_auth,
                    semantic_workspace,
                    legacy_tui,
                    server_running,
                )
            })
            .context("failed to start Yeet remote HTTP server")?;

        Ok(Self {
            address,
            input_rx,
            frame,
            auth,
            running,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn auth_handle(&self) -> Arc<RemoteAuthRuntime> {
        Arc::clone(&self.auth)
    }

    pub fn try_recv(&self) -> Option<RemoteInput> {
        match self.input_rx.try_recv() {
            Ok(input) => Some(input.normalized()),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub fn publish(&self, buffer: &Buffer) {
        let html = render_buffer(buffer);
        let width = buffer.area.width;
        let height = buffer.area.height;
        let Ok(mut frame) = self.frame.lock() else {
            return;
        };
        if frame.html == html && frame.width == width && frame.height == height {
            return;
        }
        frame.sequence = frame.sequence.wrapping_add(1).max(1);
        frame.width = width;
        frame.height = height;
        frame.html = html;
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, Deserialize)]
struct KeyLoginRequest {
    key: String,
}

#[derive(Debug, Deserialize)]
struct EnrollmentBeginRequest {
    token: String,
}

#[derive(Debug, Deserialize)]
struct RegistrationFinishRequest {
    transaction: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Deserialize)]
struct AuthenticationFinishRequest {
    transaction: String,
    credential: PublicKeyCredential,
    #[serde(default)]
    enrollment_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireInput {
    Key {
        key: String,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        shift: bool,
    },
    Text {
        text: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Wheel {
        delta_y: i32,
    },
}

impl WireInput {
    fn into_remote_input(self) -> Option<RemoteInput> {
        match self {
            Self::Key {
                key,
                ctrl,
                alt,
                shift,
            } => {
                let mut modifiers = KeyModifiers::NONE;
                if ctrl {
                    modifiers |= KeyModifiers::CONTROL;
                }
                if alt {
                    modifiers |= KeyModifiers::ALT;
                }
                if shift {
                    modifiers |= KeyModifiers::SHIFT;
                }
                let code = browser_key_code(&key, shift)?;
                Some(RemoteInput::Key(KeyEvent::new(code, modifiers)))
            }
            Self::Text { text } if !text.is_empty() => Some(RemoteInput::Text(text)),
            Self::Text { .. } => None,
            Self::Resize { cols, rows } => Some(RemoteInput::Resize { cols, rows }),
            Self::Wheel { delta_y } if delta_y != 0 => Some(RemoteInput::Mouse(MouseEvent {
                kind: if delta_y < 0 {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })),
            Self::Wheel { .. } => None,
        }
    }
}

fn browser_key_code(key: &str, shift: bool) -> Option<KeyCode> {
    let code = match key {
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "ArrowLeft" => KeyCode::Left,
        "ArrowRight" => KeyCode::Right,
        "ArrowUp" => KeyCode::Up,
        "ArrowDown" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Tab" if shift => KeyCode::BackTab,
        "Tab" => KeyCode::Tab,
        "Insert" => KeyCode::Insert,
        _ if key.starts_with('F') => key
            .strip_prefix('F')
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|value| (1..=12).contains(value))
            .map(KeyCode::F)?,
        _ => {
            let mut chars = key.chars();
            let value = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(value)
        }
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Style},
    };
    use std::net::TcpStream;

    #[test]
    fn parses_remote_invocation_and_clamps_size() {
        let arguments = vec![
            "remote".into(),
            "--bind".into(),
            "127.0.0.1:9000".into(),
            "--size".into(),
            "999x2".into(),
        ];
        let options = RemoteOptions::parse(&arguments).unwrap().unwrap();
        assert_eq!(options.bind, "127.0.0.1:9000");
        assert_eq!((options.cols, options.rows), (MAX_COLS, MIN_ROWS));
        assert!(!options.legacy_tui);
    }

    #[test]
    fn parses_legacy_remote_fallback_flag() {
        let arguments = vec!["remote".into(), "--legacy-tui".into()];
        let options = RemoteOptions::parse(&arguments).unwrap().unwrap();
        assert!(options.legacy_tui);
    }

    #[test]
    fn ignores_non_remote_commands() {
        let arguments = vec!["doctor".into()];
        assert!(RemoteOptions::parse(&arguments).unwrap().is_none());
    }

    #[test]
    fn browser_keys_map_to_crossterm_keys() {
        assert_eq!(browser_key_code("ArrowUp", false), Some(KeyCode::Up));
        assert_eq!(browser_key_code("Tab", true), Some(KeyCode::BackTab));
        assert_eq!(browser_key_code("x", false), Some(KeyCode::Char('x')));
        assert_eq!(browser_key_code("F12", false), Some(KeyCode::F(12)));
        assert_eq!(browser_key_code("F99", false), None);
    }

    #[test]
    fn remote_server_serves_frames_and_receives_browser_input() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: true,
        };
        let server = RemoteServer::start(&options).unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        buffer.set_string(0, 0, "Yeet remote", Style::default().fg(Color::Cyan));
        server.publish(&buffer);

        let frame_response = http_request(
            server.address(),
            "GET /api/frame HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            frame_response.starts_with("HTTP/1.1 200 OK"),
            "unexpected frame response: {frame_response:?}"
        );
        assert!(frame_response.contains("Yeet remote"));

        let body = r#"{"type":"key","key":"ArrowUp","ctrl":false,"alt":false,"shift":false}"#;
        let request = format!(
            "POST /api/input HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nX-Yeet-Remote: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let input_response = http_request(server.address(), &request);
        assert!(input_response.starts_with("HTTP/1.1 204 No Content"));
        assert!(matches!(
            server.try_recv(),
            Some(RemoteInput::Key(KeyEvent {
                code: KeyCode::Up,
                ..
            }))
        ));
    }

    #[test]
    fn remote_options_accept_workspace_path_and_origin() {
        let arguments = vec![
            "remote".into(),
            "/tmp/example-workspace".into(),
            "--origin".into(),
            "https://yeet.example.test".into(),
        ];
        let options = RemoteOptions::parse(&arguments).unwrap().unwrap();
        assert_eq!(
            options.workspace.as_deref(),
            Some(Path::new("/tmp/example-workspace"))
        );
        assert_eq!(options.origin.as_deref(), Some("https://yeet.example.test"));
    }

    #[test]
    fn access_key_protects_remote_frame_until_login() {
        let hash = hash_remote_access_key("correct horse battery").unwrap();
        assert!(!hash.contains("correct horse battery"));
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: true,
        };
        let server = RemoteServer::start_with_auth_document(&options, document).unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        buffer.set_string(0, 0, "protected", Style::default());
        server.publish(&buffer);

        let denied = http_request(
            server.address(),
            "GET /api/frame HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(denied.starts_with("HTTP/1.1 401 Unauthorized"));

        let body = r#"{"key":"correct horse battery"}"#;
        let login = http_request(
            server.address(),
            &format!(
                "POST /api/auth/key HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        assert!(login.starts_with("HTTP/1.1 200 OK"), "{login:?}");
        let cookie = login
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(": ")?;
                name.eq_ignore_ascii_case("set-cookie").then_some(value)
            })
            .and_then(|value| value.split(';').next())
            .unwrap();
        let allowed = http_request(
            server.address(),
            &format!(
                "GET /api/frame HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(allowed.starts_with("HTTP/1.1 200 OK"), "{allowed:?}");
        assert!(allowed.contains("protected"));
    }

    #[test]
    fn websocket_upgrade_rejects_unauthenticated_browser() {
        let hash = hash_remote_access_key("secret").unwrap();
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let server = RemoteServer::start_with_auth_document(&options, document).unwrap();
        let response = http_request(
            server.address(),
            "GET /api/ws HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        );
        assert!(
            response.starts_with("HTTP/1.1 401 Unauthorized"),
            "{response:?}"
        );
    }

    #[test]
    fn websocket_upgrade_rejects_wrong_origin_after_authentication() {
        let hash = hash_remote_access_key("secret").unwrap();
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let server = RemoteServer::start_with_auth_document(&options, document).unwrap();
        let body = r#"{"key":"secret"}"#;
        let login = http_request(
            server.address(),
            &format!(
                "POST /api/auth/key HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        let cookie = login
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(": ")?;
                name.eq_ignore_ascii_case("set-cookie").then_some(value)
            })
            .and_then(|value| value.split(';').next())
            .unwrap();
        let response = http_request(
            server.address(),
            &format!(
                "GET /api/ws HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403 Forbidden"),
            "{response:?}"
        );
    }

    #[test]
    fn browser_auth_post_rejects_cross_origin_request() {
        let hash = hash_remote_access_key("secret").unwrap();
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let server = RemoteServer::start_with_auth_document(&options, document).unwrap();
        let body = r#"{"key":"secret"}"#;
        let response = http_request(
            server.address(),
            &format!(
                "POST /api/auth/key HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403 Forbidden"),
            "{response:?}"
        );
    }

    #[test]
    fn https_auth_cookie_is_http_only_strict_and_secure() {
        let hash = hash_remote_access_key("correct horse battery").unwrap();
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: Some("https://remote.example.test".into()),
            workspace: None,
            legacy_tui: false,
        };
        let auth = RemoteAuthRuntime::from_document(
            PathBuf::new(),
            document,
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let session = auth
            .verify_access_key("correct horse battery")
            .unwrap()
            .unwrap();
        let cookie = auth.session_cookie(&session);
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("Path=/"));
    }

    #[test]
    fn remote_auth_cookie_is_not_workspace_scoped() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: Some("https://remote.example.test".into()),
            workspace: None,
            legacy_tui: false,
        };
        let left = RemoteAuthRuntime::from_document(
            PathBuf::from("/tmp/remote-workspace-a"),
            RemoteAuthDocument::default(),
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let right = RemoteAuthRuntime::from_document(
            PathBuf::from("/tmp/remote-workspace-b"),
            RemoteAuthDocument::default(),
            &options,
            "127.0.0.1:17332".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(left.cookie_name, "yeet_remote_session");
        assert_eq!(right.cookie_name, left.cookie_name);
    }

    #[test]
    fn expired_auth_session_is_rejected() {
        let hash = hash_remote_access_key("expiry-test-key").unwrap();
        let document = RemoteAuthDocument {
            key_hash: Some(hash),
            ..RemoteAuthDocument::default()
        };
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let auth = RemoteAuthRuntime::from_document(
            PathBuf::new(),
            document,
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let session = auth.verify_access_key("expiry-test-key").unwrap().unwrap();
        auth.state
            .lock()
            .unwrap()
            .sessions
            .insert(session.clone(), unix_time());
        let cookie = format!("{}={session}", auth.cookie_name);
        assert!(!auth.is_authorized_cookie_header(Some(&cookie)));
        assert!(!auth.state.lock().unwrap().sessions.contains_key(&session));
    }

    #[test]
    fn remote_server_shutdown_does_not_wait_for_open_websocket() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let server = RemoteServer::start(&options).unwrap();
        let address = server.address();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = format!(
            "GET /api/ws HTTP/1.1\r\nHost: localhost:{}\r\nOrigin: http://localhost:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: yeet.remote.v1\r\n\r\n",
            address.port(),
            address.port()
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        let mut chunk = [0_u8; 4096];
        while !String::from_utf8_lossy(&response).contains("\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => response.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("read WebSocket upgrade response: {error}"),
            }
        }
        let response = String::from_utf8_lossy(&response);
        assert!(
            response.starts_with("HTTP/1.1 101 Switching Protocols"),
            "{response}"
        );

        let started = std::time::Instant::now();
        drop(server);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "RemoteServer shutdown waited for an upgraded browser socket"
        );
    }

    #[test]
    fn passkey_enrollment_issues_real_webauthn_challenge() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let auth = RemoteAuthRuntime::from_document(
            PathBuf::new(),
            RemoteAuthDocument::default(),
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let url = auth.issue_enrollment().unwrap();
        assert!(auth.required());
        let unauthenticated = axum::http::HeaderMap::new();
        assert!(!auth.is_authorized_headers(&unauthenticated));
        let token = url.split("token=").nth(1).unwrap();
        let challenge = auth.begin_passkey_registration(token).unwrap();
        assert!(challenge["transaction"].as_str().is_some());
        assert!(
            challenge["options"]["publicKey"]["challenge"]
                .as_str()
                .is_some()
        );
        assert_eq!(challenge["options"]["publicKey"]["rp"]["id"], "localhost");
    }

    #[test]
    fn expired_passkey_enrollment_is_rejected_and_pruned() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: None,
            workspace: None,
            legacy_tui: false,
        };
        let auth = RemoteAuthRuntime::from_document(
            PathBuf::new(),
            RemoteAuthDocument::default(),
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let url = auth.issue_enrollment().unwrap();
        let token = url.split("token=").nth(1).unwrap().to_owned();
        auth.state
            .lock()
            .unwrap()
            .enrollments
            .insert(token.clone(), unix_time());

        assert!(!auth.enrollment_valid(&token));
        assert!(auth.begin_passkey_registration(&token).is_err());
        assert!(!auth.required());
        assert!(!auth.state.lock().unwrap().enrollments.contains_key(&token));
    }

    #[test]
    fn passkey_origin_supports_https_reverse_proxy_host() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: Some("https://remote.example.test".into()),
            workspace: None,
            legacy_tui: false,
        };
        let auth = RemoteAuthRuntime::from_document(
            PathBuf::new(),
            RemoteAuthDocument::default(),
            &options,
            "127.0.0.1:17331".parse().unwrap(),
        )
        .unwrap();
        let url = auth.issue_enrollment().unwrap();
        assert!(url.starts_with("https://remote.example.test/enroll?token="));
        let token = url.split("token=").nth(1).unwrap();
        let challenge = auth.begin_passkey_registration(token).unwrap();
        assert_eq!(
            challenge["options"]["publicKey"]["rp"]["id"],
            "remote.example.test"
        );
    }

    #[test]
    fn passkey_origin_rejects_insecure_non_localhost() {
        let options = RemoteOptions {
            bind: "127.0.0.1:0".into(),
            cols: 80,
            rows: 24,
            origin: Some("http://remote.example.test".into()),
            workspace: None,
            legacy_tui: false,
        };
        assert!(
            RemoteAuthRuntime::from_document(
                PathBuf::new(),
                RemoteAuthDocument::default(),
                &options,
                "127.0.0.1:17331".parse().unwrap(),
            )
            .is_err()
        );
    }

    fn http_request(address: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        // Access-key tests exercise production Argon2 parameters. Under a full
        // parallel test run that can legitimately take longer than one second,
        // so the client timeout must not turn CPU contention into an empty HTTP
        // response and a false server failure.
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => response.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("read test HTTP response: {error}"),
            }
        }
        String::from_utf8(response).unwrap()
    }
}
