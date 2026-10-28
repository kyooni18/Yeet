# Command execution

`run_shell` accepts either `command` (shell syntax), or `executable` with an
`args` string array (no shell parsing). `mode` still controls output routing;
`mode: "direct"` does not mean direct process execution. The same jobs,
working-directory selection, cancellation, sandbox and approval machinery is
used for both. Direct processes are conservatively treated as operations needing
approval under restricted policy, even for read-only executables. Shell options
cannot be combined with direct execution.

`shell` can be `auto` (default), `bash`, `zsh`, `sh`, `powershell`, or `cmd`.
Discovery is cached for the process lifetime. Auto chooses the first available
shell; an exit failure is never retried in another shell. On Windows the order
is Git Bash, PowerShell 7 (`pwsh.exe`), Windows PowerShell, then `cmd.exe`,
with known Git for Windows machine, per-user, and Scoop installs probed before
PATH scanning. Microsoft Store and WSL executable-alias shim directories are
never selected, so `auto` cannot mistake WSL Bash for Git Bash or launch a
Store stub. The default is a clean, non-interactive invocation. `loadConfig: true` permits the selected shell's normal
non-interactive startup behavior; it does not force an interactive rc file to be
read. `login: true` requests a login shell where supported. Login `sh` requires
`loadConfig: true` because it can read profiles. Sandboxed environments still
restrict home and filesystem access, including to startup files.

Windows launches quote per shell. Cmd receives the script verbatim inside one
quote pair that `cmd /S` strips positionally (with `chcp 65001` forcing UTF-8
output); PowerShell scripts travel as `-EncodedCommand` base64 UTF-16 (with
UTF-8 console encodings set inside the script), so quotes and Unicode survive
both `powershell.exe` and `pwsh.exe` command-line parsing. Bash scripts use
argv quoting matching the MSVCRT rules Git Bash parses with. Composed command
lines are validated against the Windows 32,767-unit limit with actionable
errors. Captured output additionally decodes BOM-marked UTF-16 streams.
Direct execution resolves `executable` through PATH + PATHEXT like Windows
command lookup and reports batch files (`npm`, for example) as an explicit
error directing the caller to run them through a shell command.

Only a foreground shell's working directory is tracked; independent shell
variables and functions are not retained. A direct process cannot change the
parent session directory. Cwd reporting is best effort: explicit shell `exit`,
`exec`, an unwritable sandbox scratch directory, or a shell without a supported
reporting wrapper can prevent it. Windows AppContainer cwd reporting uses a nonce-delimited captured-output trailer;
truncated output or an explicit shell exit can prevent reporting. Git Bash uses
its native Windows-path `pwd -W` for this trailer. Windows AppContainer also has its existing output/transcript and
job cancellation limitations. No fallback to an unrestricted process is made
when a sandbox cannot run the selected executable.
