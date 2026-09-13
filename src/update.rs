use std::{
    env,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[cfg(windows)]
use std::{fs, process::Stdio};

use anyhow::{Context, Result, anyhow, bail, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cli::VERSION;
#[cfg(windows)]
use crate::platform::configure_detached;

const USER_AGENT: &str = "Yeet self-updater";
const GITHUB_API: &str = "https://api.github.com";

#[derive(Debug, Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

pub fn run(args: &[String]) -> Result<()> {
    let check_only = match args {
        [] => false,
        [value] if value == "check" || value == "--check" => true,
        _ => bail!("Usage: yeet update [check]"),
    };

    let release = latest_release()?;
    let latest = parse_release_version(&release.tag_name)?;
    let current = Version::parse(VERSION).context("parse current Yeet version")?;
    if latest <= current {
        println!("Yeet {current} is up to date (latest: {latest}).");
        return Ok(());
    }
    if check_only {
        println!("Yeet {latest} is available (current: {current}).");
        return Ok(());
    }

    install_release(&release, &latest)
}

fn latest_release() -> Result<GitHubRelease> {
    let repository = release_repository()?;
    let url = format!("{GITHUB_API}/repos/{repository}/releases/latest");
    let bytes = download(&url, true)?;
    serde_json::from_slice(&bytes).context("parse latest Yeet release metadata")
}

fn release_repository() -> Result<String> {
    if let Some(value) = env::var_os("YEET_UPDATE_REPOSITORY") {
        let value = value.to_string_lossy().trim().trim_matches('/').to_owned();
        ensure!(
            value.split('/').count() == 2,
            "YEET_UPDATE_REPOSITORY must be owner/repository"
        );
        return Ok(value);
    }

    let repository = env!("CARGO_PKG_REPOSITORY")
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow!("Cargo package repository is not a GitHub URL"))?;
    ensure!(
        repository.split('/').count() == 2,
        "invalid Cargo package repository URL"
    );
    Ok(repository.to_owned())
}

fn parse_release_version(tag: &str) -> Result<Version> {
    let value = tag.trim();
    Version::parse(value.strip_prefix('v').unwrap_or(value))
        .with_context(|| format!("invalid Yeet release tag {tag:?}"))
}

fn install_release(release: &GitHubRelease, version: &Version) -> Result<()> {
    let target = release_target()?;
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    let base = format!("yeet-{version}-{target}");
    let archive_name = format!("{base}.{extension}");
    let checksum_name = format!("{archive_name}.sha256");
    let archive_url = asset_url(release, &archive_name)?;
    let checksum_url = asset_url(release, &checksum_name)?;

    let archive = download(archive_url, false)
        .with_context(|| format!("download release asset {archive_name}"))?;
    let checksum = String::from_utf8(download(checksum_url, false)?)
        .context("release checksum is not UTF-8")?;
    verify_checksum(&archive, &checksum)?;

    let temp = tempfile::Builder::new().prefix("yeet-update-").tempdir()?;
    extract_bundle(&archive, extension, temp.path())?;
    let bundle = temp.path().join(&base);
    ensure!(bundle.is_dir(), "release archive did not contain {base}");
    let prefix = install_prefix()?;

    #[cfg(windows)]
    {
        let installer = bundle.join("install.ps1");
        ensure!(
            installer.is_file(),
            "Windows release is missing install.ps1"
        );
        let staging = temp.path().to_path_buf();
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&installer)
            .arg("-WaitForProcessId")
            .arg(std::process::id().to_string())
            .arg("-CleanupBundle")
            .env("PREFIX", &prefix)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached(&mut command);
        command.spawn().context("launch staged Windows updater")?;
        std::mem::forget(temp);
        println!(
            "Yeet {version} is verified and staged. It will install after this process exits."
        );
        println!("Staging directory: {}", staging.display());
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let installer = bundle.join("install.sh");
        ensure!(installer.is_file(), "Unix release is missing install.sh");
        let status = Command::new("sh")
            .arg(&installer)
            .env("PREFIX", &prefix)
            .status()
            .context("run Yeet release installer")?;
        ensure!(status.success(), "Yeet release installer failed");
        println!("Updated Yeet to {version}.");
        Ok(())
    }
}

fn asset_url<'a>(release: &'a GitHubRelease, name: &str) -> Result<&'a str> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.as_str())
        .ok_or_else(|| anyhow!("release {} does not contain {name}", release.tag_name))
}

fn download(url: &str, github_api: bool) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(30))
        .build();
    let mut request = agent.get(url).set("User-Agent", USER_AGENT);
    if github_api {
        request = request.set("Accept", "application/vnd.github+json");
    }
    if let Ok(token) = env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        request = request.set("Authorization", &format!("Bearer {}", token.trim()));
    }
    let response = request
        .call()
        .map_err(|error| anyhow!("GET {url} failed: {error}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .with_context(|| format!("read response body from {url}"))?;
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], checksum: &str) -> Result<()> {
    let expected = checksum
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("release checksum file is empty"))?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "release checksum is not a SHA-256 digest"
    );
    let actual = format!("{:x}", Sha256::digest(bytes));
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "release SHA-256 verification failed"
    );
    Ok(())
}

fn install_prefix() -> Result<PathBuf> {
    if let Some(value) = env::var_os("YEET_UPDATE_PREFIX")
        && !value.is_empty()
    {
        return Ok(PathBuf::from(value));
    }

    let executable = env::current_exe().context("locate current Yeet executable")?;
    let bin = executable
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent directory"))?;
    ensure!(
        bin.file_name().and_then(|value| value.to_str()) == Some("bin"),
        "Yeet was not launched from a standard <prefix>/bin install; set YEET_UPDATE_PREFIX to update it"
    );
    Ok(bin
        .parent()
        .ok_or_else(|| anyhow!("current Yeet bin directory has no install prefix"))?
        .to_path_buf())
}

#[cfg(windows)]
fn extract_bundle(bytes: &[u8], extension: &str, destination: &Path) -> Result<()> {
    ensure!(
        extension == "zip",
        "unsupported Windows release archive {extension}"
    );
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("open Windows release ZIP")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("release ZIP contains an unsafe path"))?
            .to_path_buf();
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&output)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn extract_bundle(bytes: &[u8], extension: &str, destination: &Path) -> Result<()> {
    ensure!(
        extension == "tar.gz",
        "unsupported Unix release archive {extension}"
    );
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("read release tar entries")? {
        let mut entry = entry?;
        ensure!(
            entry.unpack_in(destination)?,
            "release tar contains an unsafe path"
        );
    }
    Ok(())
}

fn release_target() -> Result<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => bail!("self-update is not packaged for this OS/architecture"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_release_versions() {
        assert_eq!(
            parse_release_version("v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
    }

    #[test]
    fn checksum_verification_accepts_matching_sha256() {
        let bytes = b"portable release";
        let digest = format!("{:x}", Sha256::digest(bytes));
        verify_checksum(bytes, &format!("{digest}  yeet.tar.gz\n")).unwrap();
    }

    #[test]
    fn checksum_verification_rejects_mismatch() {
        let error =
            verify_checksum(b"changed", &format!("{}  yeet.tar.gz", "0".repeat(64))).unwrap_err();
        assert!(error.to_string().contains("verification failed"));
    }

    #[test]
    fn current_platform_has_a_release_target() {
        assert!(!release_target().unwrap().is_empty());
    }
}
