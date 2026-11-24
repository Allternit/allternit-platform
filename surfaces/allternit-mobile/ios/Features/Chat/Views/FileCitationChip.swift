import SwiftUI
import QuickLook
import MarkdownView

/// A file-line citation in an assistant reply: `[[cite:<ref>#L<start>-<end>]]`
/// (or `#L<n>` for one line). The ref may hold colons (`file:msg:<id>:<n>`,
/// `file:upload:<id>`, `file:drive:<id>`, `out:…`, `mcp:…`). The reply never
/// shows the token: `markdown(_:isStreaming:)` turns each one into an
/// `allternit-cite:` link that `FileCitationLinkRenderer` draws as a chip.
struct FileCitation: Identifiable, Hashable, Sendable {
    let ref: String
    let start: Int
    let end: Int?

    var id: String { "\(ref)#\(start)-\(end ?? start)" }

    /// `L10-20`, or `L10` for one line.
    var lines: String {
        if let end, end != start { return "L\(start)-\(end)" }
        return "L\(start)"
    }

    var spokenLines: String {
        if let end, end != start { return "lines \(start) to \(end)" }
        return "line \(start)"
    }

    /// The name, shortened in the middle so a chip stays one short run.
    var shortName: String {
        let full = name
        guard full.count > 28 else { return full }
        return String(full.prefix(16)) + "…" + String(full.suffix(10))
    }

    /// The file name for the chip: the attachment's name when this phone has
    /// seen it, else the last piece of the ref.
    var name: String { CitationAttachments.name(for: ref) }

    static let scheme = "allternit-cite"

    /// G6-2 token: group 1 ref, group 2 start, group 3 end (absent for one line).
    nonisolated(unsafe) static let token = try! NSRegularExpression(
        pattern: #"\[\[cite:([^\]\s#]+)#L(\d+)(?:-L?(\d+))?\]\]"#
    )
    /// A token still being streamed at the very end of the text.
    nonisolated(unsafe) private static let partialTail = try! NSRegularExpression(
        pattern: #"\[\[(?:c(?:i(?:t(?:e(?::[^\]\n]*\]?)?)?)?)?)?$"#
    )

    /// The text with each token as a markdown link the chip renderer draws.
    /// While streaming, a token cut off at the end is held back.
    static func markdown(_ text: String, isStreaming: Bool) -> String {
        replace(text, isStreaming: isStreaming) { citation in
            "[\(escape(citation.name)) \(citation.lines)](\(citation.url.absoluteString))"
        }
    }

    /// The text with each token as `name L10-20` (copy, share, speech).
    static func plainText(_ text: String) -> String {
        replace(text, isStreaming: false) { "\($0.name) \($0.lines)" }
    }

    var url: URL {
        var components = URLComponents()
        components.scheme = Self.scheme
        components.host = "open"
        components.queryItems = [
            URLQueryItem(name: "ref", value: ref),
            URLQueryItem(name: "l", value: end.map { "\(start)-\($0)" } ?? "\(start)"),
        ]
        // Markdown link targets can't hold spaces or bare parentheses.
        let query = (components.percentEncodedQuery ?? "")
            .replacingOccurrences(of: "(", with: "%28")
            .replacingOccurrences(of: ")", with: "%29")
        return URL(string: "\(Self.scheme)://open?\(query)")!
    }

    init(ref: String, start: Int, end: Int?) {
        self.ref = ref
        self.start = start
        self.end = end
    }

    init?(url: URL) {
        guard url.scheme == Self.scheme,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let ref = items.first(where: { $0.name == "ref" })?.value, !ref.isEmpty,
              let lines = items.first(where: { $0.name == "l" })?.value
        else { return nil }
        let bounds = lines.split(separator: "-").compactMap { Int($0) }
        guard let start = bounds.first else { return nil }
        self.init(ref: ref, start: start, end: bounds.count > 1 ? bounds[1] : nil)
    }

    // MARK: - Private

    private static func matches(in text: String) -> [(range: Range<String.Index>, citation: FileCitation)] {
        let ns = text as NSString
        return token.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { m in
            guard let range = Range(m.range, in: text),
                  let start = Int(ns.substring(with: m.range(at: 2)))
            else { return nil }
            let ref = ns.substring(with: m.range(at: 1))
            let end = m.range(at: 3).location == NSNotFound ? nil : Int(ns.substring(with: m.range(at: 3)))
            return (range: range, citation: FileCitation(ref: ref, start: start, end: end))
        }
    }

    private static func replace(_ text: String, isStreaming: Bool, with make: (FileCitation) -> String) -> String {
        guard text.contains("[[") else { return text }
        var out = ""
        var cursor = text.startIndex
        for match in matches(in: text) {
            out += text[cursor..<match.range.lowerBound]
            out += make(match.citation)
            cursor = match.range.upperBound
        }
        out += text[cursor...]
        guard isStreaming else { return out }
        let ns = out as NSString
        if let tail = partialTail.firstMatch(in: out, range: NSRange(location: 0, length: ns.length)) {
            return ns.substring(to: tail.range.location)
        }
        return out
    }

