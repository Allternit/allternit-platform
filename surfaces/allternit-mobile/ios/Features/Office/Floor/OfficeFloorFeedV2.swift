import Foundation
import UIKit

/// `GET /api/v1/floor/state` v2 (allternit-api floor_routes.rs): the pods, the main bot,
/// and per bot `pod`, `needs` and `lastResult`. Every field is optional so an older
/// server (v1 body) still decodes; v2 data is only used when the frame said `v: 2`.
struct FloorStateV2: Decodable {
    struct Pod: Decodable {
        let id: String
        let name: String?
        let color: String?
        let kind: String?
    }
    struct Needs: Decodable {
        let approval: Int?
        let question: Int?
        let suggestion: Int?
    }
    struct LastResult: Decodable {
        let text: String?
        /// Epoch milliseconds.
        let at: Double?
        let threadId: String?
    }
    struct Bot: Decodable {
        let id: String
        let pod: String?
        let needs: Needs?
        let lastResult: LastResult?
    }

    let v: Int?
    let mainBotId: String?
    let pods: [Pod]?
    let bots: [Bot]?
}

extension BotModeClient {
    /// The Workspace floor's server state. Callers keep the last good value on failure.
    func floorState() async throws -> FloorStateV2 {
        try await APIClient.shared.get(path: "floor/state")
    }
}

/// Builds the v2 additions to the `allternit:workspace` feed (websites
/// src/campus/workspace/workspace-feed.ts). Pure, so it reads and tests without UI.
enum OfficeFloorFeedV2 {
    private static let podKinds: Set<String> = ["project", "team", "chief"]
    private static let bubbleWindowMs: Double = 10 * 60 * 1000

    /// A valid `#rrggbb`, else a neutral grey (the frame guards host colours too).
    static func hex(_ value: String?) -> String {
        guard let value, value.count == 7, value.hasPrefix("#"),
              value.dropFirst().allSatisfy({ $0.isHexDigit }) else { return "#9CA3AF" }
        return value
    }

    /// The one need a bot shows: approval over question over suggestion. Nil when none.
    /// `approvals` is the app's own pending approval count for the bot (the server's is 0).
    static func need(_ needs: FloorStateV2.Needs?, approvals: Int, waitingThreads: Int) -> [String: Any]? {
        let approval = max(0, (needs?.approval ?? 0) + approvals)
        // A thread waiting on the person is a question even when the server count lags.
        let question = max(needs?.question ?? 0, waitingThreads)
        let suggestion = max(0, needs?.suggestion ?? 0)
        if approval > 0 { return ["kind": "approval", "count": approval] }
        if question > 0 { return ["kind": "question", "count": question] }
        if suggestion > 0 { return ["kind": "suggestion", "count": suggestion] }
        return nil
    }

    /// Adds `v`, `pods`, `bubbles` to the message and `pod` / `needs` / main-bot `chief` to each bot entry.
    /// `bots` entries are the v1 feed entries (with "id"); `waiting` and `approvals` are per bot id.
    static func decorate(message: inout [String: Any], bots: [[String: Any]], floor: FloorStateV2?,
                         approvals: [String: Int], waiting: [String: Int], now: Date = Date()) {
        let byId = Dictionary((floor?.bots ?? []).map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        let main = floor?.mainBotId
        var bubbles: [(at: Double, item: [String: Any])] = []
        let nowMs = now.timeIntervalSince1970 * 1000

        let out: [[String: Any]] = bots.map { source in
            var entry = source
            guard let id = entry["id"] as? String else { return entry }
            let server = byId[id]
            if let pod = server?.pod { entry["pod"] = pod }
            if let main { entry["chief"] = (id == main) }
            if let n = need(server?.needs, approvals: approvals[id] ?? 0, waitingThreads: waiting[id] ?? 0) {
                entry["needs"] = n
            }
            if let r = server?.lastResult, let text = r.text?.trimmingCharacters(in: .whitespacesAndNewlines),
               !text.isEmpty, let at = r.at, nowMs - at < bubbleWindowMs, at <= nowMs + 60_000 {
                bubbles.append((at, ["id": "result:\(id):\(Int(at))", "botId": id, "kind": "result",
                                     "text": String(text.prefix(120)), "at": Int(at)]))
            }
            return entry
        }

        message["v"] = 2
        message["bots"] = out
        let pods: [[String: Any]] = (floor?.pods ?? []).compactMap { pod in
            guard !pod.id.isEmpty, let name = pod.name, !name.isEmpty else { return nil }
            return ["id": pod.id, "name": String(name.prefix(40)), "color": hex(pod.color),
                    "kind": podKinds.contains(pod.kind ?? "") ? pod.kind! : "project"]
        }
        if !pods.isEmpty { message["pods"] = pods }
        let kept = bubbles.sorted { $0.at > $1.at }.prefix(8).map { $0.item }
        if !kept.isEmpty { message["bubbles"] = Array(kept) }
        // Handoff labels need /floor/events, which this app does not fetch yet; handoffs are omitted.
    }
}

/// Frame anchors (`allternit:workspace:anchors`) and the host to frame `focus` sender.
/// Anchors are frame CSS px; the latest set is kept for native overlays (not drawn yet).
@MainActor
final class OfficeFloorBridge {
    struct Anchor { let botId: String; let x: Double; let y: Double; let w: Double; let h: Double; let visible: Bool }

    private(set) var anchors: [Anchor] = []
    /// Hands a JSON message to the host page; set by the web view.
    var post: (String) -> Void = { _ in }

    func setAnchors(_ items: [[String: Any]]) {
        anchors = items.prefix(100).compactMap { item in
            guard let id = item["botId"] as? String, let x = item["x"] as? Double, let y = item["y"] as? Double,
                  let w = item["w"] as? Double, let h = item["h"] as? Double else { return nil }
            return Anchor(botId: id, x: x, y: y, w: w, h: h, visible: item["visible"] as? Bool ?? true)
        }
    }

    /// Aims the camera at a bot or a pod (both nil clears). `inset` is the part of the frame the
    /// native UI covers, in CSS px, so the target lands in the visible area.
    func focus(botId: String? = nil, podId: String? = nil, inset: UIEdgeInsets = .zero) {
        let payload: [String: Any] = [
            "type": "allternit:workspace:focus",
            "botId": botId.map { $0 as Any } ?? NSNull(),
            "podId": podId.map { $0 as Any } ?? NSNull(),
            "inset": ["top": inset.top, "right": inset.right, "bottom": inset.bottom, "left": inset.left],
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: payload) else { return }
        post(String(decoding: data, as: UTF8.self))
    }
}
