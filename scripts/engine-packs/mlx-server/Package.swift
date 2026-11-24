// swift-tools-version: 6.0
// allternit-mlx-server: the macOS MLX engine pack (mlx-metal-macos-arm64).
//
// A Mac-only OpenAI-compatible loopback server (Hummingbird 2) that
// local-engine spawns like llama-server. The engine itself is NOT here: it is
// the one shared Swift core, the iPhone app's package
// surfaces/allternit-mobile/ios/Packages/OnDeviceLLM (MLXEngine), taken by
// path so the Mac and iOS always run the same code and the same MLX pin
// (spec/on-device/engine-pack-spike.md, "MLX sidecar shape"; Eoj, 2026-11-15).
// The mlx-swift / mlx-swift-lm versions are set in that package, never here.
//
// Build with xcodebuild, not `swift build`: only xcodebuild compiles the Metal
// shaders into mlx-swift_Cmlx.bundle (build-mlx-server-macos.sh).
import PackageDescription

let package = Package(
    name: "allternit-mlx-server",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "allternit-mlx-server", targets: ["allternit-mlx-server"]),
    ],
    dependencies: [
        .package(path: "../../../surfaces/allternit-mobile/ios/Packages/OnDeviceLLM"),
        .package(url: "https://github.com/hummingbird-project/hummingbird", from: "2.28.0"),
    ],
    targets: [
        .executableTarget(
            name: "allternit-mlx-server",
            dependencies: [
                .product(name: "OnDeviceLLM", package: "OnDeviceLLM"),
                .product(name: "Hummingbird", package: "hummingbird"),
            ],
            // Top-level server globals are read from request handlers; Swift 5
            // mode keeps them as plain globals instead of strict-concurrency errors.
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