    /// Backslash-escapes the characters that would break a link label.
    private static func escape(_ name: String) -> String {
        var out = ""
        for ch in name {
            if "\\[]*_`<>".contains(ch) { out.append("\\") }
            out.append(ch)
        }
        return out
    }
}

// MARK: - Attachment lookup

/// Chat attachments by message and part index, recorded as history loads
/// (`ChatViewModel.mapRecord`), so `file:msg:<id>:<n>` and `file:upload:<id>`
/// refs find a name and something Quick Look can open.
enum CitationAttachments {
    struct Attachment: Sendable, Equatable {
        let name: String
        /// A `data:` URL (web sends) or `/api/v1/uploads/<id>` (phone sends).
        let url: String
    }

    /// Where a cited file can be fetched from.
    enum Source: Sendable {
        case upload(id: String, name: String)
        case data(url: String, name: String)
        case drive(id: String, name: String)
    }

    nonisolated(unsafe) private static let lock = NSLock()
    nonisolated(unsafe) private static var parts: [String: [Int: Attachment]] = [:]
    nonisolated(unsafe) private static var fileOrder: [String: [Attachment]] = [:]
    nonisolated(unsafe) private static var uploads: [String: Attachment] = [:]

    static func record(messageId: String, parts list: [AgentSessionMessage.Part]) {
        var byIndex: [Int: Attachment] = [:]
        var ordered: [Attachment] = []
        for (index, part) in list.enumerated() where part.type == "file" {
            guard let url = part.url, !url.isEmpty else { continue }
            let name = part.filename
                ?? (url.hasPrefix("data:") ? nil : url.split(separator: "/").last.map(String.init))
                ?? "Attachment"
            let attachment = Attachment(name: name, url: url)
            byIndex[index] = attachment
            ordered.append(attachment)
        }
        guard !ordered.isEmpty else { return }
        lock.lock(); defer { lock.unlock() }
        parts[messageId] = byIndex
        fileOrder[messageId] = ordered
        for attachment in ordered {
            if let id = uploadId(in: attachment.url) { uploads[id] = attachment }
        }
    }

    static func name(for ref: String) -> String {
        if let attachment = attachment(for: ref) { return attachment.name }
        let pieces = ref.split(separator: ":", omittingEmptySubsequences: true)
        switch pieces.first {
        case "file":
            return pieces.count > 1 && pieces[1] == "drive" ? "File" : "Attachment"
        default:
            let tail = ref.split(whereSeparator: { $0 == "/" || $0 == ":" }).last.map(String.init)
            return tail ?? ref
        }
    }

    static func source(for ref: String) -> Source? {
        let pieces = ref.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        if pieces.count >= 3, pieces[0] == "file", pieces[1] == "drive" {
            return .drive(id: pieces[2...].joined(separator: ":"), name: name(for: ref))
        }
        guard let attachment = attachment(for: ref) else {
            if pieces.count >= 3, pieces[0] == "file", pieces[1] == "upload" {
                return .upload(id: pieces[2...].joined(separator: ":"), name: "Attachment")
            }
            return nil
        }
        if attachment.url.hasPrefix("data:") { return .data(url: attachment.url, name: attachment.name) }
        if let id = uploadId(in: attachment.url) { return .upload(id: id, name: attachment.name) }
        return nil
    }

    private static func attachment(for ref: String) -> Attachment? {
        let pieces = ref.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        guard pieces.count >= 3, pieces[0] == "file" else { return nil }
        lock.lock(); defer { lock.unlock() }
        switch pieces[1] {
        case "msg":
            guard pieces.count >= 4, let index = Int(pieces[pieces.count - 1]) else { return nil }
            let messageId = pieces[2..<(pieces.count - 1)].joined(separator: ":")
            if let hit = parts[messageId]?[index] { return hit }
            // The index may count only the message's files.
            if let files = fileOrder[messageId], files.indices.contains(index) { return files[index] }
            return nil
        case "upload":
            return uploads[pieces[2...].joined(separator: ":")]
        default:
            return nil
        }
    }

    private static func uploadId(in url: String) -> String? {
        guard let range = url.range(of: "/uploads/") else { return nil }
        let id = url[range.upperBound...].split(separator: "?").first.map(String.init) ?? ""
        return id.isEmpty ? nil : id.removingPercentEncoding ?? id
    }
}

// MARK: - Views

