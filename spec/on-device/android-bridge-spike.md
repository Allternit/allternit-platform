# S1: Android bridge spike — verdict

Plan: `PLAN-on-device-models-2026-11-15.md` (design decision 6, step S1, risk "Android bridge"). Branch: `ondevice/s1-android-bridge` (scratch, not for main). Code and run steps: `surfaces/allternit-mobile/android/spike/s1-bridge/README.md`.

## Verdict

**Use candidate 1: the TWA postMessage channel.** It meets the pass bar on the real Moto. W9 builds it, with the four things it needs (see "What W9 builds").

| Pass bar | Result on the Moto |
|---|---|
| Airplane mode | Yes. `airplane_mode_on=1`, "Active default network: none", ping "Network is unreachable", page `navigator.onLine=false` |
| 500 streamed messages reach the page | Yes. 500/500 in order, 0 native failures, in all 22 offline runs |
| No permission prompt | None. The full-screen TWA showed only Chrome's normal "Running in Chrome" first-run bar. The spike never asks for POST_NOTIFICATIONS |
| Works again after a page reload | Yes. 21/21 reloads: 1 page-initiated and 20 from DevTools, each in a fresh document with a fresh port |
| Throughput and latency | 500 × 16 B: median 3,018 msgs/s (range 1,511–4,095). 5,000 × 16 B: median 4,404 msgs/s. Round trip p50 2.9 ms, p95 4.3 ms. First message 4 ms after the request (median) |

Candidate 2 (localhost HTTP) also moved data offline, about 3× faster. It is the fallback, not the choice. On the production origin it is blocked by m.allternit.com's own CSP. It depends on Local Network Access behaviour that does not match Chrome's docs and can change. And it opens a port that any website or app on the phone can reach. Candidate 3 (model-hosts relay) needs a network connection, so it fails "offline" by definition. It stays a fallback for the "app not installed / channel not verified" case only.

## Device and setup

- **Phone:** moto g play (2024) (`fogona`), Android 14 (API 34), USB adb.
- **Chrome:** 157.0.8103.50.
- **Build:** `playDebug` from the project wrapper (Gradle 8.10.2, AGP 8.7.3). JDK 17 is Homebrew `openjdk@17`, already installed. The SDK is the existing Homebrew `android-commandlinetools` (platform 36, build-tools 36). **Nothing new was installed system-wide.**
- **APK:** `surfaces/allternit-mobile/android/app/build/outputs/apk/play/debug/app-play-debug.apk`. It is a debug build installed as `com.allternit.app.s1`, so the Play-installed `com.allternit.app` 0.5.2 on the Moto was not touched.
- **Release builds are unaffected:** the `playRelease` manifest, dex class list and APK file list are byte-identical before and after the spike (`aapt2 dump xmltree`, `dexdump`, `unzip -l` diffs).
- **Tested flavor:** `play`, as the coordinator asked, because the bridge has to work in the Play build. Everything the bridge needs is shared code, not Phone Link code. In the spike it sits in the debug source set. W9 puts it in `src/main`.

## What the test origin was, and how far it is from production

The page cannot be m.allternit.com for an automated offline test. Its `assetlinks.json` has no `use_as_origin` relation, and adding one is a production deploy. So:

- **Mode A (the measured runs).** The app's own foreground service serves the test page from APK assets at `http://localhost:8765/s1.html`. It also serves a real `/.well-known/assetlinks.json` for its own package and signing cert, with both `handle_all_urls` and `use_as_origin`. Chrome 157 fetches that file **from the origin itself**; it does not use Google's DAL API (logcat showed the request arrive at our server). So verification is real: Chrome checks the signed APK against the statement, and no Chrome dev flag is used. `http://localhost` is the one non-https origin Chrome's `OriginVerifier` accepts.
- **The production offline path, reproduced.** Offline, m.allternit.com's `assetlinks.json` fetch fails, and Chrome falls back to the verification result it saved online. The spike reproduces that exactly. The server resets the connection on every `assetlinks.json` request (`DAL_DROP`), and Chrome is cold-started in airplane mode. Chrome logged `Digital Asset Links connection failed.` then `cr_OriginVerifier: Device is offline, checking saved verification result.`, validated, and opened the channel. The run passed, including the reload.
- **Remaining distance from production:**
  - The origin is `localhost`, not `m.allternit.com`.
  - The package is `com.allternit.app.s1` with the debug cert, not `com.allternit.app` with the release/Play cert.
  - The launcher is the spike's, not `ReviewLauncherActivity`.
  - The page is a test page, not the PWA.

  Chrome's mechanism (DAL fetch, saved result, port delivery, reload) is the same code path.
