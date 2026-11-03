//! Pinned, checksum-verified local embedding artifacts. Downloads are explicit,
//! bounded, resumable and independent of any chat-provider daemon.

use crate::platform::{set_private_directory, set_private_file, sync_directory};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelArtifact {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelManifest {
    pub id: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub license: &'static str,
    pub dimensions: usize,
    pub max_tokens: usize,
    pub runtime: &'static str,
    pub pooling: &'static str,
    pub artifacts: &'static [ModelArtifact],
}

const MINILM: LocalModelManifest = LocalModelManifest {
    id: "minilm-l6-v2",
    repository: "sentence-transformers/all-MiniLM-L6-v2",
    revision: "1110a243fdf4706b3f48f1d95db1a4f5529b4d41",
    license: "Apache-2.0",
    dimensions: 384,
    max_tokens: 256,
    runtime: "candle-cpu-0.9.2",
    pooling: "mean-l2",
    artifacts: &[
        ModelArtifact {
            name: "config.json",
            size: 612,
            sha256: "953f9c0d463486b10a6871cc2fd59f223b2c70184f49815e7efbcab5d8908b41",
        },
        ModelArtifact {
            name: "tokenizer.json",
            size: 466247,
            sha256: "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037",
        },
        ModelArtifact {
            name: "model.safetensors",
            size: 90868376,
            sha256: "53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db",
        },
    ],
};

pub fn local_model_catalog() -> &'static [LocalModelManifest] {
    &[MINILM]
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInstallStatus {
    pub model_id: String,
    pub state: String,
    pub verified_bytes: u64,
    pub total_bytes: u64,
}

/// One user-level artifact registry. Native execution is shipped with Yeet;
/// no package install, downloaded executable or model repository code is run.
pub struct ManagedModels {
    root: PathBuf,
}

impl ManagedModels {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn manifest(&self, id: &str) -> Result<&'static LocalModelManifest> {
        local_model_catalog()
            .iter()
            .find(|m| m.id == id)
            .context("unsupported local embedding model")
    }

    pub fn directory(&self, id: &str) -> Result<PathBuf> {
        let manifest = self.manifest(id)?;
        Ok(self
            .root
            .join(format!("{}-{}", manifest.id, manifest.revision)))
    }

    pub fn status(&self, id: &str) -> Result<ModelInstallStatus> {
        let manifest = self.manifest(id)?;
        let directory = self.directory(id)?;
        let total_bytes = manifest.artifacts.iter().map(|a| a.size).sum();
        let verified_bytes = manifest
            .artifacts
            .iter()
            .filter(|a| verify(&directory.join(a.name), a).unwrap_or(false))
            .map(|a| a.size)
            .sum();
        let state = if verified_bytes == total_bytes {
            "artifacts-verified"
        } else if manifest.artifacts.iter().any(|a| {
            directory.join(a.name).exists() || directory.join(format!("{}.part", a.name)).exists()
        }) {
            "incomplete"
        } else {
            "not-installed"
        };
        Ok(ModelInstallStatus {
            model_id: id.into(),
            state: state.into(),
            verified_bytes,
            total_bytes,
        })
    }

    /// Caller obtains download approval before invoking this method. Cancellation
    /// retains partial files for a later explicit retry; no automatic fallback.
    pub fn install(
        &self,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<ModelInstallStatus> {
        let manifest = self.manifest(id)?;
        let directory = self.directory(id)?;
        fs::create_dir_all(&directory)?;
        set_private_directory(&self.root)?;
        set_private_directory(&directory)?;
        let _lock = lock(&directory)?;
        let needed: u64 = manifest
            .artifacts
            .iter()
            .filter(|a| !verify(&directory.join(a.name), a).unwrap_or(false))
            .map(|a| a.size)
            .sum();
        ensure!(
            fs2::available_space(&directory)? > needed + 32 * 1024 * 1024,
            "insufficient disk space for embedding model"
        );
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .build();
        for artifact in manifest.artifacts {
            ensure!(!cancel.load(Ordering::Acquire), "cancelled");
            let destination = directory.join(artifact.name);
            if verify(&destination, artifact)? {
                progress(artifact.name, artifact.size, artifact.size);
                continue;
            }
            let partial = directory.join(format!("{}.part", artifact.name));
            let offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
            if offset >= artifact.size {
                fs::remove_file(&partial)?;
            }
            let offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
            let url = format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                manifest.repository, manifest.revision, artifact.name
            );
            let mut request = agent.get(&url);
            if offset > 0 {
                request = request.set("Range", &format!("bytes={offset}-"));
            }
            let response = request
                .call()
                .context("download local embedding artifact")?;
            let resume = response.status() == 206 && offset > 0;
            if resume {
                let prefix = format!("bytes {offset}-");
                ensure!(
                    response
                        .header("Content-Range")
                        .is_some_and(|v| v.starts_with(&prefix)),
                    "invalid download range"
                );
            }
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(!resume)
                .append(resume)
                .open(&partial)?;
            set_private_file(&partial)?;
            let mut received = if resume { offset } else { 0 };
            let mut reader = response.into_reader();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                ensure!(!cancel.load(Ordering::Acquire), "cancelled");
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                received += count as u64;
                ensure!(received <= artifact.size, "artifact exceeds declared size");
                file.write_all(&buffer[..count])?;
                progress(artifact.name, received, artifact.size);
            }
            file.sync_all()?;
            drop(file);
            if !verify(&partial, artifact)? {
                fs::remove_file(&partial)?;
                anyhow::bail!("artifact verification failed: {}", artifact.name);
            }
            crate::platform::replace_file(&partial, &destination)?;
            sync_directory(&directory)?;
        }
        self.status(id)
    }

    /// File lock excludes active native readers/installs. If in use, report a
    /// retryable error rather than deleting weights beneath a loaded encoder.
    pub fn remove(&self, id: &str) -> Result<()> {
        let directory = self.directory(id)?;
        if !directory.exists() {
            return Ok(());
        }
        let _lock = lock(&directory)?;
        for artifact in self.manifest(id)?.artifacts {
            for path in [
                directory.join(artifact.name),
                directory.join(format!("{}.part", artifact.name)),
            ] {
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
        sync_directory(&directory)?;
        Ok(())
    }

    pub(super) fn read_lease(&self, id: &str) -> Result<File> {
        let directory = self.directory(id)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.join(".lock"))?;
        FileExt::try_lock_shared(&file).context("embedding model is being installed or removed")?;
        ensure!(
            self.status(id)?.state == "artifacts-verified",
            "local model is not installed or verification failed"
        );
        Ok(file)
    }
}

fn lock(directory: &Path) -> Result<File> {
    let path = directory.join(".lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    set_private_file(&path)?;
    file.try_lock_exclusive()
        .context("local embedding model is in use")?;
    Ok(file)
}

fn verify(path: &Path, artifact: &ModelArtifact) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "invalid model artifact path"
    );
    if metadata.len() != artifact.size {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()) == artifact.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_install_never_downloads_and_corrupt_artifacts_are_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let models = ManagedModels::new(dir.path().join("models"));
        assert_eq!(models.status(MINILM.id).unwrap().state, "not-installed");
        assert!(
            models
                .install(MINILM.id, &AtomicBool::new(true), &mut |_, _, _| panic!(
                    "must not download"
                ))
                .is_err()
        );
        let directory = models.directory(MINILM.id).unwrap();
        fs::write(directory.join("config.json"), vec![0u8; 612]).unwrap();
        assert_eq!(models.status(MINILM.id).unwrap().verified_bytes, 0);
        assert!(models.read_lease(MINILM.id).is_err());
        let _lease = lock(&directory).unwrap();
        assert!(models.remove(MINILM.id).is_err());
        assert!(models.manifest("../../untrusted").is_err());
    }
}
