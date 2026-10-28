import Foundation

/// Edits v2 doc / sheet / slides bodies in place and writes them back in
/// the formats the web editors save (allternit-ai
/// src/lib/artifacts/editors/{doc,sheet,slides}-body.ts):
///
/// - doc: `application/vnd.allternit.doc+json` `{title, blocks, markdown}` —
///   paragraph, heading, list-item and table-cell text is editable; images,
///   dividers and cells holding more than one block are kept as they are.
///   `markdown` is regenerated like `documentToMarkdown`. Markdown /
///   plain-text docs stay markdown.
/// - tabbed doc (doc-tabs.ts): `{title, tabs:[{id, title, body}], markdown}`
///   — each tab's body is an ordinary doc body, edited as above, and the
///   whole thing is written back like `writeTabs` (a single tab collapses
///   to that tab's body).
/// - sheet: `application/vnd.allternit.sheet+json` workbook — the first
///   sheet's `cells["row:col"]` plus the top-level `rows` matrix
///   (`serializeSheet`). Other bodies (`{columns, rows}`, arrays, CSV) are
///   converted to a workbook on save, as the web does on first edit.
/// - slides: `application/vnd.allternit.slides+json` decks edit each
///   slide's text blocks and refresh its `title` / `body` (`serializeSlides`);
///   simple `{slides:[{title, bullets, body}]}` decks edit those fields.
///
/// Everything the phone doesn't edit (styles, positions, other sheets,
/// unknown keys) is carried through untouched.
enum ArtifactBodyEditing {
    static let docFormat = "application/vnd.allternit.doc+json"
    static let sheetFormat = "application/vnd.allternit.sheet+json"
    static let slidesFormat = "application/vnd.allternit.slides+json"

    // MARK: Models

    struct DocBlock: Identifiable, Equatable, Sendable {
        enum Role: Equatable, Sendable {
            case paragraph
            case heading(Int)
            case listItem(numbered: Bool, number: Int)
            /// A table cell (`path` = [block, row, cell]). Cells that hold
            /// anything but one paragraph/heading show their text read-only.
            case tableCell(row: Int, column: Int, editable: Bool)
            /// Not editable on the phone (image, divider…): shown as a label.
            case fixed(String)
        }
        let id: String
        /// Index into `blocks`, then into the list's `items` for list items.
        let path: [Int]
        let role: Role
        let original: String
        var text: String
    }

    /// One tab of a tabbed doc. `model` is nil when the phone can't edit
    /// that tab's body (it's kept as it is, but can still be renamed or
    /// deleted). A tab added here has an empty `originalTitle`.
    struct DocTab: Identifiable, Equatable, Sendable {
        let id: String
        var title: String
        let originalTitle: String
        let originalBody: String
        var model: Model?

        var hasChanges: Bool { title != originalTitle || (model?.hasChanges ?? false) }

        /// doc-tabs.ts addTab: `Tab N`, an empty Markdown body.
        static func added(after tabs: [DocTab]) -> DocTab {
            DocTab(id: newTabId(tabs), title: "Tab \(tabs.count + 1)", originalTitle: "",
                   originalBody: "", model: .markdown("", original: ""))
        }

        /// doc-tabs.ts newTabId.
        static func newTabId(_ tabs: [DocTab]) -> String {
            var n = tabs.count + 1
            while tabs.contains(where: { $0.id == "t\(n)" }) { n += 1 }
            return "t\(n)"
        }
    }

    struct SheetGrid: Equatable, Sendable {
        /// The cells as typed: a formula shows as `=…`.
        var rows: [[String]]
        let original: [[String]]
        /// True when the body is already the editor's workbook.
        let isWorkbook: Bool

        var columnCount: Int { rows.map(\.count).max() ?? 0 }
    }

    struct SlideText: Identifiable, Equatable, Sendable {
        let id: String
        let blockIndex: Int
        let original: String
        var text: String
    }

    struct BlockSlide: Identifiable, Equatable, Sendable {
        let id: String
        let index: Int
        var texts: [SlideText]
    }

    struct SimpleSlide: Identifiable, Equatable, Sendable {
        let id: String
        let index: Int
        var title: String
        /// One bullet per line.
        var bullets: String
        var body: String
        let original: [String]
    }

