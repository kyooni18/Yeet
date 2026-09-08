$ErrorActionPreference = "Stop"

$Root = $PSScriptRoot
Set-Location $Root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust/Cargo is required to build Yeet."
}
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw "Node.js is required for the Yeet runtime."
}

$NodeMajor = [int]((node -p "Number(process.versions.node.split('.')[0])").Trim())
if ($NodeMajor -lt 20) {
    throw "Yeet requires Node.js 20 or newer."
}

$Tsc = Join-Path $Root "RuntimeSource\node_modules\.bin\tsc.cmd"
if (-not (Test-Path $Tsc -PathType Leaf)) {
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        throw "npm is required to install Yeet runtime dependencies."
    }
    Write-Host "Installing Yeet runtime dependencies..."
    Push-Location (Join-Path $Root "RuntimeSource")
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw "npm ci failed." }
    } finally {
        Pop-Location
    }
}

Write-Host "Building and installing Yeet..."
& (Join-Path $Root "Scripts\install-local.ps1")
$env:YEET_RUNTIME_DIR = Join-Path $Root "target\install-runtime"
