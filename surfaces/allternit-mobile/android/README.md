# Allternit Phone Link for Android

`com.allternit.phonelink` lets Allternit bots use the AI apps (ChatGPT, Claude, Gemini, Grok, Telegram and others) that are already signed in on the user's own Android phone. It is the `android-app` route in `SPEC-signed-in-apps-routing-2026-10-16.md`.

The phone runs a small HTTP server on port 8200. It speaks the iCTRL protocol (protocol_version 1), the same one as the iPhone runner, so Allternit Desktop's `ictrl-client.ts` and iCTRL's `ictrl_rpc.py` work against it unchanged. An AccessibilityService reads the screen and taps and types. Setup is done once over the USB cable. After that the phone needs no Developer options, and it keeps working after restarts.

## Layout

```
app/src/main/java/com/allternit/phonelink/
  core/                         pure Kotlin, unit-tested on the JVM
    SecureEnvelope.kt           v1 envelope: HMAC-SHA256 PRF-CTR + encrypt-then-MAC
    X25519.kt                   RFC 7748 X25519 on BigInteger (minSdk 26 has no XDH)
    AuthManager.kt              pairing, sessions, replay + skew rejection (port of AuthManager.swift)
    PhoneServer.kt              /whoami /schema /skill /pair /rpc (port of JSONRPCServer.swift routes)
    HttpServer.kt               tiny HTTP/1.1 server, no dependency
    RpcDispatcher.kt            JSON-RPC methods, safety guards, element refs
    UiTree.kt                   page_source text/structured forms, filters, find_element matching
    DeviceOps.kt                interface the accessibility service implements
    Provisioning.kt             settings, PROVISION extras, one-shot pairing grant, auto-start rule
    MeshKeyBox.kt               sealed-at-rest storage for the mesh pre-auth key
    MeshController.kt           owns the embedded tsnet node and the mesh-side forwarder
    EncryptedFileSessionStore.kt sessions sealed with a BlobSealer
    Schema.kt                   GET /schema and GET /skill
  PhoneLinkAccessibilityService.kt  DeviceOps on the real device (gestures, tree, screenshot)
  PhoneLinkService.kt           foreground service + "Allternit can use your phone's apps · Stop"
  ServerController.kt           process-wide owner of the server
  Receivers.kt                  BootReceiver, ProvisionReceiver (DUMP-protected), StopReceiver
  PairingNotifier.kt            Allow/Deny notification (60 s), PairingActionReceiver
  KeystoreSealer.kt             Android Keystore AES-256-GCM
  GomobileMeshNode.kt           MeshNodeApi backed by the gomobile tsnet AAR
  MainActivity.kt               status screen (sessions, Stop/Start, Revoke) + TWA design note
scripts/provision.sh            the adb setup sequence below
scripts/conformance.sh          live run of ictrl_rpc.py against a phone
```

## Build and test

You need JDK 17 and Android SDK 35 (platform `android-35`, build-tools 35.x). The wrapper jar is not committed. Generate it once with any Gradle 8.x: `gradle wrapper --gradle-version 8.10.2`.

**Prerequisite — the mesh AAR.** The app embeds the gomobile-bound tsnet node from `infrastructure/mesh/tsnet-ios`. Build it first and copy the result to `app/libs/` (gitignored):

```sh
cd infrastructure/mesh/tsnet-ios
./build-android.sh          # needs ANDROID_HOME with ndk;27.2.12479018
cp Mesh.aar ../../../surfaces/allternit-mobile/android/app/libs/
```

Then:

```sh
cd surfaces/allternit-mobile/android
export JAVA_HOME=/path/to/jdk-17
export ANDROID_HOME=$HOME/Library/Android/sdk     # or write sdk.dir in local.properties
./gradlew :app:testDebugUnitTest :app:assembleDebug
# APK: app/build/outputs/apk/debug/app-debug.apk
```

Toolchain: AGP 8.7.3, Kotlin 2.0.21, Gradle 8.10.2, minSdk 26, target/compileSdk 35.

The app has no runtime dependencies beyond the Kotlin stdlib and the mesh AAR (tsnet/userspace WireGuard, built in-tree). Tests use JUnit 4.13.2 (EPL-1.0, test only) and `org.json:json:20240303` (public domain, test only, because android.jar ships only stubs of org.json). Everything else is in-tree. NanoHTTPD was not needed, and the X25519 code is written from RFC 7748 rather than vendored.

