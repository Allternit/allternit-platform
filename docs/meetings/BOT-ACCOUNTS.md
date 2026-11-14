# Bot Google accounts for Meet (lane 2.1/2.2, 2026-11-13; runtime delivery 2026-11-14)

Google Meet lets the bot in only when it is signed in to a Google account. Free-Gmail calls refuse guests and refuse browsers in automation mode (spike result 3, CONTRACT section 1). The spike also found that the sign-in is bound to one device and one Chrome profile: you cannot copy a password or a cookie export into a new browser. The bot therefore needs a whole Chrome profile that a person has already signed in.

This document describes how the bot gets that profile. **Google sign-in is never automated.** A person signs in by hand, and the bot only reuses the profile that sign-in produced.

**Where the bot runs (2026-11-14).** The company pods and the standby images are gone. The bot runs on the user's own runtime: their cloud computer or their Desktop (see the Meetings guide, "Where Gizzi runs"). cloud-api reaches that runtime through the signed relay. Only **Linux runtimes** (cloud computers) receive the signed-in profile. macOS and Windows runtimes join Meet signed out, as a guest, and Zoom and Teams do not use the profile at all.

## What a bot account is

- A dedicated Google account that is used only for Meet. It holds no Drive data and has no other services turned on. It uses 2-step verification with a security key kept by ops, and its recovery address is the ops mailbox.
- The account's profile name is **Gizzi (Allternit)** and its picture is the Gizzi mark. A signed-in Meet user has no name field: Meet shows the account name, so the account name is the display name. `PodConfig.displayName` is only used when the profile turns out to be signed out.
- Accounts form a pool, `gizzi-meet-01`, `gizzi-meet-02`, and so on. Each one runs at most one session at a time, because two Chromes cannot share one profile (the launcher claims an account per session; a runtime runs one bot at a time).
- Accounts are listed in `MEETINGS_BOT_GOOGLE_ACCOUNTS` (comma list) in cloud-api's env. No account is hardcoded anywhere.
- In the pod config it appears as `identity = { kind: "google_account", accountEmail, profileDir }`. `profileDir` is a path the runtime host chooses: `<data_dir>/meeting-bot/profiles/<hash>` (mode 0700), created when the session starts and deleted when the bot exits.
- The launcher hands out an account only when its `state.json` says `active`, a sealed generation is on disk, and `MEETINGS_BOT_PROFILE_KEY` is set. Accounts that are `needs_reauth`, `suspended` or never signed in are skipped. When no account qualifies or every qualifying account is busy, the bot joins signed out.

## Storage: one encrypted golden profile per account

The store is `cmd/allternit-cloud-api/src/services/bot_profiles.rs`, on the cloud-api host.

```
cloud-api host: $MEETINGS_BOT_PROFILE_DIR (default /var/lib/allternit/bot-profiles)/<account>/
        gen-<n>.tar.zst.enc     golden profile, generation n (encrypted)
        gen-<n-1>.tar.zst.enc   previous generation, kept for rollback (older ones are deleted)
        state.json              { status: active|needs_reauth|suspended, gen, last_ok_at, last_signed_out_at, last_sealed_at }
        .lock                   per-account file lock (cloud-api and the ops binary both take it)
```

- **Encrypted at rest.** The archive is sealed with ChaCha20-Poly1305 (`ring`) under `MEETINGS_BOT_PROFILE_KEY`. The key is base64 of 32 random bytes (`openssl rand -base64 32`). It is a separate key from `ALLTERNIT_CREDENTIALS_KEY`. Every seal uses a fresh random 96-bit nonce, and the account email is the associated data, so a file moved into another account's directory does not open. The plan named XChaCha20-Poly1305, but no XChaCha crate is in the workspace, and `ring` is. Seals are rare (one at sign-in, then at most one per account per day), so random 96-bit nonces are safe. File layout: `ALTBPv1\n` + nonce + ciphertext + tag. Directories are 0700 and files are 0600. Every write goes to a temp file, is fsynced, then renamed.
- **Account names** must look like an email (`[A-Za-z0-9._+-]`, exactly one `@`, no leading dot, no `..`). Any other name is refused, so no name can step outside the store.
- **Delivery per session.** The profile is not in the start request (the relay caps request bodies at 1 MB). The start request carries only `bot_profile: { account }` or `null`. The runtime then pulls the profile with its pod token from `GET /meeting-sessions/:sid/bot-profile`. cloud-api decrypts the current generation and answers with the plain `tar.zst`, or `204` when the session has no profile. Only Linux runtimes call it. The runtime unpacks the archive into its per-session profile directory and deletes the directory when the bot exits. Runtimes never see the key. If decryption fails (wrong key or a corrupt file), the account is dropped from that session's claim and the bot joins signed out.
- **Portable cookies.** On Linux, Chrome encrypts cookies with a key from the OS keyring. A cloud computer has no keyring, so the sign-in Chrome and the bot Chrome both run with `--password-store=basic`. That keeps the profile readable on any Linux runtime. The bot Chrome must be the same major version as the sign-in Chrome, or newer.

