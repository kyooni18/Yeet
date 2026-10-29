use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use crate::{platform::replace_file, web_search};

pub const HELP: &str = r#"Yeet component installer

Usage:
  yeet install [binary|runtime|web|mcp|remote|computer-use|all]...
  yeet uninstall [binary|runtime|web|mcp|remote|computer-use|all]...
  yeet install status

The main Yeet source build uses Rust plus Node.js/npm. The Remote server is built
into the binary; its experimental WebUI requires an opt-in source build.

Environment:
  YEET_PREFIX             installation prefix (default: ~/.local)
  YEET_SOURCE_ROOT        Yeet source checkout for binary/runtime installs
  YEET_NPM                npm executable for computer-use installation
  YEET_CONFIG_DIR         state directory for the managed computer-use runtime
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
            "all" if uninstall => out.extend(["computer-use", "mcp", "web", "runtime", "binary"]),
            "all" => out.extend(["binary", "runtime", "web", "mcp", "computer-use"]),
            "binary" | "runtime" | "web" | "web-search" | "search" | "mcp" | "remote"
            | "computer-use" => out.push(target),
            other => bail!(
                "unknown component '{other}'; expected binary, runtime, web, mcp, remote, computer-use, or all"
            ),
        }
    }
    Ok(out)
}

fn install_target(target: &str) -> Result<String> {
    match target {
        "binary" => install_binary(),
        "computer-use" => crate::computer_use::install(),
        "runtime" => install_runtime(),
        "web" | "web-search" | "search" => web_search::run_cli(&["install".into(), "all".into()]),
        "mcp" => crate::mcp_server::run_cli(&["start".into()]),
        "remote" => {
            Ok("The Remote server is built into Yeet. To include its experimental WebUI, rebuild with ./install.sh --build --experimental-webui or install.ps1 -Build -ExperimentalWebUI.".into())
        }
        _ => unreachable!(),
    }
}

fn uninstall_target(target: &str) -> Result<String> {
    match target {
        "binary" => uninstall_binary(),
        "computer-use" => crate::computer_use::uninstall(),
        "runtime" => uninstall_runtime(),
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
    let parent = destination
        .parent()
        .context("Yeet binary destination has no parent directory")?;
    fs::create_dir_all(parent)?;

    // Do not overwrite a running executable in place. On macOS in particular,
    // mutating the inode that backs a live linker-signed process can leave the
    // kernel code-signature cache attached to stale pages, causing subsequent
    // launches of the installed path to be killed with SIGKILL. Stage a fresh
    // inode beside the destination and atomically replace the path instead.
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .context("Yeet binary destination has no file name")?;
    let staging = parent.join(format!(".{name}.install-{}", std::process::id()));
    if staging.exists() {
        fs::remove_file(&staging)?;
    }
    fs::copy(&source, &staging)
        .with_context(|| format!("stage {} at {}", source.display(), staging.display()))?;
    if let Err(error) = replace_file(&staging, &destination) {
        let _ = fs::remove_file(&staging);
        return Err(error)
            .with_context(|| format!("install {} to {}", source.display(), destination.display()));
    }
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

fn status() -> Result<String> {
    let prefix = install_prefix()?;
    let binary = binary_destination()?;
    let runtime = runtime_destination()?;
    Ok(format!(
        "prefix={}\nbinary={}\nruntime={}\n{}",
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
        crate::computer_use::status(),
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
