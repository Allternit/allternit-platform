# Windows Code-mode terminal smoke test, against the binaries a released
# Allternit Desktop installer ships (no build).
#
# Part A: the Factory pane engine, the way allternit-api drives it
#   (cmd/allternit-api/src/terminal_routes.rs): `allternit-factory pane tty
#   ensure` -> socket path -> named pipe \\.\pipe\<path> -> newline JSON
#   factory.terminal.create / write / output / resize / close.
# Part B: allternit-api's own HTTP routes (/terminal/create, /input, /stream),
#   which is what Code mode calls on 127.0.0.1:8013.
#
# Usage: windows-terminal-smoke.ps1 -App <unpacked app dir>
param([Parameter(Mandatory = $true)][string]$App)
$ErrorActionPreference = 'Stop'
$start = Get-Date
function Step($m) { Write-Host ("[{0,6:n1}s] {1}" -f ((Get-Date) - $start).TotalSeconds, $m) }

$factory = (Get-ChildItem $App -Recurse -Filter 'allternit-factory.exe' | Select-Object -First 1).FullName
$api = (Get-ChildItem $App -Recurse -Filter 'allternit-api.exe' | Select-Object -First 1).FullName
if (-not $factory) { throw 'allternit-factory.exe not found in the installer' }
Write-Host "factory: $factory"
Write-Host "api:     $api"

# ── Part A: pane engine over its named pipe ─────────────────────────────────
# Run `pane tty ensure` the way allternit-api does (stdout captured through a
# pipe, 30 s limit), and time it. If ensure exits but the read does not end,
# the engine it started kept ensure's stdout handle: allternit-api would then
# wait its full 30 s and report "the Factory pane engine did not start".
$psi = New-Object System.Diagnostics.ProcessStartInfo($factory, 'pane tty ensure')
$psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false
$sw = [Diagnostics.Stopwatch]::StartNew()
$ens = [Diagnostics.Process]::Start($psi)
$outTask = $ens.StandardOutput.ReadToEndAsync()
$errTask = $ens.StandardError.ReadToEndAsync()
$exited = $ens.WaitForExit(30000)
Write-Host ("ensure process exited: {0} after {1} ms, exit code {2}" -f $exited, $sw.ElapsedMilliseconds, $(if ($exited) { $ens.ExitCode } else { 'n/a' }))
$readDone = $outTask.Wait(30000)
Write-Host ("ensure stdout read finished: {0} after {1} ms" -f $readDone, $sw.ElapsedMilliseconds)
if ($errTask.Wait(1000)) { Write-Host "ensure stderr: $($errTask.Result)" }
if (-not $exited) { throw 'FAIL: pane tty ensure did not exit within 30 s' }
$script:failures = @()
if (-not $readDone) {
  $script:failures += 'pane tty ensure exited but its stdout never closed (the engine it started inherited the handle); allternit-api would time out after 30 s on a cold start'
  Write-Host "FAIL (recorded, continuing): $($script:failures[-1])"
  # The engine is up now, so a second ensure starts nothing and returns at once.
  $sw2 = [Diagnostics.Stopwatch]::StartNew()
  $p2 = [Diagnostics.Process]::Start($psi)
  $out2 = $p2.StandardOutput.ReadToEndAsync()
  if (-not $out2.Wait(30000)) { throw 'FAIL: a second pane tty ensure (engine already running) also never closed stdout' }
  Write-Host ("second ensure (engine running): stdout closed after {0} ms" -f $sw2.ElapsedMilliseconds)
  $ensure = $out2.Result
} else {
  $ensure = $outTask.Result
}
Write-Host "ensure: $ensure"
$socket = ($ensure | Out-String | ConvertFrom-Json).socket
if (-not $socket) { throw 'pane tty ensure reported no socket' }
# Same mapping as terminal_routes.rs pipe_name(): keep a \\.\pipe\ path, else prefix it.
$pipe = if ($socket.StartsWith('\\.\pipe\')) { $socket.Substring(9) } else { $socket }
Write-Host "pipe: \\.\pipe\$pipe"

function Open-Pane([string]$method, [hashtable]$params) {
  $c = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipe, [System.IO.Pipes.PipeDirection]::InOut)
  $c.Connect(10000)
  $w = New-Object System.IO.StreamWriter($c)
  $w.AutoFlush = $true
  $r = New-Object System.IO.StreamReader($c)
  $frame = @{ id = [guid]::NewGuid().ToString(); method = $method; params = $params } | ConvertTo-Json -Compress -Depth 6
  $w.WriteLine($frame)
  return @{ conn = $c; reader = $r }
}

function Read-Line($reader, [int]$ms) {
  $task = $reader.ReadLineAsync()
  if ($task.Wait($ms)) { return $task.Result }
  return $null
}

function Call-Pane([string]$method, [hashtable]$params) {
  $p = Open-Pane $method $params
  try {
    $line = Read-Line $p.reader 15000
    if (-not $line) { throw "$method : no response within 15 s" }
    $resp = $line | ConvertFrom-Json
    if ($resp.error) { throw "$method : $($resp.error.code): $($resp.error.message)" }
    return $resp.result
  } finally { $p.conn.Dispose() }
}

# Read the terminal's output (replay + live) until $expect shows up or $seconds pass.
function Wait-Output([string]$tid, [string]$expect, [int]$seconds) {
  $p = Open-Pane 'factory.terminal.output' @{ terminal_id = $tid }
  $all = ''
  try {
    $first = Read-Line $p.reader 15000
    if (-not $first) { throw 'factory.terminal.output: no response' }
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline -and -not $all.Contains($expect)) {
      $line = Read-Line $p.reader 1000
      if (-not $line) { continue }
      $ev = $line | ConvertFrom-Json
      if ($ev.type -eq 'output') { $all += $ev.data }
      if ($ev.type -eq 'exit') { break }
    }
  } finally { $p.conn.Dispose() }
  return $all
}