**APK size.** The universal debug APK is ~91 MB with the mesh included; `libgojni.so` (the Go runtime + tsnet) is 94 MB uncompressed across the four ABIs (23.5 MB arm64). A single-ABI release build pays roughly one ABI's worth (~20 MB compressed). `stripDebugDebugSymbols` cannot strip gomobile's `libgojni.so`.

The unit tests cover:
- the envelope against golden values produced by `ictrl_rpc.py`;
- X25519 against the RFC 7748 vectors and the host JDK's XDH;
- the pairing wrap;
- replay, ±120 s skew, forged tags and idle expiry;
- pairing approve, deny, timeout, busy, cooldown and the armed grant;
- the armed grant and plaintext pairing being refused to mesh-forwarded (viaMesh) connections;
- every RPC method against a fake device, including the lock-screen and protected-screen guards;
- provisioning extras (including the mesh extras validation) and the auto-start rule;
- the mesh key box (sealed at rest, dropped only after a successful join) and MeshController (join, drop, failure keeps the key, viaMesh marking);
- the encrypted session store;
- a full socket run of whoami, pair, sealed RPC, replay and skew.

## Provisioning (one time, over the cable)

Allternit Desktop runs the steps below over adb. `scripts/provision.sh` does the same thing. USB debugging has to be on for this one session, and it can be switched off afterwards.

```sh
PKG=com.allternit.phonelink
# 1. install (adb install => PACKAGE_SOURCE_UNSPECIFIED: not a "restricted settings" app)
adb install -r -g app-debug.apk
# 2. only needed if the APK was ever opened from a file manager/browser (see below); harmless otherwise
adb shell appops set $PKG ACCESS_RESTRICTED_SETTINGS allow
# 3. enable the accessibility service (append to any already enabled, colon-separated)
adb shell settings put secure enabled_accessibility_services $PKG/.PhoneLinkAccessibilityService
adb shell settings put secure accessibility_enabled 1
# 4. Doze/app-standby exemption so the server answers while the phone idles
adb shell cmd deviceidle whitelist +$PKG
# 5. notifications (Android 13+ runtime permission)
adb shell pm grant $PKG android.permission.POST_NOTIFICATIONS
# 6. start the foreground service
adb shell am start-foreground-service -n $PKG/.PhoneLinkService
# 7. provision: arm ONE auto-approved pairing (loopback only, 10 min), LAN on, name
adb shell am broadcast -a com.allternit.phonelink.PROVISION -n $PKG/.ProvisionReceiver \
    --ez arm_pairing true --ez allow_remote_access true --es device_name "'Joe moto'" --ei listen_port 8200
#    -> "Broadcast completed: result=-1, data="provisioned device_id=… remote=true port=8200 armed=true""
# 8. reach the phone's server from the computer and pair through the forward
adb forward tcp:52100 tcp:8200
python3 ictrl_rpc.py --host 127.0.0.1:52100 pair --name "$(hostname)"   # or Desktop's IctrlClient.pair()
# 9. from now on use the phone's Wi-Fi address with the same key: <phone-ip>:8200
```

You can check the result with `adb shell dumpsys accessibility | grep -A3 "Bound services"`. It should list `com.allternit.phonelink/.PhoneLinkAccessibilityService`.

## Away from home: the mesh

On the same Wi-Fi the phone is reachable at its LAN address. Away from that Wi-Fi, the phone can instead join the customer's private Allternit mesh (a Headscale tailnet): provisioning passes two optional extras, and the app embeds a userspace WireGuard node (`tsnet` via gomobile — no OS VPN slot, so it cannot collide with an always-on VPN) that serves the same iCTRL server on the tailnet at `<meshIP>:8200`.

```sh
adb shell am broadcast -a com.allternit.phonelink.PROVISION -n $PKG/.ProvisionReceiver \
    --ez arm_pairing true --ez allow_remote_access true \
    --es mesh_control_url "'https://mail.news.allternit.com:8444'" \
    --es mesh_auth_key "'<preauth-key from Desktop>'"
# or: scripts/provision.sh --mesh-url https://... --mesh-key <key>
```

