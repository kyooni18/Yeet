//! Content-addressed semantic components and the manifest commit point.
//!
//! Components are written to `.objects/<sha256>` first; the manifest
//! (`.current.json`) is replaced atomically last and is the only commit point.
//! Materialized files are derived views that can be regenerated from it.

use super::*;

pub(super) fn cleanup_stale_materialization_links_in_store(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            cleanup_stale_materialization_links(&entry.path())?;
        }
    }
    Ok(())
}

pub(super) fn cleanup_stale_materialization_links(root: &Path) -> Result<()> {
    for directory in [
        root.to_path_buf(),
        root.join(RUNS_DIR),
        root.join(DEBATES_DIR),
    ] {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_stale_materialization_link(name) {
                remove_file_if_exists(&entry.path())?;
            }
        }
    }
    Ok(())
}

pub(super) fn is_stale_materialization_link(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".link") else {
        return false;
    };
    let Some((base, id)) = stem.rsplit_once('.') else {
        return false;
    };
    matches!(
        base,
        ".metadata.json"
            | ".conversation.json"
            | ".model-history.json"
            | ".index.json"
            | ".knowledge.json"
            | ".state.json"
    ) && Uuid::parse_str(id).is_ok()
}

pub(super) fn sha256_bytes(data: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(data);
    format!("{:x}", digest.finalize())
}

pub(super) fn read_manifest_component(
    root: &Path,
    manifest: &SemanticManifest,
    component: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(hash) = manifest.components.get(component).cloned().flatten() else {
        return Ok(None);
    };
    ensure!(
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid session object hash for {component}"
    );
    let path = root.join(OBJECTS_DIR).join(&hash);
    let bytes = fs::read(&path)
        .with_context(|| format!("read session object {} for {component}", path.display()))?;
    ensure!(
        sha256_bytes(&bytes) == hash,
        "session object checksum mismatch for {component}"
    );
    Ok(Some(bytes))
}

pub(super) fn materialize_component(object: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        create_private_dir(parent)?;
    }
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("component");
    let tmp = target.with_file_name(format!(".{file_name}.{}.link", Uuid::new_v4()));
    fs::hard_link(object, &tmp)?;
    if let Err(error) = replace_file(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn gc_semantic_objects(root: &Path, manifest: &SemanticManifest) -> Result<()> {
    let referenced = manifest
        .components
        .values()
        .filter_map(|value| value.as_ref())
        .cloned()
        .collect::<HashSet<_>>();
    let objects = root.join(OBJECTS_DIR);
    let Ok(entries) = fs::read_dir(&objects) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !referenced.contains(&name) {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}

pub(super) fn materialize_manifest(root: &Path, manifest: &SemanticManifest) -> Result<()> {
    for component in [
        "metadata.json",
        "conversation.json",
        "model-history.json",
        "runs/index.json",
        "debates/knowledge.json",
        "debates/state.json",
    ] {
        match manifest.components.get(component).cloned().flatten() {
            Some(hash) => {
                let object = root.join(OBJECTS_DIR).join(hash);
                ensure!(object.is_file(), "missing session object for {component}");
                materialize_component(&object, &root.join(component))?;
            }
            None => remove_file_if_exists(&root.join(component))?,
        }
    }
    Ok(())
}

pub(super) fn write_private_replace(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;

    set_private_file(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }

    set_private_file(path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}
