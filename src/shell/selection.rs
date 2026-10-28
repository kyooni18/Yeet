use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellKind {
    Bash,
    Zsh,
    Sh,
    Powershell,
    Cmd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    /// Windows: place `args` on the command line verbatim instead of applying
    /// MSVCRT argument quoting. Cmd and PowerShell parse their command lines
    /// with their own rules and must receive script text unescaped.
    #[serde(default)]
    pub verbatim: bool,
}
impl LaunchSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        #[cfg(windows)]
        if self.verbatim {
            use std::os::windows::process::CommandExt;
            for arg in &self.args {
                command.raw_arg(arg);
            }
            return command;
        }
        let _ = &self.verbatim;
        command.args(&self.args);
        command
    }

    /// Windows caps process command lines at 32,767 UTF-16 code units. Keep
    /// headroom for quoting and refuse oversized launches with an actionable
    /// error instead of a raw CreateProcess failure.
    #[cfg(windows)]
    pub fn ensure_command_line_fits(&self, context: &str) -> Result<()> {
        let mut units = self.executable.to_string_lossy().encode_utf16().count() + 3;
        for arg in &self.args {
            units += arg.encode_utf16().count() + 3;
        }
        if units > 30_000 {
            bail!(
                "{} is too long for a Windows process command line ({} UTF-16 units, limit 30,000). Write the script to a file and run it with a short command instead.",
                context,
                units
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SelectedShell {
    pub kind: ShellKind,
    pub executable: PathBuf,
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn find(name: &str) -> Option<PathBuf> {
    find_in(name, |_| true)
}

#[cfg(windows)]
fn is_store_shim_dir(dir: &Path) -> bool {
    // The Microsoft Store `WindowsApps` folder hosts executable alias stubs
    // that open the Store when the real app is not installed.
    dir.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("windowsapps")
    })
}

/// PATH scan that skips executable-alias shim directories: the Microsoft Store
/// `WindowsApps` folder (whose stubs open the Store when the app is missing)
/// and the system directories hosting the WSL `bash.exe` launcher, which is
/// not Git Bash.
#[cfg(windows)]
fn find_real(name: &str) -> Option<PathBuf> {
    find_in(name, |dir| !is_alias_shim_dir(dir))
}

fn find_in(name: &str, accept: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|p| p.is_absolute() && accept(p))
        .map(|p| p.join(name))
        .find(|p| executable(p))
}

#[cfg(windows)]
fn is_alias_shim_dir(dir: &Path) -> bool {
    if is_store_shim_dir(dir) {
        return true;
    }
    let Some(root) = std::env::var_os("SystemRoot") else {
        return false;
    };
    let dir = dir.to_string_lossy().to_ascii_lowercase();
    ["system32", "syswow64"].iter().any(|suffix| {
        dir == PathBuf::from(&root)
            .join(suffix)
            .to_string_lossy()
            .to_ascii_lowercase()
    })
}

/// Known Git for Windows layouts: machine installs under Program Files,
/// per-user installs under LocalAppData, and Scoop installs.
#[cfg(windows)]
fn git_bash_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut push = |base: &Path| {
        for suffix in [
            ["Git", "bin", "bash.exe"].as_slice(),
            ["Programs", "Git", "bin", "bash.exe"].as_slice(),
        ] {
            let mut path = base.to_path_buf();
            path.extend(suffix);
            candidates.push(path);
        }
    };
    for base in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        push(&PathBuf::from(base));
    }
    for base in [
        dirs::home_dir().map(|home| home.join("scoop").join("apps")),
        std::env::var_os("ProgramData").map(|data| PathBuf::from(data).join("scoop").join("apps")),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(
            base.join("git")
                .join("current")
                .join("bin")
                .join("bash.exe"),
        );
    }
    candidates
}

#[cfg(windows)]
fn windows_system(name: &str) -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot")?;
    Some(PathBuf::from(root).join("System32").join(name))
}

