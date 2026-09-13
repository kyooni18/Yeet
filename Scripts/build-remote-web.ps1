$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Web = Join-Path $Root 'web'
$DefaultDist = Join-Path $Web 'dist'
$Dist = if ($env:YEET_REMOTE_WEB_DIST) { $env:YEET_REMOTE_WEB_DIST } else { $DefaultDist }

if (-not (Test-Path (Join-Path $Web 'package.json'))) {
    throw "Yeet Remote WebUI source is missing: $Web/package.json"
}

if (Get-Command pnpm -ErrorAction SilentlyContinue) {
    if (-not (Test-Path (Join-Path $Web 'node_modules'))) {
        & pnpm --dir $Web install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw "Remote WebUI dependency install failed with exit code $LASTEXITCODE" }
    }
    & pnpm --dir $Web build
} elseif (Get-Command corepack -ErrorAction SilentlyContinue) {
    if (-not (Test-Path (Join-Path $Web 'node_modules'))) {
        & corepack pnpm --dir $Web install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw "Remote WebUI dependency install failed with exit code $LASTEXITCODE" }
    }
    & corepack pnpm --dir $Web build
} elseif (Get-Command npm -ErrorAction SilentlyContinue) {
    if (-not (Test-Path (Join-Path $Web 'node_modules'))) {
        throw 'pnpm is required to install the locked Remote WebUI dependencies.'
    }
    & npm --prefix $Web run build
} else {
    throw 'Node package runner not found. Install pnpm (preferred) or npm.'
}

if ($LASTEXITCODE -ne 0) {
    throw "Remote WebUI build failed with exit code $LASTEXITCODE"
}

$BuiltIndex = Join-Path $DefaultDist 'index.html'
if (-not (Test-Path $BuiltIndex)) {
    throw 'Remote WebUI build did not produce web/dist/index.html'
}

if ($Dist -ne $DefaultDist) {
    if (Test-Path $Dist) { Remove-Item -Recurse -Force $Dist }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Dist) | Out-Null
    Copy-Item -Recurse -Force $DefaultDist $Dist
}

$Index = Join-Path $Dist 'index.html'
if ((Get-Content -Raw $Index) -match '/src/main\.ts') {
    throw 'Remote WebUI output still references Vite development source.'
}

Write-Host "Remote WebUI rebuilt at $Dist"
