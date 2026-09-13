use std::{
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    auth::{AuthMode, AuthStore},
    current_workspace,
    http::{HttpOptions, HttpServer, ensure_bind_available},
    print_stdio_config, resolve_workspace_path, serve_stdio,
};
use crate::{
    config::ConfigStore,
    platform::{
        bind_local, configure_detached, connect_local, set_private_directory, set_private_file,
    },
};

const DEFAULT_PORT: u16 = 7332;
const DEFAULT_BIND: &str = "127.0.0.1";
const CONTROL_TIMEOUT: Duration = Duration::from_millis(750);
const START_RETRIES: usize = 80;
const START_DELAY: Duration = Duration::from_millis(50);
const SUPERVISOR_RESTART_DELAY: Duration = Duration::from_millis(250);
const BINARY_WATCH_INTERVAL: Duration = Duration::from_secs(1);

pub(super) const HELP: &str = r#"Yeet MCP server

Usage:
  yeet mcpserver start [--port PORT] [--bind HOST] [--workspace PATH] [--public-url URL] [--auth key|oauth|none]
  yeet mcpserver run [--port PORT] [--bind HOST] [--workspace PATH] [--public-url URL] [--auth key|oauth|none]
  yeet mcpserver restart [--port PORT] [same options as start]
  yeet mcpserver status [--port PORT]
  yeet mcpserver stop [--port PORT]
  yeet mcpserver list
  yeet mcpserver config [--port PORT]
  yeet mcpserver auth status [--port PORT]
  yeet mcpserver auth mode key|oauth|none [--port PORT]
  yeet mcpserver auth key generate|set|clear [--port PORT]
  yeet mcpserver auth oauth enable|disable|status [--port PORT]
  yeet mcpserver stdio [WORKSPACE|--workspace PATH]
  yeet mcpserver stdio-config [WORKSPACE|--workspace PATH]

Defaults:
  bind: 127.0.0.1
  port: 7332
  auth: key (a key is generated automatically on first start)

HTTP MCP is served at /mcp. OAuth can be enabled alongside the existing primary
auth mode and implements protected-resource and authorization-server discovery,
authorization-code + PKCE, CIMD, and legacy dynamic client registration. The
legacy `--auth oauth` mode remains supported. For reverse proxies or non-loopback
binds, set --public-url to the externally reachable MCP URL (for example
https://host.example/mcp). Multiple daemon instances can run on different ports;
daemon config and authentication are isolated per port."#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DaemonConfig {
    version: u32,
    bind_host: String,
    port: u16,
    default_workspace: PathBuf,
    public_url: Option<String>,
}

impl DaemonConfig {
    fn default_for(port: u16) -> Result<Self> {
        Ok(Self {
            version: 1,
            bind_host: DEFAULT_BIND.into(),
            port,
            default_workspace: current_workspace()?,
            public_url: None,
        })
    }

    fn public_url(&self) -> Result<Option<Url>> {
        self.public_url
            .as_deref()
            .map(Url::parse)
            .transpose()
            .context("invalid persisted MCP public URL")
    }
}

#[derive(Default)]
struct ServerOverrides {
    port: Option<u16>,
    bind_host: Option<String>,
    workspace: Option<String>,
    public_url: Option<String>,
    auth_mode: Option<AuthMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonStatus {
    address: String,
    port: u16,
    public_url: String,
    default_workspace: String,
    auth_mode: String,
    key_enabled: bool,
    #[serde(default)]
    oauth_enabled: bool,
    log: String,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    binary_sha256: Option<String>,
}

struct DaemonPaths {
    control: PathBuf,
    supervisor_lock: PathBuf,
    log: PathBuf,
    config: PathBuf,
}

impl DaemonPaths {
    fn new(port: u16) -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let directory = config.directory.join("mcpserver");
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        Ok(Self {
            control: directory.join(format!("daemon-{port}.sock")),
            supervisor_lock: directory.join(format!("supervisor-{port}.lock")),
            log: directory.join(format!("daemon-{port}.log")),
            config: directory.join(format!("daemon-{port}.json")),
        })
    }
}

