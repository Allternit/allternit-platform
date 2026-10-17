# Phone Link mesh — implementation notes (2026-10-17)

Session: kimi@pl-mesh on branch `factory/pl-mesh`. Task: `docs/PHONE_LINK_MESH_TASK.md`.

## What was done

### Part A — Go (`infrastructure/mesh/tsnet-ios/mesh.go`)
- `ServeLocal(port, localPort int) error`: listens on the tailnet via
  `tsnet.Server.Listen("tcp", ":<port>")`, dials `127.0.0.1:<localPort>` per
  accepted connection, bridges both ways with the existing `proxyConns`.
  Idempotent for the same port; different port while serving is an error;
  `StopServe()` stops it; `Close()` stops it too.
- `State() string`: `stopped` / `starting` / `needs_login` / `running` /
  `error:<short reason>`, from `LocalClient().Status().BackendState`
  (NeedsLogin/NeedsMachineAuth → `needs_login`).
- `Start`'s 60 s bound was already in place; unchanged.
- No `SetHostname` added: `NewNode(hostname)` already carries it; the Android
  side passes `phone-<deviceId[0..8]>`.
- Test seam: unexported `Node.listen` func field, replaced in tests with a
  plain loopback `net.Listen` — no Headscale needed. New `mesh_test.go`
  covers forwarding, idempotency, port validation, not-started failure,
  target-dial-failure EOF, and Close-stops-serve.
- Exported surface stays gomobile-safe (ints, strings, error only).

### Part B — Android build of the same package
- `build-android.sh`: `gomobile bind -target=android -androidapi 26
  -ldflags=-w -o Mesh.aar .` (idempotent, prints size). Output gitignored via
  new `infrastructure/mesh/tsnet-ios/.gitignore` (`Mesh.aar`,
  `Mesh-sources.jar`, `Mesh.xcframework/`).
- The Android NDK was NOT installed despite the task brief: installed
  `ndk;27.2.12479018` (2.4 GB) with the SDK's own sdkmanager into
  `ANDROID_HOME=/opt/homebrew/share/android-commandlinetools` (Homebrew
  cask `android-commandlinetools`; platform android-35 + build-tools 34/35
  already present). JDK 17 = Homebrew openjdk@17 17.0.20.1.
- gomobile needs `JAVA_HOME` set (it runs javac on the stubs) — first attempt
  failed with "Unable to locate a Java Runtime"; documented in the README.
- `Mesh.aar`: 33 MB compressed; `libgojni.so` 22.5 MB (armeabi-v7a),
  23.5 MB (arm64-v8a), 22.7 MB (x86), 25.1 MB (x86_64); Java classes
  `mesh.Mesh` / `mesh.Node` with `newNode/start/serveLocal/state/meshIP/
  stopServe/close/get/startProxy/stopProxy`. Confirmed the AAR exports the
  new Part A methods (built after the mesh.go changes).

### Part C — Android app (`surfaces/allternit-mobile/android`)
- `ProvisionCommand` gains `mesh_control_url` (https only; blank/non-string/
  non-https rejected) and `mesh_auth_key` (blank/non-string/>4096 B rejected)
  extras. `AppSettings.meshControlUrl` persists the URL (not secret).
- New `core/MeshKeyBox.kt`: pre-auth key sealed with the existing
  `BlobSealer` (Keystore AES-256-GCM in prod), base64'd into prefs;
  `read()` does not delete, `clear()` drops it. Never logged anywhere.
- New `core/MeshController.kt`: joins when provisioned (async worker thread;
  the Go `Start` blocks up to 60 s), node state dir `files/mesh`, hostname
  `phone-<deviceId[0..8]>`, then `ServeLocal(8200, internalPort)` where
  internalPort = server port +10000 (or −40000 to stay in range) — a
  dedicated loopback-only `HttpServer(viaMesh = true)`. Key is read for the
  join and `clear()`ed only after success; a failed join keeps it. Starts
  with the server (foreground service onStartCommand and both receivers
  funnel through `ServerController.startServer`), stops with
  `stopServer`/`restartServer`, rejoins after boot. `mesh_state`/`mesh_host`
  exposed via `ServerConfig` in sealed-RPC `device_info` and unauthenticated
  `/whoami` (new fields only); notification status line shows the mesh host.
