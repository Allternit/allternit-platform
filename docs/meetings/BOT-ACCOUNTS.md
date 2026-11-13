# Bot Google accounts for Meet pods (lane 2.1/2.2, 2026-11-13)

Google Meet lets the bot in only when it is signed in to a Google account. Free-Gmail calls refuse guests and refuse browsers in automation mode (spike result 3, CONTRACT section 1). The spike also found that the sign-in is bound to one device and one Chrome profile: you cannot copy a password or a cookie export into a new browser. A pod therefore needs a whole Chrome profile that a person has already signed in.

This document describes how pods get that profile. **Google sign-in is never automated.** A person signs in by hand, and pods only reuse the profile that sign-in produced.

## What a bot account is

- A dedicated Google account that is used only for Meet. It holds no Drive data and has no other services turned on. It uses 2-step verification with a security key kept by ops, and its recovery address is the ops mailbox.
- The account's profile name is **Gizzi (Allternit)** and its picture is the Gizzi mark. A signed-in Meet user has no name field: Meet shows the account name, so the account name is the display name. `PodConfig.displayName` is only used when the profile turns out to be signed out.
- Accounts form a pool, `gizzi-meet-01`, `gizzi-meet-02`, and so on. Each one runs at most one session at a time, because two Chromes cannot share one profile.
- In the pod config it appears as `identity = { kind: "google_account", accountEmail, profileDir: "/var/lib/allternit/bot-profile" }`. That path is the mount point the image creates (`cmd/allternit-computer-cloud/guest/build-meeting-bot-image.sh`).

## Storage: one encrypted golden profile per account

```
host: /var/lib/allternit/bot-profiles/<account>/
        gen-<n>.tar.zst.enc     golden profile, generation n (encrypted)
        gen-<n-1>.tar.zst.enc   previous generation, kept for rollback
        state.json              { status: active|needs_reauth|suspended, gen, last_ok_at, last_signed_out_at }
```

- **Encrypted at rest.** The archive is sealed with XChaCha20-Poly1305 under a host key held the same way as `ALLTERNIT_CREDENTIALS_KEY`, so it never sits in plaintext on disk. The profile is a full Google session and must be guarded like a password.
- **Copy per session.** For each session, the launcher decrypts the current generation into the container's `/var/lib/allternit/bot-profile`, owned by `meetbot`, mode 0700. When the session ends, the copy is destroyed together with the container. Pods never see the host key.
- **Portable cookies.** On Linux, Chrome encrypts cookies with a key from the OS keyring. The pod has no keyring, so the sign-in Chrome and the pod Chrome both run with `--password-store=basic`. That keeps the profile readable on any pod host. Pod Chrome must be the same major version as the sign-in Chrome, or newer.

## Keeping the session fresh, without a schedule

Google rotates session cookies as the account is used. If pods never write back, the golden copy goes stale.

- **Write-back after a good session.** When a session reached `live` and did not report `bot_profile_signed_out`, the launcher may save the pod's profile as generation n+1 (atomic rename under a per-account lock). It saves at most once per day per account, and only from sessions that ended cleanly (`left_by_user`, `empty_room`, `max_duration`). The previous generation is kept.
- No cron or launchd job refreshes profiles (Eoj, no scheduled jobs). Accounts that sit unused simply age, and the next sign-in check catches them.

## Expiry and re-sign-in (a human step)

1. **Detection.** If the Meet adapter sees the guest name field on a `google_account` profile, the profile is signed out. The adapter logs `bot_profile_signed_out`. It still tries to join as a guest, because Workspace calls that allow guests still work. The join failure detail carries `bot_profile_signed_out`.
2. **Pool update.** The launcher sets `state.json` to `needs_reauth`, takes the account out of the pool and alerts ops. Other accounts keep serving. If no account is active, Meet sessions still launch signed out and usually end `guest_blocked`. That is the honest outcome the UI shows as "Not let in".
3. **Re-sign-in.** An operator starts a container from the same image in sign-in mode, with the account's current profile decrypted into it. Chrome opens on Xvfb with `--password-store=basic`, and the operator watches over x11vnc. The operator signs in by hand, using the security key, then closes Chrome. The launcher seals the profile as a new generation and marks the account `active`.
   - It is never done in Eoj's Safari, never on a personal machine, and never by a script that types credentials.

## Risks

- Google may suspend a bot account for automated use. The bot joins openly, under its own name and tile, posts the consent notice, and makes no attempt to look human, but suspension is still possible. A suspended account becomes `suspended` in `state.json` and leaves the pool. The pool size gives headroom.
- Device Bound Session Credentials (DBSC): newer Chrome builds can tie Google cookies to a hardware key. If Google enforces this on Linux, profile copies across hosts stop working. Each account would then be pinned to one host, and the host's pods would share that host's key. Watch for `bot_profile_signed_out` happening right after a fresh sign-in.
- The profile copy lives in the pod only while the session runs. A compromised pod exposes only that one account's session. Re-sign-in plus Google's "sign out of all sessions" revokes it.

## Open decisions for Eoj

- Whether the accounts are Workspace (allternit.com) or Gmail. Workspace gives admin control and recovery. Gmail avoids Workspace policies on external calls.
- Pool size at launch. This ties to gate 5, the concurrency cap.
- Where the profile host key lives: the cloud-api credentials key, or a separate key for the pod host.
