# Setup script for the Windows desktop guest agent.
# Run this inside a freshly-imaged Windows VM to finish the agent runtime.
# The antifob/incus-windows image already includes the Incus/QEMU guest agent,
# so this script mainly ensures the PowerShell execution policy and optional
# helper programs are present.

param(
    # The Allternit Driver package (phase D1b), when the image pipeline drops
    # it next to this script: a directory holding allternit_driver\ (plus
    # packaging\guest\windows\install-driver.ps1, or a flattened copy).
    [string]$DriverDir = "$PSScriptRoot\driver"
)

$ErrorActionPreference = "Stop"

Write-Host "[allternit-windows-agent] setting execution policy"
Set-ExecutionPolicy -ExecutionPolicy Bypass -Scope LocalMachine -Force

Write-Host "[allternit-windows-agent] installing Chocolatey helpers"
# Ensure Chocolatey is available; if not, skip silently (offline images may not have it).
if (Get-Command choco -ErrorAction SilentlyContinue) {
    choco install -y --no-progress googlechrome 2>&1 | Out-Null
    choco install -y --no-progress nssm 2>&1 | Out-Null
}

Write-Host "[allternit-windows-agent] installing Tailscale"
$tailscaleUrl = "https://pkgs.tailscale.com/stable/tailscale-setup-latest.exe"
$tailscaleInstaller = "$env:TEMP\tailscale-setup.exe"
try {
    Invoke-WebRequest -Uri $tailscaleUrl -OutFile $tailscaleInstaller -UseBasicParsing
    Start-Process -FilePath $tailscaleInstaller -ArgumentList "/S" -Wait
} catch {
    Write-Warning "Could not install Tailscale: $_"
}

Write-Host "[allternit-windows-agent] verifying Incus agent service"
$agent = Get-Service -Name "Incus" -ErrorAction SilentlyContinue
if (-not $agent) {
    Write-Warning "Incus agent service not found; file/exec operations may not work."
} else {
    Write-Host "Incus agent service status: $($agent.Status)"
}

# The Allternit Driver (phase D1b): the structured computer-toolset sidecar.
# Installed when the driver package is available (the image pipeline drops it
# next to this script, like the factory binary on Linux images). The driver
# runs at logon of the desktop session (UIA needs the interactive session)
# and answers allternit-api through the guest agent on loopback.
if (Test-Path (Join-Path $DriverDir "allternit_driver")) {
    Write-Host "[allternit-windows-agent] installing the Allternit Driver from $DriverDir"
    $installScript = Join-Path $DriverDir "packaging\guest\windows\install-driver.ps1"
    if (-not (Test-Path $installScript)) {
        # The pipeline may copy the packaging tree flattened; accept either.
        $installScript = Join-Path $PSScriptRoot "install-driver.ps1"
    }
    if (Test-Path $installScript) {
        & $installScript -DriverDir $DriverDir
        Write-Host "[allternit-windows-agent] driver install finished"
    } else {
        Write-Warning "driver package found but install-driver.ps1 is missing; skipping the driver"
    }
} else {
    Write-Warning "no driver package at $DriverDir; structured toolset members stay unavailable on this image"
}

Write-Host "[allternit-windows-agent] done"
