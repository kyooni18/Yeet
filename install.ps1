
# Usage: ./install.ps1 [-Build] [-Version vX.Y.Z]
#
# Without options, download and install the latest Windows release of Yeet.
# The release archive is SHA-256 verified before anything is installed.
#
#   -Build    Build from a Yeet source checkout instead (requires git, cargo,
#             node, and npm), then install the built artifacts.
#   -Version  Install a specific release tag (defaults to $env:YEET_VERSION,
#             then the latest GitHub release).
#   YEET_PREFIX   Installation prefix (defaults to $env:PREFIX,
#             then $HOME\.local). No administrator rights are required.
param(
    [switch]$Build,
    [string]$Version = $env:YEET_VERSION
)

$ErrorActionPreference = "Stop"


$Repository = if ($env:YEET_REPOSITORY) { $env:YEET_REPOSITORY } else { "kyooni18/Yeet" }
$RepoUrl = if ($env:YEET_REPO) { $env:YEET_REPO } else { "https://github.com/$Repository.git" }
$Source = if ($env:YEET_SOURCE_DIR) { $env:YEET_SOURCE_DIR } else { Join-Path $HOME ".local\src\yeet" }
$Prefix = if ($env:YEET_PREFIX) { $env:YEET_PREFIX } elseif ($env:PREFIX) { $env:PREFIX } else { Join-Path $HOME ".local" }

function Need([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Yeet installer requires: $Name"
    }
}

function Stop-YeetProcesses {
    foreach ($Name in @("yeet", "YeetRemote", "yeet-remote")) {
        Get-Process -Name $Name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
}

function Install-Bundle([string]$Bundle) {
    $Binary = Join-Path $Bundle "bin\yeet.exe"
    $Runtime = Join-Path $Bundle "share\yeet\runtime"
    if (-not (Test-Path $Binary -PathType Leaf)) { throw "install source is missing bin\yeet.exe" }
    if (-not (Test-Path (Join-Path $Runtime "dist\bridge.js") -PathType Leaf)) { throw "install source is missing the Yeet runtime" }
    if (-not (Test-Path (Join-Path $Runtime "skills") -PathType Container)) { throw "install source is missing runtime skills" }
    if (-not (Test-Path (Join-Path $Runtime "package.json") -PathType Leaf)) { throw "install source is missing runtime package metadata" }
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Write-Warning "Node.js 20+ is required to run the Yeet runtime; install it from https://nodejs.org before starting Yeet."
    }

    Stop-YeetProcesses
    $BinDir = Join-Path $Prefix "bin"
    $RuntimeRoot = Join-Path $Prefix "share\yeet"
    New-Item -ItemType Directory -Force $BinDir | Out-Null
    New-Item -ItemType Directory -Force $RuntimeRoot | Out-Null

    # Stage beside the destination and replace, so a partially written
    # installation can never shadow a working one.
    $BinaryStaging = Join-Path $BinDir ".yeet.exe.install-$PID"
    Copy-Item -Force $Binary $BinaryStaging
    Move-Item -Force $BinaryStaging (Join-Path $BinDir "yeet.exe")

    $RuntimeNew = Join-Path $RuntimeRoot ".runtime-new-$PID"
    $RuntimeOld = Join-Path $RuntimeRoot ".runtime-old-$PID"
    if (Test-Path $RuntimeNew) { Remove-Item -Recurse -Force $RuntimeNew }
    if (Test-Path $RuntimeOld) { Remove-Item -Recurse -Force $RuntimeOld }
    New-Item -ItemType Directory -Force $RuntimeNew | Out-Null
    Copy-Item -Recurse -Force (Join-Path $Runtime "dist") (Join-Path $RuntimeNew "dist")
    Copy-Item -Recurse -Force (Join-Path $Runtime "skills") (Join-Path $RuntimeNew "skills")
    Copy-Item -Force (Join-Path $Runtime "package.json") (Join-Path $RuntimeNew "package.json")
    if (Test-Path (Join-Path $RuntimeRoot "runtime")) { Move-Item -Force (Join-Path $RuntimeRoot "runtime") $RuntimeOld }
    Move-Item -Force $RuntimeNew (Join-Path $RuntimeRoot "runtime")
    if (Test-Path $RuntimeOld) { Remove-Item -Recurse -Force $RuntimeOld }

    $Installed = Join-Path $RuntimeRoot "runtime\dist\bridge.js"
    if (-not (Test-Path $Installed -PathType Leaf)) { throw "installed Yeet runtime is incomplete" }

    $BinPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (("$env:YEET_NO_PATH_UPDATE" -ne "1") -and ( ";$BinPath;" -notlike "*;$BinDir;*") -and ( ";$BinPath;" -notlike "*;$BinDir\;*")) {
        [Environment]::SetEnvironmentVariable("Path", "$BinPath;$BinDir", "User")
        Write-Host "Added $BinDir to your user PATH. Open a new terminal to use 'yeet'."
    }

    $InstalledVersion = (& (Join-Path $BinDir "yeet.exe") --version 2>$null | Out-String).Trim()
    Write-Host "Installed Yeet $InstalledVersion to $BinDir\yeet.exe"
    Write-Host "Installed Yeet runtime to $(Join-Path $RuntimeRoot 'runtime')"
}

