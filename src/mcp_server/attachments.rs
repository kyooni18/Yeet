use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    core::McpServerConfiguration,
    platform::{replace_file, set_private_file},
};

const EXTERNAL_MCP_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(super) struct AttachmentSnapshot {
    pub(super) generation: u64,
    pub(super) servers: Vec<McpServerConfiguration>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AttachmentUpdate {
    pub(super) changed: usize,
    pub(super) total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAttachments {
    version: u32,
    servers: BTreeMap<String, McpServerConfiguration>,
}

#[derive(Debug, Deserialize)]
struct ImportedMcpConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, ImportedMcpServer>,
}

#[derive(Debug, Deserialize)]
struct ImportedMcpServer {
    #[serde(rename = "type")]
    kind: Option<String>,
    transport: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    cwd: Option<String>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
}

#[derive(Debug)]
pub(super) struct AttachmentRegistry {
    servers: RwLock<BTreeMap<String, McpServerConfiguration>>,
    generation: AtomicU64,
    persistence_path: Option<PathBuf>,
}

impl Default for AttachmentRegistry {
    fn default() -> Self {
        Self {
            servers: RwLock::new(BTreeMap::new()),
            generation: AtomicU64::new(0),
            persistence_path: None,
        }
    }
}

static GLOBAL_ATTACHMENTS: OnceLock<Arc<AttachmentRegistry>> = OnceLock::new();

pub(super) fn initialize(path: PathBuf) -> Result<Arc<AttachmentRegistry>> {
    if let Some(existing) = GLOBAL_ATTACHMENTS.get() {
        return Ok(Arc::clone(existing));
    }
    let registry = Arc::new(AttachmentRegistry::open(path)?);
    let _ = GLOBAL_ATTACHMENTS.set(Arc::clone(&registry));
    Ok(GLOBAL_ATTACHMENTS.get().map(Arc::clone).unwrap_or(registry))
}

pub(super) fn global() -> Arc<AttachmentRegistry> {
    Arc::clone(GLOBAL_ATTACHMENTS.get_or_init(|| Arc::new(AttachmentRegistry::default())))
}

pub(super) fn import_standard_config(path: &Path) -> Result<Vec<McpServerConfiguration>> {
    let bytes = fs::read(path).with_context(|| format!("read MCP config {}", path.display()))?;
    parse_standard_config(&bytes).with_context(|| format!("parse MCP config {}", path.display()))
}

impl AttachmentRegistry {
    pub(super) fn open(path: PathBuf) -> Result<Self> {
        let servers = match fs::read(&path) {
            Ok(bytes) => {
                let persisted: PersistedAttachments = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode external MCP registry {}", path.display()))?;
                if persisted.version != EXTERNAL_MCP_FILE_VERSION {
                    bail!(
                        "unsupported external MCP registry version {} in {}",
                        persisted.version,
                        path.display()
                    );
                }
                for server in persisted.servers.values() {
                    validate_configuration(server)?;
                }
                persisted.servers
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            servers: RwLock::new(servers),
            generation: AtomicU64::new(0),
            persistence_path: Some(path),
        })
    }

    pub(super) fn attach(&self, server: McpServerConfiguration) -> Result<bool> {
        Ok(self.attach_many([server])?.changed > 0)
    }

    pub(super) fn attach_many<I>(&self, incoming: I) -> Result<AttachmentUpdate>
    where
        I: IntoIterator<Item = McpServerConfiguration>,
    {
        let incoming = incoming.into_iter().collect::<Vec<_>>();
        for server in &incoming {
            validate_configuration(server)?;
        }

        let mut servers = self
            .servers
            .write()
            .map_err(|_| anyhow!("external MCP registry lock poisoned"))?;
        let mut updated = servers.clone();
        let mut changed = 0;
        for server in incoming {
            if updated.get(&server.name) != Some(&server) {
                changed += 1;
                updated.insert(server.name.clone(), server);
            }
        }
        if changed > 0 {
            self.persist(&updated)?;
            *servers = updated;
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(AttachmentUpdate {
            changed,
            total: servers.len(),
        })
    }

    pub(super) fn detach(&self, name: &str) -> Result<bool> {
        let mut servers = self
            .servers
            .write()
            .map_err(|_| anyhow!("external MCP registry lock poisoned"))?;
        if !servers.contains_key(name) {
            return Ok(false);
        }
        let mut updated = servers.clone();
        updated.remove(name);
        self.persist(&updated)?;
        *servers = updated;
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }

    pub(super) fn snapshot(&self) -> Result<AttachmentSnapshot> {
        let generation = self.generation.load(Ordering::Acquire);
        let servers = self
            .servers
            .read()
            .map_err(|_| anyhow!("external MCP registry lock poisoned"))?
            .values()
            .cloned()
            .collect();
        Ok(AttachmentSnapshot {
            generation,
            servers,
        })
    }

    fn persist(&self, servers: &BTreeMap<String, McpServerConfiguration>) -> Result<()> {
        let Some(path) = self.persistence_path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let payload = serde_json::to_vec_pretty(&PersistedAttachments {
            version: EXTERNAL_MCP_FILE_VERSION,
            servers: servers.clone(),
        })?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, payload)?;
        set_private_file(&temporary)?;
        replace_file(&temporary, path)?;
        set_private_file(path)?;
        Ok(())
    }
}

fn parse_standard_config(bytes: &[u8]) -> Result<Vec<McpServerConfiguration>> {
    let imported: ImportedMcpConfig = serde_json::from_slice(bytes)?;
    let mut servers = Vec::with_capacity(imported.mcp_servers.len());
    for (name, server) in imported.mcp_servers {
        let declared = server.kind.as_deref().or(server.transport.as_deref());
        let transport = match declared {
            Some(value) if value.eq_ignore_ascii_case("stdio") => "stdio",
            Some(value)
                if value.eq_ignore_ascii_case("http")
                    || value.eq_ignore_ascii_case("streamable-http")
                    || value.eq_ignore_ascii_case("streamableHttp") =>
            {
                "http"
            }
            Some(value) => bail!("unsupported MCP transport for {name}: {value}"),
            None if server.command.is_some() => "stdio",
            None if server.url.is_some() => "http",
            None => bail!("MCP server {name} requires type/transport, command, or url"),
        };

        let configuration = McpServerConfiguration {
            name,
            transport: transport.into(),
            command: server.command,
            args: server.args,
            env: server.env,
            cwd: server.cwd,
            url: server.url,
            headers: server.headers,
        };
        validate_configuration(&configuration)?;
        servers.push(configuration);
    }
    Ok(servers)
}

fn validate_configuration(server: &McpServerConfiguration) -> Result<()> {
    if !valid_server_name(&server.name) {
        bail!(
            "MCP server name must use letters, digits, dot, underscore, or dash: {}",
            server.name
        );
    }
    match server.transport.as_str() {
        "stdio" => {
            if server
                .command
                .as_deref()
                .is_none_or(|command| command.trim().is_empty())
            {
                bail!("MCP stdio command is required");
            }
        }
        "http" => {
            let raw = server
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("MCP HTTP URL is required"))?;
            let url = Url::parse(raw).context("invalid MCP HTTP URL")?;
            if !matches!(url.scheme(), "http" | "https") {
                bail!("MCP HTTP URL must use http or https");
            }
        }
        transport => bail!("unsupported MCP transport: {transport}"),
    }
    Ok(())
}

