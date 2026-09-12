# pack.ps1 - Create MSIX package using MakeAppx.exe
param(
    [string]$Target = "x64",
    [string]$Version = "1.3.5.0"
)

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$StageDir = Join-Path $ScriptDir "stage"
$outputDir = Join-Path $ScriptDir "output"
$outputMsix = Join-Path $outputDir "PeriTray_${Version}_${Target}.msix"

# Ensure stage directory exists
if (-not (Test-Path $StageDir)) {
    Write-Error "Stage directory not found. Run stage.ps1 first."
    exit 1
}

# Create output directory
if (-not (Test-Path $outputDir)) { 
    New-Item -ItemType Directory -Path $outputDir -Force | Out-Null 
}

# Find MakeAppx.exe
$makeAppxPaths = @(
    "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\MakeAppx.exe",
    "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\MakeAppx.exe",
    "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22000.0\x64\MakeAppx.exe",
    "C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\MakeAppx.exe"
)

$makeAppx = $null
foreach ($path in $makeAppxPaths) {
    if (Test-Path $path) { $makeAppx = $path; break }
}

if (-not $makeAppx) {
    Write-Error "MakeAppx.exe not found. Please install Windows SDK."
    exit 1
}

Write-Host "Using MakeAppx: $makeAppx" -ForegroundColor Cyan

# Remove old file
if (Test-Path $outputMsix) { Remove-Item $outputMsix -Force }

# Create MSIX package
Write-Host "Creating MSIX package..." -ForegroundColor Cyan
& $makeAppx pack /d $StageDir /p $outputMsix /nv /o

if ($LASTEXITCODE -ne 0) {
    Write-Error "MakeAppx packaging failed"
    exit 1
}

Write-Host "MSIX package created successfully: $outputMsix" -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. Upload to Partner Center" -ForegroundColor White
Write-Host "  2. Or test locally: Add-AppxPackage -Path $outputMsix" -ForegroundColor White
