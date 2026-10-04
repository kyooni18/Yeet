//! Session export and archival.
//!
//! Exports copy a session's materialized directory into a `.tar.gz` archive
//! (optionally deleting the source) under the per-session exclusive lock, and
//! refuse to delete the active session.

use super::*;

pub(super) fn create_session_archive(source: &Path, id: &str, destination: &Path) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut builder = tar::Builder::new(file);
    let archive_root = Path::new(id);
    builder.append_dir(archive_root, source)?;
    append_session_archive_tree(&mut builder, source, archive_root)?;
    builder.finish()?;
    builder.into_inner()?.sync_all()?;
    Ok(())
}

pub(super) fn append_session_archive_tree(
    builder: &mut tar::Builder<fs::File>,
    source: &Path,
    archive_root: &Path,
) -> Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if ignored_archive_name(&name.to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        let archive_path = archive_root.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            builder.append_dir(&archive_path, &path)?;
            append_session_archive_tree(builder, &path, &archive_path)?;
        } else if file_type.is_file() {
            builder.append_path_with_name(&path, &archive_path)?;
        } else {
            bail!(
                "Refusing to export non-regular session entry {}",
                path.display()
            );
        }
    }
    Ok(())
}

pub(super) fn ignored_archive_name(name: &str) -> bool {
    name == ".DS_Store"
        || name.starts_with("._")
        || name == "exports"
        || name.ends_with(".tar")
        || name.ends_with(".tar.gz")
}

pub(super) fn session_archive_listing(path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(path)?;
    let mut archive = tar::Archive::new(file);
    let mut listing = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        listing.push(entry.path()?.to_string_lossy().replace('\\', "/"));
    }
    Ok(listing)
}

impl SessionStore {
    pub fn export_session(
        &self,
        id: &str,
        destination: Option<&Path>,
        delete_source: bool,
        active_session_id: Option<&str>,
    ) -> Result<SessionExportResult> {
        self.serialized_session(id, true, || {
            self.export_session_unlocked(id, destination, delete_source, active_session_id)
        })
    }

    pub(super) fn export_session_unlocked(
        &self,
        id: &str,
        destination: Option<&Path>,
        delete_source: bool,
        active_session_id: Option<&str>,
    ) -> Result<SessionExportResult> {
        validate_id(id)?;
        let root = self.directory.join(id);
        ensure!(root.is_dir(), "Session directory does not exist: {id}");
        if let Ok(bytes) = fs::read(root.join(MANIFEST_FILE))
            && let Ok(manifest) = serde_json::from_slice::<SemanticManifest>(&bytes)
        {
            materialize_manifest(&root, &manifest)?;
        }
        if delete_source && active_session_id == Some(id) {
            bail!("Refusing to delete the active session {id}");
        }

        let config_root = self
            .directory
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.directory.clone());
        let exports = config_root.join("exports");
        create_private_dir(&exports)?;
        let destination = destination
            .map(Path::to_path_buf)
            .unwrap_or_else(|| exports.join(format!("{id}.tar")));
        let destination = if destination.is_absolute() {
            destination
        } else {
            std::env::current_dir()?.join(destination)
        };
        let destination = normalize_lexical(&destination);
        let root_normalized = root
            .canonicalize()
            .unwrap_or_else(|_| normalize_lexical(&root));
        ensure!(
            !destination.starts_with(&root_normalized),
            "Session exports must be written outside the source session directory"
        );
        ensure!(
            !destination.exists(),
            "Export destination already exists: {}",
            destination.display()
        );
        let parent = destination
            .parent()
            .context("export destination has no parent")?;
        create_private_dir(parent)?;
        let temp = parent.join(format!(".{}.{}.tmp", id, Uuid::new_v4()));

        if let Err(error) = create_session_archive(&root, id, &temp) {
            let _ = fs::remove_file(&temp);
            return Err(error).context(format!("create session archive for {id}"));
        }
        let listing = match session_archive_listing(&temp) {
            Ok(listing) => listing,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error).context(format!("verify session archive for {id}"));
            }
        };
        let required_metadata = format!("{id}/metadata.json");
        let required_conversation = format!("{id}/conversation.json");
        ensure!(
            listing.iter().any(|line| line == &required_metadata)
                && listing.iter().any(|line| line == &required_conversation),
            "Session archive verification failed: required session files are missing"
        );
        ensure!(
            !listing.iter().any(|line| {
                let name = Path::new(line)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                name == ".DS_Store"
                    || name.starts_with("._")
                    || name.ends_with(".tar")
                    || name.ends_with(".tar.gz")
            }),
            "Session archive verification failed: ignored recursive/archive metadata leaked into export"
        );

        let mut file = fs::File::open(&temp)?;
        file.sync_all()?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let bytes = file.metadata()?.len();
        let sha256 = format!("{:x}", digest.finalize());
        drop(file);
        fs::rename(&temp, &destination)?;

        let mut source_deleted = false;
        if delete_source {
            ensure!(
                active_session_id != Some(id),
                "Session became active again before deletion; export kept and source preserved"
            );
            let trash_root = config_root.join("trash");
            create_private_dir(&trash_root)?;
            let trash = trash_root.join(format!("{id}-{}", Uuid::new_v4()));
            fs::rename(&root, &trash)?;
            // Deletion happens only after archive creation, reopen/list
            // verification, fsync, checksum, and an inactive-session recheck.
            // If cleanup fails, the source remains recoverable under trash/.
            match fs::remove_dir_all(&trash) {
                Ok(()) => source_deleted = true,
                Err(error) => {
                    return Err(error).context(format!(
                        "Export succeeded at {}, but cleanup of quarantined source {} failed",
                        destination.display(),
                        trash.display()
                    ));
                }
            }
        }

        Ok(SessionExportResult {
            session_id: id.to_owned(),
            path: destination.display().to_string(),
            sha256,
            bytes,
            source_deleted,
        })
    }
}
