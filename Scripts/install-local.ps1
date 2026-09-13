$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust/Cargo is required to build Yeet."
}
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw "Node.js is required for the Yeet runtime."
}

$Prefix = if ($env:PREFIX) {
    $env:PREFIX
} elseif ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA "Programs\Yeet"
} elseif ($HOME) {
    Join-Path $HOME ".local\yeet"
} else {
    throw "Unable to determine a local install prefix. Set PREFIX and retry."
}

$Destination = Join-Path $Prefix "bin"
$RuntimeDestination = Join-Path $Prefix "share\yeet\runtime"
$RuntimeBuild = Join-Path $Root "target\install-runtime"
$RuntimeDist = Join-Path $RuntimeBuild "dist"

Push-Location $Root
try {
    $env:YEET_RUNTIME_OUT_DIR = $RuntimeDist
    & (Join-Path $Root "Scripts\rebuild-runtime.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Runtime build failed." }

    & (Join-Path $Root "Scripts\build-remote-web.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Remote WebUI build failed." }

    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Rust release build failed." }

    New-Item -ItemType Directory -Force $Destination | Out-Null
    New-Item -ItemType Directory -Force $RuntimeDestination | Out-Null

    $BuiltBinary = Join-Path $Root "target\release\yeet.exe"
    $InstalledBinary = Join-Path $Destination "yeet.exe"
    $RunningMcpPorts = @()
    if (Test-Path $InstalledBinary) {
        $McpList = & $InstalledBinary mcpserver list 2>$null
        if ($LASTEXITCODE -eq 0) {
            foreach ($Line in $McpList) {
                $Fields = @($Line -split '\s+' | Where-Object { $_ })
                if ($Fields.Count -ge 2 -and $Fields[1] -eq "running") {
                    $RunningMcpPorts += $Fields[0]
                }
            }
        }
        foreach ($Port in $RunningMcpPorts) {
            Write-Host "Stopping Yeet MCP daemon on port $Port before replacing the executable"
            & $InstalledBinary mcpserver stop --port $Port | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to stop Yeet MCP daemon on port $Port before install."
            }
        }
    }
    Copy-Item -Force $BuiltBinary $InstalledBinary

    if ((Get-FileHash $BuiltBinary -Algorithm SHA256).Hash -ne
        (Get-FileHash $InstalledBinary -Algorithm SHA256).Hash) {
        throw "Installed yeet.exe does not match the freshly built release."
    }

    $InstalledDist = Join-Path $RuntimeDestination "dist"
    if (Test-Path $InstalledDist) {
        Remove-Item -Recurse -Force $InstalledDist
    }
    Copy-Item -Recurse -Force $RuntimeDist $InstalledDist
    Copy-Item -Force (Join-Path $Root "RuntimeSource\package.json") (Join-Path $RuntimeDestination "package.json")

    # Resume every daemon that was running before the install using the freshly
    # replaced executable and runtime. Stopping first is required on Windows,
    # where an executing image may not be safely overwritten in place.
    foreach ($Port in $RunningMcpPorts) {
        Write-Host "Starting Yeet MCP daemon on port $Port after local install"
        & $InstalledBinary mcpserver start --port $Port | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to start Yeet MCP daemon on port $Port after install."
        }
    }
} finally {
    Pop-Location
}

Write-Host "Install prefix: $Prefix"
Write-Host "Installed yeet to $(Join-Path $Destination 'yeet.exe')"
Write-Host "Installed runtime to $RuntimeDestination"

$PathEntries = ($env:Path -split ';')
if ($PathEntries -notcontains $Destination) {
    Write-Host "Note: add $Destination to PATH to run yeet by name."
}
