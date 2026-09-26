use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    config::ConfigStore,
    core::{BridgeClient, McpServerConfiguration, node_executable},
    project_settings::{ProjectSettingsStore, ServiceBackend},
    web_search,
};

const FOUNDATION_SERVER: &str = "foundation";
const DEFAULT_FOUNDATION_REPO: &str = "https://github.com/kyooni18/Foundation.git";

pub const HELP: &str = r#"Yeet component installer

Usage:
  yeet install [binary|runtime|foundation|web|mcp|remote|all]...
  yeet uninstall [binary|runtime|foundation|web|mcp|remote|all]...
  yeet install status

The main Yeet source build uses Rust plus Node.js/npm. Foundation is optional:
`yeet install foundation` clones and builds it only when requested. Yeet Remote is built
into the main binary and has no separate installation or service lifecycle.

Environment:
  YEET_PREFIX             installation prefix (default: ~/.local)
  YEET_SOURCE_ROOT        Yeet source checkout for binary/runtime installs
  YEET_FOUNDATION_REPO    Foundation Git repository override
"#;

pub fn run_cli(args: &[String], uninstall: bool) -> Result<String> {
    if args
        .first()
        .is_some_and(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(HELP.into());
    }
    if !uninstall && args.first().is_some_and(|arg| arg == "status") {
        return status();
    }

    if args.is_empty() {
        return Ok(HELP.into());
    }
    let targets = args.iter().map(String::as_str).collect::<Vec<_>>();
    let expanded = expand_targets(&targets, uninstall)?;
    let mut messages = Vec::new();
    for target in expanded {
        let message = if uninstall {
            uninstall_target(target)?
        } else {
            install_target(target)?
        };
        if !message.is_empty() {
            messages.push(message);
        }
    }
    Ok(messages.join("\n"))
}

fn expand_targets<'a>(targets: &[&'a str], uninstall: bool) -> Result<Vec<&'a str>> {
    let mut out = Vec::new();
    for target in targets {
        match *target {
            "all" if uninstall => out.extend(["mcp", "foundation", "web", "runtime", "binary"]),
            "all" => out.extend(["binary", "runtime", "web", "foundation", "mcp"]),
            "binary" | "runtime" | "foundation" | "web" | "web-search" | "search" | "mcp"
            | "remote" => out.push(target),
            other => bail!(
                "unknown component '{other}'; expected binary, runtime, foundation, web, mcp, remote, or all"
            ),
        }
    }
    Ok(out)
}

fn install_target(target: &str) -> Result<String> {
    match target {
        "binary" => install_binary(),
        "runtime" => install_runtime(),
        "foundation" => install_foundation(),
        "web" | "web-search" | "search" => web_search::run_cli(&["install".into(), "all".into()]),
        "mcp" => crate::mcp_server::run_cli(&["start".into()]),
        "remote" => {
            Ok("Yeet Remote is built into the Yeet binary; nothing extra to install".into())
        }
        _ => unreachable!(),
    }
}

fn uninstall_target(target: &str) -> Result<String> {
    match target {
        "binary" => uninstall_binary(),
        "runtime" => uninstall_runtime(),
        "foundation" => uninstall_foundation(),
        "web" | "web-search" | "search" => web_search::run_cli(&["remove".into(), "all".into()]),
        "mcp" => crate::mcp_server::run_cli(&["stop".into()]),
        "remote" => {
            Ok("Yeet Remote is built into the Yeet binary; nothing separate to uninstall".into())
        }
        _ => unreachable!(),
    }
}

fn install_binary() -> Result<String> {
    let source = env::current_exe().context("locate running Yeet executable")?;
    let destination = binary_destination()?;
    if source == destination {
        return Ok(format!(
            "Yeet binary already installed at {}",
            destination.display()
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&source, &destination)
        .with_context(|| format!("install {} to {}", source.display(), destination.display()))?;
    Ok(format!(
        "Installed Yeet binary to {}",
        destination.display()
    ))
}

fn uninstall_binary() -> Result<String> {
    let path = binary_destination()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        Ok(format!("Removed Yeet binary from {}", path.display()))
    } else {
        Ok(format!(
            "Yeet binary is not installed at {}",
            path.display()
        ))
    }
}

fn install_runtime() -> Result<String> {
    let root = source_root()?;
    let source = root.join("RuntimeSource");
    ensure!(
        source.join("dist/bridge.js").is_file(),
        "RuntimeSource is not built; run `npm --prefix RuntimeSource ci && npm --prefix RuntimeSource run build` first"
    );
    let destination = runtime_destination()?;
    fs::create_dir_all(&destination)?;
    copy_tree_replace(&source.join("dist"), &destination.join("dist"))?;
    copy_tree_replace(&source.join("skills"), &destination.join("skills"))?;
    fs::copy(
        source.join("package.json"),
        destination.join("package.json"),
    )?;
    Ok(format!(
        "Installed Yeet runtime to {}",
        destination.display()
    ))
}

fn uninstall_runtime() -> Result<String> {
    let path = runtime_destination()?;
    if path.exists() {
        fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))?;
        Ok(format!("Removed Yeet runtime from {}", path.display()))
    } else {
        Ok(format!(
            "Yeet runtime is not installed at {}",
            path.display()
        ))
    }
}

