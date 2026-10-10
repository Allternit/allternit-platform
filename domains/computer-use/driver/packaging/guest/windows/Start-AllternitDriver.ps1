# Start the Allternit Driver in the interactive desktop session.
# Registered as a scheduled task at user logon by install-driver.ps1; UIA
# only sees windows of the session it runs in, which is why this is a logon
# task and not a SYSTEM service.
$ErrorActionPreference = "Stop"

$installDir = "C:\Program Files\Allternit\Driver"
$stateDir = "C:\ProgramData\Allternit\Driver"
$endpointFile = Join-Path $stateDir "endpoint"
$tokenFile = Join-Path $stateDir "token"
$logDir = Join-Path $stateDir "logs"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$token = Get-Content -Raw $tokenFile
$env:ALLTERNIT_DRIVER_TOKEN = $token.Trim()

# comtypes ships in the image (pip); PYTHONPATH points at the driver package.
$env:PYTHONPATH = $installDir
$python = "python"
if (Get-Command py -ErrorAction SilentlyContinue) { $python = "py" }

& $python -m allternit_driver `
    --listen tcp:127.0.0.1:0 `
    --endpoint-file $endpointFile `
    --engine uia `
    --state-dir (Join-Path $stateDir "state") `
    *> (Join-Path $logDir "driver.log")
