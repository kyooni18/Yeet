use super::*;

pub(super) fn install(target: &str) -> Result<String> {
    match target {
        "all" | "native" => {
            let paths = SearchPaths::discover();
            fs::create_dir_all(&paths.root)?;
            fs::write(paths.root.join("native-exa.enabled"), b"1\n")?;
            Ok("Native Exa web search enabled. No additional dependencies are required; run `yeet search test` to check connectivity.".into())
        }
        "searxng" | "searx" => install_searxng(),
        "agent-reach" | "agent_reach" | "exa" => install_agent_reach(),
        value => bail!("unknown search install target '{value}'"),
    }
}

pub(super) fn install_searxng() -> Result<String> {
    let paths = SearchPaths::discover();
    fs::create_dir_all(&paths.root).with_context(|| format!("create {}", paths.root.display()))?;

    if paths.source.join(".git").is_dir() {
        run(
            Command::new("git")
                .arg("-C")
                .arg(&paths.source)
                .arg("pull")
                .arg("--ff-only"),
            "update SearXNG source",
        )?;
    } else {
        if paths.source.exists() {
            fs::remove_dir_all(&paths.source)?;
        }
        run(
            Command::new("git")
                .arg("clone")
                .arg("--depth")
                .arg("1")
                .arg("https://github.com/searxng/searxng.git")
                .arg(&paths.source),
            "clone SearXNG",
        )?;
    }

    if !paths.python().is_file() {
        let system_python = env::var_os("YEET_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        run(
            Command::new(system_python)
                .arg("-m")
                .arg("venv")
                .arg(&paths.venv),
            "create SearXNG virtual environment",
        )?;
    }

    let python = paths.python();
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .args([
                "pip",
                "setuptools",
                "wheel",
                "pyyaml",
                "msgspec",
                "typing-extensions",
                "pybind11",
            ]),
        "prepare SearXNG Python environment",
    )?;
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--use-pep517")
            .arg("--no-build-isolation")
            .arg("-e")
            .arg(&paths.source),
        "install SearXNG",
    )?;

    let secret = Uuid::new_v4().simple().to_string();
    let settings = format!(
        "use_default_settings: true\n\ngeneral:\n  debug: false\n  instance_name: \"Yeet Search\"\n\nsearch:\n  safe_search: 0\n  formats:\n    - html\n    - json\n\nserver:\n  secret_key: \"{secret}\"\n  bind_address: \"127.0.0.1\"\n  port: 8888\n  limiter: false\n  public_instance: false\n  image_proxy: false\n"
    );
    fs::write(&paths.settings, settings)?;
    Ok(format!(
        "SearXNG installed at {}. Web search is attached by default in new projects and starts lazily on the first search.",
        paths.root.display()
    ))
}

pub(super) fn install_agent_reach() -> Result<String> {
    let paths = SearchPaths::discover();
    fs::create_dir_all(&paths.root).with_context(|| format!("create {}", paths.root.display()))?;

    if !paths.agent_reach_python().is_file() {
        let system_python = env::var_os("YEET_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        run(
            Command::new(system_python)
                .arg("-m")
                .arg("venv")
                .arg(&paths.agent_reach_venv),
            "create Agent-Reach virtual environment",
        )?;
    }
    let python = paths.agent_reach_python();
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip"),
        "prepare Agent-Reach Python environment",
    )?;
    run(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg(AGENT_REACH_URL),
        "install Agent-Reach",
    )?;

    fs::create_dir_all(&paths.agent_reach_node)?;
    let npm = env::var_os("YEET_NPM")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("npm"));
    run(
        Command::new(npm)
            .arg("install")
            .arg("--prefix")
            .arg(&paths.agent_reach_node)
            .arg("mcporter"),
        "install Agent-Reach mcporter backend",
    )?;

    let config = json!({
        "mcpServers": {
            "exa": {
                "baseUrl": EXA_MCP_URL
            }
        }
    });
    fs::write(
        &paths.agent_reach_mcporter_config,
        serde_json::to_vec_pretty(&config)?,
    )?;

    Ok(format!(
        "Agent-Reach + Exa installed at {}. web_search now prefers Agent-Reach automatically and falls back to SearXNG.",
        paths.agent_reach_venv.display()
    ))
}

pub(super) fn remove(target: &str) -> Result<String> {
    let paths = SearchPaths::discover();
    let client = WebSearchClient::default();
    match target {
        "all" => {
            ensure_searxng_stopped(&client)?;
            if paths.root.exists() {
                fs::remove_dir_all(&paths.root)?;
            }
            Ok(format!(
                "Removed managed web search files from {}.",
                paths.root.display()
            ))
        }
        "searxng" | "searx" => {
            ensure_searxng_stopped(&client)?;
            for path in [&paths.source, &paths.venv] {
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            for path in [&paths.settings, &paths.log] {
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
            Ok("Removed managed SearXNG backend.".into())
        }
        "native" => {
            let marker = paths.root.join("native-exa.enabled");
            if marker.is_file() {
                fs::remove_file(marker)?;
            }
            Ok("Disabled native Exa web search.".into())
        }
        "agent-reach" | "agent_reach" | "exa" => {
            for path in [&paths.agent_reach_venv, &paths.agent_reach_node] {
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            if paths.agent_reach_mcporter_config.exists() {
                fs::remove_file(&paths.agent_reach_mcporter_config)?;
            }
            Ok("Removed managed Agent-Reach backend.".into())
        }
        value => bail!("unknown search remove target '{value}'"),
    }
}

pub(super) fn ensure_searxng_stopped(client: &WebSearchClient) -> Result<()> {
    if env::var_os("YEET_SEARXNG_URL").is_none() && client.probe(DEFAULT_URL) {
        bail!(
            "A SearXNG instance is still running at {DEFAULT_URL}. Close the Yeet process that owns it before removing the managed installation."
        );
    }
    Ok(())
}

pub(super) fn run(command: &mut Command, purpose: &str) -> Result<()> {
    configure_hidden_process_group(command);
    let mut child = command.spawn().with_context(|| purpose.to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().with_context(|| purpose.to_owned())? {
            return if status.success() {
                Ok(())
            } else {
                Err(anyhow!("{purpose} failed with {status}"))
            };
        }
        if Instant::now() >= deadline {
            kill_child_group(&mut child);
            let _ = child.wait();
            bail!(
                "{purpose} timed out after 180 seconds; check your network connection and try again"
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn command_available(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(search_path) = env::var_os("PATH") else {
        return false;
    };
    for directory in env::split_paths(&search_path) {
        if directory.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        for extension in ["exe", "cmd", "bat"] {
            if directory.join(format!("{name}.{extension}")).is_file() {
                return true;
            }
        }
    }
    false
}
