// swift-tools-version: 6.0
import PackageDescription

// On-device Kokoro speech for replies, with no espeak-ng (GPL) anywhere.
// Vendored, not fetched, because upstream declares iOS 18 while the app
// targets iOS 17 (the app gates Kokoro on iOS 18 at runtime):
//   KokoroSwift     https://github.com/mlalma/kokoro-ios      tag 1.0.9
//                   87602d0738306758921df1aeef55785b091104e7  MIT
//   MisakiSwift     https://github.com/mlalma/MisakiSwift     tag 1.0.6
//                   6835a1ce4a8854075c89f18ff75c74b13ef58e15  Apache-2.0
//   MLXUtilsLibrary https://github.com/mlalma/MLXUtilsLibrary tag 0.0.6
//                   41f6cfd5d68b65aa3c65a34efe3b71c371ed915b  Apache-2.0
// Licences in LICENSES/. Allternit patches (keep them on update):
//   - MisakiSwift: lexicons + BART weights load from
//     `MisakiResources.directory` (downloaded voice pack), not the bundle.
//   - KokoroSwift: WeightLoader upcasts fp16 weights to fp32.
//   - KokoroSwift: eSpeakNGG2PProcessor.swift removed.
//   - MLXUtilsLibrary: NpyzReader removed (voices ship as safetensors), so
//     no ZIPFoundation.
// To update: copy the three Sources folders from newer tags, reapply the
// patches, and change the commits above.
let package = Package(
    name: "KokoroKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "KokoroSwift", targets: ["KokoroSwift"]),
    ],
    dependencies: [
        // MIT. Same exact pin upstream kokoro-ios 1.0.9 resolves.
        .package(url: "https://github.com/ml-explore/mlx-swift", exact: "0.30.2"),
    ],
    targets: [
        .target(name: "MLXUtilsLibrary", dependencies: [.product(name: "MLX", package: "mlx-swift")]),
        .target(name: "MisakiSwift", dependencies: [
            .product(name: "MLX", package: "mlx-swift"),
            .product(name: "MLXNN", package: "mlx-swift"),
            "MLXUtilsLibrary",
        ]),
        .target(name: "KokoroSwift", dependencies: [
            .product(name: "MLX", package: "mlx-swift"),
            .product(name: "MLXNN", package: "mlx-swift"),
            .product(name: "MLXRandom", package: "mlx-swift"),
            .product(name: "MLXFFT", package: "mlx-swift"),
            "MisakiSwift",
            "MLXUtilsLibrary",
        ], resources: [.copy("Resources")]),
    ]
)
