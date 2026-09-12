# stage.ps1 - Assemble MSIX packaging directory
param(
    [string]$Target = "x64",
    [string]$Version = "1.3.5.0",
    [string]$PublisherDN = ""
)

if (-not $PublisherDN) { $PublisherDN = $env:MSIX_PUBLISHER_DN }

$ErrorActionPreference = "Stop"

# Path configuration (relative to script location)
$ScriptDir = $PSScriptRoot
$ProjectRoot = Split-Path (Split-Path $ScriptDir -Parent) -Parent
$ReleaseDir = Join-Path $ProjectRoot "src-tauri\target\x86_64-pc-windows-msvc\release"
$StageDir = Join-Path $ScriptDir "stage"
$AssetsDir = Join-Path $ScriptDir "assets"
$ManifestTemplate = Join-Path $ScriptDir "AppxManifest.xml"

Write-Host "ScriptDir: $ScriptDir" -ForegroundColor Cyan
Write-Host "ReleaseDir: $ReleaseDir" -ForegroundColor Cyan
Write-Host "StageDir: $StageDir" -ForegroundColor Cyan

# Clean old directory
if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
New-Item -ItemType Directory -Path $StageDir -Force | Out-Null
New-Item -ItemType Directory -Path "$StageDir\Assets" -Force | Out-Null

# Copy Tauri build output (EXE + DLLs)
Write-Host "Copying build output..." -ForegroundColor Cyan
$exePath = Join-Path $ReleaseDir "PeriTray.exe"
if (-not (Test-Path $exePath)) {
    Write-Error "PeriTray.exe not found at: $exePath"
    exit 1
}
Copy-Item -Path $exePath -Destination $StageDir

# Copy all DLLs (WebView2Loader.dll etc.)
Get-ChildItem -Path $ReleaseDir -Filter "*.dll" | ForEach-Object {
    Copy-Item -Path $_.FullName -Destination $StageDir
}

# Copy frontend resources (if not embedded by Tauri)
$distDir = Join-Path $ProjectRoot "dist"
if (Test-Path $distDir) {
    Write-Host "Copying frontend resources..." -ForegroundColor Cyan
    Copy-Item -Path $distDir -Destination "$StageDir\dist" -Recurse -Force
}

# Copy Store icons
Write-Host "Copying Store icons..." -ForegroundColor Cyan
Copy-Item -Path "$AssetsDir\*.png" -Destination "$StageDir\Assets"

# Generate AppxManifest.xml (replace version number and publisher DN)
Write-Host "Generating AppxManifest.xml..." -ForegroundColor Cyan
if (-not $PublisherDN) {
    Write-Error "MSIX_PUBLISHER_DN environment variable is not set. Please set it to your Partner Center publisher CN."
    exit 1
}
$manifest = Get-Content $ManifestTemplate -Raw
$manifest = $manifest -replace 'Version="1\.3\.5\.0"', "Version=`"$Version`""
$manifest = $manifest -replace '__PUBLISHER_DN__', $PublisherDN
$manifest | Set-Content -Path "$StageDir\AppxManifest.xml" -Encoding UTF8

Write-Host "Stage directory ready: $StageDir" -ForegroundColor Green
