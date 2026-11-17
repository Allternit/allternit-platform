// swift-tools-version:5.10
//
// AllternitBTHID: a Bluetooth LE keyboard and mouse (HID over GATT) built on
// public CoreBluetooth, for the Phone Use Bluetooth routes
// (docs/contracts/CONTRACT-phone-use-2026-11-16.md §1, §6):
//   - `bt-mac`:   Allternit Desktop's helper cmd/allternit-bthid (macOS 14+)
//   - `bt-apple`: a second iPhone or iPad running the Allternit app (iOS 17+,
//                 plan step 13; not wired into project.yml yet)
//
// All code here is ours, written from the Bluetooth HID over GATT Profile
// (HOGP 1.0), the HID Service spec (HIDS 1.0), the HID 1.11 report descriptor
// rules and Apple's CoreBluetooth documentation. No code from
// jqssun/darwin-bt-remote (AGPL) or sryo/clak (no licence) was opened.
//
// UNDOCUMENTED BEHAVIOUR (accepted by Eoj 2026-11-17): CoreBluetooth refuses
// the HID service under its 16-bit id 0x1812. This package registers it under
// the full Bluetooth Base UUID form 00001812-0000-1000-8000-00805F9B34FB,
// which a central reads as the same service. Apple may stop allowing that in
// any update; HIDPeripheral then reports `refused` and the route says so.
//
// Targets:
//   BTHIDCore       pure encoding (report map, reports, US key table, GATT
//                   layout, read offsets); no CoreBluetooth; unit-tested
//   BTHIDObjC       one function that turns an Objective-C exception into a
//                   value, so a refused descriptor is an error, not a crash
//   AllternitBTHID  the CBPeripheralManager wrapper (advertise, subscribers,
//                   flow-controlled sends)
//
// Tests: `swift test --package-path surfaces/allternit-mobile/ios/Packages/AllternitBTHID`
//
// Swift 5 language mode on purpose: CoreBluetooth is a delegate-callback API;
// the wrapper serialises everything on one queue. Public types are Sendable.
import PackageDescription

let package = Package(
    name: "AllternitBTHID",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "AllternitBTHID", targets: ["AllternitBTHID"]),
        .library(name: "BTHIDCore", targets: ["BTHIDCore"]),
    ],
    targets: [
        .target(name: "BTHIDCore", path: "Sources/BTHIDCore"),
        .target(name: "BTHIDObjC", path: "Sources/BTHIDObjC"),
        .target(
            name: "AllternitBTHID",
            dependencies: ["BTHIDCore", "BTHIDObjC"],
            path: "Sources/AllternitBTHID",
            linkerSettings: [.linkedFramework("CoreBluetooth")]
        ),
        .testTarget(name: "BTHIDCoreTests", dependencies: ["BTHIDCore"], path: "Tests/BTHIDCoreTests"),
    ]
)