pub(super) fn run_cli(args: &[String]) -> Result<String> {
    if args.is_empty() {
        return start_command(&[]);
    }
    let subcommand = args.first().map(String::as_str).unwrap_or("start");
    match subcommand {
        "help" | "-h" | "--help" => Ok(HELP.into()),
        "stdio" => {
            serve_stdio(&args[1..])?;
            Ok(String::new())
        }
        "stdio-config" => {
            print_stdio_config(&args[1..])?;
            Ok(String::new())
        }
        "start" => start_command(&args[1..]),
        "run" => run_command(&args[1..]),
        "restart" => restart_command(&args[1..]),
        "status" => status_command(&args[1..]),
        "stop" => stop_command(&args[1..]),
        "list" => list_command(&args[1..]),
        "config" => config_command(&args[1..]),
        "auth" => auth_command(&args[1..]),
        "__supervisor" => supervisor_child_command(&args[1..]),
        "__daemon" => daemon_child_command(&args[1..]),
        other => bail!("unknown mcpserver command: {other}\n\n{HELP}"),
    }
}

fn start_command(args: &[String]) -> Result<String> {
    let overrides = parse_server_overrides(args)?;
    let port = overrides.port.unwrap_or(DEFAULT_PORT);
    let has_runtime_overrides = overrides.bind_host.is_some()
        || overrides.workspace.is_some()
        || overrides.public_url.is_some()
        || overrides.auth_mode.is_some();
    if let Some(status) = daemon_status(port)? {
        if daemon_uses_current_binary(&status)? {
            if has_runtime_overrides {
                bail!(
                    "Yeet MCP daemon is already running on port {port}; use `yeet mcpserver restart --port {port} ...` to change its configuration"
                );
            }
            return Ok(format_status(&status, true));
        }
        eprintln!(
            "yeet mcpserver: running daemon on port {port} uses an older executable; restarting it before continuing"
        );
        let _ = stop_daemon(port)?;
    }
    let paths = DaemonPaths::new(port)?;
    if supervisor_is_running(&paths)? {
        if has_runtime_overrides {
            bail!(
                "Yeet MCP supervisor is already active on port {port}; use `yeet mcpserver restart --port {port} ...` to change its configuration"
            );
        }
        let mut still_running = true;
        for _ in 0..START_RETRIES {
            if let Some(status) = daemon_status(port)? {
                return Ok(format_launch(&status, None, true));
            }
            if !supervisor_is_running(&paths)? {
                still_running = false;
                break;
            }
            thread::sleep(START_DELAY);
        }
        if still_running {
            bail!(
                "Yeet MCP supervisor is already active on port {port}, but its daemon did not become ready; see {}",
                paths.log.display()
            );
        }
    }
    let mut config = load_daemon_config(port)?.unwrap_or(DaemonConfig::default_for(port)?);
    apply_overrides(&mut config, &overrides)?;
    let generated_key = prepare_auth(&config, overrides.auth_mode)?;
    validate_security(&config)?;
    save_daemon_config(&config)?;

    ensure_bind_available(&config.bind_host, config.port).with_context(|| {
        format!(
            "cannot start Yeet MCP daemon because {}:{} is already in use",
            config.bind_host, config.port
        )
    })?;
    let _ = fs::remove_file(&paths.control);
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("open MCP daemon log {}", paths.log.display()))?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(executable);
    command
        .arg("mcpserver")
        .arg("__supervisor")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    configure_detached(&mut command);
    let mut child = command.spawn().context("start Yeet MCP daemon")?;
    for _ in 0..START_RETRIES {
        if let Some(status) = daemon_status(port)? {
            return Ok(format_launch(&status, generated_key.as_deref(), false));
        }
        if let Some(exit) = child.try_wait()? {
            if let Some(status) = daemon_status(port)? {
                return Ok(format_launch(&status, generated_key.as_deref(), true));
            }
            if supervisor_is_running(&paths)? {
                thread::sleep(START_DELAY);
                continue;
            }
            bail!(
                "Yeet MCP daemon exited during startup ({exit}); see {}",
                paths.log.display()
            );
        }
        thread::sleep(START_DELAY);
    }
    bail!(
        "Yeet MCP daemon did not become ready; see {}",
        paths.log.display()
    )
}

