// swift-tools-version:5.9
//
// AllternitFilesKit: the Files drive client (protocol `files/2`) shared by the
// iOS app, the AllternitFileProvider extension and, later, the macOS File
// Provider (plan step 23). Foundation and Security only, so the File Provider
// extension stays far under iOS's ~20 MB extension memory limit (spike 5).
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
