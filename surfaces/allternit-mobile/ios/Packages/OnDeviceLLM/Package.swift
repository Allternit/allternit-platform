// swift-tools-version: 6.0
import PackageDescription

// On-device language models, shared by the iPhone app (Core/OnDevice) and,
// later, the Mac MLX sidecar `allternit-mlx-server` (engine pack
// mlx-metal-macos-arm64, spec/on-device/engine-pack-spike.md). Nothing here
// is iOS-only: no UIKit, no app lifecycle, no network session. Those live in
// the app (Core/OnDevice) so the Mac sidecar can wrap the same engines in its
// own HTTP server.
//
// Targets:
//   FreeweightsVerify  minisign (Ed25519 + BLAKE2b) manifest signatures and
//                      sha256 file checks. CryptoKit only, no MLX.
//   OnDeviceLLM        the engine protocol plus the MLX engine and the Apple
//                      Foundation Models engine (iOS/macOS 26+, checked at run time).
// The llama.cpp (GGUF) engine is app-side (Core/OnDevice/LlamaEngine.swift)
// because the Mac runs GGUF through the signed llama-server engine pack.
//
// One mlx-swift version for the whole app: KokoroKit (TTS) pins the same
// exact 0.31.3. mlx-swift-lm 2.31.3 (the first release with Qwen3.5,
// `qwen3_5`) requires 0.31.3 (upToNextMinor). Bump both packages together,
// never one.
let package = Package(
    name: "OnDeviceLLM",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "OnDeviceLLM", targets: ["OnDeviceLLM", "FreeweightsVerify"]),
    ],
    dependencies: [
        // MIT. Exact, shared with Packages/KokoroKit.
        .package(url: "https://github.com/ml-explore/mlx-swift", exact: "0.31.3"),
        // MIT. Exact. The Mac MLX sidecar depends on this package by path,
        // so it runs the same pin.
        .package(url: "https://github.com/ml-explore/mlx-swift-lm", exact: "2.31.3"),
    ],
    targets: [
        .target(name: "FreeweightsVerify"),
        .target(name: "OnDeviceLLM", dependencies: [
            .product(name: "MLX", package: "mlx-swift"),
            .product(name: "MLXLLM", package: "mlx-swift-lm"),
            .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
        ]),
        .testTarget(name: "FreeweightsVerifyTests", dependencies: ["FreeweightsVerify"],
                    resources: [.copy("Fixtures")]),
    ]
)
