// swift-tools-version:5.7
import PackageDescription

let package = Package(
    name: "ax-bridge",
    platforms: [.macOS(.v12)],
    targets: [
        .executableTarget(name: "ax-bridge", path: "Sources/ax-bridge")
    ]
)
