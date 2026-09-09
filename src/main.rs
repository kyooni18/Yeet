use std::{
    io,
    io::{IsTerminal, Write},
    path::PathBuf,
    process,
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    layout::Rect,
};

use yeet::{
    app::App,
    backend::{self, Backend, BackendEvent},
    remote::{
        RemoteDaemonControl, RemoteInput, RemoteOptions, RemoteServer, clear_remote_access_key,
        clear_remote_passkeys, generate_remote_access_key, launch_remote_daemon,
        remote_auth_status, remote_daemon_status, request_remote_passkey_enrollment,
        set_remote_access_key, stop_remote_daemon,
    },
    ui,
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
    if arguments.first().map(String::as_str) == Some("__remote-daemon") {
        let workspace = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("missing remote daemon workspace"))?;
        std::env::set_current_dir(&workspace)
            .with_context(|| format!("enter remote daemon workspace {}", workspace.display()))?;
        let mut remote_arguments = vec!["remote".to_owned()];
        remote_arguments.extend(arguments.iter().skip(2).cloned());
        let options = RemoteOptions::parse(&remote_arguments)?
            .ok_or_else(|| anyhow::anyhow!("invalid remote daemon arguments"))?;
        return run_remote_tui(options, workspace);
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
                let workspace = parse_remote_workspace(&arguments[2..])?;
                if let Some(status) = remote_daemon_status(&workspace)? {
                    println!(
                        "Yeet remote UI: {}\nWorkspace: {}",
                        status.address,
                        workspace.display()
                    );
                } else {
                    println!("Yeet remote UI is not running for {}", workspace.display());
                }
                return Ok(());
            }
            Some("stop") => {
                let workspace = parse_remote_workspace(&arguments[2..])?;
                if stop_remote_daemon(&workspace)? {
                    println!("Stopped Yeet remote UI for {}", workspace.display());
                } else {
                    println!("Yeet remote UI is not running for {}", workspace.display());
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
        let workspace = resolve_remote_workspace(options.workspace.clone())?;
        let result = launch_remote_daemon(&workspace, &options)?;
        if result.already_running {
            println!(
                "Yeet remote UI already running: {}\nWorkspace: {}",
                result.status.address,
                workspace.display()
            );
        } else {
            println!(
                "Yeet remote UI: {}\nWorkspace: {}",
                result.status.address,
                workspace.display()
            );
        }
        return Ok(());
    }
    if !arguments.is_empty() {
        process::exit(backend::forward_cli(&arguments)?);
    }

    run_tui()
}

