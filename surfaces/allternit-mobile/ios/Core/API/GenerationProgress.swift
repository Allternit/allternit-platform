import Foundation

/// Live progress of one picture or motion generation: the single shape every
/// surface mirrors (gizzi `PaneRender.ProgressDetail`, platform #2802; web
/// `src/lib/media/progress.ts`). The phone sends it while it renders a
/// pane_render request (`POST /pane-render/:id/progress`) and reads it back
/// from a running `media_generate` call's `metadata.progress_detail`.
enum GenerationLane: String, Codable, Sendable { case picture, motion }

enum GenerationStage: String, Codable, Sendable {
    case queued, drawing, rendering, encoding, saving, downloading
}

struct GenerationProgressDetail: Codable, Equatable, Sendable {
    var lane: GenerationLane
    var stage: GenerationStage
    /// What is happening, in plain words ("Frame 12 of 180").
    var label: String
    /// Motion: the frame being drawn (1-based) and the total.
    var frame: Int? = nil
    var frames: Int? = nil
    /// 0 to 1, of the current stage.
    var fraction: Double? = nil
    /// A thumbnail (data:image URL, at most 320 px wide) or an API path.
    var preview: String? = nil
    /// Downloading: real bytes so far and in all, and seconds left.
    var bytes: Int? = nil
    var totalBytes: Int? = nil
    var etaSeconds: Double? = nil

    private enum CodingKeys: String, CodingKey {
        case lane, stage, label, frame, frames, fraction, preview, bytes
        case totalBytes = "total_bytes"
        case etaSeconds = "eta_s"
    }

