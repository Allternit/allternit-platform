# allternit-bthid

The `bt-mac` Phone Use route on a Mac
(`docs/contracts/CONTRACT-phone-use-2026-11-16.md`, route 2b). This Mac shows up
as a Bluetooth keyboard and mouse called "Allternit Keyboard". The person pairs
it in their own iPhone's Settings > Bluetooth and turns on AssistiveTouch, so the
mouse becomes an on-screen pointer. Allternit Desktop then moves the pointer,
clicks and types on that iPhone, and watches the result through the iPhone's
Allternit screen broadcast. Every send has already passed the approval step.

Desktop runs this tool from `Contents/Resources/bin/allternit-bthid` and talks
to it in `surfaces/allternit-desktop/src/main/phone-bt-mac.ts`. It speaks
newline-delimited JSON on stdin and stdout; the protocol is in
`Sources/BTHIDTool/Protocol.swift`. Keep both sides in sync.

The Bluetooth code is the shared package
`surfaces/allternit-mobile/ios/Packages/AllternitBTHID` (macOS 14+, iOS 17+),
which the iPhone and iPad app will use for the `bt-apple` route.

## How it works

- Bluetooth LE HID over GATT (HOGP): a HID service with HID Information,
  Report Map, Protocol Mode, Control Point and three Report characteristics
  (keyboard input, mouse input, keyboard LED output), each with its Report
  Reference descriptor; plus Device Information (PnP ID) and Battery.
- Report 1 is a boot-style keyboard, report 2 a relative mouse with three
  buttons and X/Y/wheel at -127..127. The report map is byte for byte the
  Android sender's (`bt-android`), so Desktop treats both the same.
- The Report Map and reports need an encrypted link, so the iPhone bonds.
- Reports go out with flow control: when CoreBluetooth's transmit queue is
  full, the tool waits for `peripheralManagerIsReady(toUpdateSubscribers:)`.

## Undocumented behaviour

CoreBluetooth refuses the HID service under its 16-bit id `0x1812`. Published
Mac App Store apps (Typeeto, KeyPad, BT Keys, BT Remote) register it under the
full 128-bit Bluetooth Base UUID form, `00001812-0000-1000-8000-00805F9B34FB`,
which iOS reads as the standard HID service. Apple does not document this, and
an Apple update may stop it. Eoj accepted that on 2026-11-17. If macOS refuses
the service, `state` reports `refused: true` and Desktop shows the route as
"not available: macOS refused the keyboard service; Apple may have changed this".

All code here is ours, written from the Bluetooth HOGP/HIDS specifications and
Apple's CoreBluetooth documentation. No code from `jqssun/darwin-bt-remote`
(AGPL-3.0) or `sryo/clak` (no licence) was opened or copied.

## Permissions

Bluetooth. Inside Allternit Desktop, macOS asks on behalf of the app, whose
Info.plist carries `NSBluetoothAlwaysUsageDescription`. The binary also embeds
its own Info.plist (`__TEXT,__info_plist`) with the same string, for runs
from Terminal during development. `state` never asks; `pair` asks the first time.

## Limits

- Every command has a time limit. All waits inside are bounded below it;
  past it the tool answers `timeout` and exits, and Desktop starts it again
  and pairs again.
- The tool exits when stdin closes. It lets go of every key and button and
  removes the keyboard first.
- A Bluetooth peripheral is not told its host's name, so `state.name` is null.
  The iPhone may list the Mac under the Mac's own name rather than
  "Allternit Keyboard" once connected (not checked).
- Typing is printable ASCII on the iPhone's US hardware-keyboard layout.

## Not checked on hardware

Plan check P2: pairing, the AssistiveTouch pointer, typing, reconnects after
sleep. Run only on your own Mac and iPhone.

## Build and test

```sh
swift test --package-path surfaces/allternit-mobile/ios/Packages/AllternitBTHID
swift test --package-path cmd/allternit-bthid
swift build -c release --arch arm64 --arch x86_64 --package-path cmd/allternit-bthid
```