- `mesh_control_url` (https only) and `mesh_auth_key` are optional. Desktop mints the single-use key with `POST /api/v1/mesh/enroll` at cable setup.
- The key is sealed with the Keystore sealer on arrival, never logged, and dropped after the first successful join; from then on tsnet re-authenticates with the node key in the app's private `files/mesh` dir. A failed join keeps the sealed key.
- The node starts and stops with the foreground service and rejoins after reboot like the server. `device_info` and the unauthenticated `/whoami` report `mesh_state` (`stopped`/`starting`/`needs_login`/`running`/`error:…`) and `mesh_host` (`<100.x>:8200` or null). The notification shows the mesh host once joined.
- Network changes (Wi-Fi ↔ cellular, new Wi-Fi) are handled by tsnet/magicsock — there is no app-level reconnect loop.
- The mesh path lands on a dedicated internal loopback listener whose requests are marked `viaMesh`. That marking is load-bearing: the bridge forwards tailnet connections from 127.0.0.1, and without it a mesh peer could consume the one-shot armed pairing grant or use plaintext pairing. Mesh-forwarded requests are treated as remote — sealed envelopes required, approval or an existing session needed, armed grant refused. The existing server settings (`allow_remote_access`, `listen_port`) are untouched: the mesh reaches the same server through loopback.

### Which commands the shell user may run on Android 14

These were checked on 2026-10-16 against AOSP `android14-release` sources, fetched from android.googlesource.com.

| Step | Needs | Shell has it? (source) |
|---|---|---|
| `settings put secure …` | `WRITE_SECURE_SETTINGS` | Yes: `packages/Shell/AndroidManifest.xml` |
| `cmd deviceidle whitelist +pkg` | `DEVICE_POWER`: `DeviceIdleController.onShellCommand` calls `enforceCallingOrSelfPermission(DEVICE_POWER)` for "whitelist" (apex/jobscheduler/…/DeviceIdleController.java, around line 4898) | Yes: Shell manifest |
| `pm grant … POST_NOTIFICATIONS` | `GRANT_RUNTIME_PERMISSIONS` | Yes: Shell manifest |
| `appops set … ACCESS_RESTRICTED_SETTINGS allow` | `MANAGE_APP_OPS_MODES` | Yes: Shell manifest |
| `am start-foreground-service` | The service is `exported` and guarded by `android.permission.DUMP`. Starting a foreground service from the background is allowed because the caller holds `START_FOREGROUND_SERVICES_FROM_BACKGROUND`. | Yes: Shell manifest has both `DUMP` and `START_FOREGROUND_SERVICES_FROM_BACKGROUND` |
| `am broadcast … PROVISION` | Receiver guarded by `android.permission.DUMP` | Yes: Shell manifest. No ordinary app can hold DUMP (signature/privileged/development). |
| `adb forward` | Runs on the host; adbd connects to the phone's 127.0.0.1:8200 | n/a |

**Restricted settings (Android 13+).** On Android 14, `AccessibilityManagerService.updateServicesLocked()` checks `isAccessibilityTargetAllowed()` before it binds an enabled service. When `config_enhancedConfirmationModeEnabled` is true (the AOSP default), that check also requires the app op `ACCESS_RESTRICTED_SETTINGS` to be `MODE_ALLOWED`. If it isn't, the service is disabled again, even when `enabled_accessibility_services` names it. So writing the setting over adb does not bypass the restriction by itself. What keeps it from applying is the install source. `InstallPackageHelper` sets the op to `MODE_ERRORED` only for `PACKAGE_SOURCE_LOCAL_FILE` and `PACKAGE_SOURCE_DOWNLOADED_FILE`, which are the sources a file manager or browser install through the system installer uses. `adb install` leaves the source unspecified, so the op stays allowed. Step 2 sets it to `allow` explicitly in case the APK was ever installed by tapping a file.

**After a reboot**, three things bring the server back with no user action:
- The system rebinds the enabled accessibility service after the first unlock. `onServiceConnected` starts the server and the foreground service.
- `BootReceiver` (`BOOT_COMPLETED`, `MY_PACKAGE_REPLACED`, `QUICKBOOT_POWERON`) starts the foreground service. Android 15 allows the `specialUse` type to start from `BOOT_COMPLETED`.
- `deviceidle whitelist` persists across reboots and keeps the app reachable in Doze.

`LOCKED_BOOT_COMPLETED` is deliberately not used. Nothing useful can run before the first unlock: the accessibility service isn't direct-boot aware, the session file is in credential-encrypted storage, and every UI method refuses while the phone is locked.

## Protocol compatibility with iCTRL (protocol_version 1)

