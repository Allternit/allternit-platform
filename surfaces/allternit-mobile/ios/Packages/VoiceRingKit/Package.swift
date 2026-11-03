// swift-tools-version: 5.9
import PackageDescription

// Vendored from https://github.com/Jakubantalik/Libraries.dev
// (packages/voice-ring/ports/ios/VoiceRingKit) at commit
// 1c87c11703be85d119af35d4c9298996f2ec0ec7, MIT (LICENSE). Only the ring
// (VoiceRingKit) and its shared core (VoiceCore) are copied; the paid
// emotion kit and the direction model kit are left out. The web app pins the
// same library as npm voice-ring 0.1.2. Sources/VoiceRingKit/OrbTypes.swift
// is ours: the web's five ink types (orbit, sash, signal, swell, pixel)
// ported onto the same core. To update: copy both source folders from a
// newer commit (keep OrbTypes.swift), and change the commit above.
let package = Package(
    name: "VoiceRingKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "VoiceRingKit", targets: ["VoiceRingKit"]),
    ],
    targets: [
        .target(name: "VoiceCore", resources: [.process("Render/Refract.metal")]),
        .target(name: "VoiceRingKit", dependencies: ["VoiceCore"]),
    ]
)
