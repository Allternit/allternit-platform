import Foundation

/// Reads v2 artifact bodies the way the web viewers do
/// (allternit-ai src/components/artifacts/viewers.tsx and the editors'
/// body helpers). Pure functions — no UI.
enum ArtifactBodyDecoder {
    private static func json(_ body: String) -> Any? {
        try? JSONSerialization.jsonObject(with: Data(body.utf8), options: [.fragmentsAllowed])
    }

    // MARK: Doc

    /// Readable markdown out of a doc body (markdown, doc+json, legacy
    /// sections) — port of editors/doc-markdown.ts `docToMarkdown`.
    static func docMarkdown(body: String, format: String) -> String? {
        if format == "text/markdown" || format == "text/plain" { return body }
        guard let root = json(body) else { return nil }
        if let dict = root as? [String: Any] {
            if let markdown = dict["markdown"] as? String { return markdown }
            if let tabs = dict["tabs"] as? [Any] {
                return tabs.enumerated().map { index, raw -> String in
                    let tab = raw as? [String: Any] ?? [:]
                    let title = (tab["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Tab \(index + 1)"
                    var inner = ""
                    if let markdown = tab["markdown"] as? String {
                        inner = markdown
                    } else if let body = tab["body"] as? String {
                        let fmt = body.trimmingCharacters(in: .whitespaces).hasPrefix("{")
                            ? "application/vnd.allternit.doc+json" : "text/markdown"
                        inner = docMarkdown(body: body, format: fmt) ?? body
                    }
                    return "# \(title)\n\n\(inner)"
                }.joined(separator: "\n\n")
            }
        }
        let sections: [Any]?
        if let array = root as? [Any] {
            sections = array
        } else if let dict = root as? [String: Any] {
            sections = (dict["sections"] as? [Any]) ?? (dict["blocks"] as? [Any])
        } else {
            sections = nil
        }
        guard let sections else { return nil }
        return sections.compactMap { raw -> String? in
            if let s = raw as? String { return s }
            let o = raw as? [String: Any] ?? [:]
            let heading = (o["heading"] as? String) ?? (o["title"] as? String) ?? ""
            let text = (o["body"] as? String) ?? (o["text"] as? String) ?? (o["content"] as? String) ?? ""
            if (o["type"] as? String) == "heading", !text.isEmpty { return "## \(text)" }
            let joined = [heading.isEmpty ? "" : "## \(heading)", text].filter { !$0.isEmpty }.joined(separator: "\n\n")
            return joined.isEmpty ? nil : joined
        }.joined(separator: "\n\n")
    }

    // MARK: Sheet

    struct SheetGrid: Sendable {
        let columns: [String]
        let rows: [[String]]
    }

    private static func cellText(_ value: Any?) -> String {
        guard let value else { return "" }
        switch value {
        case is NSNull: return ""
        case let s as String: return s
        case let n as NSNumber:
            // JSONSerialization bools arrive as NSNumber too.
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue ? "TRUE" : "FALSE" }
            return n.stringValue
        default: return String(describing: value)
        }
    }

    /// Rows from `{columns?, rows}`, a bare 2-D array, the Sheets editor's
    /// workbook (`sheets[0].cells["r:c"]`), or CSV — port of `sheetRows`.
    static func sheetGrid(body: String, format: String) -> SheetGrid? {
        if format == "text/csv" {
            let lines = body.split(whereSeparator: \.isNewline).map { $0.split(separator: ",", omittingEmptySubsequences: false).map(String.init) }
            return SheetGrid(columns: lines.first ?? [], rows: Array(lines.dropFirst()))
        }
        guard let root = json(body) else { return nil }
        if let dict = root as? [String: Any],
           let first = (dict["sheets"] as? [Any])?.first as? [String: Any],
           let cells = first["cells"] as? [String: Any] {
            var maxRow = -1, maxCol = -1
            for key in cells.keys {
                let parts = key.split(separator: ":").compactMap { Int($0) }
                guard parts.count == 2 else { continue }
                maxRow = max(maxRow, parts[0]); maxCol = max(maxCol, parts[1])
            }
            guard maxRow >= 0 else { return SheetGrid(columns: [], rows: []) }
            let rows = (0...maxRow).map { r in
                (0...max(maxCol, 0)).map { c -> String in
                    guard let cell = cells["\(r):\(c)"] as? [String: Any] else { return "" }
                    if let value = cell["value"] { return cellText(value) }
                    if let formula = cell["formula"] as? String { return formula.hasPrefix("=") ? formula : "=\(formula)" }
                    return ""
                }
            }
            return SheetGrid(columns: [], rows: rows)
        }
        let sheet: [String: Any]?
        if root is [Any] {
            sheet = nil
        } else if let dict = root as? [String: Any], let first = (dict["sheets"] as? [Any])?.first as? [String: Any] {
            sheet = first
        } else {
            sheet = root as? [String: Any]
        }
        guard let rawRows = (root as? [Any]) ?? (sheet?["rows"] as? [Any]) else { return nil }
        var columns: [String] = (sheet?["columns"] as? [Any])?.map { c in
            if let s = c as? String { return s }
            return cellText((c as? [String: Any])?["name"])
        } ?? []
        if columns.isEmpty, let firstObject = rawRows.first as? [String: Any] {
            columns = firstObject.keys.sorted()
        }
        let rows = rawRows.map { row -> [String] in
            if let array = row as? [Any] { return array.map { cellText($0) } }
            if let object = row as? [String: Any] {
                return columns.isEmpty ? object.values.map { cellText($0) } : columns.map { cellText(object[$0]) }
            }
            return [cellText(row)]
        }
        return SheetGrid(columns: columns, rows: rows)
    }

    // MARK: Slides

    struct Slide: Sendable {
        let title: String?
        let bullets: [String]
        let body: String?
    }

    static func slides(body: String) -> [Slide]? {
        guard let root = json(body) else { return nil }
        let list = (root as? [Any]) ?? ((root as? [String: Any])?["slides"] as? [Any])
        guard let list else { return nil }
        return list.map { raw in
            let o = raw as? [String: Any] ?? [:]
            return Slide(
                title: o["title"] as? String,
                bullets: (o["bullets"] as? [Any])?.compactMap { $0 as? String } ?? [],
                body: (o["body"] as? String) ?? (o["content"] as? String)
            )
        }
    }

    // MARK: Media

    /// First non-comment line of a `text/uri-list` body.
    static func firstURI(_ body: String) -> String? {
        body.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty && !$0.hasPrefix("#") }
    }

    /// Footage URL of a video artifact (`meta.media.url`, or a body that is
    /// the export URL) — port of `footageRefFromMeta`.
    static func videoURL(body: String, meta: [String: JSONValue]?) -> String? {
        if case .object(let media)? = meta?["media"], case .string(let url)? = media["url"], !url.isEmpty {
            return url
        }
        guard let fromBody = firstURI(body) else { return nil }
        if case .string("video")? = meta?["media"] { return fromBody }
        let lowered = fromBody.lowercased()
        if lowered.hasPrefix("http://") || lowered.hasPrefix("https://") || lowered.hasPrefix("/api/") || lowered.hasPrefix("data:video") {
            return fromBody
        }
        return nil
    }

    /// A design body that is plain HTML (older designs) — canvas designs
    /// are JSON and need the Design studio.
    static func designHTML(body: String, format: String) -> String? {
        if format == "text/html" { return body }
        let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("<") { return body }
        if let dict = json(body) as? [String: Any], let html = dict["html"] as? String, !html.isEmpty { return html }
        return nil
    }
}
