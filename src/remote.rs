mod assets;
pub mod protocol;
mod render;
mod server;
mod websocket;
use render::render_buffer;

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
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
        set_private_file, tracked_children_snapshot,
    },
};

const DEFAULT_BIND: &str = "0.0.0.0:7331";
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 12;
const MAX_COLS: u16 = 300;
const MAX_ROWS: u16 = 120;
const REMOTE_STOP_RETRIES: usize = 80;
const REMOTE_STOP_DELAY: Duration = Duration::from_millis(50);
const REMOTE_CONTROL_TIMEOUT: Duration = Duration::from_millis(600);
const REMOTE_BACKGROUND_START_RETRIES: usize = 100;
const REMOTE_BACKGROUND_START_DELAY: Duration = Duration::from_millis(50);
const AUTH_SESSION_SECONDS: u64 = 12 * 60 * 60;
const PASSKEY_ENROLL_SECONDS: u64 = 10 * 60;
const MAX_AUTH_SESSIONS: usize = 256;
const MAX_PASSKEY_ENROLLMENTS: usize = 32;
const MAX_PENDING_PASSKEY_REGISTRATIONS: usize = 32;
const MAX_PENDING_PASSKEY_AUTHENTICATIONS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Configures the remote HTTP/WebSocket frontend server and authentication boundary.
pub struct RemoteOptions {
    pub bind: String,
    pub cols: u16,
    pub rows: u16,
    pub origin: Option<String>,
    pub background: bool,
    pub legacy_tui: bool,
}

impl Default for RemoteOptions {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.into(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            origin: None,
            background: false,
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
                "--background" => {
                    options.background = true;
                }
                "--legacy-tui" => {
                    options.legacy_tui = true;
                }
                value => bail!("unknown remote option: {value}\n\n{}", remote_help()),
            }
            index += 1;
        }
        Ok(Some(options))
    }
}

pub fn remote_help() -> &'static str {
    "Yeet remote mode\n\n  yeet remote [--background] [--bind ADDRESS] [--origin URL] [--legacy-tui]\n  yeet remote status\n  yeet remote stop\n  yeet remote auth status\n  yeet remote auth key generate|set|clear\n  yeet remote auth passkey add|clear\n  yeet --remote [--background] [--bind ADDRESS] [--origin URL]\n\nDefaults:\n  client workspace: restored per browser client, otherwise home directory\n  --bind 0.0.0.0:7331\n  frontend: semantic WebUI\n\nOptions:\n  --background       detach one Remote process; no launchd/systemd service is installed\n  --legacy-tui       serve the previous browser-rendered Ratatui frontend\n  --size COLSxROWS   terminal size for --legacy-tui (default 120x40)\n\n`yeet remote` is workspace-neutral: the Remote server itself owns no project directory and never derives one from the shell current directory. Each WebUI client restores or selects its own workspace; a new client starts at the home directory and may switch to any valid workspace path. `--background` only detaches the current Remote process; it does not register a supervisor or restart policy, so a killed Remote stays dead. The production WebUI is served directly by Yeet and does not require a Node development server. WebAuthn credentials remain scoped to the configured relying-party origin as required by WebAuthn. Network tunneling and port forwarding are intentionally outside Yeet."
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatus {
    pub address: String,
    pub owned_children: Vec<String>,
}

struct RemoteControlPaths {
    socket: PathBuf,
    legacy_auth: PathBuf,
}