fn run_command(args: &[String]) -> Result<String> {
    let overrides = parse_server_overrides(args)?;
    let port = overrides.port.unwrap_or(DEFAULT_PORT);
    let paths = DaemonPaths::new(port)?;
    if daemon_status(port)?.is_some() || supervisor_is_running(&paths)? {
        bail!("an MCP daemon is already managed on port {port}; stop it before foreground run");
    }
    let mut config = load_daemon_config(port)?.unwrap_or(DaemonConfig::default_for(port)?);
    apply_overrides(&mut config, &overrides)?;
    let generated_key = prepare_auth(&config, overrides.auth_mode)?;
    validate_security(&config)?;
    save_daemon_config(&config)?;
    if let Some(key) = generated_key {
        eprintln!("Generated MCP access/authorization key (store it now):\n{key}");
    }
    run_daemon(port)?;
    Ok(String::new())
}

fn restart_command(args: &[String]) -> Result<String> {
    let overrides = parse_server_overrides(args)?;
    let port = overrides.port.unwrap_or(DEFAULT_PORT);
    let _ = stop_daemon(port)?;
    start_command(args)
}

fn status_command(args: &[String]) -> Result<String> {
    let port = parse_port_only(args)?;
    match daemon_status(port)? {
        Some(status) => Ok(format_status(&status, false)),
        None => Ok(format!("Yeet MCP daemon is not running on port {port}")),
    }
}

fn stop_command(args: &[String]) -> Result<String> {
    let port = parse_port_only(args)?;
    if stop_daemon(port)? {
        Ok(format!("Stopped Yeet MCP daemon on port {port}"))
    } else {
        Ok(format!("Yeet MCP daemon is not running on port {port}"))
    }
}