function Test-Shell([string]$label, [string[]]$command, [string]$text, [string]$expect) {
  $tid = [guid]::NewGuid().ToString()
  # Same params allternit-api sends. An empty command = the engine's default
  # shell; that is what Code mode gets on Windows (the web UI asks for /bin/zsh).
  $created = Call-Pane 'factory.terminal.create' @{
    terminal_id = $tid; cwd = $env:RUNNER_TEMP; cols = 120; rows = 30
    command = $command; env = @{ TERM = 'xterm-256color'; COLORTERM = 'truecolor' }
  }
  Step "[$label] create done"
  Write-Host "[$label] created: $($created | ConvertTo-Json -Compress -Depth 6)"
  Start-Sleep -Seconds 2
  Step "[$label] write"
  Call-Pane 'factory.terminal.write' @{ terminal_id = $tid; data = $text } | Out-Null
  Step "[$label] reading output"
  $out = Wait-Output $tid $expect 30
  Call-Pane 'factory.terminal.resize' @{ terminal_id = $tid; cols = 100; rows = 40 } | Out-Null
  Call-Pane 'factory.terminal.close' @{ terminal_id = $tid } | Out-Null
  $clean = ($out -replace "`e\[[0-9;?]*[ -/]*[@-~]", '' -replace "`e\][^`a]*`a", '')
  Write-Host "[$label] output (ANSI stripped, last 1500 chars):"
  Write-Host ($clean.Substring([Math]::Max(0, $clean.Length - 1500)))
  if (-not $out.Contains($expect)) { throw "[$label] FAIL: '$expect' never appeared" }
  Write-Host "[$label] PASS: '$expect' came back through ConPTY"
}

# The expected text only appears when the shell ran the command (the typed
# line echoes '6*7', not '42').
Test-Shell 'default shell' @() "Write-Output ('allt-ok-' + (6*7))`r" 'allt-ok-42'
Test-Shell 'cmd.exe' @('cmd.exe') "set /a 6*7+1000`r" '1042'
Write-Host 'PART A PASS: pane engine terminals work on Windows'