/// Draws `allternit-cite:` links as chips (register with
/// `.markdownElementRenderer(.link(FileCitationLinkRenderer(), urlScheme: FileCitation.scheme))`).
struct FileCitationLinkRenderer: MarkdownLinkRenderer {
    @ViewBuilder
    func makeBody(configuration: MarkdownLinkRendererConfiguration) -> some View {
        if let citation = FileCitation(url: configuration.url) {
            FileCitationChip(citation: citation)
        } else {
            configuration.label
        }
    }
}

/// The chip: file name plus `L10-20`. Tapping asks the nearest
/// `fileCitationHost()` to open it.
struct FileCitationChip: View {
    let citation: FileCitation

    @Environment(\.openFileCitation) private var openCitation

    var body: some View {
        Button {
            openCitation?.open(citation)
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "doc.text")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(Color("AccentPrimary"))
                Text(citation.shortName)
                    .lineLimit(1)
                Text(citation.lines)
                    .monospacedDigit()
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.system(size: 12, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(Capsule().fill(Color("FillNeutral")))
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 0.5))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(citation.name), \(citation.spokenLines)")
        .accessibilityHint("Opens the cited file")
    }
}

/// What a chip calls on tap; set by `fileCitationHost()`.
struct OpenFileCitationAction {
    let open: @MainActor (FileCitation) -> Void
}

private struct OpenFileCitationKey: EnvironmentKey {
    static var defaultValue: OpenFileCitationAction? { nil }
}

extension EnvironmentValues {
    var openFileCitation: OpenFileCitationAction? {
        get { self[OpenFileCitationKey.self] }
        set { self[OpenFileCitationKey.self] = newValue }
    }
}

extension View {
    /// Opens tapped citation chips below this view: an attachment in Quick
    /// Look (the same `quickLookPreview` hook the agent file rows use), any
    /// other ref in a small sheet with the ref and lines.
    func fileCitationHost() -> some View { modifier(FileCitationHost()) }
}

private struct FileCitationHost: ViewModifier {
    @State private var previewURL: URL?
    @State private var detail: FileCitationDetail?
    @State private var opening: String?

    func body(content: Content) -> some View {
        content
            .environment(\.openFileCitation, OpenFileCitationAction { citation in
                guard opening == nil else { return }
                Task { await open(citation) }
            })
            .quickLookPreview($previewURL)
            .sheet(item: $detail) { item in
                FileCitationSheet(detail: item)
                    .presentationDetents([.height(260), .medium])
            }
    }

    @MainActor
    private func open(_ citation: FileCitation) async {
        guard let source = CitationAttachments.source(for: citation.ref) else {
            detail = FileCitationDetail(citation: citation, problem: nil)
            return
        }
        opening = citation.id
        defer { opening = nil }
        if let url = await FileCitationFetch.localCopy(of: source) {
            previewURL = url
        } else {
            detail = FileCitationDetail(citation: citation, problem: "Couldn't open this file right now.")
        }
    }
}

private struct FileCitationDetail: Identifiable {
    let citation: FileCitation
    let problem: String?
    var id: String { citation.id }
}

/// A ref Quick Look can't open: the file, the lines and the ref.
private struct FileCitationSheet: View {
    let detail: FileCitationDetail
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Label {
                    Text(detail.citation.name)
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                        .truncationMode(.middle)
                } icon: {
                    Image(systemName: "doc.text")
                        .foregroundColor(Color("AccentPrimary"))
                }
                Text("Lines \(detail.citation.lines.dropFirst())")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                Text(detail.citation.ref)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                if let problem = detail.problem {
                    Text(problem)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 0)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .navigationTitle("Cited lines")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

/// Gets a cited attachment onto disk for Quick Look.
enum FileCitationFetch {
    @MainActor
    static func localCopy(of source: CitationAttachments.Source) async -> URL? {
        switch source {
        case .drive(let id, _):
            return await FilesDriveQuickLook.localCopy(itemId: id)
        case .upload(let id, let name):
            guard let data = try? await UploadFetcher.data(uploadId: id) else { return nil }
            return write(data, name: name)
        case .data(let url, let name):
            guard let data = decode(dataURL: url) else { return nil }
            return write(data, name: name)
        }
    }

    private static func decode(dataURL: String) -> Data? {
        guard let comma = dataURL.firstIndex(of: ",") else { return nil }
        let header = dataURL[..<comma]
        let payload = String(dataURL[dataURL.index(after: comma)...])
        if header.hasSuffix(";base64") { return Data(base64Encoded: payload, options: .ignoreUnknownCharacters) }
        return payload.removingPercentEncoding?.data(using: .utf8)
    }

    private static func write(_ data: Data, name: String) -> URL? {
        let safe = name.replacingOccurrences(of: "/", with: "-")
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("citations", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let url = folder.appendingPathComponent(safe.isEmpty ? "Attachment" : safe)
            try data.write(to: url, options: .atomic)
            return url
        } catch {
            return nil
        }
    }
}