fn list_command(args: &[String]) -> Result<String> {
    if !args.is_empty() {
        bail!("Usage: yeet mcpserver list");
    }
    let directory = ConfigStore::default().directory.join("mcpserver");
    if !directory.exists() {
        return Ok("No configured MCP daemons".into());
    }
    let mut ports = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(value) = name
            .strip_prefix("daemon-")
            .and_then(|value| value.strip_suffix(".json"))
        else {
            continue;
        };
        if let Ok(port) = value.parse::<u16>() {
            ports.push(port);
        }
    }
    ports.sort_unstable();
    ports.dedup();
    if ports.is_empty() {
        return Ok("No configured MCP daemons".into());
    }
    let mut lines = Vec::new();
    for port in ports {
        if let Some(status) = daemon_status(port)? {
            lines.push(format!(
                "{}\trunning\t{}\t{}",
                port, status.address, status.auth_mode
            ));
        } else if let Some(config) = load_daemon_config(port)? {
            lines.push(format!(
                "{}\tstopped\t{}:{}",
                port, config.bind_host, config.port
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn config_command(args: &[String]) -> Result<String> {
    let port = parse_port_only(args)?;
    let config = load_daemon_config(port)?.unwrap_or(DaemonConfig::default_for(port)?);
    let auth = AuthStore::new(port)?.status()?;
    Ok(serde_json::to_string_pretty(&json!({
        "bind": config.bind_host,
        "port": config.port,
        "workspace": config.default_workspace,
        "publicUrl": config.public_url,
        "auth": {
            "mode":auth.mode.as_str(),
            "keyEnabled":auth.key_enabled,
            "oauthEnabled":auth.oauth_enabled
        },
        "running": daemon_status(port)?.is_some(),
        "log": DaemonPaths::new(port)?.log,
    }))?)
}

fn auth_command(args: &[String]) -> Result<String> {
    let Some(category) = args.first().map(String::as_str) else {
        bail!("Usage: yeet mcpserver auth [status|mode|key|oauth] ...");
    };
    match category {
        "status" => {
            let port = parse_port_only(&args[1..])?;
            let status = AuthStore::new(port)?.status()?;
            Ok(format!(
                "Port: {port}\nMode: {}\nAccess key: {}\nOAuth: {}",
                status.mode.as_str(),
                if status.key_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if status.oauth_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ))
        }
        "mode" => {
            let mode = args.get(1).ok_or_else(|| {
                anyhow!("Usage: yeet mcpserver auth mode key|oauth|none [--port PORT]")
            })?;
            let mode = AuthMode::parse(mode)?;
            let port = parse_port_only(&args[2..])?;
            AuthStore::new(port)?.set_mode(mode)?;
            Ok(format!(
                "MCP authentication mode on port {port}: {}",
                mode.as_str()
            ))
        }
        "key" => {
            let action = args.get(1).map(String::as_str).ok_or_else(|| {
                anyhow!("Usage: yeet mcpserver auth key generate|set|clear [--port PORT]")
            })?;
            let port = parse_port_only(&args[2..])?;
            let store = AuthStore::new(port)?;
            match action {
                "generate" => {
                    let key = store.generate_key()?;
                    Ok(format!(
                        "MCP access/authorization key for port {port}:\n{key}\nStore this key now; Yeet only keeps its Argon2 hash."
                    ))
                }
                "set" => {
                    let key = read_access_key()?;
                    store.set_key(&key)?;
                    Ok(format!(
                        "MCP access/authorization key updated for port {port}"
                    ))
                }
                "clear" => {
                    store.clear_key()?;
                    Ok(format!(
                        "MCP access key removed and auth disabled on port {port}"
                    ))
                }
                _ => bail!("Usage: yeet mcpserver auth key generate|set|clear [--port PORT]"),
            }
        }
        "oauth" => {
            let action = args.get(1).map(String::as_str).ok_or_else(|| {
                anyhow!("Usage: yeet mcpserver auth oauth enable|disable|status [--port PORT]")
            })?;
            let port = parse_port_only(&args[2..])?;
            let store = AuthStore::new(port)?;
            match action {
                "enable" => {
                    let config =
                        load_daemon_config(port)?.unwrap_or(DaemonConfig::default_for(port)?);
                    validate_oauth_transport(&config)?;
                    store.set_oauth_enabled(true)?;
                    Ok(format!(
                        "OAuth enabled alongside {} auth on port {port}",
                        store.status()?.mode.as_str()
                    ))
                }
                "disable" => {
                    store.set_oauth_enabled(false)?;
                    Ok(format!("Additive OAuth disabled on port {port}"))
                }
                "status" => Ok(format!(
                    "OAuth on port {port}: {}",
                    if store.status()?.oauth_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )),
                _ => bail!("Usage: yeet mcpserver auth oauth enable|disable|status [--port PORT]"),
            }
        }
        _ => bail!("Usage: yeet mcpserver auth [status|mode|key|oauth] ..."),
    }
}

fn daemon_child_command(args: &[String]) -> Result<String> {
    let port = parse_port_only(args)?;
    run_daemon(port)?;
    Ok(String::new())
}

fn supervisor_child_command(args: &[String]) -> Result<String> {
    let port = parse_port_only(args)?;
    let paths = DaemonPaths::new(port)?;
    let _supervisor = SupervisorLock::acquire(&paths.supervisor_lock, port)?;
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    loop {
        let config = load_daemon_config(port)?
            .ok_or_else(|| anyhow!("MCP daemon config for port {port} does not exist"))?;
        ensure_bind_available(&config.bind_host, config.port).with_context(|| {
            format!(
                "Yeet MCP supervisor cannot bind {}:{}; another process already owns the endpoint",
                config.bind_host, config.port
            )
        })?;
        let status = Command::new(&executable)
            .arg("mcpserver")
            .arg("__daemon")
            .arg("--port")
            .arg(port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("start supervised Yeet MCP daemon")?;
        if status.success() {
            return Ok(String::new());
        }
        eprintln!(
            "yeet mcpserver: daemon on port {port} exited unexpectedly ({status}); restarting"
        );
        thread::sleep(SUPERVISOR_RESTART_DELAY);
    }
}

struct SupervisorLock {
    file: fs::File,
}

impl SupervisorLock {
    fn acquire(path: &Path, port: u16) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open MCP supervisor lock {}", path.display()))?;
        set_private_file(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file }),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                bail!("another Yeet MCP supervisor is already active on port {port}")
            }
            Err(error) => Err(error).context("lock Yeet MCP supervisor"),
        }
    }
}

impl Drop for SupervisorLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn supervisor_is_running(paths: &DaemonPaths) -> Result<bool> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.supervisor_lock)
        .with_context(|| {
            format!(
                "open MCP supervisor lock {}",
                paths.supervisor_lock.display()
            )
        })?;
    set_private_file(&paths.supervisor_lock)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            file.unlock()?;
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(true),
        Err(error) => Err(error).context("inspect Yeet MCP supervisor lock"),
    }
}

