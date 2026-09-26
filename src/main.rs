use std::{
    io,
    io::{IsTerminal, Write},
    process,
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use crossterm::{
    cursor::MoveToColumn,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};

#[cfg(not(target_os = "windows"))]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
        RemoteControl, RemoteInput, RemoteOptions, RemoteServer, clear_remote_access_key,
        clear_remote_passkeys, generate_remote_access_key, remote_auth_status, remote_browser_url,
        remote_status, request_remote_passkey_enrollment, set_remote_access_key,
        start_remote_background, stop_remote,
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
            run_remote_tui(options)
        } else {
            run_remote_web(options)
        };
    }
    if !arguments.is_empty() {
        process::exit(backend::forward_cli(&arguments)?);
    }

    run_tui()
}

fn run_remote_tui(options: RemoteOptions) -> Result<()> {
    let mut backend = Backend::spawn_remote()?;
    ui::initialize_theme();
    let mut app = App::default();
    let remote = RemoteServer::start(&options)?;
    let control = RemoteControl::start(remote.address(), remote.auth_handle(), true)?;
    let mut width = options.cols;
    let mut height = options.rows;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .context("failed to initialize remote terminal")?;
    let mut rendered_session = None;

    loop {
        if control.should_stop() {
            // Legacy Remote-TUI owns its backend in this Yeet process.
            // Returning drops that backend and any sidecars it actually started.
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
                RemoteInput::Mouse(mouse) => {
                    app.handle_mouse(mouse);
                    if let Some(session_id) = app.take_sidebar_load_request() {
                        backend.send(yeet::model::FrontendCommand::LoadSession { session_id })?;
                    }
                }
                RemoteInput::Resize { cols, rows } if cols != width || rows != height => {
                    width = cols;
                    height = rows;
                    terminal.resize(Rect::new(0, 0, width, height))?;
                }
                RemoteInput::Resize { .. } => {}
            }
        }

        draw_tui_frame(&mut terminal, &mut app, &mut rendered_session)?;
        remote.publish(terminal.backend().buffer());
        if app.quit {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
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

const TUI_STARTUP_NOTICE: &str = "YEET // STARTING RUNTIME...";
const MAX_BACKEND_EVENTS_PER_FRAME: usize = 128;

fn run_tui() -> Result<()> {
    let mut stdout = io::stdout();
    if !io::stdin().is_terminal() || !stdout.is_terminal() {
        anyhow::bail!(
            "Yeet TUI requires an interactive terminal; in Docker use `docker run -it ...`, or run a non-interactive command such as `yeet doctor` or `yeet mcpserver run`"
        );
    }
    show_tui_startup_notice(&mut stdout)?;

    let backend_result = Backend::spawn();

    execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    let mut backend = backend_result?;
    ui::initialize_theme();
    let mut app = App::default();
    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &mut app, &mut backend);
    let terminal_disconnected = terminal_input_disconnected().unwrap_or(false)
        || result
            .as_ref()
            .err()
            .is_some_and(terminal_error_indicates_disconnect);
    // The foreground Yeet process owns the backend. Dropping it tears down
    // only sidecars that were actually started by this runtime.
    drop(backend);
    if let Err(error) = restore_terminal(&mut terminal)
        && !terminal_disconnected
    {
        return Err(error);
    }
    if terminal_disconnected {
        Ok(())
    } else {
        result.context("TUI event loop failed")
    }
}

fn show_tui_startup_notice(writer: &mut impl Write) -> io::Result<()> {
    write!(writer, "{TUI_STARTUP_NOTICE}")?;
    writer.flush()
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    backend: &mut Backend,
) -> Result<()> {
    let terminal_events = spawn_terminal_event_reader();
    let mut rendered_session = None;

    loop {
        for _ in 0..MAX_BACKEND_EVENTS_PER_FRAME {
            let Some(event) = backend.try_recv() else {
                break;
            };
            apply_backend_event(app, event);
        }

        // Crossterm 0.28 can spin inside its Unix event reader when a PTY read
        // returns EOF. Keep that reader off the UI thread so this liveness check
        // can still terminate the process when the terminal peer disappears.
        if terminal_input_disconnected().context("failed to check terminal input state")? {
            return Ok(());
        }

        draw_tui_frame(terminal, app, &mut rendered_session)?;
        if app.quit {
            return Ok(());
        }

        match terminal_events.recv_timeout(Duration::from_millis(16)) {
            Ok(Ok(event)) => {
                match event {
                    Event::Key(key) => app.handle_key(key, backend)?,
                    Event::Mouse(mouse) => {
                        app.handle_mouse(mouse);
                        if let Some(session_id) = app.take_sidebar_load_request() {
                            backend
                                .send(yeet::model::FrontendCommand::LoadSession { session_id })?;
                        }
                    }
                    Event::Paste(text) => app.handle_paste(&text),
                    Event::Resize(width, height) => {
                        terminal.resize(Rect::new(0, 0, width, height))?;
                    }
                    _ => {}
                }
                if let Some(text) = app.take_clipboard_request() {
                    copy_via_osc52(&text)?;
                }
            }
            Ok(Err(error)) => {
                if terminal_input_disconnected().unwrap_or(false) {
                    return Ok(());
                }
                return Err(anyhow::Error::new(error).context("failed to read terminal event"));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if terminal_input_disconnected().unwrap_or(false) {
                    return Ok(());
                }
                anyhow::bail!("terminal event reader stopped unexpectedly");
            }
        }
    }
}

// A session boundary needs a physical clear as well as a new frame. Resetting
// Ratatui's back buffer makes unchanged cells repaint too, removing terminal
// artifacts that an ordinary incremental draw cannot detect.
fn draw_tui_frame<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rendered_session: &mut Option<String>,
) -> Result<()> {
    if *rendered_session != app.state.current_session_id {
        terminal.clear().context("failed to refresh terminal")?;
    }
    terminal
        .draw(|frame| ui::draw(frame, app))
        .context("failed to draw terminal")?;
    rendered_session.clone_from(&app.state.current_session_id);
    Ok(())
}

