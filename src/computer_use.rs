//! Installation of the independent Open Computer Use runtime for the built-in tool.
use std::{fs, path::PathBuf, process::Command};

use anyhow::{Context, Result, ensure};

// Keep the adapter/CLI contract reproducible; update after validating upstream changes.
const PACKAGE: &str = "open-computer-use@1.1.2";

fn directory() -> PathBuf {
    crate::platform::default_config_directory().join("computer-use")
}

pub(crate) fn status() -> String {
    let root = directory().join("node_modules/open-computer-use");
    let installed = [
        "package.json",
        "bin/open-computer-use",
        "scripts/node-repl/open-computer-use-repl.mjs",
        "scripts/node-repl/open-computer-use-kernel.mjs",
    ]
    .iter()
    .all(|path| root.join(path).is_file());
    format!(
        "computer-use={}",
        if installed { "installed" } else { "missing" }
    )
}

pub(crate) fn availability() -> Result<()> {
    if !cfg!(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )) {
        anyhow::bail!("Computer Use is unavailable on this platform");
    }
    if status() != "computer-use=installed" {
        anyhow::bail!("Computer Use runtime is missing; run `yeet install computer-use`");
    }
    Ok(())
}

pub(crate) fn install() -> Result<String> {
    let destination = directory();
    let parent = destination
        .parent()
        .context("computer-use directory has no parent")?;
    fs::create_dir_all(parent)?;
    // Build and validate separately so a failed npm install preserves the last runtime.
    let staging = tempfile::Builder::new()
        .prefix(".computer-use-")
        .tempdir_in(parent)?;
    let npm = std::env::var_os("YEET_NPM").unwrap_or_else(|| {
        if cfg!(windows) {
            "npm.cmd".into()
        } else {
            "npm".into()
        }
    });
    let result = Command::new(npm)
        .args([
            "install",
            "--no-audit",
            "--no-fund",
            "--save-exact",
            "--prefix",
        ])
        .arg(staging.path())
        .arg(PACKAGE)
        .status()
        .context("start npm to install Open Computer Use (npm is required)")?;
    ensure!(
        result.success(),
        "Open Computer Use npm installation failed: {result}"
    );
    let launcher = staging
        .path()
        .join("node_modules/open-computer-use/bin/open-computer-use");
    let result = Command::new(crate::core::node_executable()?)
        .arg(&launcher)
        .args(["capabilities", "--json"])
        .output()
        .context("validate Open Computer Use runtime")?;
    ensure!(
        result.status.success(),
        "Open Computer Use validation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout)
        .context("read Open Computer Use capability report")?;
    ensure!(
        report
            .pointer("/capabilities/js/available")
            .and_then(serde_json::Value::as_bool)
            == Some(true),
        "Open Computer Use JavaScript/native runtime is unavailable: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let backup = parent.join(format!(".computer-use-backup-{}", uuid::Uuid::new_v4()));
    let had_previous = destination.exists();
    if had_previous {
        fs::rename(&destination, &backup)?;
    }
    if let Err(error) = fs::rename(staging.path(), &destination) {
        if had_previous {
            fs::rename(&backup, &destination)
                .context("restore previous Open Computer Use installation")?;
        }
        return Err(error).context("publish Open Computer Use installation");
    }
    if had_previous {
        fs::remove_dir_all(&backup)?;
    }
    Ok(format!(
        "Installed {PACKAGE} at {}. Restart active Yeet sessions to use the new runtime. On macOS 14+, grant Accessibility and Screen Recording when prompted.",
        destination.display()
    ))
}

pub(crate) fn uninstall() -> Result<String> {
    let path = directory();
    if path.exists() {
        fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(format!(
        "Removed Open Computer Use from {}. Restart active Yeet sessions to stop the previous runtime.",
        path.display()
    ))
}
