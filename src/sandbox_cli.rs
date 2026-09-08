use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Result, bail};

use crate::sandbox::{
    NetworkEndpoint, SandboxLimits, SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead,
    validate_environment, validate_relative_path, validate_secret_id,
};

pub const HELP: &str = r#"Project sandbox policy

Usage:
  yeet sandbox show
  yeet sandbox path
  yeet sandbox reset
  yeet sandbox mode sandboxed|unlimited
  yeet sandbox auto-approve on|off
  yeet sandbox scratch on|off
  yeet sandbox workspace none
  yeet sandbox workspace all
  yeet sandbox workspace add RELATIVE_PATH
  yeet sandbox workspace remove RELATIVE_PATH
  yeet sandbox network none
  yeet sandbox network add HOST PORT
  yeet sandbox network add HOST *
  yeet sandbox network remove HOST PORT
  yeet sandbox network remove HOST *
  yeet sandbox env clear
  yeet sandbox env set KEY VALUE
  yeet sandbox env unset KEY
  yeet sandbox secret clear
  yeet sandbox secret add ID
  yeet sandbox secret remove ID
  yeet sandbox limits reset
  yeet sandbox limits set [--wall-time-seconds N] [--stdout-bytes N] [--stderr-bytes N] [--memory-bytes N] [--processes N]

The file is ./.yeet/sandbox.json. It requests capabilities only; Yeet's
trusted execution ceiling can still reduce them."#;

pub fn run(workspace: &Path, args: &[String]) -> Result<String> {
    let store = SandboxStore::new(workspace)?;
    let command = args.first().map(String::as_str).unwrap_or("show");
    match command {
        "show" => {
            if !args.is_empty() {
                exact(args, 1, "yeet sandbox show")?;
            }
            store.render(&store.load()?)
        }
        "path" => {
            exact(args, 1, "yeet sandbox path")?;
            Ok(store.path().display().to_string())
        }
        "reset" => {
            exact(args, 1, "yeet sandbox reset")?;
            store.reset()?;
            Ok(store.render(&SandboxPolicy::default())?)
        }
        "mode" => mode(&store, args),
        "auto-approve" => auto_approve(&store, args),
        "scratch" => scratch(&store, args),
        "workspace" => workspace_policy(&store, args),
        "network" => network(&store, args),
        "env" => environment(&store, args),
        "secret" => secrets(&store, args),
        "limits" => limits(&store, args),
        "help" | "-h" | "--help" => Ok(HELP.into()),
        other => bail!("Unknown sandbox command: {other}\n\n{HELP}"),
    }
}

fn mode(store: &SandboxStore, args: &[String]) -> Result<String> {
    if args.len() != 2 {
        bail!("Usage: yeet sandbox mode sandboxed|unlimited");
    }
    let mut policy = store.load()?;
    policy.mode = match args[1].as_str() {
        "sandboxed" => SandboxMode::Sandboxed,
        "unlimited" => SandboxMode::Unlimited,
        _ => bail!("Usage: yeet sandbox mode sandboxed|unlimited"),
    };
    store.save(&policy)?;
    store.render(&policy)
}

fn auto_approve(store: &SandboxStore, args: &[String]) -> Result<String> {
    if args.len() != 2 {
        bail!("Usage: yeet sandbox auto-approve on|off");
    }
    let mut policy = store.load()?;
    policy.auto_approve = match args[1].as_str() {
        "on" => true,
        "off" => false,
        _ => bail!("Usage: yeet sandbox auto-approve on|off"),
    };
    store.save(&policy)?;
    store.render(&policy)
}

fn scratch(store: &SandboxStore, args: &[String]) -> Result<String> {
    if args.len() != 2 {
        bail!("Usage: yeet sandbox scratch on|off");
    }
    let value = match args[1].as_str() {
        "on" => true,
        "off" => false,
        _ => bail!("Usage: yeet sandbox scratch on|off"),
    };
    let mut policy = store.load()?;
    policy.scratch_writable = value;
    store.save(&policy)?;
    store.render(&policy)
}