These parts are byte-identical to iCTRL:
- Labels: `ictrl-v1|mac`, `ictrl-v1|enc-req`, `ictrl-v1|enc-resp`, `ictrl-pair`, `ictrl-wrap`.
- `tok` = SHA-256(K)[0..4] in hex.
- The PRF-CTR keystream.
- The tag over `"v1"‖tok‖be64(ts)‖nonce‖ct`.
- The reply tag over `"resp"‖nonce‖ct`.
- The X25519 pairing wrap `K ⊕ HMAC(HMAC(shared, "ictrl-pair"‖c_pub‖d_pub), "ictrl-wrap")`.
- `plain:true` pairing, allowed from loopback only.
- Error shapes `{error:{code,message,server_ts,retry_after?}}` with the same codes: `auth_required`, `bad_envelope`, `auth_failed` (same text), `ts_skew`, `bad_tag`, `nonce_replayed`, `nonce_flood`, `pair_denied`, `pair_busy`, `pair_cooldown`, `plain_forbidden`.
- Sealed replies are always HTTP 200.
- The `X-ICTRL-Device` header is on every response.
- `/whoami` without CORS.
- The 30-day sliding idle expiry, 100 sessions, a 60 s approval window, a 15 s cooldown, and separate loopback and remote pairing slots.

What had to adapt:
1. **Coordinates are pixels, not points.** `device_info` reports `coordinate_space: "pixels"`, `scale: 1` and `point_width == pixel_width`. A client that converts screenshot pixels to tap coordinates with `pixel / screenshot.scale`, as `ictrl_rpc.py shot` prints, still gets it right. A `screenshot{scale}` above 1 is clamped to 1.
2. **Apps are package names.** `bundle_id` is accepted and holds `com.openai.chatgpt` and so on. `list_apps` adds `package` and `system`. "System" means a preinstalled app with no Play update; pass `element_type:"all"` to include those.
3. **page_source** keeps iCTRL's line shape (`Class, label: '…', value: '…', id: '…', {{x, y}, {w, h}}`) with Android meanings:
   - label = content-desc, or the text if there is no content-desc;
   - value = an editable field's text, or the text when content-desc is the label;
   - id = resource-id;
   - flags such as `clickable editable focused scrollable checked offscreen` come after the frame;
   - each window starts with `Window(application|input_method|system)`.

   `structured:true` adds a `nodes` array with raw fields and a `ref` per node. `filter`, `visible_only`, `max_depth` and `max_elements` work as in iCTRL.
4. **screen_text** reads text nodes from the accessibility tree (`source:"tree"`, no OCR dependency). `include_image:true` attaches a screenshot.
5. **screenshot** uses `AccessibilityService.takeScreenshot`, so it needs Android 11 (API 30) or later. Windows that set FLAG_SECURE return `secure_window`.
6. **New method `back`.** `home` and `back` are global actions. `terminate_app` is best effort: it presses Home if the app is in front, then calls `killBackgroundProcesses`, because Android lets no ordinary app force-stop another.
7. **Pairing approval** happens in a notification with Allow and Deny. On Android 12+ Allow needs the phone unlocked. The same request also shows in the app, so it can be answered even without notification permission.
8. **The armed one-shot grant** (the Android version of `ICTRL_ALLOW_AUTH_ONCE`) is persisted for 10 minutes rather than lasting for the process, because the broadcast and the pairing can land in different processes. It only applies to loopback requests, which here means the adb forward, so a LAN neighbour can't race Desktop for it.
9. **Not implemented:** `/stream`, `stream_ticket` (returns -32601), `/remote`, `/logs`, nearby discovery (`config{nearby_discovery:true}` → `unsupported`), `resolve_app`, `page`, `collect`, `scroll_to_element`, `lock`/`unlock`, `reset`, `logs`. `scroll` uses `ACTION_SCROLL_*` on the main list and falls back to a swipe.
10. **Typed errors.** JSON-RPC errors carry `data.type`. The values are `screen_locked`, `screen_off`, `protected_screen`, `protected_app`, `protected_url`, `accessibility_off`, `no_focus`, `element_not_found`, `stale_ref`, `app_not_installed`, `no_handler`, `gesture_failed`, `secure_window`, `invalid_params` and `unsupported`. The code is -32000, or -32602 for bad params.
11. `listen_port` must be 1024–65535, since Android apps cannot bind lower ports.

## Security model

