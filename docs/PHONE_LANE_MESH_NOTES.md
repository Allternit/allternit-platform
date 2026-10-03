# Phone lane over the Allternit mesh (design, no code)

Status: design for the phase after Lane 2a. Lane 2a (shipped) needs the phone and Desktop on one LAN.

## Today

Desktop runs adb against a phone on the same Wi-Fi: QR pairing, mDNS (`_adb-tls-pairing._tcp`, `_adb-tls-connect._tcp`), then `adb connect host:port`. The mesh already exists for computers: `mesh-manager.ts` runs a userspace tsnet sidecar (`mesh-node`) that joins the Allternit tailnet and exposes one fixed target per loopback port (`--reverse`), enrolled with the device credential through `POST /api/v1/mesh/enroll`.

## Goal

The phone is reachable from Desktop (and from a cloud computer) wherever the phone is: home Wi-Fi, another network, or cellular.

## Option A: Tailscale Android app with Allternit's control server (recommended first)

- The official Tailscale Android app supports a custom control server (Settings → Accounts → "Use an alternate server"), which is how headscale is used.
- Enrollment: Desktop shows a second QR in the wizard carrying `{controlUrl, preauthKey}` minted by `/api/v1/mesh/enroll` for a node tagged `tag:phone`. The user scans it with the phone's camera; the app opens, joins the tailnet. No Allternit app on the phone.
- Reaching adb: on Android 11+, Wireless debugging listens on a changing port (found only by mDNS, which does not cross the tailnet). Two ways to get a fixed port:
  1. One-time per boot over the LAN: `adb tcpip 5555` (needs a first authorised adb connection, which Lane 2a already has). The phone then serves adb on tcp/5555 on every interface, including the tailnet address `100.x.y.z`. Desktop connects through the existing `--reverse` loopback proxy to `100.x.y.z:5555`.
  2. Pin the Wireless-debugging port is not possible from the shell; skip.
- ACL: `tag:phone` may only be dialled on tcp/5555 from the owner's `tag:desktop` and `tag:cloud-computer` nodes. A phone must not be able to dial out to the user's other devices.
- Cellular: works, the tailnet traverses NAT (DERP relay when direct fails). Battery cost is the Tailscale app's keep-alive; acceptable for a spare phone on a charger.

## Option B: Allternit mesh-node build for Android

- Build `mesh-node` (Go, tsnet) as an Android `.so` via gomobile, wrapped in a small foreground-service app ("Allternit Phone Link") that only joins the mesh and forwards tcp/5555 to localhost.
- Pros: one app, our branding, we control reconnect behaviour and can bind adb's port. Cons: another APK to distribute outside Google Play, a verified-developer account (required from 2026–27), and Android 16/17 Advanced Protection can block sideloaded apps. This is also the stepping stone to the Lane 2c companion app (AccessibilityService + outbound WebSocket, no ADB at all).
- Decision: ship Option A now; build B only together with Lane 2c so we maintain one APK, not two.

## Keeping adb alive

- Wireless debugging switches itself off on reboot and when the phone leaves a trusted Wi-Fi. `adb tcpip 5555` is also lost on reboot.
- Reboot recovery without a person: not possible over adb alone (Android requires a user to re-enable it, and on Android 11+ there is no persistent setting). Options: (1) a Tasker/MacroDroid-style rule on the phone that re-enables Wireless debugging on boot (needs `WRITE_SECURE_SETTINGS`, granted once via `adb shell pm grant`); (2) the Lane 2c companion app, which removes the dependency on adb for everything except scrcpy.
- Desktop already backs off and retries lost phones (5 s doubling to 60 s), keeps the stored host/port, and re-discovers the connect port over mDNS on the LAN. Over the mesh it should retry the fixed `100.x:5555` target the same way.
- Show the real reason in the Phones UI: "Phone is off the network", "Wireless debugging is off (it turns off after a reboot)", "Not authorised on this computer".

## Security

- adb gives full device control: the tailnet ACL above is the boundary, plus the existing per-action approval for every text and call. Never expose tcp/5555 on a public interface; the tailnet address is the only route.
- Rotate the pre-auth key per phone, single use, short expiry; revoke the node when the user removes the phone in Settings.

## Open questions

- Does Tailscale's Android app keep a headscale node reachable for hours with the screen off on Samsung/Xiaomi power managers? Needs a soak test per OEM.
- Cloud computers reaching a phone: route through the same `tag:cloud-computer` ACL, with adb run from the cloud computer rather than Desktop; the tool gateway would then need a cloud-side twin.
