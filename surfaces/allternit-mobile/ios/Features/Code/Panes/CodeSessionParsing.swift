import Foundation

// MARK: - Progress

/// The session's plan as a checklist (ProgressCard / checklist.ts
/// `applyTodoCall`): todowrite / TodoWrite snapshots replace the list,
/// TaskCreate adds a step, TaskUpdate changes or deletes one, in order.
enum CodeProgress {
    enum Status: Equatable { case pending, inProgress, completed, failed }

    struct Step: Identifiable, Equatable {
        let id: String
        var content: String
        var activeForm: String?
        var status: Status
    }

    static func steps(from messages: [AgentSessionMessage]) -> [Step] {
        var list: [Step] = []
        for message in messages where message.role == "assistant" {
            for part in message.metadata?.parts ?? [] where part.type == "tool" {
                list = apply(list, tool: part.tool, input: part.state?.rawInput, output: part.state?.output?.value)
            }
        }
        return list
    }

    /// Done counts failed steps too: they are settled, not waiting.
    static func done(_ steps: [Step]) -> Int {
        steps.filter { $0.status == .completed || $0.status == .failed }.count
    }

    /// checklist.ts `isTodoTool`.
    static func isTodoTool(_ tool: String?) -> Bool {
        let name = normalized(tool)
        return name.contains("todo") || name.range(of: "task(create|update|list|get)$", options: .regularExpression) != nil
            || name.hasSuffix("planwrite")
    }

    static func apply(_ list: [Step], tool: String?, input: [String: AnyCodable]?, output: Any?) -> [Step] {
        guard isTodoTool(tool) else { return list }
        let name = normalized(tool)
        let inp = (input ?? [:]).mapValues(\.value)
        let isTaskCall = name.hasSuffix("taskcreate") || name.hasSuffix("taskupdate")
        if let snapshot = (inp["todos"] as? [Any]) ?? (isTaskCall ? nil : todos(fromResult: output)) {
            return snapshotSteps(snapshot)
        }
        if name.hasSuffix("taskcreate") {
            let content = text(inp["subject"]) ?? text(inp["content"]) ?? text(inp["title"]) ?? text(inp["description"])
            guard let content else { return list }
            let id = taskNumber(in: output) ?? String(list.count + 1)
            guard !list.contains(where: { $0.id == id }) else { return list }
            return list + [Step(id: id, content: content, activeForm: text(inp["activeForm"]), status: status(inp["status"]))]
        }
        if name.hasSuffix("taskupdate") {
            guard let id = text(inp["taskId"]) ?? text(inp["id"]) ?? taskNumber(in: output) else { return list }
            if text(inp["status"])?.lowercased() == "deleted" { return list.filter { $0.id != id } }
            let at = list.firstIndex { $0.id == id }
            var next = at.map { list[$0] } ?? Step(id: id, content: "Task \(id)", activeForm: nil, status: .pending)
            if let subject = text(inp["subject"]) { next.content = subject }
            if let activeForm = text(inp["activeForm"]) { next.activeForm = activeForm }
            if inp["status"] != nil { next.status = status(inp["status"]) }
            var out = list
            if let at { out[at] = next } else { out.append(next) }
            return out
        }
        return list
    }

    /// checklist.ts `checklistStatus`.
    static func status(_ raw: Any?, done: Any? = nil) -> Status {
        let value = (text(raw) ?? ((done as? Bool) == true ? "completed" : "pending")).lowercased()
        switch value {
        case "complete", "completed", "done": return .completed
        case "running", "in-progress", "in_progress", "active": return .inProgress
        case "cancelled", "canceled", "failed", "error": return .failed
        default: return .pending
        }
    }

    /// gizzi's todowrite/todoread output is the list as JSON; some runtimes
    /// wrap it as `{todos}` or `{metadata:{todos}}`.
    static func todos(fromResult result: Any?) -> [Any]? {
        var value = result
        if let string = value as? String {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            guard trimmed.hasPrefix("[") || trimmed.hasPrefix("{"),
                  let data = trimmed.data(using: .utf8),
                  let parsed = try? JSONSerialization.jsonObject(with: data) else { return nil }
            value = parsed
        }
        if let array = value as? [Any] { return array.allSatisfy { $0 is [String: Any] } ? array : nil }
        if let object = value as? [String: Any] {
            if let todos = object["todos"] as? [Any] { return todos }
            if let todos = (object["metadata"] as? [String: Any])?["todos"] as? [Any] { return todos }
            if let output = object["output"] { return todos(fromResult: output) }
        }
        return nil
    }

    private static func snapshotSteps(_ items: [Any]) -> [Step] {
        items.enumerated().compactMap { index, raw in
            let item = raw as? [String: Any] ?? [:]
            let content = (raw as? String) ?? text(item["content"]) ?? text(item["text"]) ?? text(item["title"]) ?? text(item["subject"])
            guard let content, !content.isEmpty else { return nil }
            return Step(id: text(item["id"]) ?? "todo-\(index)", content: content,
                        activeForm: text(item["activeForm"]), status: status(item["status"], done: item["done"]))
        }
    }

    private static func taskNumber(in output: Any?) -> String? {
        let string: String? = (output as? String)
            ?? (output as? [String: Any]).flatMap { text($0["text"]) ?? text($0["output"]) ?? text($0["content"]) ?? text($0["message"]) }
        guard let string, let range = string.range(of: "#\\d+", options: .regularExpression) else { return nil }
        return String(string[range].dropFirst())
    }

    private static func normalized(_ tool: String?) -> String {
        (tool ?? "").lowercased().replacingOccurrences(of: "[_\\-\\s]", with: "", options: .regularExpression)
    }

