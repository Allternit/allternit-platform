# Standby vantage probe

A second set of eyes for the status page. This host probes the same targets as the Cloudflare Worker, from outside Cloudflare, and posts the results to the Worker. It also checks that the Worker's own data is fresh and emails when it is not.

Files: `probe.sh` (one probe round), `install.sh` (systemd service and timer). Needs `bash`, `curl`, `openssl`, `awk`, and a local `sendmail` (Postfix) for alerts.

## What one run does

1. Gets `https://status.allternit.com/api/v1/targets.json`. Keeps the last good copy in the state dir and uses it when the fetch fails.
2. Probes every target that lists `allternit-standby`, all at once: status code, body marker, latency, 8 s timeout.
3. Builds the `allternit.ingest/v1` JSON (see `../contract/`), signs the raw body with HMAC-SHA256 using the secret in `/etc/allternit-status/ingest.key`, and POSTs it to `/api/v1/ingest` with `X-Allternit-Signature: sha256=<hex>`.
4. Calls `/api/health`. If it is stale (503) or unreachable on 3 runs in a row, it sends one email through `sendmail`, one more each hour while it stays bad, and one "back" email when it passes again. State is in `/var/lib/allternit-status-probe/watch.state`.

Exit code is non-zero when the ingest POST failed, so `systemctl status` shows it.

## Install (not run anywhere yet)

```
sudo bash install.sh --alert-to you@example.com
sudo sh -c 'printf %s "<ingest secret>" > /etc/allternit-status/ingest.key'   # same value as INGEST_HMAC_SECRET
sudo systemctl start allternit-status-probe.timer
journalctl -u allternit-status-probe -f
```

The installer is safe to repeat. It keeps an existing key and config, sets the key to mode 600 root, and enables the timer (it starts it only when the key file is not empty). Remove with `sudo bash install.sh --uninstall` (add `--purge` to delete config, key and state).

Config is `/etc/allternit-status/probe.conf` (`STATUS_BASE`, `KEY_FILE`, `STATE_DIR`, `ALERT_TO`, `ALERT_FROM` (default `status@allternit.com`, which the standby signs with DKIM and allternit.com's SPF allows; set with `--alert-from`), `SENDMAIL`). Environment variables override the file.

Installed on allternit-standby 2026-11-12 with `--alert-to allternitpbc@gmail.com`; a test alert was accepted by Gmail.

## Notes

- Port 25 outbound must be open on this host, or Postfix must relay through a smarthost. The Mac used for development blocks it.
- `openssl -hmac` puts the secret in this host's process list for a few milliseconds each run. Fine on a single-user box.
- The probe sends no cookies or credentials. The mail relay target expects a 401, which counts as healthy.

## Test on a Mac against a local Worker

```
STATUS_BASE=http://localhost:8787 KEY_FILE=./dev.key STATE_DIR=/tmp/probe-state ALLTERNIT_STATUS_CONF=/dev/null bash probe.sh
```
Use `127.0.0.1` rather than `localhost` if another service already listens on IPv6 `::1:8787`.
