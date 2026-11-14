import Foundation

/// Connect-kit answers for `-sample-workspace` (DEBUG screenshots and UI
/// checks), in the wire shape of CONTRACT-connections-2026-11 §3–4. The
/// cloud host answers the account rows; the relayed computer answers its
/// own rows (CLI tools, local brain). Writes run on an in-memory copy: a key
/// starting with "sk-" or "AIza" checks out, anything else is rejected, and a
/// device-code sign-in finishes on the fourth `/login` poll (about 8 s).
final class SampleConnections: @unchecked Sendable {
    static let shared = SampleConnections()

    private let lock = NSLock()
    private var cloud: [String: [String: Any]] = [:]
    private var runtime: [String: [String: Any]] = [:]
    private var polls: [String: Int] = [:]

    private init() {
        func row(_ id: String, _ group: String, _ state: String, found: String? = nil, reason: String? = nil, code: String? = nil,
                 runsOn: String = "server", available: Bool = true, unavailable: String? = nil, fallbacks: [String] = [], source: String) -> [String: Any] {
            [
                "id": id, "group": group, "state": state, "found": found ?? NSNull(), "reason": reason ?? NSNull(),
                "reason_code": code ?? NSNull(), "runs_on": runsOn, "available": available,
                "unavailable_reason": unavailable ?? NSNull(), "fallbacks": fallbacks, "terms": "cleared", "saved": "account",
                "models": [String](), "source": source,
            ]
        }
        let c = "cloud", r = "runtime:\(SampleWorkspace.runtimeId)"
        for item in [
            row("allternit", "brains", "on", found: "Free · $2 a month free", source: c),
            row("openai-key", "brains", "on", found: "Key works · GPT models · ends in 4f2a", source: c),
            row("anthropic-key", "brains", "needs", reason: "The key was rejected", code: "key_rejected", source: c),
            row("claude", "brains", "off", code: "not_available_yet", runsOn: "runner", available: false,
                unavailable: "Not available yet. Use a key for now.", fallbacks: ["anthropic-key"], source: c),
            row("grok", "brains", "off", code: "not_available_yet", runsOn: "runner", available: false,
                unavailable: "Not available yet. Use a key for now.", fallbacks: ["xai-key"], source: c),
            row("kimi", "brains", "off", code: "not_available_yet", runsOn: "runner", available: false,
                unavailable: "Not available yet. Use a key for now.", fallbacks: ["kimi-key"], source: c),
            row("copilot", "brains", "off", available: false, unavailable: "Coming soon.", fallbacks: ["github"], source: c),
            row("chatgpt", "brains", "off", runsOn: "computer", source: c),
            row("cloud-computer", "computers", "off", source: c),
            row("this-mac", "computers", "on", found: "Harbor MacBook Pro · online", runsOn: "computer", source: c),
            row("gmail", "apps", "on", found: "orders@harborbakery.example", source: c),
            row("telegram", "channels", "on", found: "@harbor_bakery_bot", source: c),
        ] { cloud[item["id"] as! String] = item }
        for item in [
            row("codex", "tools", "on", found: "Uses your ChatGPT Plus", runsOn: "computer", source: r),
            row("claude-code", "tools", "needs", reason: "Sign-in expired", code: "login_expired", runsOn: "computer", source: r),
            row("chatgpt", "brains", "off", runsOn: "computer", source: r),
            row("local", "brains", "off", runsOn: "computer", source: r),
        ] { runtime[item["id"] as! String] = item }
    }

    /// nil = not a connections route.
    func answer(method: String, url: URL, parts: [String], body: Data?) -> SampleWorkspaceAPI.Answer? {
        guard parts.first == "connections" else { return nil }
        let isRuntime = url.scheme == "allternit-relay"
        lock.lock()
        defer { lock.unlock() }
        var table = isRuntime ? runtime : cloud
        defer { if isRuntime { runtime = table } else { cloud = table } }
        let payload = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        switch (method, parts.count) {
        case ("GET", 1):
            return SampleWorkspaceAPI.json(["version": 1, "connections": Array(table.values).sorted { ($0["id"] as! String) < ($1["id"] as! String) }])
        case ("POST", 3) where parts[2] == "submit":
            let id = parts[1]
            let value = payload["value"] as? String ?? ""
            var row = table[id] ?? ["id": id, "group": "brains", "runs_on": "server", "available": true, "fallbacks": [String](), "source": isRuntime ? "runtime:\(SampleWorkspace.runtimeId)" : "cloud"]
            if value.hasPrefix("sk-") || value.hasPrefix("AIza") || isRuntime {
                row["state"] = "on"
                row["found"] = isRuntime ? "Signed in" : "Key works · ends in \(value.suffix(4))"
                row["reason"] = NSNull()
                row["reason_code"] = NSNull()
            } else {
                row["state"] = "needs"
                row["reason"] = "The key was rejected"
                row["reason_code"] = "key_rejected"
            }
            table[id] = row
            return SampleWorkspaceAPI.json(row)
        case ("POST", 3) where parts[2] == "connect":
            let id = parts[1]
            polls[id] = 0
            let expires = ISO8601DateFormatter().string(from: Date().addingTimeInterval(900))
            if id == "claude-code" {
                return SampleWorkspaceAPI.json(["login": ["kind": "link_paste", "url": "https://claude.ai/oauth/authorize", "expires_at": expires, "poll": true]])
            }
            if id == "grok-cli" {
                return SampleWorkspaceAPI.json(["login": ["kind": "device_code", "code": "KQ7M-24TX", "url": "https://accounts.x.ai/oauth2/device",
                                                         "url_with_code": "https://accounts.x.ai/oauth2/device?user_code=KQ7M-24TX", "expires_at": expires, "poll": true]])
            }
            return SampleWorkspaceAPI.json(["login": ["kind": "device_code", "code": "KQ7M-24TX", "url": "https://auth.openai.com/codex/device", "expires_at": expires, "poll": true]])
        case ("GET", 3) where parts[2] == "login":
            let id = parts[1]
            let n = (polls[id] ?? 0) + 1
            polls[id] = n
            guard n >= 4 else { return SampleWorkspaceAPI.json(["status": "pending"]) }
            var row = table[id] ?? ["id": id, "available": true, "fallbacks": [String](), "source": "runtime:\(SampleWorkspace.runtimeId)"]
            row["state"] = "on"
            row["found"] = id == "chatgpt" ? "ChatGPT Plus" : "Signed in"
            row["reason"] = NSNull()
            row["reason_code"] = NSNull()
            table[id] = row
            return SampleWorkspaceAPI.json(["status": "done", "row": row])
        case ("DELETE", 2):
            if var row = table[parts[1]] {
                row["state"] = "off"
                row["found"] = NSNull()
                table[parts[1]] = row
            }
            return SampleWorkspaceAPI.Answer(status: 204, body: Data())
        default:
            return SampleWorkspaceAPI.json(["error": "not_found"], status: 404)
        }
    }
}