- Security (the load-bearing bit): mesh-forwarded connections arrive from
  127.0.0.1, so they are marked `HttpRequest.viaMesh` at the internal
  listener. `PhoneServer` now treats them as NOT local: the one-shot armed
  grant cannot be consumed by a mesh peer, plaintext (key-revealing) pairing
  is refused to them, and RPC channel is `lan`. Cable-provisioning over adb
  forward keeps working while the mesh is up.
- `GomobileMeshNode.kt` is the only file referencing `mesh.*` (native);
  `MeshNodeApi` seam keeps unit tests off the Go runtime.
- `app/build.gradle.kts`: `implementation(files("libs/Mesh.aar"))` — required
  (not optional); `app/libs/` gitignored; README documents the prerequisite.
- `scripts/provision.sh`: `--mesh-url URL --mesh-key KEY` (single-quoted for
  the phone's shell like `--name`).

### Part D — repo truth + docs
- `infrastructure/mesh/headscale/`: README + OPS.md marked LEGACY
  (production = mail VPS `https://mail.news.allternit.com:8444`,
  `/etc/headscale/config.yaml` on the host); `config.yaml` DERP block now
  matches production: embedded DERP region 999 "allternit", STUN
  `:3478`, `urls: []`, `auto_update_enabled: false`. No deploys, no ssh.
- `surfaces/docs/guides/phone-link.mdx`: "Away from home" section.
  `python3 surfaces/docs/scripts/check_links.py` → 0 problem(s).
- `docs/dependency-map/features.json` `feature:bot-phones`: dated decision
  2026-10-17 (wording per task) + evidence entries; touches got
  `allternit/cmd/allternit-cloud-api/src/routes/mesh_*` (the two new dir
  paths are not manifest components in the map, so they are evidence, not
  touches).

## Verification (all run, green)
- `go vet ./...` + `go test ./...` in `infrastructure/mesh/tsnet-ios`:
  `ok allternit/mesh`, `ok allternit/mesh/cmd/mesh-node` (includes the new
  ServeLocal tests and the pre-existing proxy_test.go).
- `./build-android.sh` → `Mesh.aar` 33 MB (sizes above).
- `./gradlew :app:testDebugUnitTest :app:assembleDebug` → BUILD SUCCESSFUL,
  85 tests, 0 failures (15 new: mesh extras parsing, MeshKeyBox, 6
  MeshController, armed-grant-not-via-mesh socket test incl. plain pairing
  refusal, /whoami mesh fields). `stripDebugDebugSymbols` warns it cannot
  strip `libgojni.so` (expected).
- `python3 surfaces/docs/scripts/check_links.py` → 0 problem(s).
- `python3.11 scripts/dependency-map.py --validate` →
  "Valid: 33 products, 43 features, 11 journeys, 1250 components, 6088
  relationships, 17680 files." (run with `--ai /tmp/allternit-ai-validate`,
  a throwaway `gh repo clone --depth 1` of the private allternit-ai, because
  the sibling checkout is on the TCC-blocked Desktop folder; the clone and
  other /tmp scratch were deleted afterwards).

## Sizes
- Mesh.aar 33 MB compressed (94 MB uncompressed, 4 ABIs).
- `app-debug.apk` (universal debug): 91 MB with the mesh; `classes.dex`
  2.3 MB. Per-ABI cost ≈ one libgojni.so (~20 MB compressed arm64). A
  pre-mesh APK was not preserved on disk, so an exact before/after delta was
  not measured; the delta is the libgojni.so entries listed above.

## Not done / honest notes
- Out of scope per task: Desktop minting the key + passing extras, the iOS
  runner integration, any deploy, APK publishing. Nothing was deployed,
  ssh'd, pushed, or merged.
- `git` was blocked for most of the session by macOS TCC (the worktree's
  gitdir lives under `~/Desktop/allternit-workspace/...`, "Operation not
  permitted"); access was granted near the end, so the commit below was
  made on `factory/pl-mesh` without pushing.
- The `Mesh.aar` is on disk at `infrastructure/mesh/tsnet-ios/Mesh.aar` and
  copied to `app/libs/` (both gitignored); it is a build artifact, not
  committed.
- `/tmp/allternit-ai-validate`, `/tmp/meshaar` and `/tmp/mapout` (validation
  clone + scratch) were deleted at the end of the session; they lived outside
  the repo and were never part of the change.
- AGENTS.md shows a harness-injected bot block modification — not part of
  this change and left uncommitted. `docs/PHONE_LINK_MESH_TASK.md` (the task
  brief) also left uncommitted.

Co-Authored-By: Kimi <noreply@moonshot.ai>