/// Resolves a direct `executable` the way Windows command lookup would
/// (PATH plus PATHEXT), returning the concrete path to launch. Batch files
/// are rejected with an explicit error: cmd re-expands their arguments
/// (`%*`), so arbitrary arguments cannot be passed safely without a shell
/// script of the caller's own.
#[cfg(windows)]
pub fn resolve_windows_executable(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    let mut batch_fallback = None;
    let mut consider = |candidate: PathBuf| -> Option<PathBuf> {
        if !executable(&candidate) {
            return None;
        }
        let extension = candidate
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some("exe") | Some("com") => Some(candidate),
            Some("bat") | Some("cmd") => {
                batch_fallback.get_or_insert(candidate);
                None
            }
            _ => None,
        }
    };
    let exts = std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let exts = std::env::split_paths(&exts)
        .map(|ext| ext.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let found = if name.contains(['\\', '/']) || path.is_absolute() {
        let mut found = None;
        for candidate in path.with_extension_candidates(&exts) {
            if let Some(resolved) = consider(candidate) {
                found = Some(resolved);
                break;
            }
        }
        found
    } else {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .filter(|dir| dir.is_absolute() && !is_store_shim_dir(dir))
            .flat_map(|dir| dir.join(name).with_extension_candidates(&exts))
            .find_map(&mut consider)
    };
    if let Some(found) = found {
        return Ok(found);
    }
    if let Some(batch) = batch_fallback {
        bail!(
            "'{}' resolves to the Windows batch file '{}', which cannot take arbitrary arguments safely. Run it through a shell command instead (for example command: \"{} ...\"), or point executable at an .exe.",
            name,
            batch.display(),
            name
        );
    }
    bail!("Executable '{name}' was not found on PATH")
}

/// The candidate path itself plus PATHEXT-suffixed variants, in lookup order.
#[cfg(windows)]
trait ExtensionCandidates {
    fn with_extension_candidates(&self, exts: &[String]) -> Vec<PathBuf>;
}
#[cfg(windows)]
impl ExtensionCandidates for Path {
    fn with_extension_candidates(&self, exts: &[String]) -> Vec<PathBuf> {
        // Windows appends PATHEXT suffixes only to extension-less names.
        if self.extension().is_some() {
            return vec![self.to_path_buf()];
        }
        let mut candidates = Vec::with_capacity(exts.len() + 1);
        for ext in exts {
            let ext = ext.trim_start_matches('.');
            if !ext.is_empty() {
                candidates.push(self.with_extension(ext));
            }
        }
        candidates.push(self.to_path_buf());
        candidates
    }
}

fn discover(kind: ShellKind) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        match kind {
            ShellKind::Bash => {
                // Probe known Git for Windows locations first, so the PATH
                // scan cannot mistake the WSL or Store bash launcher for
                // Git Bash.
                git_bash_candidates()
                    .into_iter()
                    .find(|p| executable(p))
                    .or_else(|| find_real("bash.exe"))
            }
            ShellKind::Powershell => find_real("pwsh.exe")
                .or_else(|| {
                    let root = std::env::var_os("SystemRoot")?;
                    let path = PathBuf::from(root)
                        .join("System32")
                        .join("WindowsPowerShell")
                        .join("v1.0")
                        .join("powershell.exe");
                    executable(&path).then_some(path)
                })
                .or_else(|| find_real("powershell.exe")),
            ShellKind::Cmd => windows_system("cmd.exe")
                .filter(|path| executable(path))
                .or_else(|| find("cmd.exe")),
            ShellKind::Zsh => find_real("zsh.exe"),
            ShellKind::Sh => find_real("sh.exe"),
        }
    }
    #[cfg(not(windows))]
    {
        let name = match kind {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Sh => "sh",
            ShellKind::Powershell => "pwsh",
            ShellKind::Cmd => return None,
        };
        find(name).or_else(|| {
            let p = Path::new("/bin").join(name);
            executable(&p).then_some(p)
        })
    }
}
fn preferred() -> &'static [ShellKind] {
    #[cfg(windows)]
    {
        &[ShellKind::Bash, ShellKind::Powershell, ShellKind::Cmd]
    }
    #[cfg(target_os = "macos")]
    {
        &[ShellKind::Bash, ShellKind::Zsh, ShellKind::Sh]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        &[ShellKind::Bash, ShellKind::Sh]
    }
}
pub fn select(request: &str) -> Result<SelectedShell> {
    let kinds: &[ShellKind] = match request {
        "auto" => preferred(),
        "bash" => &[ShellKind::Bash],
        "zsh" => &[ShellKind::Zsh],
        "sh" => &[ShellKind::Sh],
        "powershell" => &[ShellKind::Powershell],
        "cmd" => &[ShellKind::Cmd],
        _ => bail!("shell must be auto, bash, zsh, sh, powershell, or cmd"),
    };
    static CACHE: OnceLock<Mutex<HashMap<ShellKind, Option<PathBuf>>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(selected) = choose(kinds, |kind| {
        cache.entry(kind).or_insert_with(|| discover(kind)).clone()
    }) {
        return Ok(selected);
    }
    bail!("Requested shell '{request}' is unavailable")
}
fn choose(
    kinds: &[ShellKind],
    mut discover: impl FnMut(ShellKind) -> Option<PathBuf>,
) -> Option<SelectedShell> {
    kinds
        .iter()
        .find_map(|&kind| discover(kind).map(|executable| SelectedShell { kind, executable }))
}
#[cfg(windows)]
fn powershell_utf8_script(script: &str) -> String {
    // Emit UTF-8 regardless of the console code page so Unicode output
    // decodes; tolerate hosts without a console.
    format!(
        "try {{ [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) }} catch {{ }}\n$OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n{script}"
    )
}