function Get-TargetTriple {
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" { return "aarch64-pc-windows-msvc" }
        default { throw "unsupported CPU architecture: $env:PROCESSOR_ARCHITECTURE" }
    }
}

function Invoke-GitHubApi([string]$Uri) {
    $Headers = @{ "Accept" = "application/vnd.github+json"; "User-Agent" = "Yeet installer" }
    if ($env:GITHUB_TOKEN) { $Headers["Authorization"] = "Bearer $env:GITHUB_TOKEN" }
    Invoke-RestMethod -Headers $Headers -Uri $Uri
}

if ($Build) {
    if ((Test-Path "Cargo.toml") -and (Test-Path "RuntimeSource") -and (Test-Path "web")) {
        $Root = (Get-Location).Path
    } else {
        Need "git"
        if (Test-Path (Join-Path $Source ".git")) {
            git -C $Source pull --ff-only
        } else {
            if (Test-Path $Source) { Remove-Item -Recurse -Force $Source }
            New-Item -ItemType Directory -Force (Split-Path -Parent $Source) | Out-Null
            git clone --depth 1 $RepoUrl $Source
        }
        $Root = $Source
    }

    Need "cargo"
    Need "node"
    Need "npm"

    Stop-YeetProcesses
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
    Write-Host "Yeet is installed at $(Join-Path $Prefix 'bin\yeet.exe')"
    return
}

# An extracted release bundle installs directly.
$LocalBundle = $PSScriptRoot
if (Test-Path (Join-Path $LocalBundle "bin\yeet.exe") -PathType Leaf) {
    Install-Bundle $LocalBundle
    return
}

if ($Version -and $Version -notlike "v*") { $Version = "v$Version" }
if (-not $Version) {
    $Release = Invoke-GitHubApi "https://api.github.com/repos/$Repository/releases/latest"
    $Version = $Release.tag_name
    if (-not $Version) { throw "could not determine the latest GitHub release" }
}
$Number = $Version.TrimStart("v")
$Target = Get-TargetTriple
$Base = "yeet-$Number-$Target"
$Archive = "$Base.zip"
$Url = "https://github.com/$Repository/releases/download/$Version/$Archive"

$Temp = Join-Path ([IO.Path]::GetTempPath()) "yeet-install-$PID"
try {
    New-Item -ItemType Directory -Force $Temp | Out-Null
    Write-Host "Downloading Yeet $Version for $Target..."
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile (Join-Path $Temp $Archive) -ErrorAction Stop
    Invoke-WebRequest -UseBasicParsing -Uri "$Url.sha256" -OutFile (Join-Path $Temp "$Archive.sha256") -ErrorAction Stop
    $Expected = ((Get-Content (Join-Path $Temp "$Archive.sha256") -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
    $Actual = (Get-FileHash (Join-Path $Temp $Archive) -Algorithm SHA256).Hash.ToLowerInvariant()
    if (-not $Expected) { throw "release checksum is empty" }
    if ($Actual -ne $Expected) { throw "release SHA-256 verification failed" }

    Expand-Archive -Force (Join-Path $Temp $Archive) -DestinationPath $Temp
    $Bundle = Join-Path $Temp $Base
    if (-not (Test-Path $Bundle -PathType Container)) { throw "release archive did not contain $Base" }
    Install-Bundle $Bundle
} catch {
    if ($_.Exception.Message -like "*404*") {
        throw "No published release '$Version' provides $Archive. Publish a release or use -Build against a source checkout."
    }
    throw
} finally {
    if (Test-Path $Temp) { Remove-Item -Recurse -Force $Temp -ErrorAction SilentlyContinue }
}
