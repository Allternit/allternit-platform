// swift-tools-version:5.10
//
// allternit-bthid: the `bt-mac` Phone Use route's hands. This Mac shows up as
// a Bluetooth keyboard and mouse called "Allternit Keyboard"; the person
// pairs it in their iPhone's Settings > Bluetooth. See README.md.
//
// The Bluetooth code is the shared package AllternitBTHID
// (surfaces/allternit-mobile/ios/Packages/AllternitBTHID), which the iPhone
// and iPad app uses too (plan step 13). Public Apple frameworks only.

import Foundation
import PackageDescription

/// Info.plist with the Bluetooth usage string, linked into the binary's __TEXT,__info_plist
/// section so a standalone run (from Terminal, in development) can ask for Bluetooth.
/// Inside Allternit Desktop, macOS asks on behalf of the app, whose own Info.plist carries
/// the same string (package.json build.mac.extendInfo).
let infoPlist = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Sources/allternit-bthid/Info.plist").path

let package = Package(
    name: "allternit-bthid",
    platforms: [.macOS("14.0")],
    products: [
        .executable(name: "allternit-bthid", targets: ["allternit-bthid"]),
    ],
    dependencies: [
        .package(path: "../../surfaces/allternit-mobile/ios/Packages/AllternitBTHID"),
    ],
    targets: [
        // Pure logic (the JSON protocol, time limits, state → JSON): unit-tested, no CoreBluetooth.
        .target(
            name: "BTHIDTool",
            dependencies: [.product(name: "BTHIDCore", package: "AllternitBTHID")],
            path: "Sources/BTHIDTool"
        ),
        .executableTarget(
            name: "allternit-bthid",
            dependencies: ["BTHIDTool", .product(name: "AllternitBTHID", package: "AllternitBTHID")],
            path: "Sources/allternit-bthid",
            exclude: ["Info.plist"],
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__info_plist", "-Xlinker", infoPlist]),
            ]
        ),
        .testTarget(name: "BTHIDToolTests", dependencies: ["BTHIDTool"], path: "Tests/BTHIDToolTests"),
    ]
)