- **Who can pair.** Pairing needs one of two things:
  - a tap on Allow on the phone; or
  - the one-shot grant, which only `adb shell` or the system can arm. The PROVISION receiver and the service require `android.permission.DUMP`, and the grant is consumed by the first loopback pair within 10 minutes.

  A refused or expired request starts a 15 s cooldown, and only one request can wait per slot.
- **What travels.** Every `/rpc` body is a sealed v1 envelope. There is no plaintext path. The key K never crosses the network after pairing: X25519 mode wraps it, and plain mode is loopback only. Replays inside the ±120 s window are refused per session. The MAC is checked before the replay cache is touched.
- **Where keys live.** Session keys are kept in `files/sessions.bin`, sealed with AES-256-GCM under a non-exportable Android Keystore key. EncryptedSharedPreferences is not used, since it is deprecated. Backups and device transfer exclude all app data. If the blob can't be read, it loads as empty and clients re-pair.
- **What the bot can't do.**
  - It never acts while the keyguard is showing or the screen is off. All screen methods refuse with `screen_locked` or `screen_off`.
  - Touch and type methods refuse while Settings, the permission controller, the package installer or Phone Link itself is in front (`protected_screen`).
  - `launch_app` and `open_url` refuse those packages, and `open_url` refuses `intent:`, `file:`, `content:` and `package:` URLs.
  - Reading the screen and `home`/`back` stay allowed, so a bot can leave a protected screen.
- **Stop.** The notification's Stop closes the listening socket at once, drops in-flight connections and stops the foreground service. Stop is remembered across reboots until the user taps Start in the app or Desktop provisions again. `config{stop_server:true}` does the same about 1 s after it replies.
- **Revoking.** The app lists every paired computer with Revoke and Revoke all. `config{revoke_session_id|clear_sessions}` does the same over RPC.
- **X25519 timing.** The BigInteger ladder is not constant-time. It only runs at pairing, on a fresh device key each time, so nothing long-lived can leak through timing.

## Known limits

- The phone must be unlocked and its screen on for UI methods. There is no remote unlock, by design. For long runs, set the screen timeout high in Settings › Display; this does not need Developer options.
- Wi-Fi reachability is up to the router and the phone's Wi-Fi sleep policy. If the LAN route drops, Desktop should fall back to the adb forward or report `NO_ROUTE`.
- `type` uses `ACTION_SET_TEXT`. The paste fallback overwrites the clipboard. `submit` needs Android 11+ (`ACTION_IME_ENTER`); below that, tap the app's Send button.
- Text drawn inside images or canvases isn't in the tree and there is no OCR. Use `screenshot` and vision when that matters.
- The phone's own UI tree is reported in pixels of the current rotation. Re-read `device_info` after rotating.
- Some OEMs (Samsung, Xiaomi) add their own background killers on top of Doze. The `deviceidle` exemption covers stock Android and Motorola. Others may need their battery setting changed by hand.
- QUICKBOOT_POWERON isn't a protected broadcast, so any app can send it. The worst it does is start the server on a phone that is already provisioned.

## Later: Trusted Web Activity (not built)

The plan is to wrap m.allternit.com as a TWA in this same app, so one install gives the Allternit phone UI plus Phone Link. This needs three things:
- `https://m.allternit.com/.well-known/assetlinks.json` with `com.allternit.phonelink` and the release signing certificate's SHA-256;
- an `asset_statements` resource;
- `androidx.browser` with a LauncherActivity as the launcher, with this status screen moved behind a "Phone Link" entry.

It waits on the release signing key and the assetlinks deploy. The design note is in `MainActivity.kt`.

## Live conformance

```sh
# provision over USB, forward 52100 -> 8200, then run the reference client against the phone
scripts/conformance.sh 127.0.0.1:52100 --provision --serial <adb-serial>
scripts/conformance.sh 127.0.0.1:52100 --act        # also home/swipe/tap/back/open_url checks
scripts/conformance.sh 192.168.1.20:8200            # over Wi-Fi (pairing needs a tap on Allow)
```

The script checks `/whoami` and the device header, that an unsealed `/rpc` gets a 401, X25519 pairing, `device_info`, `is_locked`, `list_apps`, `config`, an unknown method, `page_source` (text and structured), `screen_text`, `find_element` and `screenshot`. It also confirms that a replayed nonce, a skewed timestamp and a forged tag are rejected, using `ictrl_rpc.py`'s own crypto. At the end it revokes its own key. It uses a throwaway `HOME`, so `~/.ictrl/token.json` is never touched.
