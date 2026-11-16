import Foundation

// NSItemProvider is safe to load from any thread (Apple documents loading
// item representations off the main thread), but the SDK does not mark it
// Sendable. Files hands providers from the main actor to the nonisolated
// staging helpers (share sheet, drag and drop onto a computer), which only
// read them. Compiled into the app and the share extension (Core/Files/Shared).
extension NSItemProvider: @retroactive @unchecked Sendable {}