impl RemoteControlPaths {
    fn new() -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let directory = config.directory.join("remote");
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        Ok(Self {
            socket: directory.join("control.sock"),
            legacy_auth: directory.join("daemon.auth.json"),
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

fn load_remote_auth_document() -> Result<RemoteAuthDocument> {
    let auth_path = remote_auth_path()?;
    match fs::read_to_string(&auth_path) {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("decode remote authentication file {}", auth_path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Migrate the previous Remote auth filename once so existing access
            // keys and passkeys keep working with the process-wide auth file.
            let legacy_path = RemoteControlPaths::new()?.legacy_auth;
            match fs::read_to_string(&legacy_path) {
                Ok(contents) => {
                    let document: RemoteAuthDocument = serde_json::from_str(&contents)
                        .with_context(|| {
                            format!(
                                "decode legacy remote authentication file {}",
                                legacy_path.display()
                            )
                        })?;
                    save_remote_auth_document(&document)?;
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

fn save_remote_auth_document(document: &RemoteAuthDocument) -> Result<()> {
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

pub fn remote_auth_status() -> Result<RemoteAuthStatus> {
    let document = load_remote_auth_document()?;
    Ok(RemoteAuthStatus {
        key_enabled: document.key_hash.is_some(),
        passkey_count: document.passkeys.len(),
    })
}

pub fn generate_remote_access_key() -> Result<String> {
    let key = format!(
        "yeet_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    set_remote_access_key(&key)?;
    Ok(key)
}

pub fn set_remote_access_key(key: &str) -> Result<()> {
    let key = key.trim();
    if key.len() < 12 {
        bail!("remote access key must be at least 12 characters");
    }
    let mut document = load_remote_auth_document()?;
    document.key_hash = Some(hash_remote_access_key(key)?);
    save_remote_auth_document(&document)?;
    notify_remote_auth_reload();
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

pub fn clear_remote_access_key() -> Result<()> {
    let mut document = load_remote_auth_document()?;
    document.key_hash = None;
    save_remote_auth_document(&document)?;
    notify_remote_auth_reload();
    Ok(())
}

pub fn clear_remote_passkeys() -> Result<()> {
    let mut document = load_remote_auth_document()?;
    document.passkeys.clear();
    document.passkey_user_id = None;
    save_remote_auth_document(&document)?;
    notify_remote_auth_reload();
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

fn ensure_remote_auth_capacity(kind: &str, current: usize, maximum: usize) -> Result<()> {
    if current >= maximum {
        bail!("Remote {kind} capacity reached ({maximum}); retry after an older entry expires");
    }
    Ok(())
}

#[derive(Clone)]
pub struct RemoteAuthRuntime {
    cookie_name: String,
    webauthn: Option<Arc<Webauthn>>,
    public_origin: Option<Url>,
    state: Arc<Mutex<RemoteAuthState>>,
}

impl RemoteAuthRuntime {
    fn for_remote(options: &RemoteOptions, address: SocketAddr) -> Result<Self> {
        let document = load_remote_auth_document()?;
        Self::from_document(document, options, address)
    }

    fn from_document(
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
            // Authentication belongs to Remote, not to whichever workspace
            // a client selects after it connects.
            cookie_name: "yeet_remote_session".into(),
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

    fn reload_document(&self) -> Result<()> {
        self.reload_document_inner(false)
    }

    fn refresh_document(&self) -> Result<()> {
        self.reload_document_inner(true)
    }

    fn reload_document_inner(&self, preserve_sessions: bool) -> Result<()> {
        let document = load_remote_auth_document()?;
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

    fn websocket_origin_allowed(&self, _headers: &axum::http::HeaderMap) -> bool {
        true
    }

    fn http_auth_origin_allowed(&self, _headers: &axum::http::HeaderMap) -> bool {
        true
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
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state.sessions.retain(|_, expires_at| *expires_at > now);
        if state.sessions.len() >= MAX_AUTH_SESSIONS
            && let Some(oldest) = state
                .sessions
                .iter()
                .min_by_key(|(_, expires_at)| *expires_at)
                .map(|(token, _)| token.clone())
        {
            state.sessions.remove(&oldest);
        }
        state
            .sessions
            .insert(token.clone(), now + AUTH_SESSION_SECONDS);
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
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state.enrollments.retain(|_, expires_at| *expires_at > now);
        ensure_remote_auth_capacity(
            "passkey enrollment",
            state.enrollments.len(),
            MAX_PASSKEY_ENROLLMENTS,
        )?;
        state
            .enrollments
            .insert(token.clone(), now + PASSKEY_ENROLL_SECONDS);
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
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state
            .registrations
            .retain(|_, pending| pending.expires_at > now);
        ensure_remote_auth_capacity(
            "pending passkey registration",
            state.registrations.len(),
            MAX_PENDING_PASSKEY_REGISTRATIONS,
        )?;
        state.registrations.insert(
            transaction.clone(),
            PendingPasskeyRegistration {
                enrollment_token: enrollment_token.to_owned(),
                state: registration,
                expires_at: now + PASSKEY_ENROLL_SECONDS,
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
            save_remote_auth_document(&state.document)?;
        }
        notify_other_remote_auth_refresh();
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
        let now = unix_time();
        let mut state = self.state.lock().unwrap();
        state
            .authentications
            .retain(|_, pending| pending.expires_at > now);
        ensure_remote_auth_capacity(
            "pending passkey authentication",
            state.authentications.len(),
            MAX_PENDING_PASSKEY_AUTHENTICATIONS,
        )?;
        state.authentications.insert(
            transaction.clone(),
            PendingPasskeyAuthentication {
                state: authentication,
                expires_at: now + PASSKEY_ENROLL_SECONDS,
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
                bail!("authenticated passkey is not registered for this Remote");
            }
            if let Some(token) = enrollment_token {
                state.enrollments.remove(token);
            }
            if changed {
                save_remote_auth_document(&state.document)?;
            }
            changed
        };
        if changed {
            notify_other_remote_auth_refresh();
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

pub fn remote_status() -> Result<Option<RemoteStatus>> {
    let Some(response) = remote_control_request("status")? else {
        return Ok(None);
    };
    let address = response.trim();
    if address.is_empty() || address == "unknown" {
        return Ok(None);
    }
    let owned_children = remote_control_request("children")?
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Some(RemoteStatus {
        address: address.to_owned(),
        owned_children,
    }))
}

pub fn remote_browser_url(status: &RemoteStatus) -> Result<String> {
    Ok(remote_origin()?.unwrap_or_else(|| status.address.clone()))
}

pub fn remote_pid() -> Result<Option<u32>> {
    let Some(response) = remote_control_request("pid")? else {
        return Ok(None);
    };
    let value = response.trim();
    if value.is_empty() || value == "unknown" {
        return Ok(None);
    }
    let pid = value.parse::<u32>().context("parse remote PID")?;
    Ok(Some(pid))
}

fn resolve_executable() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("YEET_EXE").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
    }
    let current = std::env::current_exe().context("locate Yeet executable")?;
    if let Some(parent) = current.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some("deps") {
            let candidate = parent
                .parent()
                .unwrap_or(parent)
                .join(if cfg!(windows) { "yeet.exe" } else { "yeet" });
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Ok(current)
}

pub fn start_remote_background(options: &RemoteOptions) -> Result<RemoteStatus> {
    if options.legacy_tui {
        bail!("--background is only supported by the semantic Remote WebUI");
    }
    if remote_status()?.is_some() {
        bail!("Yeet Remote is already running");
    }

    let executable = resolve_executable()?;
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let paths = RemoteControlPaths::new()?;
    let directory = paths
        .socket
        .parent()
        .ok_or_else(|| anyhow!("Remote control path has no parent directory"))?;
    let log_path = directory.join("background.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open Remote background log {}", log_path.display()))?;
    set_private_file(&log_path)?;
    let log_err = log.try_clone()?;

    let mut command = Command::new(executable);
    command
        .arg("remote")
        .arg("--bind")
        .arg(&options.bind)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    if let Some(origin) = &options.origin {
        command.arg("--origin").arg(origin);
    }
    configure_detached(&mut command);

    let mut child = command.spawn().context("start Yeet Remote in background")?;
    for _ in 0..REMOTE_BACKGROUND_START_RETRIES {
        if let Some(status) = remote_status()? {
            return Ok(status);
        }
        if let Some(exit) = child.try_wait()? {
            bail!(
                "Yeet Remote background process exited during startup ({exit}); see {}",
                log_path.display()
            );
        }
        thread::sleep(REMOTE_BACKGROUND_START_DELAY);
    }

    bail!(
        "Yeet Remote background process did not become ready; see {}",
        log_path.display()
    )
}

fn remote_control_disconnected(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

fn remote_control_request(command: &str) -> Result<Option<String>> {
    let paths = RemoteControlPaths::new()?;
    let mut stream = match connect_local(&paths.socket) {
        Ok(stream) => stream,
        Err(error) if remote_control_disconnected(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(REMOTE_CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(REMOTE_CONTROL_TIMEOUT))?;

    if let Err(error) = (|| -> io::Result<()> {
        stream.write_all(command.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        stream.shutdown(Shutdown::Write)?;
        Ok(())
    })() {
        if remote_control_disconnected(&error) {
            return Ok(None);
        }
        return Err(error.into());
    }

    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        if remote_control_disconnected(&error) {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(response.trim().to_owned()))
}

fn remote_origin() -> Result<Option<String>> {
    let Some(response) = remote_control_request("origin")? else {
        return Ok(None);
    };
    let value = response.trim();
    if value.is_empty() || matches!(value, "none" | "unknown") {
        return Ok(None);
    }
    Ok(Some(value.to_owned()))
}

fn notify_remote_auth_reload() {
    let _ = remote_control_request("auth-reload");
    let exclude = RemoteControlPaths::new().ok().map(|paths| paths.socket);
    notify_remote_auth_command("auth-reload", exclude.as_deref());
}

fn notify_other_remote_auth_refresh() {
    let exclude = RemoteControlPaths::new().ok().map(|paths| paths.socket);
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
        let _ = stream.set_write_timeout(Some(REMOTE_CONTROL_TIMEOUT));
        let _ = stream.write_all(command.as_bytes());
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

pub fn request_remote_passkey_enrollment() -> Result<String> {
    let response = remote_control_request("passkey-add")?
        .ok_or_else(|| anyhow!("Yeet Remote is not running"))?;
    if let Some(error) = response.strip_prefix("error:") {
        bail!(error.trim().to_owned());
    }
    if !response.starts_with("http://") && !response.starts_with("https://") {
        bail!("Yeet Remote returned an invalid passkey enrollment URL");
    }
    Ok(response)
}

pub fn stop_remote() -> Result<bool> {
    let paths = RemoteControlPaths::new()?;
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
    stream.set_write_timeout(Some(REMOTE_CONTROL_TIMEOUT))?;
    stream.write_all(b"stop\n")?;
    stream.flush()?;
    drop(stream);
    for _ in 0..REMOTE_STOP_RETRIES {
        if remote_status()?.is_none() {
            return Ok(true);
        }
        thread::sleep(REMOTE_STOP_DELAY);
    }
    bail!("Yeet Remote did not stop");
}

pub struct RemoteControl {
    stop: Arc<AtomicBool>,
    socket: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl RemoteControl {
    pub fn start(
        address: SocketAddr,
        auth: Arc<RemoteAuthRuntime>,
        legacy_tui: bool,
    ) -> Result<Self> {
        let paths = RemoteControlPaths::new()?;
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
                            let _ = stream.set_read_timeout(Some(REMOTE_CONTROL_TIMEOUT));
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
                                "pid" => {
                                    let _ = writeln!(stream, "{}", std::process::id());
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
                                "children" => {
                                    let children = tracked_children_snapshot()
                                        .into_iter()
                                        .map(|child| format!("{}:{}", child.role, child.pid))
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    let _ = writeln!(stream, "{children}");
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

impl Drop for RemoteControl {
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
        Self::start_inner(options, None)
    }

    #[cfg(test)]
    fn start_with_auth_document(
        options: &RemoteOptions,
        document: RemoteAuthDocument,
    ) -> Result<Self> {
        Self::start_inner(options, Some(document))
    }

    fn start_inner(
        options: &RemoteOptions,
        auth_document: Option<RemoteAuthDocument>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(&options.bind)
            .with_context(|| format!("failed to bind Yeet remote UI to {}", options.bind))?;
        listener
            .set_nonblocking(true)
            .context("failed to configure Yeet remote listener")?;
        let address = listener.local_addr()?;
        let auth = Arc::new(match auth_document {
            Some(document) => RemoteAuthRuntime::from_document(document, options, address)?,
            None => RemoteAuthRuntime::for_remote(options, address)?,
        });
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
mod lifecycle_tests;
