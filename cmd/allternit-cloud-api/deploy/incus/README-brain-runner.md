# Brain runner: image and host setup

Brain runners are small containers that hold one person's vendor CLI sign-ins (claude, codex, grok, kimi) and run chat turns with tools off. Plan: `PLAN-brain-runner-2026-11-14.md` (D1 to D5). Runner protocol: `CONTRACT-connections-2026-11.md` section 9.7.

## 1. Build the image (on the standby Linux builder, never on `mail`)

```bash
bash cmd/allternit-computer-cloud/guest/build-brain-runner-image.sh \
  --runner-bin ./allternit-brain-runner --out-dir ./out
```

`--runner-bin` is the linux-x64 `allternit-brain-runner` from step 1.2. The script launches a Debian 12 container, installs Node 22.23.5 and the pinned CLIs (claude 2.1.325, codex 0.169.0, grok 1.0.77, kimi 2.5.2), bakes the self-update-off env into `/etc/environment`, runs `--version` for each CLI, then publishes and exports `out/allternit-brain-runner-<date>.tar.gz` (+ `.sha256`) and prints the size (target 1.5 GB or less) and the `incus image import` line. Change a pin with `CLAUDE_VERSION=... bash build-brain-runner-image.sh ...`.

The image has no SSH, no desktop and does not use systemd. Entry point is `/usr/local/bin/allternit-runner-init`: cloud-api sets `raw.lxc: lxc.init.cmd = /usr/local/bin/allternit-runner-init` and the `ALLTERNIT_RUNNER_*` env (URL, TOKEN, HOME, VENDORS; names as cloud-api `brain_runners.rs` sets them) when it creates the instance.

## 2. Set up the host

Copy the tarball to the serving host, then as root:

```bash
bash brain-runner-host.sh --image-tarball /root/allternit-brain-runner-<date>.tar.gz           # dry run, prints everything
bash brain-runner-host.sh --image-tarball /root/allternit-brain-runner-<date>.tar.gz --confirm
```

Flags: `--pool-size 60GiB` (new btrfs pool `runner`), `--reuse-cow` (use the existing `cow` pool instead). The script backs up to `/root/backups/brain-runner-<time>/` first, then sets the inotify sysctl, pool, bridge `runnerbr0` + ACL, host guard, profile `brain-runner` and imports the image. Every step skips itself when done, so re-running is safe.

**Egress ACL refresh.** Incus 6.0 ACLs match IPs, not names. The script resolves the vendor domains when it runs. When a vendor starts failing from a runner (CDN moved), run `bash brain-runner-host.sh --refresh-acl --confirm`. A stale ACL fails closed. The domain list is `DOMAINS` at the top of the script; keep it in step with `ALLTERNIT_RUNNER_VENDORS`.

**inotify.** `/etc/sysctl.d/90-incus-inotify.conf` is written, but on standby something resets the live value to 128 after boot. After a reboot check `sysctl fs.inotify.max_user_instances`; if it is low, `sysctl -p /etc/sysctl.d/90-incus-inotify.conf`. Incus network failures ("Network ... unavailable") mean look here first.

## 3. Roll back

Runners first: `incus list -c n | grep runner` then `incus delete -f <name>` for runner instances (cloud-api normally does this). Then:

```bash
incus profile delete brain-runner
incus network set runnerbr0 security.acls=            # detach
incus network delete runnerbr0
incus network acl delete allternit-brain-runner-egress
incus storage delete runner                            # only if this script created it; never delete cow
incus image delete allternit-brain-runner
systemctl disable --now allternit-runner-guard.service
rm /etc/systemd/system/allternit-runner-guard.service /usr/local/sbin/allternit-runner-guard
nft delete table inet allternit_runner_guard
```

The sysctl file is harmless to keep. The pre-change state is in `/root/backups/brain-runner-<time>/`.