#[cfg(windows)]
fn encode_powershell_script(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl SelectedShell {
    pub fn launch(&self, script: &str, load_config: bool, login: bool) -> LaunchSpec {
        let mut args = Vec::new();
        match self.kind {
            ShellKind::Bash => {
                if !load_config {
                    args.extend(["--noprofile".into(), "--norc".into()]);
                }
                if login {
                    args.push("-l".into());
                }
                args.extend(["-c".into(), script.into()]);
            }
            ShellKind::Zsh => {
                if !load_config {
                    args.push("-f".into());
                }
                if login {
                    args.push("-l".into());
                }
                args.extend(["-c".into(), script.into()]);
            }
            ShellKind::Sh => {
                if login {
                    args.push("-l".into());
                }
                args.extend(["-c".into(), script.into()]);
            }
            ShellKind::Powershell => {
                if !load_config {
                    args.push("-NoProfile".into());
                }
                args.push("-NonInteractive".into());
                #[cfg(windows)]
                {
                    // PowerShell reparses its command line with its own rules,
                    // so any script text containing quotes can be mangled as
                    // -Command. An encoded script is quote-proof and Unicode
                    // safe for both powershell.exe and pwsh.exe.
                    args.push("-EncodedCommand".into());
                    args.push(encode_powershell_script(&powershell_utf8_script(script)));
                }
                #[cfg(not(windows))]
                {
                    args.extend(["-Command".into(), script.into()]);
                }
            }
            ShellKind::Cmd => {
                args.extend(["/D".into(), "/S".into(), "/C".into()]);
                #[cfg(windows)]
                {
                    // chcp 65001 makes cmd's internal commands emit UTF-8 so
                    // Unicode output decodes correctly; a console-less host
                    // keeps its code page and the error stays silenced. The
                    // script rides the command line verbatim inside one quote
                    // pair, which cmd /S strips positionally, so embedded
                    // quotes survive untouched.
                    args.push(format!("\"chcp 65001 >nul 2>&1 & {script}\""));
                }
                #[cfg(not(windows))]
                {
                    args.push(script.into());
                }
            }
        }
        LaunchSpec {
            executable: self.executable.clone(),
            args,
            // Cmd reparses its command line itself, so its script argument is
            // placed verbatim; every other shell consumes argv entries.
            verbatim: cfg!(windows) && self.kind == ShellKind::Cmd,
        }
    }
    /// AppContainer cannot write the host's private report file. Its backend
    /// captures output after termination, so emit a nonce-delimited trailer.
    #[cfg(windows)]
    pub fn wrap_cwd_trailer(&self, script: &str, marker: &str) -> String {
        match self.kind {
            ShellKind::Cmd => format!(
                r#"{script} & call set "__yeet_status=%%errorlevel%%" & echo. & echo {marker} & cd & call exit /b %%__yeet_status%%"#
            ),
            ShellKind::Powershell => format!(
                "{script}\n; $__yeet_ok = $?; $__yeet_code = $LASTEXITCODE; [Console]::Out.WriteLine(); [Console]::Out.WriteLine('{marker}'); [Console]::Out.WriteLine((Get-Location).ProviderPath); if ($null -ne $__yeet_code) {{ exit $__yeet_code }} elseif (!$__yeet_ok) {{ exit 1 }}"
            ),
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => format!(
                "{script}\n__yeet_status=$?\nprintf '\n%s\n' '{marker}'\ncommand pwd -W\nexit $__yeet_status"
            ),
        }
    }
    pub fn wrap_cwd(&self, script: &str, path: &Path) -> Option<String> {
        let p = path.to_str()?;
        match self.kind {
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
                let pwd = if cfg!(windows) {
                    "command pwd -W"
                } else {
                    "pwd -P"
                };
                Some(format!(
                    "{script}\n__yeet_status=$?\nprintf '%s' \"$({pwd})\" > '{}'\nexit $__yeet_status",
                    p.replace('\'', "'\\''")
                ))
            }
            ShellKind::Powershell => Some(format!(
                "{script}\n; $__yeet_ok = $?; $__yeet_code = $LASTEXITCODE; [IO.File]::WriteAllText('{}', (Get-Location).ProviderPath); if ($null -ne $__yeet_code) {{ exit $__yeet_code }} elseif (!$__yeet_ok) {{ exit 1 }}",
                p.replace('\'', "''")
            )),
            // The report path is supplied via an environment variable rather than
            // interpolated into cmd syntax (where %, &, and quotes are special).
            // CALL performs a second expansion after the user command, retaining
            // its exit status before CD writes the report.
            ShellKind::Cmd => Some(format!(
                r#"{script} & call set "__yeet_status=%%errorlevel%%" & cd > "%YEET_INTERNAL_CWD_REPORT%" & call exit /b %%__yeet_status%%"#
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_falls_back_and_override_is_validated() {
        let chosen = choose(&[ShellKind::Bash, ShellKind::Sh], |kind| {
            (kind == ShellKind::Sh).then(|| PathBuf::from("/test/sh"))
        })
        .unwrap();
        assert_eq!(chosen.kind, ShellKind::Sh);
        assert!(choose(&[ShellKind::Bash], |_| None).is_none());
        assert!(select("bogus").is_err());
        for (name, kind) in [
            ("bash", ShellKind::Bash),
            ("zsh", ShellKind::Zsh),
            ("sh", ShellKind::Sh),
            ("powershell", ShellKind::Powershell),
            ("cmd", ShellKind::Cmd),
        ] {
            match select(name) {
                Ok(shell) => {
                    assert_eq!(shell.kind, kind);
                    assert_eq!(shell.executable, select(name).unwrap().executable);
                }
                Err(error) => assert!(error.to_string().contains("unavailable")),
            }
        }
        assert_eq!(
            select("auto").unwrap().kind,
            preferred()
                .iter()
                .copied()
                .find(|k| discover(*k).is_some())
                .unwrap()
        );
    }
    #[test]
    fn shell_flags_and_cwd_quoting_are_shell_specific() {
        let shell = SelectedShell {
            kind: ShellKind::Bash,
            executable: "bash".into(),
        };
        let script = shell.wrap_cwd("true", Path::new("/tmp/a'b")).unwrap();
        assert!(script.contains("'/tmp/a'\\''b'"));
        assert_eq!(
            &shell.launch("true", false, false).args[..2],
            ["--noprofile", "--norc"]
        );
        let configured = shell.launch("true", true, true);
        assert!(!configured.args.contains(&"--noprofile".to_owned()));
        assert!(configured.args.contains(&"-l".to_owned()));
        let shell = SelectedShell {
            kind: ShellKind::Powershell,
            executable: "pwsh".into(),
        };
        assert!(
            shell
                .wrap_cwd("true", Path::new("a'b"))
                .unwrap()
                .contains("'a''b'")
        );
        assert_eq!(shell.launch("true", false, false).args[0], "-NoProfile");
        let shell = SelectedShell {
            kind: ShellKind::Cmd,
            executable: "cmd.exe".into(),
        };
        let wrapped = shell
            .wrap_cwd("cd sub", Path::new("C:\u{5c}a & %b%\u{5c}cwd"))
            .unwrap();
        assert!(wrapped.contains("%YEET_INTERNAL_CWD_REPORT%"));
        assert!(!wrapped.contains("a & %b%"));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn cmd_reports_directory_and_keeps_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub & space");
        std::fs::create_dir(&sub).unwrap();
        let report = root.path().join("report % &.txt");
        let shell = select("cmd").unwrap();
        let script = shell
            .wrap_cwd("cd /d \"sub & space\" & cmd /c exit 7", &report)
            .unwrap();
        let status = shell
            .launch(&script, false, false)
            .command()
            .env("YEET_INTERNAL_CWD_REPORT", &report)
            .current_dir(root.path())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(7));
        let path = std::fs::read_to_string(report).unwrap();
        assert_eq!(
            PathBuf::from(path.trim_end_matches(['\r', '\n']))
                .canonicalize()
                .unwrap(),
            sub.canonicalize().unwrap()
        );
    }

    #[test]
    fn powershell_reports_unicode_directory_and_keeps_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("space & ünicode");
        std::fs::create_dir(&sub).unwrap();
        let report = root.path().join("report ' &.txt");
        let shell = select("powershell").expect("Windows PowerShell should be available");
        let script = shell
            .wrap_cwd(
                "Set-Location -LiteralPath 'space & ünicode'; cmd /c exit 7",
                &report,
            )
            .unwrap();
        let output = shell
            .launch(&script, false, false)
            .command()
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{:?}", output);
        let path = std::fs::read_to_string(report).unwrap();
        assert_eq!(
            PathBuf::from(path).canonicalize().unwrap(),
            sub.canonicalize().unwrap()
        );
    }

    #[test]
    fn git_bash_reports_directory_and_keeps_exit_status_when_installed() {
        let Ok(shell) = select("bash") else {
            eprintln!("Git Bash not installed; this test requires it");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("space & dir");
        std::fs::create_dir(&sub).unwrap();
        let report = root.path().join("report ' &.txt");
        let script = shell.wrap_cwd("cd 'space & dir'; false", &report).unwrap();
        let output = shell
            .launch(&script, false, false)
            .command()
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{:?}", output);
        let path = std::fs::read_to_string(report).unwrap();
        assert_eq!(
            PathBuf::from(path).canonicalize().unwrap(),
            sub.canonicalize().unwrap()
        );
    }

    #[test]
    fn cmd_launch_is_verbatim_quoted_and_forces_utf8_output() {
        let shell = SelectedShell {
            kind: ShellKind::Cmd,
            executable: "cmd.exe".into(),
        };
        let script = r#"echo "a b" & echo %PATH%"#;
        let spec = shell.launch(script, false, false);
        assert!(spec.verbatim);
        assert_eq!(spec.args[..3], ["/D", "/S", "/C"]);
        // The script keeps its quotes untouched and is wrapped in one quote
        // pair that cmd /S strips positionally.
        assert_eq!(spec.args[3], format!("\"chcp 65001 >nul 2>&1 & {script}\""));
        assert!(spec.args[3].contains(r#"echo "a b""#));
    }

    #[test]
    fn powershell_launch_encodes_the_script_verbatim() {
        let shell = SelectedShell {
            kind: ShellKind::Powershell,
            executable: "pwsh".into(),
        };
        let script = r#"Write-Output "quoted & spaced"; Get-Location"#;
        let spec = shell.launch(script, false, false);
        assert!(!spec.verbatim, "base64 payload is argv-safe");
        assert_eq!(spec.args[0], "-NoProfile");
        assert_eq!(spec.args[1], "-NonInteractive");
        assert_eq!(spec.args[2], "-EncodedCommand");
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&spec.args[3])
            .unwrap();
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let decoded = String::from_utf16(&units).unwrap();
        assert!(decoded.ends_with(script), "{decoded}");
        assert!(decoded.contains("[Console]::OutputEncoding"), "{decoded}");
        let configured = shell.launch(script, true, false);
        assert!(!configured.args.contains(&"-NoProfile".to_owned()));
    }

    #[test]
    fn executable_resolution_prefers_runnable_images_and_rejects_batch() {
        let directory = tempfile::tempdir().unwrap();
        let batch = directory.path().join("tool.cmd");
        std::fs::write(&batch, "@echo off").unwrap();
        let error = resolve_windows_executable(directory.path().join("tool").to_str().unwrap())
            .unwrap_err();
        assert!(error.to_string().contains("batch file"), "{error}");

        let image = directory.path().join("tool.exe");
        std::fs::write(&image, []).unwrap();
        let resolved =
            resolve_windows_executable(directory.path().join("tool").to_str().unwrap()).unwrap();
        assert_eq!(resolved, image);
        std::fs::remove_file(&image).unwrap();

        let error = resolve_windows_executable(directory.path().join("missing").to_str().unwrap())
            .unwrap_err();
        assert!(error.to_string().contains("not found"), "{error}");
    }
}
