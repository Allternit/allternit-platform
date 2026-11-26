import Foundation

/// MCP tool part metadata (PLAN contract §3). It arrives on the tool part as
/// `state.metadata.mcp` / `state.metadata.mcpScript`; a frame that carries the
/// metadata object at the top level is read too.
struct McpToolMeta: Equatable {
    let server: String
    let tool: String
    let structured: Any?
    let canReport: Bool

    static func == (a: McpToolMeta, b: McpToolMeta) -> Bool {
        a.server == b.server && a.tool == b.tool && a.canReport == b.canReport
            && McpMetaReader.pretty(a.structured) == McpMetaReader.pretty(b.structured)
    }
}

struct McpScriptMeta: Equatable {
    struct Call: Equatable, Identifiable {
        let id: Int
        let server: String
        let tool: String
        let ms: Int
        let ok: Bool
        let error: String?
    }
    let script: String
    let servers: [String]
    let calls: [Call]
    let durationMs: Int
    let resultText: String

    var collapsedTitle: String {
        "Ran a script · \(servers.count) server\(servers.count == 1 ? "" : "s") · \(calls.count) call\(calls.count == 1 ? "" : "s")"
    }
}

enum McpMetaReader {
    /// The object holding `mcp` / `mcpScript`: the metadata root, `state.metadata`, or `metadata`.
    private static func holder(_ metadata: String?) -> [String: Any]? {
        guard let metadata, let data = metadata.data(using: .utf8),
              let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        let candidates: [[String: Any]?] = [
            (root["state"] as? [String: Any])?["metadata"] as? [String: Any],
            root,
            root["metadata"] as? [String: Any],
        ]
        for case let c? in candidates where c["mcp"] != nil || c["mcpScript"] != nil { return c }
        return nil
    }

    static func tool(_ metadata: String?) -> McpToolMeta? {
        guard let obj = holder(metadata)?["mcp"] as? [String: Any],
              let server = obj["server"] as? String, let tool = obj["tool"] as? String else { return nil }
        let structured = obj["structuredContent"]
        return McpToolMeta(
            server: server, tool: tool,
            structured: structured is NSNull ? nil : structured,
            canReport: (obj["canReport"] as? Bool) ?? false
        )
    }

    static func script(_ metadata: String?) -> McpScriptMeta? {
        guard let obj = holder(metadata)?["mcpScript"] as? [String: Any],
              let script = obj["script"] as? String else { return nil }
        let calls: [McpScriptMeta.Call] = ((obj["calls"] as? [[String: Any]]) ?? []).enumerated().map { i, c in
            .init(id: i, server: c["server"] as? String ?? "", tool: c["tool"] as? String ?? "",
                  ms: (c["ms"] as? NSNumber)?.intValue ?? 0, ok: (c["ok"] as? Bool) ?? true, error: c["error"] as? String)
        }
        var servers = (obj["servers"] as? [String]) ?? []
        if servers.isEmpty { for c in calls where !servers.contains(c.server) { servers.append(c.server) } }
        return McpScriptMeta(
            script: script, servers: servers, calls: calls,
            durationMs: (obj["durationMs"] as? NSNumber)?.intValue ?? 0,
            resultText: pretty(obj["result"])
        )
    }

    /// Pretty JSON (or the string itself) for display.
    static func pretty(_ value: Any?) -> String {
        guard let value, !(value is NSNull) else { return "" }
        if let s = value as? String { return s }
        if JSONSerialization.isValidJSONObject(value),
           let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]) {
            return String(data: data, encoding: .utf8) ?? ""
        }
        return "\(value)"
    }

    /// One-line text for a cell.
    static func cell(_ value: Any?) -> String {
        guard let value, !(value is NSNull) else { return "" }
        if let s = value as? String { return s }
        if let n = value as? NSNumber { return n.stringValue }
        if JSONSerialization.isValidJSONObject(value),
           let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
            return String(data: data, encoding: .utf8) ?? ""
        }
        return "\(value)"
    }
}
