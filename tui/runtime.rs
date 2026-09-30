//! Native terminal lifecycle and legacy remote terminal event loops.
use std::{
    io,
    io::{IsTerminal, Write},
    time::{Duration, Instant},
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

use super::{app::App, ui};
use crate::{
    backend::{Backend, BackendEvent},
    remote::{RemoteControl, RemoteInput, RemoteOptions, RemoteServer},
};

pub fn run_remote(options: RemoteOptions) -> Result<()> {
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
                    if let Some(command) = app.take_workbench_command() {
                        backend.send(command)?;
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

const TUI_STARTUP_NOTICE: &str = "YEET // STARTING RUNTIME...";
const MAX_BACKEND_EVENTS_PER_FRAME: usize = 128;
const MAX_INPUT_EVENTS_PER_FRAME: usize = 64;
const BACKEND_FRAME_BUDGET: Duration = Duration::from_millis(4);

pub fn run() -> Result<()> {
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
        let backend_started = Instant::now();
        for _ in 0..MAX_BACKEND_EVENTS_PER_FRAME {
            let Some(event) = backend.try_recv() else {
                break;
            };
            apply_backend_event(app, event);
            if backend_started.elapsed() >= BACKEND_FRAME_BUDGET {
                break;
            }
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

        let mut input = terminal_events.recv_timeout(Duration::from_millis(16));
        for input_index in 0..MAX_INPUT_EVENTS_PER_FRAME {
            match input {
                Ok(Ok(event)) => {
                    match event {
                        Event::Key(key) => app.handle_key(key, backend)?,
                        Event::Mouse(mouse) => {
                            app.handle_mouse(mouse);
                            if let Some(command) = app.take_workbench_command() {
                                backend.send(command)?;
                            }
                        }
                        Event::Paste(text) => app.handle_paste(&text),
                        Event::Resize(width, height) => {
                            terminal.resize(Rect::new(0, 0, width, height))?;
                        }
                        _ => {}
                    }
                    if app.take_clipboard_paste_request() {
                        match read_system_clipboard() {
                            Ok(text) => {
                                app.handle_paste(&text);
                            }
                            Err(error) => {
                                app.backend_message = Some(format!("Paste failed: {error}"))
                            }
                        }
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
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if terminal_input_disconnected().unwrap_or(false) {
                        return Ok(());
                    }
                    anyhow::bail!("terminal event reader stopped unexpectedly");
                }
            }
            if app.quit {
                return Ok(());
            }
            // Do not dequeue an event we cannot process in this frame.
            if input_index + 1 == MAX_INPUT_EVENTS_PER_FRAME {
                break;
            }
            input = match terminal_events.try_recv() {
                Ok(event) => Ok(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
                }
            };
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
            "error" => {
                if let Some(message) = &envelope.message {
                    app.toasts
                        .push(message.clone(), Instant::now(), Duration::from_secs(6));
                }
                app.backend_message = envelope.message;
            }
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

fn read_system_clipboard() -> Result<String> {
    #[cfg(target_os = "macos")]
    let output = std::process::Command::new("pbpaste").output()?;
    #[cfg(target_os = "windows")]
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Clipboard -Raw"])
        .output()?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let output = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()?
    } else {
        std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()?
    };
    anyhow::ensure!(output.status.success(), "system clipboard unavailable");
    Ok(String::from_utf8(output.stdout)?)
}

fn copy_via_osc52(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let written = stdin.write_all(text.as_bytes()).is_ok();
                drop(stdin);
                if child.wait().is_ok_and(|status| status.success()) && written {
                    return Ok(());
                }
            }
        }
    }
    let payload = STANDARD.encode(text.as_bytes());
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{payload}\x07")?;
    stdout.flush()?;
    Ok(())
}