    enum Model: Equatable, Sendable {
        case markdown(String, original: String)
        case docBlocks([DocBlock])
        case docMarkdownField(String, original: String)
        /// `originalIds` = the saved tab order, so adding, deleting or
        /// reordering a tab counts as a change. Empty when the phone just
        /// turned a plain doc into tabs.
        case docTabs([DocTab], originalIds: [String])
        case sheet(SheetGrid)
        case blockSlides([BlockSlide])
        case simpleSlides([SimpleSlide])

        var hasChanges: Bool {
            switch self {
            case .markdown(let text, let original), .docMarkdownField(let text, let original):
                return text != original
            case .docBlocks(let blocks):
                return blocks.contains { $0.text != $0.original }
            case .docTabs(let tabs, let originalIds):
                return tabs.map(\.id) != originalIds || tabs.contains(where: \.hasChanges)
            case .sheet(let grid):
                return grid.rows != grid.original
            case .blockSlides(let slides):
                return slides.contains { $0.texts.contains { $0.text != $0.original } }
            case .simpleSlides(let slides):
                return slides.contains { [$0.title, $0.bullets, $0.body] != $0.original }
            }
        }
    }

    enum EditError: LocalizedError {
        case unreadable
        var errorDescription: String? { "This artifact changed shape while you edited it. Reload and try again." }
    }

    /// Kinds the phone edits.
    static func isEditable(kind: String) -> Bool {
        ["doc", "sheet", "slides"].contains(kind)
    }

    // MARK: JSON helpers

    private static func parse(_ body: String) -> Any? {
        try? JSONSerialization.jsonObject(with: Data(body.utf8), options: [.fragmentsAllowed])
    }

