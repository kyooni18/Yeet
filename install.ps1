$ErrorActionPreference = "Stop"

$Repo = if ($env:YEET_REPO) { $env:YEET_REPO } else { "https://github.com/kyooni18/Yeet.git" }
$Source = if ($env:YEET_SOURCE_DIR) { $env:YEET_SOURCE_DIR } else { Join-Path $HOME ".local\src\yeet" }

function Need([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Yeet build requires: $Name"
    }
}

function Stop-YeetProcesses {
    foreach ($Name in @("yeet", "YeetRemote", "yeet-remote")) {
        Get-Process -Name $Name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
}

Stop-YeetProcesses

if ((Test-Path "Cargo.toml") -and (Test-Path "RuntimeSource") -and (Test-Path "web")) {
    $Root = (Get-Location).Path
} else {
    Need "git"
    if (Test-Path (Join-Path $Source ".git")) {
        git -C $Source pull --ff-only
    } else {
        if (Test-Path $Source) { Remove-Item -Recurse -Force $Source }
        New-Item -ItemType Directory -Force (Split-Path -Parent $Source) | Out-Null
        git clone --depth 1 $Repo $Source
    }
    $Root = $Source
}

Need "cargo"
Need "node"
Need "npm"

Push-Location $Root
try {
    npm --prefix RuntimeSource ci
    npm --prefix web ci
    npm --prefix RuntimeSource run build
    npm --prefix web run build
    cargo build --release
    $env:YEET_SOURCE_ROOT = $Root
    & (Join-Path $Root "target\release\yeet.exe") install binary runtime
} finally {
    Pop-Location
}

$Prefix = if ($env:YEET_PREFIX) { $env:YEET_PREFIX } elseif ($env:PREFIX) { $env:PREFIX } else { Join-Path $HOME ".local" }
Write-Host "Yeet is installed at $(Join-Path $Prefix 'bin\yeet.exe')"
