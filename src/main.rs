use anyhow::Result;
use std::{
    io::{self, IsTerminal},
    process,
    time::Duration,
};
use yeet::{
    harness,
    remote::{
        RemoteControl, RemoteOptions, RemoteServer, clear_remote_access_key, clear_remote_passkeys,
        generate_remote_access_key, remote_auth_status, remote_browser_url, remote_status,
        request_remote_passkey_enrollment, set_remote_access_key, start_remote_background,
        stop_remote,
    },
};

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "linux")]
    if arguments.first().map(String::as_str) == Some("__linux-sandbox-shell") {
        let plan = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("missing Linux sandbox plan"))?;
        let command = arguments
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("missing Linux sandbox command"))?;
        process::exit(yeet::shell::run_linux_sandbox_shell_helper(&plan, command)?);
    }
    #[cfg(target_os = "windows")]
    if arguments.first().map(String::as_str) == Some("__windows-sandbox-shell") {
        let command = arguments
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("missing Windows sandbox command"))?;
        let environment = arguments
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("missing Windows sandbox environment"))?;
        process::exit(yeet::shell::run_windows_sandbox_shell_helper(
            command,
            environment,
        )?);
    }
    if arguments.first().map(String::as_str) == Some("__background-daemon") {
        let workspace = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("missing daemon workspace"))?;
        let scope = arguments.get(2).cloned();
        return yeet::background::run_daemon(workspace, scope);
    }
    if let Err(error) = yeet::background::cleanup_stale_artifacts() {
        eprintln!("yeet: stale background cleanup skipped: {error:#}");
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("remote" | "--remote")
    ) && matches!(arguments.get(1).map(String::as_str), Some("-h" | "--help"))
    {
        println!("{}", yeet::remote::remote_help());
        return Ok(());
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("remote" | "--remote")
    ) {
        match arguments.get(1).map(String::as_str) {
            Some("status") => {
                if let Some(status) = remote_status()? {
                    let browser_url = remote_browser_url(&status)?;
                    println!("Yeet remote UI: {browser_url}");
                    if status.owned_children.is_empty() {
                        println!("Owned children: none");
                    } else {
                        println!("Owned children: {}", status.owned_children.join(", "));
                    }
                } else {
                    println!("Yeet remote UI is not running");
                }
                return Ok(());
            }
            Some("stop") => {
                if stop_remote()? {
                    println!("Stopped Yeet remote UI");
                } else {
                    println!("Yeet remote UI is not running");
                }
                return Ok(());
            }
            Some("auth") => {
                run_remote_auth_command(&arguments[2..])?;
                return Ok(());
            }
            _ => {}
        }
    }
    if let Some(options) = RemoteOptions::parse(&arguments)? {
        if let Some(status) = remote_status()? {
            let browser_url = remote_browser_url(&status)?;
            println!("Yeet remote UI already running: {browser_url}");
            return Ok(());
        }
        if options.background {
            let status = start_remote_background(&options)?;
            println!("Yeet remote UI: {}", remote_browser_url(&status)?);
            println!("Running in background");
            return Ok(());
        }
        return if options.legacy_tui {
            yeet::tui::run_remote(options)
        } else {
            run_remote_web(options)
        };
    }
    if !arguments.is_empty() {
        process::exit(harness::forward_cli(&arguments)?);
    }

    yeet::tui::run()
}

fn run_remote_web(options: RemoteOptions) -> Result<()> {
    let remote = RemoteServer::start(&options)?;
    let control = RemoteControl::start(remote.address(), remote.auth_handle(), false)?;
    println!("Yeet remote UI: http://{}", remote.address());
    while !control.should_stop() {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn run_remote_auth_command(arguments: &[String]) -> Result<()> {
    let Some(category) = arguments.first().map(String::as_str) else {
        anyhow::bail!("Usage: yeet remote auth [status|key|passkey] ...");
    };
    match category {
        "status" => {
            if arguments.len() != 1 {
                anyhow::bail!("Usage: yeet remote auth status");
            }
            let status = remote_auth_status()?;
            println!(
                "Remote authentication\nAccess key: {}\nPasskeys: {}",
                if status.key_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                status.passkey_count
            );
        }
        "key" => {
            let Some(action) = arguments.get(1).map(String::as_str) else {
                anyhow::bail!("Usage: yeet remote auth key [generate|set|clear]");
            };
            if arguments.len() != 2 {
                anyhow::bail!("Usage: yeet remote auth key [generate|set|clear]");
            }
            match action {
                "generate" => {
                    let key = generate_remote_access_key()?;
                    println!(
                        "Remote access key:\n{}\nStore this key now; Yeet only keeps its Argon2 hash.",
                        key
                    );
                }
                "set" => {
                    let key = read_remote_access_key()?;
                    set_remote_access_key(&key)?;
                    println!("Remote access key updated");
                }
                "clear" => {
                    clear_remote_access_key()?;
                    println!("Remote access key removed");
                }
                _ => anyhow::bail!("Usage: yeet remote auth key [generate|set|clear]"),
            }
        }
        "passkey" => {
            let Some(action) = arguments.get(1).map(String::as_str) else {
                anyhow::bail!("Usage: yeet remote auth passkey [add|clear]");
            };
            if arguments.len() != 2 {
                anyhow::bail!("Usage: yeet remote auth passkey [add|clear]");
            }
            match action {
                "add" => {
                    let url = request_remote_passkey_enrollment()?;
                    println!(
                        "Open this one-time URL in the browser that will access Yeet Remote:\n{url}\nIt expires in 10 minutes."
                    );
                }
                "clear" => {
                    clear_remote_passkeys()?;
                    println!("Removed all Remote passkeys");
                }
                _ => anyhow::bail!("Usage: yeet remote auth passkey [add|clear]"),
            }
        }
        _ => anyhow::bail!("Usage: yeet remote auth [status|key|passkey] ..."),
    }
    Ok(())
}

fn read_remote_access_key() -> Result<String> {
    let key = if io::stdin().is_terminal() {
        let first = rpassword::prompt_password("Remote access key: ")?;
        let second = rpassword::prompt_password("Confirm remote access key: ")?;
        if first != second {
            anyhow::bail!("remote access keys did not match");
        }
        first
    } else {
        use std::io::Read;
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        value.trim_end_matches(['\r', '\n']).to_owned()
    };
    if key.is_empty() {
        anyhow::bail!("remote access key cannot be empty");
    }
    Ok(key)
}