    private static func string(_ object: Any) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes])
        return String(decoding: data, as: UTF8.self)
    }

    private static func cellText(_ value: Any?) -> String {
        guard let value else { return "" }
        switch value {
        case is NSNull: return ""
        case let s as String: return s
        case let n as NSNumber:
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue ? "TRUE" : "FALSE" }
            return n.stringValue
        default: return String(describing: value)
        }
    }

    // MARK: Build

    /// The editable model for a body, or nil when the phone can't edit it,
    /// which keeps the viewer read-only.
    static func model(kind: String, body: String, format: String) -> Model? {
        switch kind {
        case "doc": return docModel(body: body, format: format)
        case "sheet": return sheetModel(body: body, format: format)
        case "slides": return slidesModel(body: body)
        default: return nil
        }
    }

    private static func runsText(_ content: Any?) -> String {
        ((content as? [Any]) ?? []).map { cellText(($0 as? [String: Any])?["text"]) }.joined()
    }

    /// The web's "Add tab" on a plain doc: the whole doc becomes Tab 1
    /// (edits kept) and an empty Tab 2 follows. nil for models that aren't
    /// a single doc body.
    static func addingFirstTab(to model: Model, originalBody: String) -> Model? {
        switch model {
        case .markdown, .docMarkdownField, .docBlocks:
            let first = DocTab(id: "t1", title: "Tab 1", originalTitle: "Tab 1", originalBody: originalBody, model: model)
            return .docTabs([first, DocTab.added(after: [first])], originalIds: [])
        default:
            return nil
        }
    }

    /// A tab body's own format (doc-tabs.ts `formatOf`).
    static func tabFormat(_ body: String) -> String {
        body.drop(while: { $0.isWhitespace }).hasPrefix("{") ? docFormat : "text/markdown"
    }

    /// Port of doc-tabs.ts `readTabs` (tabs only — a plain doc returns nil).
    private static func readTabs(_ root: [String: Any]) -> [(id: String, title: String, body: String)]? {
        guard let raw = root["tabs"] as? [Any], !raw.isEmpty else { return nil }
        return raw.enumerated().map { i, item in
            let t = item as? [String: Any] ?? [:]
            let body: String
            if let b = t["body"] as? String {
                body = b
            } else if let md = t["markdown"] as? String {
                body = md
            } else {
                let doc = t["doc"] ?? [String: Any]()
                body = JSONSerialization.isValidJSONObject(doc) ? ((try? string(doc)) ?? "{}") : "{}"
            }
            let id = (t["id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "t\(i + 1)"
            let title = (t["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Tab \(i + 1)"
            return (id, title, body)
        }
    }

    /// A table cell's text when it is one paragraph/heading (or empty).
    private static func editableCellText(_ cellBlocks: [Any]) -> String? {
        if cellBlocks.isEmpty { return "" }
        guard cellBlocks.count == 1, let only = cellBlocks[0] as? [String: Any],
              ["paragraph", "heading"].contains(only["type"] as? String ?? "") else { return nil }
        return runsText(only["content"])
    }

    private static func docModel(body: String, format: String, allowTabs: Bool = true) -> Model? {
        if format == "text/markdown" || format == "text/plain" { return .markdown(body, original: body) }
        guard let root = parse(body) as? [String: Any] else {
            return format.hasPrefix("text/") ? .markdown(body, original: body) : nil
        }
        // Tabs first, like the web's DocTabsEditor (readTabs before parseDocBody).
        if allowTabs, let tabs = readTabs(root) {
            return .docTabs(tabs.map { tab in
                DocTab(id: tab.id, title: tab.title, originalTitle: tab.title, originalBody: tab.body,
                       model: docModel(body: tab.body, format: tabFormat(tab.body), allowTabs: false))
            }, originalIds: tabs.map(\.id))
        }
        if let blocks = root["blocks"] as? [Any] {
            var out: [DocBlock] = []
            for (i, raw) in blocks.enumerated() {
                let block = raw as? [String: Any] ?? [:]
                switch block["type"] as? String {
                case "paragraph":
                    let text = runsText(block["content"])
                    out.append(DocBlock(id: "\(i)", path: [i], role: .paragraph, original: text, text: text))
                case "heading":
                    let text = runsText(block["content"])
                    let level = (block["level"] as? Int) ?? 1
                    out.append(DocBlock(id: "\(i)", path: [i], role: .heading(level), original: text, text: text))
                case "list":
                    let numbered = (block["style"] as? String) == "numbered"
                    for (j, itemRaw) in ((block["items"] as? [Any]) ?? []).enumerated() {
                        let item = itemRaw as? [String: Any] ?? [:]
                        if item["type"] as? String == "paragraph" || item["type"] as? String == "heading" {
                            let text = runsText(item["content"])
                            out.append(DocBlock(id: "\(i).\(j)", path: [i, j], role: .listItem(numbered: numbered, number: j + 1), original: text, text: text))
                        } else {
                            out.append(DocBlock(id: "\(i).\(j)", path: [i, j], role: .fixed("Nested item"), original: "", text: ""))
                        }
                    }
                case "table":
                    var cellCount = 0
                    for (r, rowRaw) in ((block["rows"] as? [Any]) ?? []).enumerated() {
                        for (c, cellRaw) in (((rowRaw as? [String: Any])?["cells"] as? [Any]) ?? []).enumerated() {
                            let cell = cellRaw as? [String: Any]
                            let cellBlocks = (cell?["blocks"] as? [Any]) ?? []
                            let editable = cell != nil ? editableCellText(cellBlocks) : nil
                            let text = editable ?? cellBlocks.map(blockMarkdown).joined(separator: " ")
                            out.append(DocBlock(id: "\(i).r\(r).c\(c)", path: [i, r, c],
                                                role: .tableCell(row: r, column: c, editable: editable != nil),
                                                original: text, text: text))
                            cellCount += 1
                        }
                    }
                    if cellCount == 0 { out.append(DocBlock(id: "\(i)", path: [i], role: .fixed("Table"), original: "", text: "")) }
                case "image": out.append(DocBlock(id: "\(i)", path: [i], role: .fixed("Image"), original: "", text: ""))
                case "divider": out.append(DocBlock(id: "\(i)", path: [i], role: .fixed("Divider"), original: "", text: ""))
                default: out.append(DocBlock(id: "\(i)", path: [i], role: .fixed("Block"), original: "", text: ""))
                }
            }
            return .docBlocks(out)
        }
        // A tab whose body is itself tabbed: the web doesn't nest tabs either.
        if root["tabs"] != nil { return nil }
        if let markdown = root["markdown"] as? String { return .docMarkdownField(markdown, original: markdown) }
        return nil
    }

    private static func sheetModel(body: String, format: String) -> Model? {
        if let root = parse(body) as? [String: Any],
           let first = (root["sheets"] as? [Any])?.first as? [String: Any],
           let cells = first["cells"] as? [String: Any] {
            var maxRow = -1, maxCol = -1
            for key in cells.keys {
                let parts = key.split(separator: ":").compactMap { Int($0) }
                guard parts.count == 2 else { continue }
                maxRow = max(maxRow, parts[0]); maxCol = max(maxCol, parts[1])
            }
            let rows: [[String]] = maxRow < 0 ? [] : (0...maxRow).map { r in
                (0...max(maxCol, 0)).map { c in
                    guard let cell = cells["\(r):\(c)"] as? [String: Any] else { return "" }
                    if let formula = cell["formula"] as? String, !formula.isEmpty {
                        return formula.hasPrefix("=") ? formula : "=\(formula)"
                    }
                    return cellText(cell["value"])
                }
            }
            let grid = rows.isEmpty ? [Array(repeating: "", count: 3)] : rows
            return .sheet(SheetGrid(rows: grid, original: grid, isWorkbook: true))
        }
        guard let decoded = ArtifactBodyDecoder.sheetGrid(body: body, format: format) else {
            return body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? .sheet(SheetGrid(rows: [["", "", ""]], original: [["", "", ""]], isWorkbook: false))
                : nil
        }
        var rows = decoded.rows
        if !decoded.columns.isEmpty { rows.insert(decoded.columns, at: 0) }
        if rows.isEmpty { rows = [["", "", ""]] }
        let width = rows.map(\.count).max() ?? 0
        rows = rows.map { $0 + Array(repeating: "", count: width - $0.count) }
        return .sheet(SheetGrid(rows: rows, original: rows, isWorkbook: false))
    }

    private static func sortedTextBlocks(_ blocks: [Any]) -> [(Int, [String: Any])] {
        blocks.enumerated()
            .compactMap { i, raw -> (Int, [String: Any])? in
                guard let b = raw as? [String: Any], b["type"] as? String == "text" else { return nil }
                return (i, b)
            }
            .sorted { lhs, rhs in
                let ly = (lhs.1["y"] as? Double) ?? 0, ry = (rhs.1["y"] as? Double) ?? 0
                if ly == ry { return ((lhs.1["x"] as? Double) ?? 0) < ((rhs.1["x"] as? Double) ?? 0) }
                return ly < ry
            }
    }

    private static func slidesModel(body: String) -> Model? {
        guard let root = parse(body) else { return nil }
        guard let list = (root as? [Any]) ?? ((root as? [String: Any])?["slides"] as? [Any]) else { return nil }
        let dicts = list.map { $0 as? [String: Any] ?? [:] }
        if dicts.contains(where: { $0["blocks"] is [Any] }) {
            return .blockSlides(dicts.enumerated().map { i, slide in
                let texts = sortedTextBlocks(slide["blocks"] as? [Any] ?? []).map { index, block in
                    let text = cellText(block["text"])
                    return SlideText(id: "\(i).\(index)", blockIndex: index, original: text, text: text)
                }
                return BlockSlide(id: "\(i)", index: i, texts: texts)
            })
        }
        return .simpleSlides(dicts.enumerated().map { i, slide in
            let title = cellText(slide["title"])
            let bullets = ((slide["bullets"] as? [Any]) ?? []).map { cellText($0) }.joined(separator: "\n")
            let text = cellText(slide["body"] ?? slide["content"])
            return SimpleSlide(id: "\(i)", index: i, title: title, bullets: bullets, body: text, original: [title, bullets, text])
        })
    }

    // MARK: Serialize

    /// The new body and format for an edited model.
    static func serialize(_ model: Model, kind: String, originalBody: String, originalFormat: String, title: String) throws -> (body: String, format: String) {
        switch model {
        case .markdown(let text, _):
            return (text, originalFormat.hasPrefix("text/") ? originalFormat : "text/markdown")
        case .docMarkdownField(let text, _):
            guard var root = parse(originalBody) as? [String: Any] else { throw EditError.unreadable }
            root["markdown"] = text
            return (try string(root), originalFormat)
        case .docBlocks(let edits):
            return (try serializeDoc(edits, originalBody: originalBody, title: title), docFormat)
        case .docTabs(let tabs, _):
            return try serializeTabs(tabs, title: title)
        case .sheet(let grid):
            return (try serializeSheet(grid, originalBody: originalBody, title: title), sheetFormat)
        case .blockSlides(let slides):
            return (try serializeBlockSlides(slides, originalBody: originalBody), slidesFormat)
        case .simpleSlides(let slides):
            return (try serializeSimpleSlides(slides, originalBody: originalBody), originalFormat)
        }
    }

    /// A run list for edited text: the block's marks survive when every run
    /// shared them; mixed formatting becomes plain text for that block.
    private static func runs(for text: String, replacing content: Any?) -> [[String: Any]] {
        let old = ((content as? [Any]) ?? []).compactMap { $0 as? [String: Any] }
        let markKeys = ["bold", "italic", "underline", "strike", "code", "color", "highlight"]
        var run: [String: Any] = ["text": text]
        if let first = old.first {
            let shared = old.allSatisfy { other in
                markKeys.allSatisfy { key in
                    String(describing: other[key] ?? "") == String(describing: first[key] ?? "")
                }
            }
            if shared { for key in markKeys { if let v = first[key] { run[key] = v } } }
        }
        return [run]
    }

    private static func serializeDoc(_ edits: [DocBlock], originalBody: String, title: String) throws -> String {
        guard var root = parse(originalBody) as? [String: Any], var blocks = root["blocks"] as? [Any] else {
            throw EditError.unreadable
        }
        for edit in edits where edit.text != edit.original {
            if edit.path.count == 1, var block = blocks[safe: edit.path[0]] as? [String: Any] {
                block["content"] = runs(for: edit.text, replacing: block["content"])
                blocks[edit.path[0]] = block
            } else if edit.path.count == 3, var table = blocks[safe: edit.path[0]] as? [String: Any],
                      var rows = table["rows"] as? [Any], var row = rows[safe: edit.path[1]] as? [String: Any],
                      var cells = row["cells"] as? [Any], var cell = cells[safe: edit.path[2]] as? [String: Any] {
                var cellBlocks = (cell["blocks"] as? [Any]) ?? []
                if var first = cellBlocks.first as? [String: Any] {
                    first["content"] = runs(for: edit.text, replacing: first["content"])
                    cellBlocks[0] = first
                } else {
                    cellBlocks = [["type": "paragraph", "content": [["text": edit.text]]]]
                }
                cell["blocks"] = cellBlocks
                cells[edit.path[2]] = cell
                row["cells"] = cells
                rows[edit.path[1]] = row
                table["rows"] = rows
                blocks[edit.path[0]] = table
            } else if edit.path.count == 2, var list = blocks[safe: edit.path[0]] as? [String: Any],
                      var items = list["items"] as? [Any], var item = items[safe: edit.path[1]] as? [String: Any] {
                item["content"] = runs(for: edit.text, replacing: item["content"])
                items[edit.path[1]] = item
                list["items"] = items
                blocks[edit.path[0]] = list
            }
        }
        root["blocks"] = blocks
        if root["title"] == nil { root["title"] = title }
        root["markdown"] = blocks.map(blockMarkdown).filter { !$0.isEmpty }.joined(separator: "\n\n")
        return try string(root)
    }

    /// Port of doc-tabs.ts `writeTabs`: one tab collapses to its own body;
    /// several become `{title, tabs:[{id, title, body}], markdown}` with every
    /// tab joined under its title in `markdown`.
    private static func serializeTabs(_ tabs: [DocTab], title: String) throws -> (body: String, format: String) {
        var written: [(id: String, title: String, body: String)] = []
        for tab in tabs {
            var body = tab.originalBody
            if let model = tab.model, model.hasChanges {
                body = try serialize(model, kind: "doc", originalBody: tab.originalBody,
                                     originalFormat: tabFormat(tab.originalBody), title: tab.title).body
            }
            written.append((tab.id, tab.title, body))
        }
        if written.count == 1, let only = written.first { return (only.body, tabFormat(only.body)) }
        let markdown = written.map { tab in
            "# \(tab.title)\n\n\(ArtifactBodyDecoder.docMarkdown(body: tab.body, format: tabFormat(tab.body)) ?? tab.body)"
        }.joined(separator: "\n\n")
        let root: [String: Any] = [
            "title": title,
            "tabs": written.map { ["id": $0.id, "title": $0.title, "body": $0.body] },
            "markdown": markdown,
        ]
        return (try string(root), docFormat)
    }

    // Port of file-io.ts runsToMarkdown / blockToMarkdown.
    private static func runsMarkdown(_ content: Any?) -> String {
        ((content as? [Any]) ?? []).compactMap { $0 as? [String: Any] }.map { run in
            var text = cellText(run["text"])
            if run["code"] as? Bool == true { text = "`\(text.replacingOccurrences(of: "`", with: "\\`"))`" }
            if run["strike"] as? Bool == true { text = "~~\(text)~~" }
            if run["italic"] as? Bool == true { text = "_\(text)_" }
            if run["bold"] as? Bool == true { text = "**\(text)**" }
            return text
        }.joined()
    }

    private static func blockMarkdown(_ raw: Any) -> String {
        guard let block = raw as? [String: Any] else { return "" }
        switch block["type"] as? String {
        case "paragraph":
            return runsMarkdown(block["content"])
        case "heading":
            let level = max(1, min(6, (block["level"] as? Int) ?? 1))
            return String(repeating: "#", count: level) + " " + runsMarkdown(block["content"])
        case "list":
            let numbered = (block["style"] as? String) == "numbered"
            return ((block["items"] as? [Any]) ?? []).enumerated().map { index, item in
                (numbered ? "\(index + 1). " : "- ") + blockMarkdown(item).replacingOccurrences(of: "\n", with: "\n  ")
            }.joined(separator: "\n")
        case "table":
            let rows = ((block["rows"] as? [Any]) ?? []).map { rowRaw -> [String] in
                (((rowRaw as? [String: Any])?["cells"] as? [Any]) ?? []).map { cellRaw in
                    let text = (((cellRaw as? [String: Any])?["blocks"] as? [Any]) ?? []).map(blockMarkdown).joined(separator: " ")
                        .replacingOccurrences(of: "|", with: "\\|")
                    return text.isEmpty ? " " : text
                }
            }
            guard let head = rows.first else { return "" }
            let widths = head.indices.map { i in max(3, rows.map { $0[safe: i]?.count ?? 0 }.max() ?? 0) }
            func pad(_ row: [String]) -> String {
                row.enumerated().map { i, cell in
                    cell.padding(toLength: max(cell.count, widths[safe: i] ?? cell.count), withPad: " ", startingAt: 0)
                }.joined(separator: " | ")
            }
            let divider = widths.map { String(repeating: "-", count: $0) }.joined(separator: " | ")
            return ([pad(head), divider] + rows.dropFirst().map(pad)).joined(separator: "\n")
        case "image":
            return "![\(cellText(block["alt"]))](\(cellText(block["src"])))"
        case "divider":
            return "---"
        default:
            return ""
        }
    }

    /// Port of sheet-body.ts `cellFromValue`.
    private static func cell(from text: String) -> [String: Any]? {
        if text.isEmpty { return nil }
        if text.hasPrefix("=") { return ["formula": text] }
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty, let number = Double(trimmed), number.isFinite {
            if number.rounded() == number, abs(number) < 1e15 { return ["value": Int(number)] }
            return ["value": number]
        }
        return ["value": text]
    }

    private static func serializeSheet(_ grid: SheetGrid, originalBody: String, title: String) throws -> String {
        var root: [String: Any]
        var sheets: [Any]
        var first: [String: Any]
        var cells: [String: Any]
        if grid.isWorkbook {
            guard let parsed = parse(originalBody) as? [String: Any],
                  let parsedSheets = parsed["sheets"] as? [Any],
                  let parsedFirst = parsedSheets.first as? [String: Any] else { throw EditError.unreadable }
            root = parsed
            sheets = parsedSheets
            first = parsedFirst
            cells = parsedFirst["cells"] as? [String: Any] ?? [:]
            // Only cells the user changed are rewritten, so cached formula
            // values and styles on the rest survive.
            let height = max(grid.rows.count, grid.original.count)
            for r in 0..<height {
                let width = max(grid.rows[safe: r]?.count ?? 0, grid.original[safe: r]?.count ?? 0)
                for c in 0..<width {
                    let now = grid.rows[safe: r]?[safe: c] ?? ""
                    let before = grid.original[safe: r]?[safe: c] ?? ""
                    guard now != before else { continue }
                    let key = "\(r):\(c)"
                    if let next = cell(from: now) {
                        var merged = (cells[key] as? [String: Any]) ?? [:]
                        merged.removeValue(forKey: "value")
                        merged.removeValue(forKey: "formula")
                        for (k, v) in next { merged[k] = v }
                        cells[key] = merged
                    } else {
                        cells.removeValue(forKey: key)
                    }
                }
            }
        } else {
            root = ["name": title]
            first = ["id": "Sheet1", "name": "Sheet1", "merges": [Any](), "columnWidths": [String: Any](), "rowHeights": [String: Any]()]
            sheets = [first]
            cells = [:]
            for (r, row) in grid.rows.enumerated() {
                for (c, text) in row.enumerated() {
                    if let next = cell(from: text) { cells["\(r):\(c)"] = next }
                }
            }
        }
        first["cells"] = cells
        sheets[0] = first
        root["sheets"] = sheets
        if root["name"] == nil { root["name"] = title }
        root["rows"] = matrix(cells)
        return try string(root)
    }

    /// Port of `sheetToMatrix`: values, `=FORMULA` for formula cells with no cached value.
    private static func matrix(_ cells: [String: Any]) -> [[Any]] {
        var maxRow = -1, maxCol = -1
        for key in cells.keys {
            let parts = key.split(separator: ":").compactMap { Int($0) }
            guard parts.count == 2 else { continue }
            maxRow = max(maxRow, parts[0]); maxCol = max(maxCol, parts[1])
        }
        guard maxRow >= 0, maxCol >= 0 else { return [] }
        return (0...maxRow).map { r in
            (0...maxCol).map { c -> Any in
                guard let cell = cells["\(r):\(c)"] as? [String: Any] else { return "" }
                if let value = cell["value"], !(value is NSNull) { return value }
                if let formula = cell["formula"] as? String, !formula.isEmpty { return formula.hasPrefix("=") ? formula : "=\(formula)" }
                return ""
            }
        }
    }

    private static func serializeBlockSlides(_ slides: [BlockSlide], originalBody: String) throws -> String {
        guard let parsed = parse(originalBody) else { throw EditError.unreadable }
        var root: [String: Any] = (parsed as? [String: Any]) ?? [:]
        guard var list = (parsed as? [Any]) ?? (root["slides"] as? [Any]) else { throw EditError.unreadable }
        for edit in slides {
            guard var slide = list[safe: edit.index] as? [String: Any], var blocks = slide["blocks"] as? [Any] else { continue }
            for text in edit.texts where text.text != text.original {
                if var block = blocks[safe: text.blockIndex] as? [String: Any] {
                    block["text"] = text.text
                    blocks[text.blockIndex] = block
                }
            }
            slide["blocks"] = blocks
            // serializeSlides: each slide carries title/body text from its blocks.
            let texts = sortedTextBlocks(blocks).map { cellText($0.1["text"]) }
            slide["title"] = texts.first ?? ""
            slide["body"] = texts.dropFirst().filter { !$0.isEmpty }.joined(separator: "\n")
            list[edit.index] = slide
        }
        root["slides"] = list
        return try string(root)
    }

    private static func serializeSimpleSlides(_ slides: [SimpleSlide], originalBody: String) throws -> String {
        guard let parsed = parse(originalBody) else { throw EditError.unreadable }
        let isArray = parsed is [Any]
        var root: [String: Any] = (parsed as? [String: Any]) ?? [:]
        guard var list = (parsed as? [Any]) ?? (root["slides"] as? [Any]) else { throw EditError.unreadable }
        for edit in slides where [edit.title, edit.bullets, edit.body] != edit.original {
            var slide = (list[safe: edit.index] as? [String: Any]) ?? [:]
            slide["title"] = edit.title
            let lines = edit.bullets.split(whereSeparator: \.isNewline).map { String($0).trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            if slide["bullets"] != nil || !lines.isEmpty { slide["bullets"] = lines }
            if slide["content"] != nil && slide["body"] == nil { slide["content"] = edit.body } else { slide["body"] = edit.body }
            if edit.index < list.count { list[edit.index] = slide }
        }
        if isArray { return try string(list) }
        root["slides"] = list
        return try string(root)
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
