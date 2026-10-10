# Install the Allternit Driver into a Windows guest image (phase D1b).
# Run inside the Windows VM (setup-windows-agent.ps1 calls this), elevated.
#
#   -Parameter DriverDir: the directory holding allternit_driver\ (the
#     packaging windows\ folder's sibling driver checkout on the build host,
#     mounted or copied into the VM).
#
# Installs Python + comtypes if needed, copies the driver, generates the
# launch token, and registers a scheduled task that starts the driver at logon
# of the desktop session on loopback TCP with token auth.
param(
    [Parameter(Mandatory = $true)][string]$DriverDir
)

$ErrorActionPreference = "Stop"

$installDir = "C:\Program Files\Allternit\Driver"
$stateDir = "C:\ProgramData\Allternit\Driver"
$taskName = "AllternitDriver"

Write-Host "[allternit-driver] ensuring Python"
$python = $null
if (Get-Command py -ErrorAction SilentlyContinue) { $python = "py" }
elseif (Get-Command python -ErrorAction SilentlyContinue) { $python = "python" }
if (-not $python) {
    if (Get-Command choco -ErrorAction SilentlyContinue) {
        choco install -y --no-progress python311 2>&1 | Out-Null
    }
    if (Get-Command py -ErrorAction SilentlyContinue) { $python = "py" }
    elseif (Get-Command python -ErrorAction SilentlyContinue) { $python = "python" }
}
if (-not $python) {
    Write-Host "[allternit-driver] downloading the Python installer"
    $installer = "$env:TEMP\python-3.11.9-amd64.exe"
    Invoke-WebRequest -Uri "https://www.python.org/ftp/python/3.11.9/python-3.11.9-amd64.exe" -OutFile $installer -UseBasicParsing
    Start-Process -FilePath $installer -ArgumentList "/quiet","InstallAllUsers=1","PrependPath=1","Include_pip=1" -Wait
    $python = "python"
}
& $python --version

Write-Host "[allternit-driver] ensuring comtypes"
& $python -m pip install --disable-pip-version-check --quiet comtypes
if ($LASTEXITCODE -ne 0) {
    # comtypes is pure Python; --user works even on locked-down SYSTEM contexts.
    & $python -m pip install --disable-pip-version-check --quiet --user comtypes
}

Write-Host "[allternit-driver] installing the driver package"
if (-not (Test-Path (Join-Path $DriverDir "allternit_driver"))) {
    throw "no allternit_driver package in $DriverDir"
}
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
if (Test-Path (Join-Path $installDir "allternit_driver")) {
    Remove-Item -Recurse -Force (Join-Path $installDir "allternit_driver")
}
Copy-Item -Recurse (Join-Path $DriverDir "allternit_driver") (Join-Path $installDir "allternit_driver")
Copy-Item (Join-Path $DriverDir "allternit_driver\guest_rpc.py") (Join-Path $installDir "guest-rpc.py")

Write-Host "[allternit-driver] writing the state dir, endpoint and token"
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
$tokenBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($tokenBytes)
$token = ($tokenBytes | ForEach-Object { $_.ToString("x2") }) -join ""
Set-Content -Path (Join-Path $stateDir "token") -Value $token -NoNewline -Encoding ascii
# Only SYSTEM and Administrators may read the token and endpoint.
icacls $stateDir /inheritance:r /grant "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null

$wrapper = Join-Path $installDir "Start-AllternitDriver.ps1"
Copy-Item (Join-Path $PSScriptRoot "Start-AllternitDriver.ps1") $wrapper

Write-Host "[allternit-driver] registering the logon scheduled task"
$action = New-ScheduledTaskAction -Execute "powershell.exe" `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$wrapper`""
$trigger = New-ScheduledTaskTrigger -AtLogOn
$principal = New-ScheduledTaskPrincipal -GroupId "BUILTIN\Users" -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null

Write-Host "[allternit-driver] verifying the install"
& $python -c "import sys; sys.path.insert(0, r'$installDir'); import allternit_driver.guest_rpc; print('guest_rpc ok')"
if ($LASTEXITCODE -ne 0) { throw "the driver package doesn't import" }

Write-Host "[allternit-driver] done"
