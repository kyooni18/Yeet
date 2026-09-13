param(
    [switch]$Uninstall,
    [int]$WaitForProcessId = 0,
    [switch]$CleanupBundle
)

$ErrorActionPreference = "Stop"

if ($WaitForProcessId -gt 0) {
    Wait-Process -Id $WaitForProcessId -ErrorAction SilentlyContinue
}
$BundleRoot = $PSScriptRoot
$Prefix = if ($env:PREFIX) {
    $env:PREFIX
} elseif ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA "Programs\Yeet"
} elseif ($HOME) {
    Join-Path $HOME ".local\yeet"
} else {
    throw "Unable to determine an install prefix. Set PREFIX and retry."
}

$Destination = Join-Path $Prefix "bin"
$RuntimeDestination = Join-Path $Prefix "share\yeet\runtime"
$SourceBinary = Join-Path $BundleRoot "bin\yeet.exe"
$SourceRuntime = Join-Path $BundleRoot "share\yeet\runtime"
$InstalledBinary = Join-Path $Destination "yeet.exe"

function Get-RunningMcpPorts {
    param([string]$Executable)
    $Ports = @()
    if (-not (Test-Path $Executable -PathType Leaf)) { return $Ports }
    $Lines = & $Executable mcpserver list 2>$null
    if ($LASTEXITCODE -ne 0) { return $Ports }
    foreach ($Line in $Lines) {
        $Fields = @($Line -split '\s+' | Where-Object { $_ })
        if ($Fields.Count -ge 2 -and $Fields[1] -eq "running") {
            $Ports += $Fields[0]
        }
    }
    return $Ports
}

$RunningMcpPorts = @(Get-RunningMcpPorts $InstalledBinary)
foreach ($Port in $RunningMcpPorts) {
    & $InstalledBinary mcpserver stop --port $Port | Out-Null
}

if ($Uninstall) {
    if (Test-Path $InstalledBinary) { Remove-Item -Force $InstalledBinary }
    $ShareRoot = Join-Path $Prefix "share\yeet"
    if (Test-Path $ShareRoot) { Remove-Item -Recurse -Force $ShareRoot }
    Write-Host "Removed Yeet from $Prefix"
    exit 0
}

if (-not (Test-Path $SourceBinary -PathType Leaf) -or
    -not (Test-Path (Join-Path $SourceRuntime "dist\bridge.js") -PathType Leaf)) {
    throw "This installer must be run from an extracted Yeet release bundle."
}

New-Item -ItemType Directory -Force $Destination | Out-Null
New-Item -ItemType Directory -Force $RuntimeDestination | Out-Null
Copy-Item -Force $SourceBinary $InstalledBinary
$InstalledDist = Join-Path $RuntimeDestination "dist"
if (Test-Path $InstalledDist) { Remove-Item -Recurse -Force $InstalledDist }
Copy-Item -Recurse -Force (Join-Path $SourceRuntime "dist") $InstalledDist
Copy-Item -Force (Join-Path $SourceRuntime "package.json") (Join-Path $RuntimeDestination "package.json")

foreach ($Port in $RunningMcpPorts) {
    & $InstalledBinary mcpserver start --port $Port | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Failed to restart Yeet MCP daemon on port $Port." }
}

if ($CleanupBundle) {
    $CleanupRoot = $BundleRoot
    $EscapedCleanupRoot = $CleanupRoot.Replace("'", "''")
    Start-Process -WindowStyle Hidden powershell.exe -ArgumentList @(
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        "Start-Sleep -Milliseconds 500; Remove-Item -LiteralPath '$EscapedCleanupRoot' -Recurse -Force -ErrorAction SilentlyContinue"
    ) | Out-Null
}

Write-Host "Installed Yeet to $InstalledBinary"
Write-Host "Installed runtime to $RuntimeDestination"
if (($env:Path -split ';') -notcontains $Destination) {
    Write-Host "Add $Destination to PATH to run yeet by name."
}
