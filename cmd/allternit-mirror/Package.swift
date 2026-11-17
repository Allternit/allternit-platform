// swift-tools-version:5.10
//
// allternit-mirror: the `mirror` Phone Use route's eyes and hands on a Mac.
// See README.md. Public Apple frameworks only; all code here is ours.

import PackageDescription

let package = Package(
    name: "allternit-mirror",
    platforms: [.macOS("15.0")],
    products: [
        .executable(name: "allternit-mirror", targets: ["allternit-mirror"]),
    ],
    targets: [
        // Pure logic (protocol, geometry, health, keys): no AppKit, unit-tested.
        .target(name: "MirrorCore", path: "Sources/MirrorCore"),
        // The system side: window lookup, ScreenCaptureKit, Vision, CGEvent.
        .executableTarget(
            name: "allternit-mirror",
            dependencies: ["MirrorCore"],
            path: "Sources/allternit-mirror",
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("Vision"),
            ]
        ),
        .testTarget(name: "MirrorCoreTests", dependencies: ["MirrorCore"], path: "Tests/MirrorCoreTests"),
    ]
)