    /// `metadata.progress_detail` from a tool part's metadata JSON text; nil
    /// when absent or malformed (a bad lane or stage is dropped, never guessed).
    static func read(metadata: String?) -> GenerationProgressDetail? {
        guard let metadata, let data = metadata.data(using: .utf8),
              let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let detail = root["progress_detail"] as? [String: Any],
              let lane = (detail["lane"] as? String).flatMap(GenerationLane.init(rawValue:)),
              let stage = (detail["stage"] as? String).flatMap(GenerationStage.init(rawValue:)) else { return nil }
        func int(_ key: String) -> Int? { (detail[key] as? NSNumber).map { $0.intValue } }
        func dbl(_ key: String) -> Double? { (detail[key] as? NSNumber).map { $0.doubleValue } }
        let preview = (detail["preview"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        return GenerationProgressDetail(
            lane: lane, stage: stage, label: (detail["label"] as? String) ?? "",
            frame: int("frame"), frames: int("frames"), fraction: dbl("fraction"),
            preview: preview, bytes: int("bytes"), totalBytes: int("total_bytes"), etaSeconds: dbl("eta_s")
        )
    }

    // MARK: - What the card says (same words and rules as the web card)

    static let openingLabel: [GenerationLane: String] = [.picture: "Generating image", .motion: "Rendering video"]

    static func stageWords(_ stage: GenerationStage) -> String {
        switch stage {
        case .queued: return "Waiting in line"
        case .drawing: return "Drawing"
        case .rendering: return "Rendering frames"
        case .encoding: return "Encoding the video"
        case .saving: return "Saving"
        case .downloading: return "Downloading"
        }
    }

    /// The words for the stage, from the runtime's label when it sent one.
    static func stageLabel(_ detail: GenerationProgressDetail?, lane: GenerationLane) -> String {
        guard let detail else { return openingLabel[lane] ?? "Generating" }
        let label = detail.label.trimmingCharacters(in: .whitespacesAndNewlines)
        return label.isEmpty ? stageWords(detail.stage) : label
    }

    /// The main line: "Frame N of M" while motion frames arrive, else the stage label.
    static func mainLine(_ detail: GenerationProgressDetail?, lane: GenerationLane) -> String {
        if lane == .motion, let d = detail, d.stage == .rendering, let frames = d.frames, let frame = d.frame {
            return "Frame \(min(frame, frames)) of \(frames)"
        }
        return stageLabel(detail, lane: lane)
    }

    /// 0...1 for the bar; nil when nothing is known (no bar).
    static func barFraction(_ detail: GenerationProgressDetail?) -> Double? {
        guard let d = detail else { return nil }
        if let f = d.fraction { return max(0, min(1, f)) }
        if d.stage == .rendering, let frames = d.frames, frames > 0, let frame = d.frame {
            return max(0, min(1, Double(frame) / Double(frames)))
        }
        return nil
    }

    /// Which motion chip is active: -1 before frames start, 0 render, 1 encode, 2 save.
    static func motionStageIndex(_ stage: GenerationStage?) -> Int {
        switch stage {
        case .rendering: return 0
        case .encoding: return 1
        case .saving, .downloading: return 2
        default: return -1
        }
    }

    /// Picture rows of the history card: 0 queued, 1 drawing, 2 refining, 3 saving.
    static func pictureStepIndex(_ stage: GenerationStage?) -> Int {
        switch stage {
        case .none, .queued: return 0
        case .drawing: return 1
        case .rendering: return 2
        default: return 3
        }
    }

    /// Blur (points) of a picture preview: heavy early, none once it is saving.
    var previewBlur: Double {
        if stage == .saving || stage == .downloading { return 0 }
        let f = max(0, min(1, fraction ?? 0.35))
        return (24 * (1 - f)).rounded()
    }

    /// A real picture to show instead of the pulse.
    var hasPreview: Bool { preview != nil && stage != .queued && stage != .drawing }
}

/// Sends progress about once a second, but a new stage always goes at once
/// (the card must not sit on "Drawing" while the clip is already encoding).
/// Same rule as gizzi's `progressForwarder`.
struct GenerationProgressThrottle: Sendable {
    var interval: TimeInterval = 1
    private var lastStage: GenerationStage?
    private var lastAt: TimeInterval?

    init(interval: TimeInterval = 1) { self.interval = interval }

    /// True when a report made at `now` (seconds, any clock) should be sent.
    mutating func shouldSend(stage: GenerationStage, now: TimeInterval) -> Bool {
        if stage == lastStage, let lastAt, now - lastAt < interval { return false }
        lastStage = stage
        lastAt = now
        return true
    }
}

/// Live progress of media generations, keyed by the `media_generate` tool call
/// id. The chat list recycles cards of older turns, so what a card shows
/// (stage, start time, the last motion thumbnails) lives here and survives a
/// remount. The press-and-hold peek and the running strip read the same store.
@MainActor
final class GenerationProgressStore: ObservableObject {
    static let shared = GenerationProgressStore()

    struct Entry: Equatable {
        var detail: GenerationProgressDetail
        var startedAt: Date
        /// Motion only: the last distinct previews, oldest first.
        var thumbs: [String]
    }

    /// Motion keeps this many thumbnails (the history card shows them).
    static let maxThumbs = 4
    /// Entries kept at once; the oldest goes first.
    static let maxEntries = 24

    @Published private(set) var entries: [String: Entry] = [:]
    /// Oldest first.
    @Published private(set) var order: [String] = []

    func entry(_ id: String) -> Entry? { entries[id] }

    func set(_ id: String, detail: GenerationProgressDetail, now: Date = Date()) {
        let prev = entries[id]
        var thumbs = prev?.thumbs ?? []
        if detail.lane == .motion, let preview = detail.preview, thumbs.last != preview {
            thumbs = Array((thumbs + [preview]).suffix(Self.maxThumbs))
        }
        let next = Entry(detail: detail, startedAt: prev?.startedAt ?? now, thumbs: thumbs)
        if prev == next { return }
        entries[id] = next
        if !order.contains(id) { order.append(id) }
        while order.count > Self.maxEntries { entries[order.removeFirst()] = nil }
    }

    func clear(_ id: String) {
        guard entries[id] != nil else { return }
        entries[id] = nil
        order.removeAll { $0 == id }
    }

    /// Keeps the store in step with one message's tool calls: a running
    /// `media_generate` call of a live reply with a report is recorded, any
    /// other call is cleared (done, failed, stopped, or the reply ended).
    func sync(calls: [ToolCallRecord], replyIsLive: Bool) {
        for call in calls where Self.isMediaGenerate(call.toolName) {
            if replyIsLive, call.state == .running {
                if let detail = GenerationProgressDetail.read(metadata: call.metadata) { set(call.id, detail: detail) }
            } else {
                clear(call.id)
            }
        }
    }

    nonisolated static func isMediaGenerate(_ toolName: String) -> Bool {
        ToolDescriber.normalized(toolName) == "mediagenerate"
    }
}
