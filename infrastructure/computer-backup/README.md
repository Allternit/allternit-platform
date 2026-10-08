# Cloud-computer backups

Nightly off-host backups of every customer computer (Incus instances named
`allternit-user-*`, `allternit-free-*`, `allternit-bot-*`) to the R2 bucket
`allternit-backups`.

| | |
|---|---|
| Host | every Incus host with customer computers (allternit-standby today) |
| Schedule | `allternit-computer-backup.timer`, 02:20 server time (+ up to 10 min) |
| Script | `/usr/local/sbin/allternit-computer-backup.sh` |
| Config | `/etc/allternit-backup/backup.env` (R2 + optional `ALLTERNIT_OPS_ALERT_URL/_TOKEN`), `backup-public.pem` |
| Retention | R2 lifecycle: `daily/` 8 days, `weekly/` (Sundays) 35 days |
| Failure | systemd unit fails + ops-alert email to allternitpbc@gmail.com |
| Logs | `journalctl -u allternit-computer-backup` |

Install or update on a host: `./install.sh root@<host>`.

First run 2026-10-08 on allternit-standby: 3 computers, 7.7 GB, ~6 min; a
decrypt of `key.bin` + the first part with the keychain key was checked.

The owner-facing side (restore points and "Download a copy") is in cloud-api
(`/api/v1/provisioned-instances/:id/snapshots|exports`). Its download route
streams several GB through the mail host's nginx, which has a
`location /api/v1/computer-exports/` block with `proxy_buffering off` and
`proxy_max_temp_file_size 0` (`/etc/nginx/sites-enabled/api-allternit`, not
in this repo) so a download never spools onto that nearly full disk.

## What a run writes

```
<daily|weekly>/<host>/<YYYY-MM-DD>/computers/
  key.bin                       per-run AES key, RSA-OAEP encrypted to backup-public.pem
  manifest.txt                  one line per computer: parts, bytes, sha256 of the export
  <instance>/part-000.enc ...   encrypted `incus export` (zstd), split into 1 GiB parts
```

Servers and the bucket can't decrypt anything. The private key is only in Eoj's
Mac keychain (`com.allternit.backup-private-key`), same as the database backups.

## Restore a computer

On the Mac (has the private key):

```bash
DAY=2026-10-08 HOST=allternit-standby NAME=allternit-user-...
P=daily/$HOST/$DAY/computers
# 1. download key.bin, manifest.txt and $NAME/part-*.enc from R2 (rclone / aws s3 cp / dashboard)
# 2. recover the run key. Use Homebrew OpenSSL: macOS's LibreSSL can't do OAEP-SHA256.
#    The keychain value is the PEM, base64-encoded.
O=/opt/homebrew/bin/openssl
security find-generic-password -s com.allternit.backup-private-key -w | base64 -d > /tmp/k.pem
$O pkeyutl -decrypt -inkey /tmp/k.pem -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 -in key.bin > run.key; rm /tmp/k.pem
# 3. decrypt and check against the manifest (the result starts with zstd magic 28 b5 2f fd)
cat $NAME/part-*.enc | BACKUP_KEY=$(cat run.key) $O enc -d -aes-256-cbc -pbkdf2 \
  -iter 200000 -pass env:BACKUP_KEY > $NAME.tar.zst
shasum -a 256 $NAME.tar.zst; grep $NAME manifest.txt
```

Then copy `$NAME.tar.zst` to an Incus host and import it:

```bash
incus import $NAME.tar.zst            # same name, or: incus import file.tar.zst new-name
incus start $NAME
```

The restored container keeps its config. If the original still exists, import
under a new name, check it, then swap. The computers table row
(`native_id`) must point at the instance name that should serve the customer.
