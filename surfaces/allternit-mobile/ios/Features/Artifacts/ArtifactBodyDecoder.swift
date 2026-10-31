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
            let lines = csvRows(body)
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

    /// CSV rows, keeping commas and line breaks inside quoted cells.
    private static func csvRows(_ text: String) -> [[String]] {
        var rows: [[String]] = []
        var row: [String] = []
        var cell = ""
        var quoted = false
        var chars = Array(text)[...]
        while let c = chars.popFirst() {
            if quoted {
                if c == "\"" {
                    if chars.first == "\"" { cell.append(c); chars.removeFirst() } else { quoted = false }
                } else {
                    cell.append(c)
                }
            } else if c == "\"" {
                quoted = true
            } else if c == "," {
                row.append(cell)
                cell = ""
            } else if c.isNewline {
                row.append(cell)
                cell = ""
                if row != [""] { rows.append(row) }
                row = []
            } else {
                cell.append(c)
            }
        }
        row.append(cell)
        if row != [""] { rows.append(row) }
        return rows
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
            // The Slides editor's blocks are the source of the text (slideText
            // in slides-body.ts); its derived title/body may be missing.
            if let blocks = o["blocks"] as? [Any] {
                let text = slideText(blocks)
                return Slide(title: text.title, bullets: [], body: text.body)
            }
            return Slide(
                title: o["title"] as? String,
                bullets: (o["bullets"] as? [Any])?.compactMap { $0 as? String } ?? [],
                body: (o["body"] as? String) ?? (o["content"] as? String)
            )
        }
    }

    /// Text blocks top to bottom: the first is the title, the rest the body.
    private static func slideText(_ blocks: [Any]) -> (title: String, body: String) {
        func at(_ block: [String: Any], _ key: String) -> Double { (block[key] as? NSNumber)?.doubleValue ?? 0 }
        let texts = blocks
            .compactMap { $0 as? [String: Any] }
            .filter { ($0["type"] as? String) == "text" }
            .sorted { a, b in at(a, "y") == at(b, "y") ? at(a, "x") < at(b, "x") : at(a, "y") < at(b, "y") }
            .map { ($0["text"] as? String) ?? "" }
        return (texts.first ?? "", texts.dropFirst().filter { !$0.isEmpty }.joined(separator: "\n"))
    }

    /// The first slide's lines for a card (slidesPlainText + firstSlideLines):
    /// a JSON deck, or text split at `---` and top-level headings.
    static func firstSlideLines(body: String, format: String) -> [String] {
        if let first = slides(body: body)?.first {
            var lines: [String] = first.title.map { [$0] } ?? []
            lines += first.bullets.map { "• \($0)" }
            lines += (first.body ?? "").split(whereSeparator: \.isNewline).map(String.init)
            return lines.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        }
        guard format.hasPrefix("text/") else { return [] }
        let firstPart = body
            .replacingOccurrences(of: #"\n-{3,}\n"#, with: "\u{0}", options: .regularExpression)
            .split(separator: "\u{0}").first.map(String.init) ?? body
        var lines: [String] = []
        for line in firstPart.split(whereSeparator: \.isNewline).map(String.init) {
            if line.hasPrefix("# "), !lines.isEmpty { break }
            let text = line.replacingOccurrences(of: #"^#+\s*"#, with: "", options: .regularExpression)
            if !text.trimmingCharacters(in: .whitespaces).isEmpty { lines.append(text) }
        }
        return lines
    }

    // MARK: Dashboard

    /// Tile titles of a dashboard body, or panel titles of an ops sheet (first
    /// tab first); nil for the other formats.
    static func dashboardTileTitles(body: String) -> [String]? {
        guard let dict = json(body) as? [String: Any] else { return nil }
        if let tabs = dict["tabs"] as? [[String: Any]] {
            return tabs.flatMap { ($0["panels"] as? [Any]) ?? [] }
                .compactMap { ($0 as? [String: Any])?["title"] as? String }
        }
        guard let tiles = dict["tiles"] as? [Any] else { return nil }
        return tiles.compactMap { ($0 as? [String: Any])?["title"] as? String }
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

    // MARK: Design card

    /// Markup for a design's card: an HTML design as it is, else the first
    /// artboard of a canvas (port of canvasToHtml / layerHtml in
    /// editors/design-canvas.ts for the layers a card can show). A page layer
    /// is shown itself instead of the iframe the export wraps it in.
    static func designPreviewHTML(body: String, format: String) -> String? {
        guard let doc = json(body) as? [String: Any], let layers = doc["layers"] as? [[String: Any]] else {
            let html = designHTML(body: body, format: format)
            return html?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false ? html : nil
        }
        let board = (doc["artboards"] as? [[String: Any]])?.first
        let boardId = board?["id"] as? String
        let own = layers.filter { ($0["artboard"] as? String) == boardId }
        if let html = (own + layers).lazy.compactMap({ layer -> String? in
            guard (layer["type"] as? String) == "design-html" else { return nil }
            return (layer["props"] as? [String: Any])?["html"] as? String
        }).first(where: { !$0.isEmpty }) {
            return html
        }
        guard let board else { return nil }
        let kids = own.map(designLayerHTML).joined()
        guard !kids.isEmpty else { return nil }
        let width = max((board["w"] as? NSNumber)?.doubleValue ?? 1280, 1)
        let height = (board["h"] as? NSNumber)?.doubleValue ?? 800
        let fill = designColour(board["fill"], fallback: "#ffffff")
        return "<div style=\"zoom:\(1000 / width);position:relative;width:\(width)px;height:\(height)px;background:\(fill);overflow:hidden\">\(kids)</div>"
    }

    private static func designLayerHTML(_ layer: [String: Any]) -> String {
        func number(_ key: String, in dict: [String: Any], _ fallback: Double = 0) -> Double {
            (dict[key] as? NSNumber)?.doubleValue ?? fallback
        }
        let p = layer["props"] as? [String: Any] ?? [:]
        let w = max(number("w", in: layer), 0)
        let rotation = number("rotation", in: layer)
        let opacity = number("opacity", in: p, 1)
        let box = "position:absolute;left:\(number("x", in: layer))px;top:\(number("y", in: layer))px;width:\(w)px;height:\(max(number("h", in: layer), 0))px;"
            + (rotation != 0 ? "transform:rotate(\(rotation)rad);" : "")
            + "box-sizing:border-box;" + (opacity < 1 ? "opacity:\(opacity);" : "")
        let centered = "display:flex;align-items:center;justify-content:center;"
        switch layer["type"] as? String {
        case "design-component":
            let label = escapeHTML(p["label"] as? String ?? "")
            return "<div style=\"\(box)\(centered)background:\(designColour(p["fill"], fallback: "#f3f4f6"));border:1.5px solid \(designColour(p["stroke"], fallback: "#d1d5db"));border-radius:\(number("radius", in: p, 8))px;font:600 14px system-ui,sans-serif\">\(label)</div>"
        case "design-uiblock":
            let variant = p["variant"] as? String ?? "card"
            let style = uiBlockStyles[variant] ?? uiBlockStyles["card"]!
            let middle = variant == "divider" ? "<div style=\"width:100%;height:1px;background:#d1d5db\"></div>" : style.label
            return "<div style=\"\(box)\(centered)background:\(style.background);color:\(style.foreground);border:1px solid \(style.border);border-radius:\(style.radius)px;font:500 14px system-ui,sans-serif\">\(middle)</div>"
        case "text":
            let sizes: [String: Int] = ["s": 18, "m": 24, "l": 36, "xl": 44]
            let aligns = ["middle": "center", "end": "right"]
            let size = sizes[p["size"] as? String ?? "m"] ?? 24
            let align = aligns[p["textAlign"] as? String ?? ""] ?? "left"
            let width = max(w, number("w", in: p))
            return "<p style=\"\(box)margin:0;width:\(width)px;height:auto;color:\(designColour(p["color"], fallback: "#1d1d1d"));font:\(size)px/1.35 system-ui,sans-serif;text-align:\(align);white-space:pre-wrap\">\(escapeHTML(p["text"] as? String ?? ""))</p>"
        case "geo":
            let colour = designColour(p["color"], fallback: "#1d1d1d")
            let fill = p["fill"] as? String ?? "none"
            let background = (fill == "solid" || fill == "fill") ? colour : fill == "semi" ? "\(colour)33" : "transparent"
            let radius = (p["geo"] as? String) == "ellipse" ? "50%" : "0"
            return "<div style=\"\(box)\(centered)background:\(background);border:2px solid \(colour);border-radius:\(radius);font:16px system-ui,sans-serif;color:\(colour)\">\(escapeHTML(p["text"] as? String ?? ""))</div>"
        default:
            return ""
        }
    }

    private static let uiBlockStyles: [String: (background: String, foreground: String, border: String, radius: Int, label: String)] = [
        "button-primary": ("#111827", "#ffffff", "transparent", 8, "Button"),
        "button-secondary": ("#ffffff", "#111827", "#d1d5db", 8, "Button"),
        "input": ("#ffffff", "#9ca3af", "#d1d5db", 8, "Input"),
        "card": ("#ffffff", "#111827", "#e5e7eb", 12, ""),
        "nav-bar": ("#ffffff", "#111827", "#e5e7eb", 0, "Navigation"),
        "badge": ("#eef2ff", "#4338ca", "transparent", 999, "Badge"),
        "avatar": ("#e5e7eb", "#6b7280", "transparent", 999, ""),
        "divider": ("transparent", "#9ca3af", "transparent", 0, ""),
    ]

    private static let designNamedColours: [String: String] = [
        "black": "#1d1d1d", "grey": "#9fa8b2", "light-violet": "#e085f4", "violet": "#ae3ec9", "blue": "#4465e9",
        "light-blue": "#4ba1f1", "yellow": "#f1ac4b", "orange": "#e16919", "green": "#099268", "light-green": "#4cb05e",
        "light-red": "#f87777", "red": "#e03131", "white": "#ffffff",
    ]

    /// Only plain CSS colours reach a style attribute.
    private static func designColour(_ value: Any?, fallback: String) -> String {
        guard let raw = value as? String, !raw.isEmpty else { return fallback }
        if let named = designNamedColours[raw] { return named }
        let plain = raw.range(of: #"^#[0-9a-fA-F]{3,8}$"#, options: .regularExpression) != nil
            || raw.range(of: #"^(rgb|hsl)a?\([\d\s.,%/-]+\)$"#, options: [.regularExpression, .caseInsensitive]) != nil
        return plain ? raw : fallback
    }

    private static func escapeHTML(_ text: String) -> String {
        text.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }
}
