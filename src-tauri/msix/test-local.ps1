# test-local.ps1 - Test MSIX locally (requires Developer Mode)
param(
    [string]$Version = "1.3.5.0",
    [string]$Target = "x64"
)

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$outputDir = Join-Path $ScriptDir "output"
$outputMsix = Join-Path $outputDir "PeriTray_${Version}_${Target}.msix"
$stageDir = Join-Path $ScriptDir "stage"

if (-not (Test-Path $outputMsix)) {
    Write-Error "MSIX file not found: $outputMsix"
    exit 1
}

# Check Developer Mode
$devMode = Get-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" -Name "AllowDevelopmentWithoutDevLicense" -ErrorAction SilentlyContinue
if ($devMode.AllowDevelopmentWithoutDevLicense -ne 1) {
    Write-Host "Developer Mode not enabled, attempting to enable..." -ForegroundColor Yellow
    reg add "HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" /v AllowDevelopmentWithoutDevLicense /t REG_DWORD /d 1 /f
    Write-Host "Developer Mode enabled (may require restart)" -ForegroundColor Green
}

# Uninstall old version (if exists)
Write-Host "Uninstalling old version..." -ForegroundColor Cyan
Get-AppxPackage -Name "Oneday.PeriTray" | Remove-AppxPackage -ErrorAction SilentlyContinue

# For unsigned MSIX, use -Register with AppxManifest.xml instead
Write-Host "Installing MSIX (using Register method for unsigned package)..." -ForegroundColor Cyan
$manifestPath = Join-Path $stageDir "AppxManifest.xml"
if (-not (Test-Path $manifestPath)) {
    Write-Error "AppxManifest.xml not found in stage directory. Run stage.ps1 first."
    exit 1
}

Add-AppxPackage -Register $manifestPath

# Clean up duplicate Start Menu shortcut (loose EXE entry from -Register)
$shortcutPath = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\PeriTray.lnk'
if (Test-Path $shortcutPath) {
    Remove-Item $shortcutPath -Force
    Write-Host "Removed duplicate Start Menu shortcut" -ForegroundColor Cyan
}

Write-Host "Installation complete" -ForegroundColor Green
Write-Host ""
Write-Host "Test steps:" -ForegroundColor Yellow
Write-Host "  1. Find PeriTray in Start Menu" -ForegroundColor White
Write-Host "  2. Launch the app and verify functionality" -ForegroundColor White
Write-Host "  3. Uninstall: Get-AppxPackage -Name 'Oneday.PeriTray' | Remove-AppxPackage" -ForegroundColor White