fn run_daemon(port: u16) -> Result<()> {
    let config = load_daemon_config(port)?
        .ok_or_else(|| anyhow!("MCP daemon config for port {port} does not exist"))?;
    validate_security(&config)?;
    let auth_store = AuthStore::new(port)?;
    let server = HttpServer::bind(
        HttpOptions {
            bind_host: config.bind_host.clone(),
            port: config.port,
            default_workspace: config.default_workspace.clone(),
            public_url: config.public_url()?,
        },
        auth_store.clone(),
    )?;
    let paths = DaemonPaths::new(port)?;
    let auth = auth_store.status()?;
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    let startup_sha256 = binary_sha256(&executable)?;
    let binary_stamp = binary_file_stamp(&executable)?;
    let status = DaemonStatus {
        address: server.address().to_string(),
        port,
        public_url: server.public_url().to_string(),
        default_workspace: config.default_workspace.display().to_string(),
        auth_mode: auth.mode.as_str().into(),
        key_enabled: auth.key_enabled,
        oauth_enabled: auth.oauth_enabled,
        log: paths.log.display().to_string(),
        pid: Some(std::process::id()),
        binary_sha256: Some(startup_sha256.clone()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let control = DaemonControl::start(port, status, Arc::clone(&stop))?;
    let restart_for_binary_change = Arc::new(AtomicBool::new(false));
    let watch_stop = Arc::clone(&stop);
    let watch_restart = Arc::clone(&restart_for_binary_change);
    let watcher = thread::Builder::new()
        .name(format!("yeet-mcp-binary-watch-{port}"))
        .spawn(move || {
            let mut observed_stamp = binary_stamp;
            while !watch_stop.load(Ordering::Acquire) {
                thread::sleep(BINARY_WATCH_INTERVAL);
                if watch_stop.load(Ordering::Acquire) {
                    break;
                }
                let stamp = match binary_file_stamp(&executable) {
                    Ok(stamp) => stamp,
                    Err(error) => {
                        eprintln!(
                            "yeet mcpserver: cannot inspect executable for replacement: {error}"
                        );
                        continue;
                    }
                };
                if stamp == observed_stamp {
                    continue;
                }
                match binary_sha256(&executable) {
                    Ok(current_sha256) if current_sha256 != startup_sha256 => {
                        eprintln!(
                            "yeet mcpserver: executable changed on disk; restarting daemon on port {port}"
                        );
                        watch_restart.store(true, Ordering::Release);
                        watch_stop.store(true, Ordering::Release);
                        break;
                    }
                    Ok(_) => observed_stamp = stamp,
                    Err(error) => eprintln!(
                        "yeet mcpserver: cannot hash replaced executable for verification: {error}"
                    ),
                }
            }
        })
        .context("start MCP executable change watcher")?;
    let result = server.serve(Arc::clone(&stop));
    stop.store(true, Ordering::Release);
    let _ = watcher.join();
    drop(control);
    if restart_for_binary_change.load(Ordering::Acquire) {
        bail!("Yeet MCP executable changed on disk; supervised restart requested");
    }
    result
}

fn prepare_auth(config: &DaemonConfig, requested: Option<AuthMode>) -> Result<Option<String>> {
    let store = AuthStore::new(config.port)?;
    let mut status = store.status()?;
    let target = requested.unwrap_or(status.mode);
    let generated = if matches!(target, AuthMode::Key | AuthMode::Oauth) && !status.key_enabled {
        let key = store.generate_key()?;
        status = store.status()?;
        Some(key)
    } else {
        None
    };
    if status.mode != target {
        store.set_mode(target)?;
    }
    Ok(generated)
}

fn validate_security(config: &DaemonConfig) -> Result<()> {
    let auth = AuthStore::new(config.port)?.status()?;
    if auth.mode == AuthMode::None && !is_loopback_host(&config.bind_host) {
        bail!("unauthenticated MCP daemons may only bind to localhost/loopback");
    }
    if auth.oauth_enabled {
        validate_oauth_transport(config)?;
    }
    if matches!(auth.mode, AuthMode::Key | AuthMode::Oauth) && !auth.key_enabled {
        bail!("MCP authentication requires an access key");
    }
    if auth.oauth_enabled && !auth.key_enabled {
        bail!("OAuth requires an MCP access key for authorization approval");
    }
    Ok(())
}

fn validate_oauth_transport(config: &DaemonConfig) -> Result<()> {
    if !is_loopback_host(&config.bind_host) && config.public_url.is_none() {
        bail!("OAuth on a non-loopback bind requires --public-url https://host.example/mcp");
    }
    if let Some(public_url) = config.public_url()?
        && public_url.scheme() != "https"
        && !public_url.host_str().is_some_and(is_loopback_host)
    {
        bail!("OAuth public URLs must use HTTPS except for localhost/loopback testing");
    }
    Ok(())
}

fn apply_overrides(config: &mut DaemonConfig, overrides: &ServerOverrides) -> Result<()> {
    if let Some(port) = overrides.port {
        config.port = port;
    }
    if let Some(bind) = &overrides.bind_host {
        config.bind_host = bind.clone();
    }
    if let Some(workspace) = overrides.workspace.as_deref() {
        config.default_workspace = resolve_workspace_path(&current_workspace()?, workspace)?;
    } else {
        config.default_workspace = config
            .default_workspace
            .canonicalize()
            .with_context(|| format!("resolve workspace {}", config.default_workspace.display()))?;
    }
    if let Some(public_url) = &overrides.public_url {
        let url = Url::parse(public_url).context("invalid --public-url")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("--public-url must use http or https");
        }
        config.public_url = Some(url.to_string());
    }
    Ok(())
}

fn parse_server_overrides(args: &[String]) -> Result<ServerOverrides> {
    let mut result = ServerOverrides::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--port" => {
                index += 1;
                result.port = Some(parse_port_value(args.get(index))?);
            }
            "--bind" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow!("--bind requires a host"))?;
                if value.trim().is_empty() {
                    bail!("--bind host cannot be empty");
                }
                result.bind_host = Some(value.trim().to_owned());
            }
            "--workspace" => {
                index += 1;
                result.workspace = Some(
                    args.get(index)
                        .ok_or_else(|| anyhow!("--workspace requires a path"))?
                        .clone(),
                );
            }
            "--public-url" => {
                index += 1;
                result.public_url = Some(
                    args.get(index)
                        .ok_or_else(|| anyhow!("--public-url requires a URL"))?
                        .clone(),
                );
            }
            "--auth" => {
                index += 1;
                result.auth_mode =
                    Some(AuthMode::parse(args.get(index).ok_or_else(|| {
                        anyhow!("--auth requires key, oauth, or none")
                    })?)?);
            }
            value => bail!("unknown mcpserver option: {value}"),
        }
        index += 1;
    }
    Ok(result)
}

