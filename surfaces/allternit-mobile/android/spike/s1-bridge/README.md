# Spike S1: Android bridge (debug builds only)

This spike tests whether the m.allternit.com page inside the Android app (a Trusted Web Activity) can stream messages with native code in the same app. It must work offline, with no permission prompt, and keep working after a page reload. The verdict and the numbers are in `spec/on-device/android-bridge-spike.md` at the repo root.

This folder is on the scratch branch `ondevice/s1-android-bridge` only. Do not merge it to main. W9 builds the production version.

## What is wired where

`app/build.gradle.kts` adds this folder to the **debug** source set only (manifest, `src/`, `web/`). It also sets `debugImplementation("androidx.browser:browser:1.8.0")` and `applicationIdSuffix = ".s1"`. So `playDebug` (and `phonelinkDebug`) install as `com.allternit.app.s1`, next to the Play app. Release builds do not change: the `playRelease` manifest, the dex class list and the APK file list were diffed before and after, and they are identical.

| File | What it does |
|---|---|
| `AndroidManifest.xml` | Adds the "S1 bridge" launcher, `androidx.browser.customtabs.PostMessageService`, the foreground service and an adb-only receiver (guarded by `android.permission.DUMP`) |
| `src/.../S1Bridge.kt` | Candidate 1. Holds one app-scoped CustomTabs binding and creates a new session for each launch. Requests the postMessage channel on every `NAVIGATION_FINISHED`. Implements the wire protocol and the streaming loop |
| `src/.../S1BridgeActivity.kt` | Starts the service, opens the TWA on our own session, then finishes |
| `src/.../S1Service.kt` | Foreground service (`specialUse`). Runs the loopback server and builds this APK's `assetlinks.json` (package plus signing-cert SHA-256, `handle_all_urls` + `use_as_origin`) |
| `src/.../S1HttpServer.kt` | Loopback-only HTTP server on 127.0.0.1 and ::1, port 8765. Serves the test page and `/.well-known/assetlinks.json`, plus the candidate-2 endpoints `/c2/ping` and `/c2/stream` |
| `src/.../S1CommandReceiver.kt` | adb controls: `REQUEST_CHANNEL`, `STATUS`, `DAL_DROP` |
| `web/s1/s1.html`, `web/s1/s1-client.js` | The test page and the page-side client. The client can also be pasted into the real m.allternit.com page |

## Build and install

Use JDK 17 and the Homebrew Android SDK that is already on this Mac. Nothing new was installed.

```sh
cd surfaces/allternit-mobile/android
export JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
./gradlew :app:assemblePlayDebug
adb install -r app/build/outputs/apk/play/debug/app-play-debug.apk
```

`phonelinkDebug` needs the gitignored `app/libs/Mesh.aar` (see the app README). This is not related to the spike.

## Run the test (mode A, fully offline)

```sh
adb shell cmd connectivity airplane-mode enable
adb shell am force-stop com.android.chrome            # optional: cold Chrome
adb logcat -c
adb shell am start -n com.allternit.app.s1/com.allternit.app.spike.s1.S1BridgeActivity
sleep 20
adb logcat -d 'S1Bridge:*' '*:S' | grep -E 'validation|HELLO|STREAM|reloadTest'
adb shell cmd connectivity airplane-mode disable
```

The app serves `http://localhost:8765/s1.html?launch=<n>` from its own APK assets and opens it as a TWA. The page receives the port, does the hello/welcome handshake, and runs these checks:

- 100 pings
- 500 × 16 B, 5,000 × 16 B and 500 × 1 KB over the TWA port
- the same transfers over loopback HTTP (candidate 2, same-origin)

It sends its results to native, which logs them as `RESULT {...}`. It then reloads itself once and runs again, and logs `reloadTest: PASS|FAIL`. To test more reloads, reload from DevTools (`adb forward tcp:9333 localabstract:chrome_devtools_remote`, then `Page.reload` or `location.reload()`).

The **saved-verification** path is the one production uses offline. To test it:

1. Run mode A once.
2. Run `adb shell am broadcast -a com.allternit.app.s1.DAL_DROP --ez on true -n com.allternit.app.s1/com.allternit.app.spike.s1.S1CommandReceiver`. From then on, the server resets the connection on every `assetlinks.json` request, which is the error Chrome gets offline.
3. Force-stop Chrome and launch again. Chrome logs `Device is offline, checking saved verification result.` and the channel still opens.

## Mode B (the real origin)

```sh
adb shell am start -n com.allternit.app.s1/com.allternit.app.spike.s1.S1BridgeActivity \
  --es url https://m.allternit.com/ --ez auto false
```

Paste `web/s1/s1-client.js` into the page from chrome://inspect, with `window.S1_NO_AUTORELOAD=true` first. Then send `com.allternit.app.s1.REQUEST_CHANNEL`. Today validation fails and no port is delivered (`S1.state()` shows `ports: 0`), because m.allternit.com's `assetlinks.json` has no `use_as_origin` relation. That relation is the W9 prerequisite. `S1.c2probe()` tries the cross-origin loopback fetch, which m.allternit.com's CSP `connect-src` blocks today.

## Clean up

```sh
adb shell am force-stop com.allternit.app.s1     # stops the service and port 8765
adb uninstall com.allternit.app.s1               # removes the spike app; the Play app is untouched
```