fn workspace_policy(store: &SandboxStore, args: &[String]) -> Result<String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        bail!("Usage: yeet sandbox workspace [none|all|add RELATIVE_PATH|remove RELATIVE_PATH]");
    };
    let mut policy = store.load()?;
    match subcommand {
        "none" => {
            exact(args, 2, "yeet sandbox workspace none")?;
            policy.workspace_read = WorkspaceRead::None;
        }
        "all" => {
            exact(args, 2, "yeet sandbox workspace all")?;
            policy.workspace_read = WorkspaceRead::All;
        }
        "add" => {
            if args.len() != 3 {
                bail!("Usage: yeet sandbox workspace add RELATIVE_PATH");
            }
            let path = validate_relative_path(&args[2])?;
            let mut paths = match policy.workspace_read {
                WorkspaceRead::None => BTreeSet::new(),
                WorkspaceRead::Paths(ref paths) => paths.clone(),
                WorkspaceRead::All => bail!(
                    "Workspace access is set to all. Set it to none before editing individual paths."
                ),
            };
            paths.insert(path);
            policy.workspace_read = WorkspaceRead::Paths(paths);
        }
        "remove" => {
            if args.len() != 3 {
                bail!("Usage: yeet sandbox workspace remove RELATIVE_PATH");
            }
            let path = validate_relative_path(&args[2])?;
            let mut paths = match policy.workspace_read {
                WorkspaceRead::Paths(ref paths) => paths.clone(),
                WorkspaceRead::All => bail!(
                    "Workspace access is set to all. Set it to none before editing individual paths."
                ),
                WorkspaceRead::None => BTreeSet::new(),
            };
            paths.remove(&path);
            policy.workspace_read = if paths.is_empty() {
                WorkspaceRead::None
            } else {
                WorkspaceRead::Paths(paths)
            };
        }
        _ => {
            bail!("Usage: yeet sandbox workspace [none|all|add RELATIVE_PATH|remove RELATIVE_PATH]")
        }
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn network(store: &SandboxStore, args: &[String]) -> Result<String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        bail!("Usage: yeet sandbox network [none|add HOST PORT|remove HOST PORT]");
    };
    let mut policy = store.load()?;
    match subcommand {
        "none" => {
            exact(args, 2, "yeet sandbox network none")?;
            policy.network_allow.clear();
        }
        "add" => {
            if args.len() != 4 {
                bail!("Usage: yeet sandbox network add HOST PORT|*");
            }
            let endpoint = parse_endpoint(&args[2], &args[3])?;
            policy.network_allow.insert(endpoint);
            policy.normalize_network();
        }
        "remove" => {
            if args.len() != 4 {
                bail!("Usage: yeet sandbox network remove HOST PORT|*");
            }
            let endpoint = parse_endpoint(&args[2], &args[3])?;
            if endpoint.port.is_some()
                && policy
                    .network_allow
                    .iter()
                    .any(|value| value.host == endpoint.host && value.port.is_none())
            {
                bail!(
                    "{} is still allowed on all ports. Remove the '*' grant first.",
                    endpoint.host
                );
            }
            policy.network_allow.remove(&endpoint);
        }
        _ => bail!("Usage: yeet sandbox network [none|add HOST PORT|remove HOST PORT]"),
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn environment(store: &SandboxStore, args: &[String]) -> Result<String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        bail!("Usage: yeet sandbox env [clear|set KEY VALUE|unset KEY]");
    };
    let mut policy = store.load()?;
    match subcommand {
        "clear" => {
            exact(args, 2, "yeet sandbox env clear")?;
            policy.environment.clear();
        }
        "set" => {
            if args.len() != 4 {
                bail!("Usage: yeet sandbox env set KEY VALUE");
            }
            let draft = BTreeMap::from([(args[2].clone(), args[3].clone())]);
            validate_environment(&draft)?;
            policy.environment.insert(args[2].clone(), args[3].clone());
        }
        "unset" => {
            if args.len() != 3 {
                bail!("Usage: yeet sandbox env unset KEY");
            }
            let draft = BTreeMap::from([(args[2].clone(), String::new())]);
            validate_environment(&draft)?;
            policy.environment.remove(&args[2]);
        }
        _ => bail!("Usage: yeet sandbox env [clear|set KEY VALUE|unset KEY]"),
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn secrets(store: &SandboxStore, args: &[String]) -> Result<String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        bail!("Usage: yeet sandbox secret [clear|add ID|remove ID]");
    };
    let mut policy = store.load()?;
    match subcommand {
        "clear" => {
            exact(args, 2, "yeet sandbox secret clear")?;
            policy.secret_ids.clear();
        }
        "add" => {
            if args.len() != 3 {
                bail!("Usage: yeet sandbox secret add ID");
            }
            validate_secret_id(&args[2])?;
            policy.secret_ids.insert(args[2].clone());
        }
        "remove" => {
            if args.len() != 3 {
                bail!("Usage: yeet sandbox secret remove ID");
            }
            validate_secret_id(&args[2])?;
            policy.secret_ids.remove(&args[2]);
        }
        _ => bail!("Usage: yeet sandbox secret [clear|add ID|remove ID]"),
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn limits(store: &SandboxStore, args: &[String]) -> Result<String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        bail!("Usage: yeet sandbox limits [reset|set FLAGS]");
    };
    let mut policy = store.load()?;
    match subcommand {
        "reset" => {
            exact(args, 2, "yeet sandbox limits reset")?;
            policy.limits = SandboxLimits::default();
        }
        "set" => {
            let mut draft = policy.limits.clone();
            let mut seen = BTreeSet::new();
            let mut index = 2;
            while index < args.len() {
                let flag = &args[index];
                if !seen.insert(flag.clone()) {
                    bail!("Duplicate sandbox limit flag: {flag}");
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("Missing value for {flag}"))?;
                match flag.as_str() {
                    "--wall-time-seconds" => draft.wall_time_seconds = value.parse()?,
                    "--stdout-bytes" => draft.max_stdout_bytes = value.parse()?,
                    "--stderr-bytes" => draft.max_stderr_bytes = value.parse()?,
                    "--memory-bytes" => draft.max_memory_bytes = value.parse()?,
                    "--processes" => draft.max_processes = value.parse()?,
                    _ => bail!("Unknown sandbox limit flag: {flag}"),
                }
                index += 2;
            }
            draft.validate()?;
            policy.limits = draft;
        }
        _ => bail!("Usage: yeet sandbox limits [reset|set FLAGS]"),
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn parse_endpoint(host: &str, port: &str) -> Result<NetworkEndpoint> {
    if port == "*" {
        NetworkEndpoint::new(host, None)
    } else {
        let port: u16 = port
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid network port: {port}. Use 1...65535 or '*'."))?;
        if port == 0 {
            bail!("Invalid network port: 0. Use 1...65535 or '*'.");
        }
        NetworkEndpoint::new(host, Some(port))
    }
}

fn exact(args: &[String], count: usize, usage: &str) -> Result<()> {
    if args.len() != count {
        bail!("Usage: {usage}");
    }
    Ok(())
}
