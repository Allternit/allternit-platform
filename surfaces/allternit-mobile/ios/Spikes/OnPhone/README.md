# S0: on-phone spike (development only)

Plan: `phone-and-messages` Wave 3, step S0. This is a separate XcodeGen project
with its own bundle ids. It is not a target of the App Store app and is never
submitted.

| Part | Bundle id | API |
|---|---|---|
| Harness app | `com.allternit.mobile.onphone-spike` | SwiftUI, ScreenCaptureKit (iOS 27), Vision, NetworkExtension |
| Loopback packet tunnel | `.tunnel` | `NEPacketTunnelProvider` |
| Keyboard (typing) | `.keys` | `UIInputViewController` |

No XCTest, no private API symbols. Team MQ85968PQP, automatic signing.

## What each part does

- **Tunnel and pairing.** The tunnel routes only `10.7.0.1/32` and writes every
  IPv4 packet back with source and destination swapped, so the app reaches this
  phone's own lockdownd on port 62078. "Probe lockdown" times 20 `QueryType`
  round trips. "Check pairing" sends `StartSession` with an imported pair record
  (made once on a computer). Lockdownd answers only for a host it paired with.
  The TLS upgrade after `StartSession` is not built, and the iOS 17+ developer
  services behind it (CoreDeviceProxy, RemoteXPC, DTX) are not built either.
  Lockdownd's wire protocol is not documented by Apple. The spike links no
  private symbols, but this route does depend on an undocumented protocol.
- **Screen capture and read.** iOS 27 marks the ReplayKit broadcast extension
  "No longer supported" (SDK deprecation text), so the spike uses ScreenCaptureKit:
  the person picks Display in the system picker. Each kept frame (2 per second)
  is timed as a JPEG encode (the on-phone screenshot). During a 20-second read
  window, Vision text recognition runs on each frame (the on-phone screen read).
  The report counts frames that arrived while another app was in front, because
  iOS may stop sending frames once this app is not active.
- **Taps: blocked.** No public API taps inside another app. HID injection exists
  only in testmanagerd (XCTest). The developer accessibility service's press
  action works only on processes with `task_for_pid-allow`
  (pymobiledevice3 `accessibilityaudit.py` `perform_press`), and Apple does not
  grant that entitlement to third parties. Apps cannot advertise the Bluetooth
  HID service. Not verified on a device.
- **Typing.** A custom keyboard types a queued 200-character job. The job exists
  only after the runtime's phone guard (`POST /api/v1/phones/guard/check`, tool
  `phone.type`) answers `allow`. Anything else blocks the job (fail closed). The
  self-test field compares the text exactly. In Notes the keyboard checks the
  text before the cursor.
- **Lock, restart, other VPN, battery.** The tunnel logs start, stop (with the
  reason), sleep, wake and a beat every 5 seconds. The person marks each event
  in the app, and the report shows heartbeat gaps and stop reasons.
  On-demand reconnect is on. Battery is measured over a soak the person starts
  and ends.

## Build

```sh
xcodegen generate --spec project.yml
xcodebuild -project OnPhoneSpike.xcodeproj -scheme OnPhoneSpike \
  -destination 'id=<udid>' -allowProvisioningUpdates build
xcrun devicectl device install app --device <udid> <DerivedData>/Build/Products/Debug-iphoneos/OnPhoneSpike.app
```

The Simulator SDK has no ScreenCaptureKit. The app builds there with capture
stubbed out. The tunnel needs a device.

## Run on the phone (needs a person)

1. Unlock the phone. Open OnPhone Spike. The launch argument `-autoHarness` runs
   the steps that need no person and writes a report.
2. Fill in the runtime URL, token and phone serial (for the guard).
3. Tap "Start tunnel" and allow the VPN configuration (passcode). Then tap "Probe lockdown x20".
4. Import the pair record (`~/Library/Lockdown`-style plist exported from the Mac
   for this UDID, sent with AirDrop to Files). Tap "Check pairing".
5. Tap "Start screen capture" and pick Display. Tap "Measure read in" for
   each of Settings, Messages, Safari, Notes, and the store app (set its URL scheme).
   Each one opens the app. Stay in the app for 20 seconds.
6. Settings > General > Keyboard > Keyboards > Add New Keyboard > OnPhone Keys,
   then Allow Full Access. Tap "Type here", switch to OnPhone Keys, and repeat for Notes.
7. Lock for 2 minutes, restart, and turn another VPN on and off. Mark each one in the app.
8. Start the battery soak, wait 30 minutes, end it, and tap "Write report".

## Collect and compare

```sh
APP=Settings RUNTIME_URL=... RUNTIME_TOKEN=... SERIAL=<udid> Scripts/helper-baseline.sh > helper-settings.json
Scripts/collect-and-compare.py --udid <udid> --out out/ --helper helper-*.json
```

`s0-result.json` and `s0-result.md` apply the pass bar: 95% of taps land,
median read at most 1.5 times the helper, typing exact. While taps are
blocked, the verdict is `fail`.