fn parse_port_only(args: &[String]) -> Result<u16> {
    if args.is_empty() {
        return Ok(DEFAULT_PORT);
    }
    if args.len() != 2 || args[0] != "--port" {
        bail!("expected optional --port PORT");
    }
    parse_port_value(args.get(1))
}

fn parse_port_value(value: Option<&String>) -> Result<u16> {
    let value = value.ok_or_else(|| anyhow!("--port requires a value"))?;
    let port: u16 = value
        .parse()
        .map_err(|_| anyhow!("invalid port: {value}"))?;
    if port == 0 {
        bail!("daemon port must be between 1 and 65535");
    }
    Ok(port)
}

fn config_path(port: u16) -> Result<PathBuf> {
    Ok(DaemonPaths::new(port)?.config)
}

fn load_daemon_config(port: u16) -> Result<Option<DaemonConfig>> {
    let path = config_path(port)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let config: DaemonConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode MCP daemon config {}", path.display()))?;
    if config.version != 1 || config.port != port {
        bail!("invalid MCP daemon config for port {port}");
    }
    Ok(Some(config))
}

fn save_daemon_config(config: &DaemonConfig) -> Result<()> {
    let path = config_path(config.port)?;
    let mut bytes = serde_json::to_vec_pretty(config)?;
    bytes.push(b'\n');
    fs::write(&path, bytes)?;
    set_private_file(&path)?;
    Ok(())
}

