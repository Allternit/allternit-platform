import Foundation

// Swift mirror of the DeviceRuntime contract (plan section 3; TypeScript in
// allternit-ai src/lib/on-device/types.ts). W8's Models screens and the
// chat picker read these; OnDeviceRuntime publishes one per engine.
// Picker groups: On this device · My other devices · Cloud.

enum DeviceRuntimeId: String, Codable, Sendable, CaseIterable {
    case desktopEngine = "desktop-engine"
    case browserWebgpu = "browser-webgpu"
    case androidApp = "android-app"
    case iosMLX = "ios-mlx"
    case iosLlama = "ios-llama"
    case appleFM = "apple-fm"
}

enum DeviceRuntimePlace: String, Codable, Sendable {
    case thisDevice = "this-device"
    case otherDevice = "other-device"
    case cloud
}

enum DeviceRuntimeState: String, Codable, Sendable {
    case ready, missing, off, unsupported
}

/// An installed model as the shared `InstalledLocalModel` shape plus the
/// on-device fields (quant, engine, verified).
struct DeviceRuntimeModel: Codable, Sendable, Equatable, Identifiable {
    var id: String
    var providerId: String
    var runtimeModelId: String
    var name: String
    var sizeBytes: Int64?
    var digest: String?
    var quant: String?
    var engine: String?
    /// Every file matched the signed Freeweights manifest (size + sha256).
    var verified: Bool?
}

struct RunningModel: Codable, Sendable, Equatable {
    var modelId: String
    var quant: String
    var memBytes: Int64
    var tokPerSec: Double
    var ctxUsed: Int
    var ctxMax: Int
    var idleUnloadSec: Int = 300
}

struct DeviceRuntime: Codable, Sendable, Equatable, Identifiable {
    var id: DeviceRuntimeId
    var place: DeviceRuntimePlace
    var deviceName: String
    var engine: String
    var backend: String?
    var version: String?
    var state: DeviceRuntimeState
    var budget: DeviceBudget
    var offline: Bool
    var models: [DeviceRuntimeModel]
    var running: RunningModel?
    /// Why the runtime is not ready ("Needs iOS 26 or later"). Not in the TS
    /// shape; screens show it under the engine row.
    var note: String?
}
