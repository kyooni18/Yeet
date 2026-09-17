use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::{bundled_runtime_digest, read_json_file};

pub(super) fn initialize_project(root: &Path, project: &Path, workspace: &Path) -> Result<()> {
    let runtime_path = root.join(".runtime").join("skyline.py");
    if !runtime_path.is_file() {
        bail!(
            "Skyline runtime is not installed at {}",
            runtime_path.display()
        );
    }
    let title = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    let output = Command::new("python3")
        .arg(&runtime_path)
        .arg("init")
        .arg("--root")
        .arg(project)
        .arg("--title")
        .arg(title)
        .arg("--source-root")
        .arg(workspace)
        .arg("--runtime-digest")
        .arg(bundled_runtime_digest())
        .current_dir(project)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .with_context(|| format!("initialize Skyline project {}", project.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        bail!(
            "Skyline initialization failed ({}): {detail}",
            output.status
        );
    }

    let skyline_dir = project.join("skyline");
    let config_path = skyline_dir.join("CONFIG.json");
    let config = read_json_file(&config_path)?;
    let configured_source = config
        .get("source_root")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!(
                "Skyline config {} has no source_root",
                config_path.display()
            )
        })?;
    let configured_source = PathBuf::from(configured_source)
        .canonicalize()
        .with_context(|| format!("resolve Skyline source_root {configured_source}"))?;
    if configured_source != workspace {
        bail!(
            "Skyline initializer bound {} to {}, expected {}",
            project.display(),
            configured_source.display(),
            workspace.display()
        );
    }
    let mission_path = skyline_dir.join("SKYLINE.md");
    if !mission_path.is_file() {
        bail!(
            "Skyline initializer did not create mission context at {}",
            mission_path.display()
        );
    }
    let project_runtime = skyline_dir.join("skyline.py");
    if !project_runtime.is_file() {
        bail!(
            "Skyline initializer did not deploy runtime at {}",
            project_runtime.display()
        );
    }
    Ok(())
}