- **Mode B (the real origin, offline and online).**
  - The real m.allternit.com page opened in the TWA, and its service worker was present.
  - The spike's client was injected over DevTools and the channel was requested.
  - Validation returned `false`, as expected without `use_as_origin`, and the page got **no port** (`ports: 0`).
  - This proves the assetlinks change is required, not optional.

## Numbers

All mode A runs, page-side timing (`performance.now()`). "First" is the time from the page's request to the first message. Native post time is the time `CustomTabsSession.postMessage` spent in the loop.

**Offline, airplane mode, cold Chrome (22 runs: 1 launch + 1 auto-reload + 20 DevTools reloads):**

| Test | min | median | max |
|---|---|---|---|
| C1 500 × 16 B, msgs/s | 1,511 | 3,018 | 4,095 |
| C1 5,000 × 16 B, msgs/s | 3,778 | 4,404 | 4,919 |
| C1 ping RTT p50, ms | 1.4 | 2.9 | 3.1 |
| C1 ping RTT p95, ms | 3.0 | 4.3 | 16.9 |
| C1 first message, ms | 1.8 | 4.0 | 9.9 |
| C2 5,000 × 16 B (loopback HTTP, same-origin), msgs/s | 11,193 | 12,293 | 14,594 |

**Single offline run, all tests (run 1 of the first airplane launch):**

| Test | received | msgs/s | first ms | total ms |
|---|---|---|---|---|
| C1 500 × 16 B | 500, in order | 3,100 | 2.8 | 161 |
| C1 5,000 × 16 B | 5,000, in order | 4,076 | 2.8 | 1,227 |
| C1 500 × 1 KB | 500, in order | 4,075 | 2.9 | 123 |
| C2 50 pings | — | — | p50 7.6 ms | — |
| C2 5,000 × 16 B | 5,000, in order | 11,834 | 10 | 423 |

The online numbers were the same: 500 × 16 B at 2,560–3,997 msgs/s, ping p50 1.6–1.8 ms.

**Where the time goes.** `CustomTabsSession.postMessage` is a synchronous Binder call into Chrome, taking about 0.23–0.4 ms per call on this phone. The page receives messages as fast as native posts them (page total ≈ native post time + 2 ms). A llama.cpp decode on this phone runs at tens of tokens per second, so the channel has more than 100× headroom. W9 should still batch tokens, for example flushing every ~30 ms or every N tokens, to cut wakeups and Binder traffic.

## What was learned (each one is a W9 requirement)

