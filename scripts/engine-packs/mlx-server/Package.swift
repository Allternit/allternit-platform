// swift-tools-version: 5.12
// allternit-mlx-server: the macOS MLX engine pack (mlx-metal-macos-arm64).
//
// Two targets, split so iOS can adopt the engine without the HTTP server
// (spec/on-device/engine-pack-spike.md, "MLX sidecar shape"):
//   OnDeviceLLM           engine core (load a local MLX model dir, stream a chat
//                         reply with token stats). No networking. W7 moves this
//                         target into surfaces/allternit-mobile/ios/Packages/OnDeviceLLM.
//   allternit-mlx-server  Mac-only OpenAI-compatible loopback server (Hummingbird 2)
//                         that local-engine spawns like llama-server.
//
// Versions: mlx-swift 0.31.3 + mlx-swift-lm 2.31.3 (Eoj's pin, 2026-11-15), the
// pair iOS uses too. The canonical OnDeviceLLM core is W7's iOS package
// (surfaces/allternit-mobile/ios/Packages/OnDeviceLLM); this copy follows it and
// is replaced by a dependency on it before the MLX pack ships.
// Build with xcodebuild, not `swift build`:
// only xcodebuild compiles the Metal shaders into mlx-swift_Cmlx.bundle.
import PackageDescription

let package = Package(
    name: "allternit-mlx-server",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "OnDeviceLLM", targets: ["OnDeviceLLM"]),
        .executable(name: "allternit-mlx-server", targets: ["allternit-mlx-server"]),
    ],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift", exact: "0.31.3"),
        .package(url: "https://github.com/ml-explore/mlx-swift-lm", exact: "2.31.3"),
        .package(url: "https://github.com/hummingbird-project/hummingbird", from: "2.28.0"),
    ],
    targets: [
        .target(
            name: "OnDeviceLLM",
            dependencies: [
                .product(name: "MLX", package: "mlx-swift"),
                .product(name: "MLXLLM", package: "mlx-swift-lm"),
                .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
            ]
        ),
        .executableTarget(
            name: "allternit-mlx-server",
            dependencies: [
                "OnDeviceLLM",
                .product(name: "Hummingbird", package: "hummingbird"),
            ]
        ),
    ]
)
