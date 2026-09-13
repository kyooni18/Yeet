use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=web/dist");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest_dir.join("web/dist");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("remote_web_assets.rs");
    let mut files = Vec::new();
    if dist.is_dir() {
        collect_files(&dist, &dist, &mut files)?;
        files.sort_by(|left, right| left.0.cmp(&right.0));
    }
    let has_index = files.iter().any(|(relative, _)| relative == "index.html");
    if env::var("PROFILE").as_deref() == Ok("release") && !has_index {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Yeet Remote WebUI is not built; run Scripts/build-remote-web.sh before cargo build --release",
        ));
    }
    for (_, absolute) in &files {
        println!("cargo:rerun-if-changed={}", absolute.display());
    }

    let mut generated = fs::File::create(out)?;
    writeln!(
        generated,
        "pub(crate) const WEBUI_AVAILABLE: bool = {has_index};"
    )?;
    writeln!(
        generated,
        "pub(crate) struct EmbeddedWebAsset {{ pub bytes: &'static [u8], pub content_type: &'static str, pub immutable: bool }}"
    )?;
    writeln!(
        generated,
        "pub(crate) fn webui_asset(path: &str) -> Option<EmbeddedWebAsset> {{"
    )?;
    writeln!(generated, "    match path {{")?;
    for (relative, absolute) in files {
        let content_type = content_type(&relative);
        let immutable = relative.starts_with("assets/");
        writeln!(
            generated,
            "        {:?} => Some(EmbeddedWebAsset {{ bytes: include_bytes!({:?}), content_type: {:?}, immutable: {} }}),",
            relative,
            absolute.to_string_lossy(),
            content_type,
            immutable,
        )?;
    }
    writeln!(generated, "        _ => None,")?;
    writeln!(generated, "    }}")?;
    writeln!(generated, "}}")?;
    Ok(())
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, output)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("asset must live under dist")
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
    Ok(())
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