1. **`use_as_origin` must be in m.allternit.com's `assetlinks.json`.** Today `allternit-ai/public/.well-known/assetlinks.json` lists only `handle_all_urls`. Chrome refuses the channel without `use_as_origin` (Chromium `PostMessageHandler.onOriginVerified`: `if (!result) return;`; seen on the Moto as `ports: 0`). Add `"delegate_permission/common.use_as_origin"` to the same statement, with both release fingerprints. This is an allternit-ai change, so it is a production deploy and needs Eoj's go-ahead.
2. **The first verification needs a network connection.** Offline, Chrome uses only a result it saved earlier. A fresh install opened for the first time in airplane mode gets no channel. The PWA must handle "no bridge": fall back to WebGPU for small models, or say "open once online to finish setup". Downloading a model needs a network connection anyway, so in practice the first run is online.
3. **The page sees `event.origin === 'android-app://<its own host>'`,** for example `android-app://m.allternit.com`. It is not the package name: Chrome builds `android-app://<verified host>/<package>`, and a MessageEvent origin drops the path. The page must check exactly that value. Only apps listed with `use_as_origin` for the host can produce it.
4. **Page messages sent right after the port arrives are usually dropped.** Chrome hands the page its port *before* its binding to the app's `PostMessageService` is connected. On the Moto, every run whose handshake count was logged needed 2–3 `hello`s (the launch run and reloads 1–19). Before the fix, one run failed outright. Fix, used by the spike and required for W9:
   - The page sends `hello` and repeats it every 250 ms until `welcome`.
   - Native also sends `ready` from `onMessageChannelReady`, which reaches the app through that same binding, and the page answers with `hello` at once.
   - Results after the fix: 22/22 runs passed.
5. **`onMessageChannelReady` is not proof of a working channel.** It also fired after a *failed* validation, with no port delivered. Treat `welcome` (app) and `hello` (page) as the only "connected" signals.
6. **One channel per document.** Chrome closes the channel on every cross-document navigation, reloads included. Native must call `requestPostMessageChannel(origin, origin)` again on every `NAVIGATION_FINISHED`; the spike does this. Single-page navigation inside the PWA (same document) keeps the channel.
7. **androidbrowserhelper's `LauncherActivity` cannot own the session.** Its `TwaLauncher` keeps the `CustomTabsSession` private. It also calls `finish()` right after launch and unbinds in `onDestroy`. W9 needs:
   - an app-scoped session holder, bound with the application context
   - a fresh `newSession` for each launch (relaunching on an old session only brings its old tab to the front; seen on the Moto)
   - a launcher that keeps today's behaviour: Play review prompt (`ReviewLauncherActivity`), share target, shortcuts, deep links, Custom Tab fallback, and notification delegation (`DelegationService`, unchanged)

   `LauncherActivityMetadata` is public and can configure the intent builder from the existing manifest meta-data.
8. **Posting is only allowed while the TWA is in front.** Chromium `CustomTabsConnection.postMessage` returns `RESULT_FAILURE_DISALLOWED` unless the caller is in the foreground or the session's tab is active. A foreground *service* does not count. When the user switches away, generation should keep running in the engine service and buffer tokens. When the page is back, it asks "resume from token N". This comes from reading the source; it was **not checked on the Moto**.
9. **androidx.browser 1.6 or newer** is needed for `requestPostMessageChannel(source, target, extras)`. androidbrowserhelper 2.5.0 brings 1.4.0. The spike adds `browser:1.8.0` for debug only; W9 adds it to `implementation`.
10. **Manifest:** W9 declares `androidx.browser.customtabs.PostMessageService` (exported) in `src/main`. The engine's foreground service needs a type. `specialUse` worked here, but for Play W9 should pick and justify the type (for example `specialUse` with a subtype, or `dataSync` for downloads).

## Candidate 2 (localhost HTTP / WebSocket): why it is only the fallback

- **Works offline, same-origin:** 11–15k msgs/s, ping p50 7–8 ms. Each fetch opens a new connection, so a real WebSocket or SSE would be faster.
- **Blocked on the production origin by our own CSP.** From the real m.allternit.com page, `fetch('http://127.0.0.1:8765/...')` fails: "violates the following Content Security Policy directive: connect-src 'self' https://api.allternit.com ...". The CSP comes from `allternit-ai/public/_headers`. Using candidate 2 would mean adding loopback to the production `connect-src`.
- **Local Network Access does not behave as documented.** Chrome's docs say a public https page reaching loopback needs the `loopback-network` permission and shows a prompt. chromestatus lists LNA for fetch at M142 and WebSockets at M147, on desktop and Android. On the Moto (Chrome 157), from `https://example.com` in a Custom Tab, `permissions.query({name:'loopback-network'})` returned `prompt`. Yet a fetch to `http://127.0.0.1:8765` succeeded at once, **with no prompt shown**. The state then read `granted`. A cross-origin 500-message stream ran at 5,695 msgs/s. So today it works silently, but a Chrome update could bring the prompt back. The spike cannot control that.
- **Security.** If loopback is granted silently, any website open on the phone can call the engine port, and any app can too. A production localhost bridge would need a per-launch secret, and delivering that secret needs a trusted channel, which is candidate 1 anyway.