## Keeping the session fresh, without a schedule

**There is no write-back.** A runtime never sends its profile copy back, so cloud-api keeps the golden copy exactly as a person sealed it. Google rotates session cookies as an account is used, so a golden copy can age out. When that happens, the bot sees the signed-out page (see below) and the account is re-signed in by hand. No cron or launchd job refreshes profiles (Eoj, no scheduled jobs).

## Expiry and re-sign-in (a human step)

1. **Detection.** If the Meet adapter sees the guest name field on a `google_account` profile, the profile is signed out. The adapter logs `bot_profile_signed_out`. It still tries to join as a guest, because Workspace calls that allow guests still work. The join failure detail carries `bot_profile_signed_out`.
2. **Pool update.** When the runtime's ingest batch carries an end detail containing `bot_profile_signed_out` (`routes/meeting_pod.rs`), cloud-api sets that session's account to `needs_reauth`, takes it out of the pool, logs an `OPS` warning and sends an ops alert email (`ops_alert`, deduplicated per account). Other accounts keep serving. If no account is active, Meet sessions still launch signed out and usually end `guest_blocked`. That is the honest outcome the UI shows as "Not let in".
3. **Re-sign-in.** An operator signs in by hand, in a Chrome started with `--password-store=basic --user-data-dir=<new dir>`, with the security key, checks meet.google.com, and closes Chrome. The `bot-profile-signin.sh` script ran inside the pod image and was removed with it. Tar the profile directory (leave out `Cache`, `Code Cache`, `GPUCache`, `Service Worker/CacheStorage`, `Crashpad`, the shader caches and the `Singleton*` locks) and, on the cloud-api host, run `meetings-bot-profile seal <account> <archive>`. That seals the archive as a new generation and marks the account `active`.
   - It is never done in Eoj's Safari, never on a personal machine, and never by a script that types credentials.

### First sign-in, step by step

Sign in by hand as described in step 3 above, on a Linux machine with the same Chrome major version the cloud computers use. Copy the archive to the cloud-api host, then run, with cloud-api's env loaded (`systemctl show -p EnvironmentFiles allternit-cloud-api` shows where it is):

```
meetings-bot-profile seal <account-email> /root/<account>.tar.zst && shred -u /root/<account>.tar.zst
meetings-bot-profile status
```

Then add the account to `MEETINGS_BOT_GOOGLE_ACCOUNTS` and restart cloud-api. For a re-sign-in, run `meetings-bot-profile open <account> /root/<account>-current.tar.zst` on the cloud-api host first.

### Ops binary

`meetings-bot-profile` (a `[[bin]]` in cloud-api) runs on the cloud-api host with cloud-api's env:

| Command | What it does |
|---|---|
| `seal <account> <archive.tar.zst>` | Seals the archive as the next generation and marks the account `active` (needs the key) |
| `status [account]` | Shows status, generation, generations on disk, whether it is deliverable, and timestamps |
| `mark <account> active\|needs_reauth\|suspended` | Sets the status by hand |
| `open <account> <out.tar.zst>` | Decrypts the current generation for a re-sign-in (needs the key; the output file must not exist yet) |

### Env (cloud-api)

| Name | Default | Purpose |
|---|---|---|
| `MEETINGS_BOT_GOOGLE_ACCOUNTS` | empty | Pool of bot accounts (comma list) |
| `MEETINGS_BOT_PROFILE_KEY` | unset (profiles are not delivered; Meet joins signed out) | base64 of 32 random bytes |
| `MEETINGS_BOT_PROFILE_DIR` | `/var/lib/allternit/bot-profiles` | Profile store |

## Risks

- Google may suspend a bot account for automated use. The bot joins openly, under its own name and tile, posts the consent notice, and makes no attempt to look human, but suspension is still possible. A suspended account becomes `suspended` in `state.json` and leaves the pool. The pool size gives headroom.
- Device Bound Session Credentials (DBSC): newer Chrome builds can tie Google cookies to a hardware key. If Google enforces this on Linux, profile copies across hosts stop working. Each account would then be pinned to one host, and each runtime would need its own sign-in. Watch for `bot_profile_signed_out` happening right after a fresh sign-in.
- The profile copy lives on the runtime only while the session runs. A compromised runtime exposes only that one account's session, and only a Linux runtime ever receives it. Re-sign-in plus Google's "sign out of all sessions" revokes it.

## Decided

- **Profile key (Eoj, 2026-11-13):** a new secret, `MEETINGS_BOT_PROFILE_KEY`, in cloud-api's env (base64 of 32 random bytes). It is not `ALLTERNIT_CREDENTIALS_KEY`.
- **Test account (Eoj, 2026-11-13):** the pool is one account, used only for testing: gizziioetrid@gmail.com. It is configured through `MEETINGS_BOT_GOOGLE_ACCOUNTS`, never in code.

## Open decisions for Eoj

- The production pool: which accounts, and whether they are Workspace (allternit.com) or Gmail. Workspace gives admin control and recovery. Gmail avoids Workspace policies on external calls.
- Pool size at launch. This ties to gate 5, the concurrency cap.