    static func text(_ value: Any?) -> String? {
        if let string = value as? String {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
        if let int = value as? Int { return String(int) }
        if let double = value as? Double { return String(Int(double)) }
        return nil
    }
}

// MARK: - Outputs

/// What the session made (OutputsCard): saved artifacts (artifact_create /
/// artifact_update results), pages it published (artifacts.allternit.com
/// links in its replies) and the files it wrote. Newest first, once each.
enum CodeOutputs {
    enum Kind: Equatable { case artifact(id: String, version: Int?), page(URL), file(path: String) }

    struct Row: Identifiable, Equatable {
        let id: String
        let title: String
        let kind: Kind

        var subtitle: String {
            switch kind {
            case .artifact(_, let version): return version.map { "Artifact · v\($0)" } ?? "Artifact"
            case .page: return "Page"
            case .file(let path): return path
            }
        }
    }

    private static let fileTools: Set<String> = ["write", "edit", "multiedit", "patch", "apply_patch", "str_replace_editor", "create_file"]

    static func rows(from messages: [AgentSessionMessage]) -> [Row] {
        var artifacts: [String: Row] = [:], artifactOrder: [String] = []
        var pages: [String: Row] = [:], pageOrder: [String] = []
        var files: [String: Row] = [:], fileOrder: [String] = []
        func put(_ row: Row, into map: inout [String: Row], order: inout [String]) {
            order.removeAll { $0 == row.id }
            order.append(row.id)
            map[row.id] = row
        }

        for message in messages where message.role == "assistant" {
            var texts = [message.content]
            for part in message.metadata?.parts ?? [] {
                if part.type == "text", let text = part.text { texts.append(text) }
                guard part.type == "tool", let tool = part.tool, let state = part.state else { continue }
                let lower = tool.lowercased()
                if let verb = artifactVerb(lower) {
                    guard state.status == "completed", state.error == nil,
                          let row = artifactRow(verb: verb, input: state.rawInput, output: state.output?.value) else { continue }
                    put(row, into: &artifacts, order: &artifactOrder)
                } else if fileTools.contains(lower.split(separator: ".").last.map(String.init) ?? lower),
                          let path = state.input["filePath"] ?? state.input["file_path"] ?? state.input["path"] {
                    let name = (path as NSString).lastPathComponent
                    put(Row(id: "f:\(path)", title: name, kind: .file(path: path)), into: &files, order: &fileOrder)
                }
            }
            for text in texts {
                for url in pageURLs(in: text) {
                    let title = url.pathComponents.last.map { ($0 as NSString).deletingPathExtension } ?? url.absoluteString
                    put(Row(id: "p:\(url.absoluteString)", title: title, kind: .page(url)), into: &pages, order: &pageOrder)
                }
            }
        }
        let titles = Set(artifacts.values.map(\.title))
        return artifactOrder.reversed().compactMap { artifacts[$0] }
            + pageOrder.reversed().compactMap { pages[$0] }
            + fileOrder.reversed().compactMap { files[$0] }.filter { !titles.contains($0.title) }
    }

    /// from-transcript.ts `artifactToolVerb`: `…artifact_create` / `…artifact_update`.
    private static func artifactVerb(_ tool: String) -> String? {
        guard let range = tool.range(of: "(^|[_.:/-])artifact_(create|update)$", options: .regularExpression) else { return nil }
        return tool[range].hasSuffix("create") ? "create" : "update"
    }

    /// from-transcript.ts `artifactRefFromToolPart`, for a finished call.
    private static func artifactRow(verb: String, input: [String: AnyCodable]?, output: Any?) -> Row? {
        let inp = (input ?? [:]).mapValues(\.value)
        var out = output as? [String: Any]
        if let content = out?["content"] as? [Any],
           let text = content.compactMap({ ($0 as? [String: Any]).flatMap { $0["type"] as? String == "text" ? $0["text"] as? String : nil } }).first {
            out = json(text) ?? out
        }
        if out?["id"] == nil, out?["artifact"] == nil, let string = output as? String {
            // Through a CLI's MCP bridge: a JSON summary line, then a sentence.
            out = json(String(string.split(separator: "\n", maxSplits: 1).first ?? "")) ?? out
        }
        let payload = out?["artifact"] as? [String: Any] ?? [:]
        let id = CodeProgress.text(out?["id"]) ?? CodeProgress.text(payload["id"]) ?? (verb == "update" ? CodeProgress.text(inp["id"]) : nil)
        guard let id else { return nil }
        let title = CodeProgress.text(inp["title"]) ?? CodeProgress.text(payload["title"]) ?? CodeProgress.text(out?["title"]) ?? "Artifact"
        return Row(id: "a:\(id)", title: title, kind: .artifact(id: id, version: out?["version"] as? Int))
    }

    private static func json(_ text: String) -> [String: Any]? {
        guard let data = text.trimmingCharacters(in: .whitespacesAndNewlines).data(using: .utf8) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    private static let pagePattern = try? NSRegularExpression(pattern: "https://artifacts\\.allternit\\.com/[^\\s)\\]>\"'`]+")

    private static func pageURLs(in text: String) -> [URL] {
        guard let pagePattern, text.contains("artifacts.allternit.com") else { return [] }
        let range = NSRange(text.startIndex..., in: text)
        return pagePattern.matches(in: text, range: range).compactMap { match in
            Range(match.range, in: text).flatMap { URL(string: String(text[$0]).trimmingCharacters(in: CharacterSet(charactersIn: ".,;:"))) }
        }
    }
}
