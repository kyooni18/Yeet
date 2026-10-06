//! Git review resource retrieval, including staged, untracked, renamed and unborn files.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(o.stdout)
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().into())
    }
}
pub fn repository_root(dir: &Path) -> Result<PathBuf, String> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .map(|bytes| PathBuf::from(String::from_utf8_lossy(&bytes).trim()))
}

pub fn review_patch(
    root: &Path,
    p: &Path,
    previous_path: Option<&Path>,
    untracked: bool,
    has_base: bool,
    full: bool,
) -> Result<Vec<String>, String> {
    let mut c = Command::new("git");
    c.arg("--literal-pathspecs").current_dir(root).args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        if full {
            "--unified=1000000"
        } else {
            "--unified=3"
        },
    ]);
    if untracked {
        c.args(["--no-index", "--", "/dev/null"]).arg(p);
    } else {
        let base = if !has_base {
            git(root, &["hash-object", "-t", "tree", "--stdin"])
                .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
                .unwrap_or_default()
        } else {
            "HEAD".into()
        };
        c.arg(base).arg("--").arg(p);
        if let Some(original) = previous_path {
            c.arg(original);
        }
    }
    let output = c.output().map_err(|error| error.to_string())?;
    if output.status.success() || (untracked && output.status.code() == Some(1)) {
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().into())
    }
}