fn spawn_terminal_event_reader() -> std::sync::mpsc::Receiver<io::Result<Event>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let result = event::read();
            let should_stop = result.is_err();
            if sender.send(result).is_err() || should_stop {
                break;
            }
        }
    });
    receiver
}

#[cfg(unix)]
fn terminal_input_disconnected() -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        // SAFETY: `descriptor` points to one initialized pollfd for the duration
        // of the call, and a zero timeout makes this a non-blocking state check.
        let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if result >= 0 {
            return Ok(result > 0 && terminal_poll_flags_disconnected(descriptor.revents));
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn terminal_poll_flags_disconnected(revents: libc::c_short) -> bool {
    let disconnect_flags = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
    revents & disconnect_flags != 0
}

#[cfg(not(unix))]
fn terminal_input_disconnected() -> io::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
fn terminal_error_indicates_disconnect(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<io::Error>()
            .and_then(|error| error.raw_os_error())
            .is_some_and(|code| code == libc::EIO || code == libc::ENXIO)
    })
}

#[cfg(not(unix))]
fn terminal_error_indicates_disconnect(_error: &anyhow::Error) -> bool {
    false
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
    if let Err(error) = execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    ) {
        rollback_terminal_setup(&mut stdout, false);
        return Err(error).context("failed to enter terminal UI mode");
    }

    // CSI-u keeps modified Enter distinct from plain Enter so Shift+Enter can insert newlines.
    let keyboard_enhancement_pushed = {
        #[cfg(not(target_os = "windows"))]
        {
            if let Err(error) = execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            ) {
                rollback_terminal_setup(&mut stdout, false);
                return Err(error).context("failed to enable terminal keyboard enhancements");
            }
            true
        }
        #[cfg(target_os = "windows")]
        {
            false
        }
    };

    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let mut stdout = io::stdout();
            rollback_terminal_setup(&mut stdout, keyboard_enhancement_pushed);
            Err(error).context("failed to initialize terminal")
        }
    }
}

fn rollback_terminal_setup(stdout: &mut io::Stdout, _keyboard_enhancement_pushed: bool) {
    #[cfg(not(target_os = "windows"))]
    if _keyboard_enhancement_pushed {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut first_error = None;
    remember_terminal_cleanup_error(
        &mut first_error,
        disable_raw_mode(),
        "failed to disable terminal raw mode",
    );
    #[cfg(not(target_os = "windows"))]
    remember_terminal_cleanup_error(
        &mut first_error,
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags),
        "failed to restore terminal keyboard mode",
    );
    remember_terminal_cleanup_error(
        &mut first_error,
        execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        ),
        "failed to leave terminal UI mode",
    );
    remember_terminal_cleanup_error(
        &mut first_error,
        terminal.show_cursor(),
        "failed to show terminal cursor",
    );

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn remember_terminal_cleanup_error(
    first_error: &mut Option<anyhow::Error>,
    result: io::Result<()>,
    context: &'static str,
) {
    if first_error.is_none()
        && let Err(error) = result
    {
        *first_error = Some(anyhow::Error::new(error).context(context));
    }
}

fn copy_via_osc52(text: &str) -> Result<()> {
    let payload = STANDARD.encode(text.as_bytes());
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{payload}\x07")?;
    stdout.flush()?;
    Ok(())
}