fn valid_server_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_server(name: &str) -> McpServerConfiguration {
        McpServerConfiguration {
            name: name.into(),
            transport: "stdio".into(),
            command: Some("node".into()),
            args: Some(vec!["server.mjs".into()]),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        }
    }

    #[test]
    fn attachment_generation_changes_only_when_registry_changes() {
        let registry = AttachmentRegistry::default();
        let server = stdio_server("local");

        assert_eq!(registry.snapshot().unwrap().generation, 0);
        assert!(registry.attach(server.clone()).unwrap());
        assert_eq!(registry.snapshot().unwrap().generation, 1);
        assert!(!registry.attach(server).unwrap());
        assert_eq!(registry.snapshot().unwrap().generation, 1);
        assert!(registry.detach("local").unwrap());
        assert_eq!(registry.snapshot().unwrap().generation, 2);
        assert!(!registry.detach("local").unwrap());
        assert_eq!(registry.snapshot().unwrap().generation, 2);
    }

    #[test]
    fn attachment_configuration_is_validated_before_registration() {
        let registry = AttachmentRegistry::default();
        let mut invalid = stdio_server("bad name");
        assert!(registry.attach(invalid.clone()).is_err());

        invalid.name = "remote".into();
        invalid.transport = "http".into();
        invalid.command = None;
        invalid.args = None;
        invalid.url = Some("file:///tmp/server".into());
        assert!(registry.attach(invalid).is_err());
        assert!(registry.snapshot().unwrap().servers.is_empty());
    }

    #[test]
    fn imports_standard_mcp_servers_shape() {
        let servers = parse_standard_config(
            br#"{
                "mcpServers": {
                    "paper": {
                        "type": "stdio",
                        "command": "/Users/example/.paper/bin/paper",
                        "args": ["mcp"]
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "paper");
        assert_eq!(servers[0].transport, "stdio");
        assert_eq!(
            servers[0].command.as_deref(),
            Some("/Users/example/.paper/bin/paper")
        );
        assert_eq!(servers[0].args.as_deref(), Some(&["mcp".to_owned()][..]));
    }
}