fn daemon_status(port: u16) -> Result<Option<DaemonStatus>> {
    let Some(response) = control_request(port, "status")? else {
        return Ok(None);
    };
    if response.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&response)
        .map(Some)
        .with_context(|| format!("decode MCP daemon status on port {port}"))
}

fn daemon_uses_current_binary(status: &DaemonStatus) -> Result<bool> {
    let Some(running) = status.binary_sha256.as_deref() else {
        // Status payloads from older Yeet builds do not carry a build identity.
        // A newly installed CLI should treat those daemons as stale and replace
        // them rather than mixing old HTTP code with new stdio children.
        return Ok(false);
    };
    Ok(running == current_binary_sha256()?)
}

fn current_binary_sha256() -> Result<String> {
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    binary_sha256(&executable)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinaryFileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn binary_file_stamp(path: &Path) -> Result<BinaryFileStamp> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("inspect Yeet executable {}", path.display()))?;
    Ok(BinaryFileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn binary_sha256(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open Yeet executable {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hash Yeet executable {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn stop_daemon(port: u16) -> Result<bool> {
    let paths = DaemonPaths::new(port)?;
    let mut stop_requested = false;
    let mut saw_managed_server = supervisor_is_running(&paths)?;
    for _ in 0..START_RETRIES {
        if !stop_requested && control_request(port, "stop")?.is_some() {
            stop_requested = true;
            saw_managed_server = true;
        }
        let supervisor_running = supervisor_is_running(&paths)?;
        if !supervisor_running && daemon_status(port)?.is_none() {
            if stop_requested || saw_managed_server {
                return Ok(true);
            }
            return Ok(false);
        }
        thread::sleep(START_DELAY);
    }
    bail!(
        "Yeet MCP daemon on port {port} did not stop; see {}",
        DaemonPaths::new(port)?.log.display()
    )
}

fn control_request(port: u16, command: &str) -> Result<Option<String>> {
    let path = DaemonPaths::new(port)?.control;
    let mut stream = match connect_local(&path) {
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
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(Some(response.trim().to_owned()))
}

struct DaemonControl {
    stop: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    socket: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl DaemonControl {
    fn start(port: u16, status: DaemonStatus, stop: Arc<AtomicBool>) -> Result<Self> {
        let paths = DaemonPaths::new(port)?;
        if paths.control.exists() {
            let _ = fs::remove_file(&paths.control);
        }
        let listener = bind_local(&paths.control)
            .with_context(|| format!("bind MCP control socket {}", paths.control.display()))?;
        set_private_file(&paths.control)?;
        listener.set_nonblocking(true)?;
        let thread_stop = Arc::clone(&stop);
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = thread::Builder::new()
            .name(format!("yeet-mcp-control-{port}"))
            .spawn(move || {
                while !thread_shutdown.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(CONTROL_TIMEOUT));
                            let mut request = String::new();
                            let _ = stream.read_to_string(&mut request);
                            match request.trim() {
                                "status" => {
                                    let mut current = status.clone();
                                    if let Ok(auth) =
                                        AuthStore::new(port).and_then(|store| store.status())
                                    {
                                        current.auth_mode = auth.mode.as_str().into();
                                        current.key_enabled = auth.key_enabled;
                                        current.oauth_enabled = auth.oauth_enabled;
                                    }
                                    let status_json = serde_json::to_string(&current)
                                        .unwrap_or_else(|_| "{}".into());
                                    let _ = writeln!(stream, "{status_json}");
                                }
                                "stop" => {
                                    thread_stop.store(true, Ordering::Release);
                                    let _ = writeln!(stream, "stopping");
                                }
                                _ => {
                                    let _ = writeln!(stream, "error: unknown command");
                                }
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            stop,
            shutdown,
            socket: paths.control,
            thread: Some(thread),
        })
    }
}

impl Drop for DaemonControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

fn format_status(status: &DaemonStatus, already_running: bool) -> String {
    let mut value = format!(
        "Yeet MCP daemon{}\nURL: {}\nBind: {}\nPort: {}\nWorkspace: {}\nAuth: {}\nOAuth: {}\nLog: {}",
        if already_running {
            " already running"
        } else {
            ""
        },
        status.public_url,
        status.address,
        status.port,
        status.default_workspace,
        status.auth_mode,
        if status.oauth_enabled {
            "enabled"
        } else {
            "disabled"
        },
        status.log
    );
    if let Some(pid) = status.pid {
        value.push_str(&format!("\nPID: {pid}"));
    }
    if let Some(hash) = status.binary_sha256.as_deref() {
        value.push_str(&format!("\nBuild: {}", &hash[..hash.len().min(12)]));
    }
    value
}

fn format_launch(
    status: &DaemonStatus,
    generated_key: Option<&str>,
    already_running: bool,
) -> String {
    let mut value = format_status(status, already_running);
    if let Some(key) = generated_key {
        value.push_str(&format!(
            "\nGenerated access/authorization key (store it now; only its Argon2 hash is kept):\n{key}"
        ));
    }
    value
}

fn read_access_key() -> Result<String> {
    let key = if io::stdin().is_terminal() {
        let first = rpassword::prompt_password("MCP access/authorization key: ")?;
        let second = rpassword::prompt_password("Confirm MCP access/authorization key: ")?;
        if first != second {
            bail!("MCP access keys did not match");
        }
        first
    } else {
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        value.trim_end_matches(['\r', '\n']).to_owned()
    };
    if key.is_empty() {
        bail!("MCP access key cannot be empty");
    }
    Ok(key)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_status(binary_sha256: Option<String>) -> DaemonStatus {
        DaemonStatus {
            address: "127.0.0.1:7332".into(),
            port: 7332,
            public_url: "http://127.0.0.1:7332/mcp".into(),
            default_workspace: "/tmp".into(),
            auth_mode: "none".into(),
            key_enabled: false,
            oauth_enabled: false,
            log: "/tmp/yeet-mcp.log".into(),
            pid: Some(std::process::id()),
            binary_sha256,
        }
    }

    #[test]
    fn server_options_accept_explicit_port_bind_and_auth() {
        let options = parse_server_overrides(&[
            "--port".into(),
            "8844".into(),
            "--bind".into(),
            "0.0.0.0".into(),
            "--auth".into(),
            "oauth".into(),
            "--public-url".into(),
            "https://mcp.example.com/mcp".into(),
        ])
        .unwrap();
        assert_eq!(options.port, Some(8844));
        assert_eq!(options.bind_host.as_deref(), Some("0.0.0.0"));
        assert_eq!(options.auth_mode, Some(AuthMode::Oauth));
    }

    #[test]
    fn unauthenticated_remote_bind_is_rejected() {
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(is_loopback_host("127.0.0.1"));
    }

    #[test]
    fn supervisor_lock_allows_only_one_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("supervisor.lock");
        let first = SupervisorLock::acquire(&path, 7332).unwrap();
        let second = SupervisorLock::acquire(&path, 7332);
        assert!(second.is_err());
        drop(first);
        SupervisorLock::acquire(&path, 7332).unwrap();
    }

    #[test]
    fn daemon_without_build_identity_is_treated_as_stale() {
        assert!(!daemon_uses_current_binary(&test_status(None)).unwrap());
    }

    #[test]
    fn daemon_build_identity_matches_current_executable() {
        let hash = current_binary_sha256().unwrap();
        assert!(daemon_uses_current_binary(&test_status(Some(hash))).unwrap());
    }
}
