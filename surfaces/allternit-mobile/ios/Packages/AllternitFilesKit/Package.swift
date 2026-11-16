// swift-tools-version:5.9
//
// AllternitFilesKit: the Files drive client (protocol `files/2`) shared by the
// iOS app, the AllternitFileProvider extension and the macOS File Provider
// in Allternit Desktop (plan step 23, surfaces/allternit-desktop/native/
// file-provider/build.sh compiles these same sources). Foundation, Security and FileProvider only (no
// UIKit, no mesh), so the File Provider extension stays far under iOS's
// ~20 MB extension memory limit (spike 5) and the same code builds for macOS.
// Platform differences stay in the targets that link it.
//
// Tests: `swift test --package-path surfaces/allternit-mobile/ios/Packages/AllternitFilesKit`
// (macOS host). ChunkerVectorTests reads contracts/files-drive/vectors/chunker-v1.json.
//
// The message types are the generated contract
// (contracts/files-drive/gen/FilesDriveContract.gen.swift). Sources/.../Contract
// holds a symlink to that file, never a copy: regenerate the contract and this
// package picks it up.
//
// Swift 5 language mode on purpose: the File Provider protocols are
// completion-handler APIs that Swift 6 strict concurrency can't check yet.
// Public types are Sendable so the Swift 6 app can use them.
import PackageDescription

let package = Package(
    name: "AllternitFilesKit",
    platforms: [.iOS(.v17), .macOS(.v13)],
    products: [
        .library(name: "AllternitFilesKit", targets: ["AllternitFilesKit"]),
    ],
    targets: [
        .target(
            name: "AllternitFilesKit",
            path: "Sources/AllternitFilesKit"
        ),
        .testTarget(
            name: "AllternitFilesKitTests",
            dependencies: ["AllternitFilesKit"],
            path: "Tests/AllternitFilesKitTests"
        ),
    ]
)
