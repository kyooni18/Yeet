$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Version = if ($env:YEET_VERSION) {
    $env:YEET_VERSION.TrimStart('v')
} else {
    $Cargo = Get-Content -Raw (Join-Path $Root "Cargo.toml")
    $Match = [regex]::Match($Cargo, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $Match.Success) { throw "Unable to determine package version from Cargo.toml." }
    $Match.Groups[1].Value
}
$HostLine = (& rustc -vV | Where-Object { $_ -like 'host: *' } | Select-Object -First 1)
if (-not $HostLine) { throw "Unable to determine Rust host target." }
$Target = $HostLine.Substring(6).Trim()
$Name = "yeet-$Version-$Target"
$Out = if ($env:YEET_PACKAGE_OUT) { $env:YEET_PACKAGE_OUT } else { Join-Path $Root "target\packages" }
$StageBase = Join-Path $Root "target\package-stage"
$Stage = Join-Path $StageBase $Name
$RuntimeBuild = Join-Path $Root "target\release-runtime-$Target"
$RuntimeDist = Join-Path $RuntimeBuild "dist"

Push-Location $Root
try {
    if (-not (Test-Path "RuntimeSource\node_modules\.bin\tsc.cmd")) {
        npm --prefix RuntimeSource ci
        if ($LASTEXITCODE -ne 0) { throw "RuntimeSource dependency install failed." }
    }

    $env:YEET_RUNTIME_OUT_DIR = $RuntimeDist
    & (Join-Path $Root "Scripts\rebuild-runtime.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Runtime build failed." }
    & (Join-Path $Root "Scripts\build-remote-web.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Remote WebUI build failed." }
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Rust release build failed." }

    $BuiltBinary = Join-Path $Root "target\release\yeet.exe"
    if ($env:YEET_SIGN_PFX) {
        if (-not $env:YEET_SIGN_PASSWORD) { throw "YEET_SIGN_PASSWORD is required when YEET_SIGN_PFX is set." }
        $SignTool = if ($env:YEET_SIGNTOOL) {
            $env:YEET_SIGNTOOL
        } else {
            $Found = Get-Command signtool.exe -ErrorAction SilentlyContinue
            if (-not $Found) { throw "signtool.exe was not found. Set YEET_SIGNTOOL explicitly." }
            $Found.Source
        }
        $Timestamp = if ($env:YEET_SIGN_TIMESTAMP_URL) { $env:YEET_SIGN_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
        & $SignTool sign /fd SHA256 /f $env:YEET_SIGN_PFX /p $env:YEET_SIGN_PASSWORD /tr $Timestamp /td SHA256 $BuiltBinary
        if ($LASTEXITCODE -ne 0) { throw "Authenticode signing failed." }
        & $SignTool verify /pa /v $BuiltBinary
        if ($LASTEXITCODE -ne 0) { throw "Authenticode verification failed." }
    }

    if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
    New-Item -ItemType Directory -Force (Join-Path $Stage "bin") | Out-Null
    $RuntimeStage = Join-Path $Stage "share\yeet\runtime"
    New-Item -ItemType Directory -Force $RuntimeStage | Out-Null
    New-Item -ItemType Directory -Force $Out | Out-Null
    Copy-Item -Force $BuiltBinary (Join-Path $Stage "bin\yeet.exe")
    Copy-Item -Recurse -Force $RuntimeDist (Join-Path $RuntimeStage "dist")
    Copy-Item -Force "RuntimeSource\package.json" (Join-Path $RuntimeStage "package.json")
    Copy-Item -Force "README.md", "LICENSE.txt" $Stage
    Copy-Item -Force "Scripts\install-release.ps1" (Join-Path $Stage "install.ps1")
    if (Test-Path "docs\PLATFORM_SUPPORT.md") {
        New-Item -ItemType Directory -Force (Join-Path $Stage "docs") | Out-Null
        Copy-Item -Force "docs\PLATFORM_SUPPORT.md" (Join-Path $Stage "docs\PLATFORM_SUPPORT.md")
    }

    $Archive = Join-Path $Out "$Name.zip"
    if (Test-Path $Archive) { Remove-Item -Force $Archive }
    Compress-Archive -Path $Stage -DestinationPath $Archive -CompressionLevel Optimal
    $Hash = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -Encoding ascii -NoNewline -Path "$Archive.sha256" -Value "$Hash  $([IO.Path]::GetFileName($Archive))`n"
    Write-Host "Created $Archive"
} finally {
    Pop-Location
}