fn install_foundation() -> Result<String> {
    command_available("git")?;
    command_available("npm")?;
    let config = ConfigStore::default();
    config.ensure()?;
    let root = config.directory.join("services/foundation");
    let repo = env::var("YEET_FOUNDATION_REPO").unwrap_or_else(|_| DEFAULT_FOUNDATION_REPO.into());

    if root.join(".git").is_dir() {
        run(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["pull", "--ff-only"]),
            "update Foundation",
        )?;
    } else {
        if root.exists() {
            bail!(
                "Foundation install path exists but is not a Git checkout: {}",
                root.display()
            );
        }
        if let Some(parent) = root.parent() {
            fs::create_dir_all(parent)?;
        }
        run(
            Command::new("git")
                .args(["clone", "--depth", "1"])
                .arg(&repo)
                .arg(&root),
            "clone Foundation",
        )?;
    }

    let npm_install = if root.join("package-lock.json").is_file() {
        "ci"
    } else {
        "install"
    };
    run(
        Command::new("npm")
            .arg("--prefix")
            .arg(&root)
            .arg(npm_install),
        "install Foundation dependencies",
    )?;
    run(
        Command::new("npm")
            .arg("--prefix")
            .arg(&root)
            .args(["run", "build"]),
        "build Foundation",
    )?;

    let stdio = root.join("dist/stdio.js");
    ensure!(
        stdio.is_file(),
        "Foundation build did not produce {}",
        stdio.display()
    );
    let bridge = BridgeClient::start()?;
    let registration = bridge.set_mcp_server(&McpServerConfiguration {
        name: FOUNDATION_SERVER.into(),
        transport: "stdio".into(),
        command: Some(node_executable()?.to_string_lossy().into_owned()),
        args: Some(vec![stdio.to_string_lossy().into_owned()]),
        env: None,
        cwd: Some(root.to_string_lossy().into_owned()),
        url: None,
        headers: None,
    });
    bridge.shutdown();
    registration?;

    let workspace = env::current_dir()?.canonicalize()?;
    let settings = ProjectSettingsStore::new(workspace)?;
    settings.save_foundation_backend(ServiceBackend::Mcp, Some(FOUNDATION_SERVER))?;
    settings.save_foundation_memory(true, Some(FOUNDATION_SERVER))?;

    Ok(format!(
        "Foundation installed and enabled from {repo} at {}",
        root.display()
    ))
}

fn uninstall_foundation() -> Result<String> {
    let workspace = env::current_dir()?.canonicalize()?;
    let settings = ProjectSettingsStore::new(workspace)?;
    settings.save_foundation_memory(false, Some(FOUNDATION_SERVER))?;

    if let Ok(bridge) = BridgeClient::start() {
        let _ = bridge.remove_mcp_server(FOUNDATION_SERVER);
        bridge.shutdown();
    }

    let root = ConfigStore::default().directory.join("services/foundation");
    if root.exists() {
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(format!(
            "Foundation disabled and removed from {}",
            root.display()
        ))
    } else {
        Ok("Foundation disabled; no managed checkout was installed".into())
    }
}

fn status() -> Result<String> {
    let prefix = install_prefix()?;
    let binary = binary_destination()?;
    let runtime = runtime_destination()?;
    let foundation = ConfigStore::default().directory.join("services/foundation");
    Ok(format!(
        "prefix={}\nbinary={}\nruntime={}\nfoundation={}",
        prefix.display(),
        if binary.is_file() {
            "installed"
        } else {
            "missing"
        },
        if runtime.join("dist/bridge.js").is_file() {
            "installed"
        } else {
            "missing"
        },
        if foundation.join("dist/stdio.js").is_file() {
            "installed"
        } else {
            "missing"
        },
    ))
}

fn source_root() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_SOURCE_ROOT") {
        let path = PathBuf::from(value);
        if is_source_root(&path) {
            return Ok(path);
        }
        bail!(
            "YEET_SOURCE_ROOT is not a Yeet source checkout: {}",
            path.display()
        );
    }
    let cwd = env::current_dir()?;
    if is_source_root(&cwd) {
        return Ok(cwd);
    }
    let exe = env::current_exe()?;
    for ancestor in exe.ancestors().take(6) {
        if is_source_root(ancestor) {
            return Ok(ancestor.to_path_buf());
        }
    }
    bail!(
        "Yeet source checkout not found; run this command from the cloned Yeet repository or set YEET_SOURCE_ROOT"
    )
}

fn is_source_root(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("RuntimeSource/package.json").is_file()
}

fn install_prefix() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_PREFIX").or_else(|| env::var_os("PREFIX")) {
        let path = PathBuf::from(value);
        ensure!(
            !path.as_os_str().is_empty(),
            "installation prefix must not be empty"
        );
        return Ok(path);
    }
    let home = dirs::home_dir().context("home directory is unavailable")?;
    Ok(home.join(".local"))
}

fn binary_destination() -> Result<PathBuf> {
    let name = if cfg!(windows) { "yeet.exe" } else { "yeet" };
    Ok(install_prefix()?.join("bin").join(name))
}

fn runtime_destination() -> Result<PathBuf> {
    Ok(install_prefix()?.join("share/yeet/runtime"))
}

fn copy_tree_replace(source: &Path, destination: &Path) -> Result<()> {
    ensure!(
        source.is_dir(),
        "missing source directory {}",
        source.display()
    );
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    copy_tree(source, destination)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn command_available(command: &str) -> Result<()> {
    let status = Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    ensure!(
        status.is_ok_and(|status| status.success()),
        "required command is unavailable: {command}"
    );
    Ok(())
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let status = command.status().with_context(|| label.to_owned())?;
    ensure!(status.success(), "{label} failed with {status}");
    Ok(())
}
