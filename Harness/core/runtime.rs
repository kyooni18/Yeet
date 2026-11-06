//! Locates the bundled JavaScript runtime and provider bridge executables.

use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};

pub fn runtime_directory() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_RUNTIME_DIR") {
        let path = PathBuf::from(value);
        if path.join("dist/bridge.js").is_file() {
            return Ok(path);
        }
        bail!(
            "YEET_RUNTIME_DIR does not contain dist/bridge.js: {}",
            path.display()
        );
    }
    if let Ok(exe) = env::current_exe()
        && let Some(bin) = exe.parent()
    {
        if let Some(prefix) = bin.parent() {
            let path = prefix.join("share/yeet/runtime");
            if path.join("dist/bridge.js").is_file() {
                return Ok(path);
            }
        }
        let beside = bin.join("runtime");
        if beside.join("dist/bridge.js").is_file() {
            return Ok(beside);
        }
        // Detached development daemons run from the selected workspace, not the
        // repository root. Recognize Cargo's target/{debug,release}/yeet layout so
        // they can still locate the repository RuntimeSource without requiring an
        // inherited YEET_RUNTIME_DIR override.
        let cargo_profile = bin
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value, "debug" | "release"));
        let cargo_target = bin
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            == Some("target");
        if cargo_profile
            && cargo_target
            && let Some(root) = bin.parent().and_then(|target| target.parent())
        {
            let development = root.join("RuntimeSource");
            if development.join("dist/bridge.js").is_file() {
                return Ok(development);
            }
        }
    }
    let cwd = PathBuf::from("RuntimeSource");
    if cwd.join("dist/bridge.js").is_file() {
        return fs::canonicalize(cwd).context("canonicalize RuntimeSource");
    }
    bail!("Yeet runtime not found; set YEET_RUNTIME_DIR or install RuntimeSource beside the binary")
}

pub fn node_executable() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_NODE") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Ok(path); // allow PATH-style command names supplied explicitly
    }
    Ok(PathBuf::from("node"))
}

pub(super) fn bridge_script() -> Result<PathBuf> {
    Ok(runtime_directory()?.join("dist/bridge.js"))
}

pub fn edit_daemon_script() -> Result<PathBuf> {
    Ok(runtime_directory()?.join("dist/edit-backend/daemon.js"))
}
