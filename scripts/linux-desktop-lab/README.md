# Linux Desktop test lab

Checks that a released Allternit Desktop `.deb` installs and starts on the Linux
versions we support, from the oldest up: Ubuntu 18.04 (glibc 2.27, the floor),
Debian 10 (2.28), Ubuntu 22.04, Debian 12, Debian 13 and Ubuntu 24.04. The floor
is the oldest glibc on which the Electron shell starts; see
`scripts/build-linux-sidecars.sh`. Debian 10 is end of life, so CI points its apt
at `archive.debian.org` first. On Ubuntu 18.04 the Meeting bot cannot run (its
LiveKit prebuilt needs glibc 2.28); nothing here starts it.
The same scripts run in CI (`.github/workflows/linux-desktop-smoke.yml`) and by
hand on the standby incus host.

| File | What it does |
| --- | --- |
| `lab.sh <name> <image> [deb]` | Makes an incus container, installs X/dbus/keyring/fonts, creates user `tester`, runs `apt-get install ./deb`, then runs `--version` on each sidecar. Proves the install resolves its own `Depends` and the sidecars link against this OS's glibc. |
| `../check-elf-floor.mjs --dir <app> --shell allternit` | CI step 2: every ELF in the extracted package, the Electron shell included, needs glibc <= the floor (the shell can never be allowlisted). |
| `smoke-api.sh` | Starts the bundled `allternit-api` and polls `/health/live` for 30 s. Fails on a `GLIBC ... not found` error or an early exit. Proves the engine can run on this OS. |
| `smoke-app.sh [user]` | Starts the app as a non-root user under Xvfb with a dbus session and an unlocked keyring, with `--no-sandbox --remote-debugging-port=9222`. |
| `smoke-logs.sh [user]` | Greps `main.log` and the app output for "Initialization Error", "Failed to initialize bundled mode" and GLIBC errors. |
| `smoke-cdp.cjs` | Connects over CDP (playwright-core), screenshots every window, and passes only if a page has `#btn-get-started` and `main.log` has no "Initialization Error" / "Failed to initialize bundled mode". Proves the app got past engine start and drew the startup window. |
| `run.sh <name>` | Manual lab: starts the app in a lab container, waits 45 s, pulls a screenshot, `main.log` and the CDP window list to `$LAB_OUT/<name>/`. |
| `drive.cjs` | Manual lab: clicks Get started and signs in with a one-time Clerk ticket, then watches pairing. |

## Run it by hand (standby)

```bash
./lab.sh lab-debian12 images:debian/12            # 1.1.15 by default, or pass a .deb path or URL
incus exec lab-debian12 -- bash -s < smoke-api.sh # expect FAIL on 1.1.15 (GLIBC)
```

For the app check, copy `smoke-app.sh` into the container, run it, tunnel the
container's port 9222 to your machine, then:

```bash
npm i --no-save playwright-core@1.49.1
node smoke-cdp.cjs ./node_modules/playwright-core 9222 ./shots /path/to/main.log
```

`main.log` is `~/.config/@allternit/desktop/main.log` of the app's user.

## Sign-in and pairing are manual

CI has no Clerk secrets and does not sign in. To test sign-in, make a one-time
Clerk sign-in ticket, write it to a file, and pass the file path:
`node drive.cjs ./node_modules/playwright-core 9222 ./shots /path/to/ticket-file`.
The ticket is read from the file and never printed. Do not commit it.

## Containers need `--no-sandbox`

An unprivileged container (CI Docker or incus without KVM) cannot create the
user namespaces Chromium's sandbox needs, so Electron exits at start. Every
script here passes `--no-sandbox`. The sandbox itself is not tested here. Real
desktops are not affected. This lab also has no GPU, so it proves start-up and
rendering of the startup window only, not video or hardware paths.
