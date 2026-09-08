$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Source = Join-Path $Root "RuntimeSource"
$Out = if ($env:YEET_RUNTIME_OUT_DIR) {
    if ([System.IO.Path]::IsPathRooted($env:YEET_RUNTIME_OUT_DIR)) {
        $env:YEET_RUNTIME_OUT_DIR
    } else {
        Join-Path $Root $env:YEET_RUNTIME_OUT_DIR
    }
} else {
    Join-Path $Source "dist"
}

$Tsc = Join-Path $Source "node_modules\.bin\tsc.cmd"
if (-not (Test-Path $Tsc -PathType Leaf)) {
    $GlobalTsc = Get-Command tsc.cmd -ErrorAction SilentlyContinue
    if (-not $GlobalTsc) {
        $GlobalTsc = Get-Command tsc -ErrorAction SilentlyContinue
    }
    if (-not $GlobalTsc) {
        throw "TypeScript compiler not found. Run npm ci in RuntimeSource first."
    }
    $Tsc = $GlobalTsc.Source
}

if (Test-Path $Out) {
    Remove-Item -Recurse -Force $Out
}
New-Item -ItemType Directory -Force $Out | Out-Null

Push-Location $Source
try {
    & $Tsc -p tsconfig.json --outDir $Out
    if ($LASTEXITCODE -ne 0) { throw "TypeScript build failed." }
    node --check (Join-Path $Out "bridge.js")
    if ($LASTEXITCODE -ne 0) { throw "bridge.js syntax check failed." }
    node --check (Join-Path $Out "edit-backend\daemon.js")
    if ($LASTEXITCODE -ne 0) { throw "edit-backend daemon syntax check failed." }
} finally {
    Pop-Location
}

Write-Host "Runtime rebuilt at $Out"
