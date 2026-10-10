# Validate the Allternit Driver on a freshly built Windows image (phase D1b).
# Run INSIDE the Windows VM (the Incus/QEMU guest agent executes it), after
# logon of an interactive session — the driver runs as that session's user:
#
#   incus exec <vm> -- powershell -NoProfile -ExecutionPolicy Bypass \
#       -File C:\allternit\validate-windows-image.ps1
#
# Checks the scheduled task, the package, the endpoint/token files, then the
# capability report and a real read_ui against a Notepad window over UIA.
$ErrorActionPreference = "Stop"

$installDir = "C:\Program Files\Allternit\Driver"
$stateDir = "C:\ProgramData\Allternit\Driver"
$rpc = Join-Path $installDir "guest-rpc.py"

function Write-Check([string]$msg) { Write-Host "[validate-windows-image] $msg" }

if (-not (Test-Path $rpc)) { throw "no driver forwarder at $rpc" }
if (-not (Test-Path (Join-Path $installDir "allternit_driver\__main__.py"))) { throw "no driver package in $installDir" }
$task = Get-ScheduledTask -TaskName "AllternitDriver" -ErrorAction SilentlyContinue
if (-not $task) { throw "the AllternitDriver scheduled task isn't registered" }
Write-Check "task state: $($task.State)"

# The driver starts at logon; if this session predates the task, start it now.
$endpoint = Join-Path $stateDir "endpoint"
if (-not (Test-Path $endpoint)) {
    Write-Check "endpoint missing; starting the scheduled task"
    Start-ScheduledTask -TaskName "AllternitDriver"
}
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    if (Test-Path $endpoint) { $ready = $true; break }
    Start-Sleep -Seconds 2
}
if (-not $ready) {
    $log = Get-Content (Join-Path $stateDir "logs\driver.log") -Tail 40 -ErrorAction SilentlyContinue
    throw "the driver endpoint never appeared. Log tail:`n$log"
}
if (-not (Test-Path (Join-Path $stateDir "token"))) { throw "the driver token file is missing" }
Write-Check "endpoint: $((Get-Content -Raw $endpoint).Trim())"

function Invoke-DriverRpc([string]$method, [hashtable]$params) {
    $req = @{ jsonrpc = "2.0"; id = 1; method = $method; params = $params } | ConvertTo-Json -Depth 8 -Compress
    $payload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($req))
    $out = & python $rpc $payload 2>$null
    if ($LASTEXITCODE -ne 0) { throw "driver forwarder failed for $method" }
    $out | ConvertFrom-Json
}

Write-Check "hello (capability report)"
$hello = Invoke-DriverRpc "hello" @{}
if ($hello.error) { throw "driver refused hello: $($hello.error.message)" }
$caps = $hello.result.hello
foreach ($member in @("read_ui", "act", "run_batch", "verify")) {
    if (-not $caps.$member) { throw "driver capability $member is not set: $($caps | ConvertTo-Json -Compress)" }
}
if (-not $caps.pixel) { throw "driver reports no pixel tools" }

Write-Check "read_ui on a Notepad window"
$notepad = Start-Process notepad -PassThru
try {
    $tree = $null
    for ($i = 0; $i -lt 20; $i++) {
        $reply = Invoke-DriverRpc "read_ui" @{ pid = $notepad.Id; max_elements = 60 }
        if ($reply.error) { Start-Sleep -Seconds 2; continue }
        $elements = @($reply.result.elements)
        if ($elements.Count -ge 2) {
            $tree = $reply.result
            break
        }
        Start-Sleep -Seconds 2
    }
    if (-not $tree) { throw "read_ui never returned a populated Notepad tree" }
    Write-Check "read_ui ok: $($elements.Count) elements from engine=$($tree.engine)"
} finally {
    Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
}

Write-Check "validation passed"