# ── Part B: allternit-api HTTP routes (what Code mode calls) ────────────────
if (-not $api) { Write-Host 'PART B SKIPPED: allternit-api.exe not in the installer'; if ($script:failures.Count) { exit 1 }; exit 0 }
$token = [guid]::NewGuid().ToString('N')
$data = Join-Path $env:RUNNER_TEMP 'allternit-data'
New-Item -ItemType Directory -Force $data | Out-Null
$env:ALLTERNIT_API_PORT = '8013'
$env:ALLTERNIT_API_HOST = '127.0.0.1'
$env:ALLTERNIT_DATA_DIR = $data
$env:ALLTERNIT_FACTORY_BIN = $factory
$env:ALLTERNIT_DESKTOP_ACCESS_TOKEN = $token
$env:ALLTERNIT_OPERATOR_API_KEY = $token
$env:ALLTERNIT_SELF_HOSTED = 'false'
$log = Join-Path $env:RUNNER_TEMP 'allternit-api.log'
$proc = Start-Process -FilePath $api -RedirectStandardOutput $log -RedirectStandardError "$log.err" -PassThru -WindowStyle Hidden
$up = $false
for ($i = 0; $i -lt 240; $i++) {
  try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 'http://127.0.0.1:8013/health/live' | Out-Null; $up = $true; break } catch { Start-Sleep -Seconds 1 }
  if ($proc.HasExited) { break }
}
Step "allternit-api up: $up (exited: $($proc.HasExited))"
if (-not $up) {
  Write-Host "allternit-api did not answer /health/live in 240 s. Exited: $($proc.HasExited)"
  Write-Host '--- listening lines:'; Select-String -Path $log, "$log.err" -Pattern 'listening|bind|error|panic' -ErrorAction SilentlyContinue | Select-Object -Last 20 | ForEach-Object { $_.Line }
  Write-Host '--- last 25 lines:'; Get-Content "$log.err", $log -Tail 25 -ErrorAction SilentlyContinue
  throw 'PART B FAIL: allternit-api did not start'
}
$h = @{ 'x-allternit-desktop-access-token' = $token; 'Content-Type' = 'application/json' }
$create = Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:8013/terminal/create' -Headers $h -Body (@{ shell = '/bin/zsh'; cols = 120; rows = 30 } | ConvertTo-Json)
Write-Host "create: $($create | ConvertTo-Json -Compress -Depth 6)"
$sid = $create.data.session_id
if (-not $sid) { $sid = $create.session_id }
if (-not $sid) { throw 'PART B FAIL: no session_id from /terminal/create' }
Start-Sleep -Seconds 2
Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:8013/terminal/$sid/input" -Headers $h -Body (@{ content = "Write-Output ('allt-api-' + (6*7))`r" } | ConvertTo-Json) | Out-Null
# The stream is server-sent events; read it for up to 20 s.
$client = New-Object System.Net.Http.HttpClient
$client.Timeout = [TimeSpan]::FromSeconds(30)
$req = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Get, "http://127.0.0.1:8013/terminal/$sid/stream")
$req.Headers.Add('x-allternit-desktop-access-token', $token)
$resp = $client.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).Result
Write-Host "stream status: $([int]$resp.StatusCode)"
$reader = New-Object System.IO.StreamReader($resp.Content.ReadAsStreamAsync().Result)
$seen = ''
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline -and -not $seen.Contains('allt-api-42')) {
  $line = Read-Line $reader 1000
  if ($line) { $seen += $line + "`n" }
}
Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:8013/terminal/$sid/close" -Headers $h | Out-Null
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
if (-not $seen.Contains('allt-api-42')) {
  Write-Host "stream (last 1500 chars): $($seen.Substring([Math]::Max(0, $seen.Length - 1500)))"
  throw 'PART B FAIL: allt-api-42 never came back on /terminal/<id>/stream'
}
Write-Host 'PART B PASS: allternit-api /terminal routes work on Windows'
if ($script:failures.Count) { Write-Host "RECORDED FAILURES: $($script:failures -join '; ')"; exit 1 }