fn run_remote_tui(options: RemoteOptions, workspace: std::path::PathBuf) -> Result<()> {
    let mut backend = Backend::spawn_remote()?;
    let mut app = App::default();
    let remote = RemoteServer::start_for_workspace(&options, &workspace)?;
    let control = RemoteDaemonControl::start(&workspace, remote.address(), remote.auth_handle())?;
    let mut width = options.cols;
    let mut height = options.rows;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .context("failed to initialize remote terminal")?;

    loop {
        if control.should_stop() {
            let _ = backend.send(yeet::model::FrontendCommand::Shutdown);
            return Ok(());
        }
        while let Some(event) = backend.try_recv() {
            apply_backend_event(&mut app, event);
        }

        while let Some(input) = remote.try_recv() {
            match input {
                RemoteInput::Key(key) => app.handle_key(key, &mut backend)?,
                RemoteInput::Text(text) => {
                    for character in text.chars() {
                        app.handle_key(
                            crossterm::event::KeyEvent::new(
                                crossterm::event::KeyCode::Char(character),
                                crossterm::event::KeyModifiers::NONE,
                            ),
                            &mut backend,
                        )?;
                    }
                }
                RemoteInput::Mouse(mouse) => app.handle_mouse(mouse),
                RemoteInput::Resize { cols, rows } if cols != width || rows != height => {
                    width = cols;
                    height = rows;
                    terminal.resize(Rect::new(0, 0, width, height))?;
                }
                RemoteInput::Resize { .. } => {}
            }
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        remote.publish(terminal.backend().buffer());
        if app.quit {
            let _ = backend.send(yeet::model::FrontendCommand::Shutdown);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn run_remote_auth_command(arguments: &[String]) -> Result<()> {
    let Some(category) = arguments.first().map(String::as_str) else {
        anyhow::bail!("Usage: yeet remote auth [status|key|passkey] ...");
    };
    match category {
        "status" => {
            let workspace = parse_remote_workspace(&arguments[1..])?;
            let status = remote_auth_status(&workspace)?;
            println!(
                "Workspace: {}\nAccess key: {}\nPasskeys: {}",
                workspace.display(),
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
                anyhow::bail!(
                    "Usage: yeet remote auth key [generate|set|clear] [--workspace PATH]"
                );
            };
            let workspace = parse_remote_workspace(&arguments[2..])?;
            match action {
                "generate" => {
                    let key = generate_remote_access_key(&workspace)?;
                    println!(
                        "Remote access key for {}:\n{}\nStore this key now; Yeet only keeps its Argon2 hash.",
                        workspace.display(),
                        key
                    );
                }
                "set" => {
                    let key = read_remote_access_key()?;
                    set_remote_access_key(&workspace, &key)?;
                    println!("Remote access key updated for {}", workspace.display());
                }
                "clear" => {
                    clear_remote_access_key(&workspace)?;
                    println!("Remote access key removed for {}", workspace.display());
                }
                _ => anyhow::bail!(
                    "Usage: yeet remote auth key [generate|set|clear] [--workspace PATH]"
                ),
            }
        }
        "passkey" => {
            let Some(action) = arguments.get(1).map(String::as_str) else {
                anyhow::bail!("Usage: yeet remote auth passkey [add|clear] [--workspace PATH]");
            };
            let workspace = parse_remote_workspace(&arguments[2..])?;
            match action {
                "add" => {
                    let url = request_remote_passkey_enrollment(&workspace)?;
                    println!(
                        "Open this one-time URL in the browser that will access Yeet Remote:\n{url}\nIt expires in 10 minutes."
                    );
                }
                "clear" => {
                    clear_remote_passkeys(&workspace)?;
                    println!("Removed all remote passkeys for {}", workspace.display());
                }
                _ => {
                    anyhow::bail!("Usage: yeet remote auth passkey [add|clear] [--workspace PATH]")
                }
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

fn parse_remote_workspace(arguments: &[String]) -> Result<PathBuf> {
    let mut workspace: Option<PathBuf> = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--workspace" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("missing path after --workspace"))?;
                if workspace.replace(PathBuf::from(value)).is_some() {
                    anyhow::bail!("workspace was specified more than once");
                }
            }
            value if !value.starts_with('-') && workspace.is_none() => {
                workspace = Some(PathBuf::from(value));
            }
            value => anyhow::bail!("unknown remote workspace option: {value}"),
        }
        index += 1;
    }
    resolve_remote_workspace(workspace)
}

fn resolve_remote_workspace(workspace: Option<PathBuf>) -> Result<PathBuf> {
    let path = workspace.unwrap_or(std::env::current_dir()?);
    let path = path
        .canonicalize()
        .with_context(|| format!("resolve remote workspace {}", path.display()))?;
    if !path.is_dir() {
        anyhow::bail!("remote workspace is not a directory: {}", path.display());
    }
    Ok(path)
}

fn run_tui() -> Result<()> {
    let mut backend = Backend::spawn()?;
    let mut app = App::default();
    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &mut app, &mut backend);
    restore_terminal(&mut terminal)?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    backend: &mut Backend,
) -> Result<()> {
    loop {
        while let Some(event) = backend.try_recv() {
            apply_backend_event(app, event);
        }

        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key, backend)?,
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Resize(_, _) => {}
                _ => {}
            }
            if let Some(text) = app.take_clipboard_request() {
                copy_via_osc52(&text)?;
            }
        }
    }
}

fn apply_backend_event(app: &mut App, event: BackendEvent) {
    match event {
        BackendEvent::Envelope(envelope) => match envelope.kind.as_str() {
            "state" => {
                if let Some(state) = envelope.state {
                    app.merge_state(state);
                }
            }
            "error" => app.backend_message = envelope.message,
            _ => {}
        },
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn copy_via_osc52(text: &str) -> Result<()> {
    let payload = STANDARD.encode(text.as_bytes());
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{payload}\x07")?;
    stdout.flush()?;
    Ok(())
}
