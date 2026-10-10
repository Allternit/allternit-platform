# Allternit Driver — guest image packaging

These files install the Allternit Driver (`domains/computer-use/driver/`) into
the images that ship to users, so every cloud computer answers the structured
computer-toolset members (`read_ui`, `act`, `run_batch`, `verify`) locally.
Image build scripts call these; nothing here runs on a production host.

| Path | Used by |
|---|---|
| `guest/linux/install-driver.sh` | `cmd/allternit-computer-cloud/guest/build-image.sh`, `build-cloud-computer-image.sh`, `build-headless-computer-image.sh` (via the base image), `infrastructure/tart-host/build-image.sh` |
| `guest/linux/allternit-driver.service` | the install script (systemd mode) |
| `guest/windows/install-driver.ps1` | `cmd/allternit-computer-cloud/guest/setup-windows-agent.ps1` (inside the Windows image) |
| `guest/windows/Start-AllternitDriver.ps1` | the install script (scheduled task at logon) |

Layout on a Linux guest:

- `/opt/allternit-driver/allternit_driver/` — the driver package.
- `/opt/allternit-driver/guest-rpc` — the exec-channel forwarder
  (`allternit_driver/guest_rpc.py`); allternit-api runs it through the guest
  agent to reach the driver's socket.
- `/usr/local/bin/allternit-driver-rpc` — wrapper so the API can invoke it by
  a fixed path.
- `allternit-driver.service` — `python3 -m allternit_driver --listen
  unix:/run/allternit/driver.sock --engine auto`, enabled at boot. xdotool and
  scrot stay installed as the explicit pixel fallback when the driver is down.

Layout on a Windows guest:

- `C:\Program Files\Allternit\Driver\allternit_driver\` — the driver package.
- `C:\Program Files\Allternit\Driver\guest-rpc.py` — the forwarder.
- `C:\ProgramData\Allternit\Driver\` — the endpoint file, the launch token
  (ACL: SYSTEM + Administrators) and state.
- A scheduled task starts the driver at logon of the desktop session (UIA
  only sees windows in the interactive session), listening on loopback TCP
  with the token; the Incus/QEMU guest agent reaches it through the
  forwarder.