## Found outside S1, and blocking W9's airplane-mode check

- **m.allternit.com does not reliably open offline.** On the Moto, the active service worker used cache `allternit-fabric-session-v174` (the live worker is `v187`), and that cache had **no shell entry**. The first offline launch showed Chrome's "This site can't be reached, ERR_FAILED" page; that is the worker's `Response.error()`. After two online launches the shell was cached, and the offline launch loaded.

  Cause, from `allternit-ai/public/fabric-session-service-worker.js`: the shell is only saved during an *online* navigation, and `activate` deletes every older cache, shell included. So after each deploy that bumps `CACHE_NAME`, the next offline launch fails until the user has opened the app online again. Fix it in the worker: keep the shell in its own cache that `activate` does not delete, or precache the shell at install.
- **Offline, the shell came up on the signed-out landing page** ("Your bots. Your workspace."), even though the same Chrome profile showed a signed-in chat online. For "chat reply in airplane mode", the PWA must boot to a usable signed-in (or local-only) chat without reaching Clerk or the API. This is a lane that W9 depends on and the plan does not name. These two items are in allternit-ai, outside S1's files, so they are reported here and not fixed.

## What W9 should build

1. **Contract (with C1/C2):**
   - The page-side bridge client lives at `allternit-ai/src/lib/local-models/providers/android-app.ts`.
   - Handshake: `hello` repeated until `welcome`, plus native `ready`.
   - Messages carry JSON and a protocol version.
   - Request/response ids for `hardware`, `list`, `load`, `generate`, `cancel`, `unload`.
   - Token stream frames are batched, with a sequence number for "resume from N".
   - The exact `event.origin` check.
2. **Android (`src/main`, shared by both flavors):**
   - an app-scoped `TwaSessionHolder`
   - a launcher replacing the `LauncherActivity` session path, keeping the review prompt, share target, shortcuts and deep links
   - `requestPostMessageChannel` on each `NAVIGATION_FINISHED`
   - `PostMessageService` declared in the manifest
   - androidx.browser ≥ 1.6
   - the llama.cpp engine in a foreground service that buffers tokens while the TWA is in the background
3. **allternit-ai (production deploy, needs Eoj):**
   - add `use_as_origin` to `public/.well-known/assetlinks.json`
   - fix the offline shell in the service worker
   - an offline boot path for signed-in chat
4. **Checks on the Moto:**
   - the real `com.allternit.app` Play-signed build against m.allternit.com
   - online once, then airplane mode: 500 tokens, reload, background and resume
   - the same check after a deploy, to cover the cache-version case

## Not checked

- The real m.allternit.com + `com.allternit.app` combination with `use_as_origin`. It is blocked on the assetlinks deploy. Chrome's code path was checked via mode A and the dropped-DAL run.
- Background and foreground behaviour (item 8). It is from Chromium source only.
- Messages larger than 1 KB.
- Any emulator: none exists on this Mac, and none was used.
- The `phonelink` flavor (its debug build needs the gitignored `Mesh.aar`). The bridge code is flavor-independent.

## Device state left behind

- The spike app `com.allternit.app.s1` is still installed but stopped, so port 8765 is closed. Remove it with `adb uninstall com.allternit.app.s1`.
- Airplane mode is off again, as it was at the start.
- No Chrome flags or `chrome-command-line` file were used.
- Chrome on the Moto now holds a `loopback-network` grant for `https://example.com` from the candidate-2 probe. Chrome granted it by itself. Remove it in Chrome → Site settings if wanted.
