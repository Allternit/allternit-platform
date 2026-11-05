import Foundation

/// A session agent's request for the app to render something (gizzi
/// `PaneRender.Request`; web pane-render-bridge.ts): a `media_generate`
/// image or video drawn from code, or an `artifact_check` of an artifact at
/// each width. The phone answers it the way the web app does.
struct PaneRenderRequest: Sendable {
    enum Kind: String, Sendable { case image, video, check }

    let requestId: String
    let sessionId: String
    let kind: Kind
    /// svg, html or canvas (image/video); html for a check.
    let format: String
    /// The drawing code, or the artifact's body for a check.
    let code: String
    let width: Int
    let height: Int
    let title: String
    // Video
    let duration: Double?
    let fps: Int?
    let motionBlur: Int?
    let audio: String?
    // Check
    let artifactKind: String?
    let bodyFormat: String?
    let widths: [Int]
    let reference: String?
    /// When the agent asked (ms), and how long it waits (ms).
    let time: Double?
    let timeoutMs: Double?

    /// Parses the sync event's JSON. Nil when a field the render needs is missing.
    init?(payload: String) {
        guard let obj = (try? JSONSerialization.jsonObject(with: Data(payload.utf8))) as? [String: Any] else { return nil }
        func str(_ keys: String...) -> String? {
            for k in keys { if let s = obj[k] as? String, !s.isEmpty { return s } }
            return nil
        }
        func num(_ keys: String...) -> Double? {
            for k in keys { if let n = obj[k] as? NSNumber { return n.doubleValue } }
            return nil
        }
        guard let requestId = str("request_id", "requestId", "id"),
              let sessionId = str("session_id", "sessionId", "sessionID"),
              let format = str("format"),
              let code = obj["code"] as? String, !code.isEmpty,
              let width = num("width"), let height = num("height") else { return nil }
        let kind = Kind(rawValue: str("kind") ?? "image") ?? .image
        self.requestId = requestId
        self.sessionId = sessionId
        self.kind = kind
        self.format = format
        self.code = code
        self.width = Int(width)
        self.height = Int(height)
        self.title = str("title") ?? (kind == .video ? "Video" : "Image")
        self.duration = num("duration")
        self.fps = num("fps").map { Int($0) }
        self.motionBlur = num("motionBlur", "motion_blur").map { Int($0) }
        self.audio = str("audio")
        self.artifactKind = str("artifactKind", "artifact_kind")
        self.bodyFormat = str("bodyFormat", "body_format")
        self.widths = ((obj["widths"] as? [NSNumber]) ?? []).map { $0.intValue }.filter { (240...3840).contains($0) }.prefix(4).map { $0 }
        let ref = str("reference")
        self.reference = ref?.hasPrefix("data:image/") == true ? ref : nil
        self.time = num("time")
        self.timeoutMs = num("timeoutMs", "timeout_ms")
        if kind == .video, format != "canvas" || (duration ?? 0) <= 0 || (fps ?? 0) <= 0 { return nil }
    }
}

/// What artifact_check saw at one width (gizzi `PaneRender.CheckWidth`).
struct PaneRenderCheckWidth: Codable, Sendable {
    var width: Int
    var height: Int
    var image: String?
    var blank: Bool
    var overflowX: Int
    var errors: [String]
    var similarity: Double?
    var mismatch: [String]?
}

/// The stored MP4 of a native video (gizzi `PaneRender.Video`).
struct PaneRenderVideo: Codable, Sendable {
    var url: String
    var mime: String
    var bytes: Int
    var duration: Double
    var fps: Int
    var codec: String?
    var audio: Bool?
}

/// The phone's answer, passed to the agent as it is (gizzi `PaneRender.Result`).
struct PaneRenderResult: Codable, Sendable {
    var ok: Bool
    var image: String? = nil
    var video: PaneRenderVideo? = nil
    var checks: [PaneRenderCheckWidth]? = nil
    var width: Int? = nil
    var height: Int? = nil
    var error: String? = nil

    static func failed(_ message: String) -> PaneRenderResult {
        PaneRenderResult(ok: false, error: message)
    }
}
