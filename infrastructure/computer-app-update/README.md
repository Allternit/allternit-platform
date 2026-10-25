# Cloud-computer app update

Keeps the Allternit Desktop app on every customer cloud computer at the current
release. Electron's built-in updater has no Linux support, and runtime packages
replace only the API, Gizzi, Factory and the screens, so without this a cloud
computer keeps the app version its image was built with.

| | |
|---|---|
| Host | every Incus host with customer computers (allternit-standby today) |
| Schedule | `allternit-computer-app-update.timer`, hourly (+ up to 15 min) |
| Script | `/usr/local/sbin/allternit-computer-app-update.sh` |
| Source of the release | `https://runtime.allternit.com/desktop/latest.json` (`linux_deb_x64`), written by `scripts/mirror-desktop-release.sh` |
| Failure | the computer is restored to its `app-update-<version>` snapshot, the unit fails, and an ops-alert email goes out when `ALLTERNIT_OPS_ALERT_URL/_TOKEN` are in `/etc/allternit-backup/backup.env` |
| Logs | `journalctl -u allternit-computer-app-update` |

A computer is updated only when Gizzi runs no session at two checks a minute
apart; a busy computer is tried again next hour. Each update:

1. takes snapshot `app-update-<version>` (the previous `app-update-*` snapshot is removed),
2. installs the `.deb` (checked to be `allternit-desktop` at the feed's version),
3. copies the active runtime's `allternit-factory` over `/opt/allternit-factory/allternit-factory`
   (the guest terminal engine, which the image bakes in) when it differs,
4. restarts `allternit-desktop` (and `allternit-factory-pane` when step 3 changed it),
5. waits up to 5 minutes for `GET :8013/health` to report healthy, or rolls back.

So a release reaches cloud computers only after it is mirrored: run
`scripts/mirror-desktop-release.sh <version> --apply` after every Desktop release.

Install or update on a host: `./install.sh root@<host>`.
Check without changing anything: `DRY_RUN=1 /usr/local/sbin/allternit-computer-app-update.sh`
(`ONLY=<instance>` limits it to one computer).
