import AVFoundation
import ImageIO
import PDFKit
import SwiftUI
import UIKit
import WebKit

/// What an Artifacts card shows: the record's own content (allternit-ai
/// ArtifactCardPreview). The server never fills `thumbnail_url`, and the list
/// endpoint returns summaries, so the body is fetched once a card appears
/// (3 at a time, newest request first, cached per version) and drawn by kind:
///
///  image              the image
///  doc/sheet/slides   first page / grid / slide
///  design/page/card   a script-free snapshot of the page
///  diagram            its SVG, or the mermaid source drawn by mermaid
///  code               its first lines
///  dashboard, ops     its tile or panel titles
///  pdf                the first page
///  video              a frame, when the footage is on the open web
///
/// Anything else (motion needs the web's canvas renderer), and anything that
/// can't be drawn, falls back to `thumbnail_url`, then the kind art.
struct ArtifactCardPreview: View {
    let summary: ArtifactV2Summary

    @State private var preview: ArtifactCardPreviewStore.Preview? = nil
    @State private var pulse = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Color("FillNeutral")
            content
        }
        .clipped()
        .accessibilityHidden(true)
        .task(id: "\(summary.id)@\(summary.currentVersion)") {
            let store = ArtifactCardPreviewStore.shared
            preview = store.cached(summary)
            if preview == nil { preview = await store.preview(for: summary) }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch preview {
        case nil:
            Color("BorderSubtle")
                .opacity(pulse ? 0.45 : 0.15)
                .onAppear {
                    guard !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { pulse = true }
                }
        case .image(let image)?:
            Color.clear.overlay(alignment: .top) {
                Image(uiImage: image).resizable().scaledToFill()
            }
            .clipped()
        case .page(let lines)?:
            ArtifactCardPaper(background: .white) { ArtifactCardPage(lines: lines) }
        case .sheet(let rows)?:
            ArtifactCardPaper(background: .white) { ArtifactCardSheet(rows: rows) }
        case .slide(let lines)?:
            ArtifactCardPaper(background: Color(hex: "#F1F3F4")) { ArtifactCardSlide(lines: lines) }
        case .code(let lines)?:
            ArtifactCardPaper(background: Color(hex: "#18181B")) { ArtifactCardCode(lines: lines) }
        case .dashboard(let titles)?:
            ArtifactCardPaper(background: Color(hex: "#F8F9FA")) { ArtifactCardDashboard(titles: titles) }
        case .art?:
            art
        }
    }

    @ViewBuilder
    private var art: some View {
        if let raw = summary.thumbnailURL, let url = URL(string: raw) {
            AsyncImage(url: url) { phase in
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    kindGlyph
                }
            }
        } else {
            kindGlyph
        }
    }

    private var kindGlyph: some View {
        Image(ArtifactV2Kind.iconAsset(for: summary.kind))
            .resizable()
            .frame(width: 32, height: 32)
    }
}

// MARK: - Paper

/// Lays a page out at 460pt and scales it to the card's width, top-left
/// anchored, so a card shows the top of the real page at any size (Scaled in
/// ArtifactPreview.tsx). The colours are the document's own, in both themes.
private struct ArtifactCardPaper<Content: View>: View {
    let background: Color
    @ViewBuilder let content: () -> Content

    var body: some View {
        GeometryReader { geometry in
            content()
                .frame(width: 460, alignment: .topLeading)
                .scaleEffect(max(geometry.size.width, 1) / 460, anchor: .topLeading)
                .frame(width: geometry.size.width, height: geometry.size.height, alignment: .topLeading)
                .clipped()
        }
        .background(background)
    }
}

private struct ArtifactCardPage: View {
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let first = lines.first {
                Text(first)
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundColor(Color(hex: "#1F2328"))
                    .lineLimit(3)
                    .padding(.bottom, 4)
            }
            ForEach(Array(lines.dropFirst().enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.system(size: 14))
                    .foregroundColor(Color(hex: "#3C4043"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 32)
        .padding(.top, 28)
    }
}

private struct ArtifactCardSheet: View {
    let rows: [[String]]

    private static let columns = 6
    private static let rowCount = 12
    private static let line = Color(hex: "#E0E3E7")
    private static let head = Color(hex: "#F8F9FA")
    private static let muted = Color(hex: "#5F6368")

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                gutter("")
                ForEach(0..<Self.columns, id: \.self) { column in
                    let letter = String(UnicodeScalar(UInt8(65 + column)))
                    Text(letter)
                        .font(.system(size: 12))
                        .foregroundColor(Self.muted)
                        .frame(maxWidth: .infinity, minHeight: 24)
                        .background(Self.head)
                        .overlay(Rectangle().stroke(Self.line, lineWidth: 1))
                }
            }
            ForEach(0..<Self.rowCount, id: \.self) { row in
                HStack(spacing: 0) {
                    gutter("\(row + 1)")
                    ForEach(0..<Self.columns, id: \.self) { column in
                        Text(cell(row, column))
                            .font(.system(size: 13, weight: row == 0 ? .semibold : .regular))
                            .foregroundColor(Color(hex: "#1F2328"))
                            .lineLimit(1)
                            .padding(.horizontal, 6)
                            .frame(maxWidth: .infinity, minHeight: 26, alignment: .leading)
                            .overlay(Rectangle().stroke(Self.line, lineWidth: 1))
                    }
                }
            }
        }
    }

    private func cell(_ row: Int, _ column: Int) -> String {
        guard rows.indices.contains(row), rows[row].indices.contains(column) else { return "" }
        return rows[row][column]
    }

    private func gutter(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(Self.muted)
            .frame(width: 36)
            .frame(minHeight: 24)
            .background(Self.head)
            .overlay(Rectangle().stroke(Self.line, lineWidth: 1))
    }
}

private struct ArtifactCardSlide: View {
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let first = lines.first {
                Text(first)
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundColor(Color(hex: "#1F2328"))
                    .lineLimit(2)
                ForEach(Array(lines.dropFirst().prefix(4).enumerated()), id: \.offset) { _, line in
                    Text(line)
                        .font(.system(size: 17))
                        .foregroundColor(Color(hex: "#5F6368"))
                        .lineLimit(1)
                }
            } else {
                Text("Empty slide")
                    .font(.system(size: 20))
                    .foregroundColor(Color(hex: "#9AA0A6"))
                    .frame(maxWidth: .infinity)
            }
        }
        .padding(.horizontal, 48)
        .frame(width: 400, height: 225, alignment: .leading)
        .background(Color.white)
        .shadow(color: .black.opacity(0.12), radius: 4, y: 2)
        .frame(maxWidth: .infinity)
        .padding(.top, 20)
    }
}

private struct ArtifactCardCode: View {
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                Text(line.isEmpty ? " " : line)
                    .font(.system(size: 14, design: .monospaced))
                    .foregroundColor(Color(hex: "#E4E4E7"))
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(20)
    }
}

private struct ArtifactCardDashboard: View {
    let titles: [String]

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
            ForEach(Array(titles.prefix(6).enumerated()), id: \.offset) { _, title in
                VStack(alignment: .leading, spacing: 12) {
                    Text(title)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color(hex: "#1F2328"))
                        .lineLimit(2)
                    Spacer(minLength: 0)
                    Image(systemName: "chart.bar.xaxis")
                        .font(.system(size: 26))
                        .foregroundColor(Color(hex: "#9AA0A6"))
                }
                .padding(14)
                .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color(hex: "#E0E3E7"), lineWidth: 1))
            }
        }
        .padding(16)
    }
}

// MARK: - Loading

/// At most `limit` holders; the newest waiter goes first, so the cards on
/// screen load before the ones already scrolled past.
@MainActor
private final class ArtifactPreviewGate {
    private var free: Int
    private var waiting: [CheckedContinuation<Void, Never>] = []

    init(limit: Int) { free = limit }

    func acquire() async {
        if free > 0 {
            free -= 1
            return
        }
        await withCheckedContinuation { waiting.append($0) }
    }

    func release() {
        if let next = waiting.popLast() {
            next.resume()
        } else {
            free += 1
        }
    }
}

@MainActor
final class ArtifactCardPreviewStore {
    static let shared = ArtifactCardPreviewStore()

    /// Immutable, so safe to hand between tasks.
    enum Preview: @unchecked Sendable {
        case image(UIImage)
        case page([String])
        case sheet([[String]])
        case slide([String])
        case code([String])
        case dashboard([String])
        case art
    }

    private final class Box {
        let value: Preview
        init(_ value: Preview) { self.value = value }
    }

    private let cache: NSCache<NSString, Box> = {
        let cache = NSCache<NSString, Box>()
        cache.countLimit = 80
        return cache
    }()
    private var running: [String: Task<Preview, Never>] = [:]
    private let bodies = ArtifactPreviewGate(limit: 3)
    private let snapshots = ArtifactPreviewGate(limit: 1)

    private func key(_ summary: ArtifactV2Summary) -> String { "\(summary.id)@\(summary.currentVersion)" }

    func cached(_ summary: ArtifactV2Summary) -> Preview? {
        cache.object(forKey: key(summary) as NSString)?.value
    }

    /// Runs on past a card that scrolled away: the result is cached for when it
    /// comes back, and two cards on one record share the work.
    func preview(for summary: ArtifactV2Summary) async -> Preview {
        let cacheKey = key(summary)
        if let task = running[cacheKey] { return await task.value }
        let task = Task { @MainActor in
            await self.build(summary, key: cacheKey)
        }
        running[cacheKey] = task
        let value = await task.value
        running[cacheKey] = nil
        return value
    }

    private func build(_ summary: ArtifactV2Summary, key: String) async -> Preview {
        // Drawn by the web's own renderer; the body alone wouldn't help here.
        if summary.kind == "motion" {
            cache.setObject(Box(.art), forKey: key as NSString)
            return .art
        }
        await bodies.acquire()
        let full = try? await ArtifactsV2Client.shared.get(id: summary.id)
        bodies.release()
        // A failed fetch isn't remembered: the next appearance tries again.
        guard let full else { return .art }
        let content = await Task.detached(priority: .utility) { await ArtifactCardContent.decode(full) }.value
        let preview: Preview
        switch content {
        case .image(let data):
            preview = UIImage(data: data).map(Preview.image) ?? .art
        case .page(let lines): preview = .page(lines)
        case .sheet(let rows): preview = .sheet(rows)
        case .slide(let lines): preview = .slide(lines)
        case .code(let lines): preview = .code(lines)
        case .dashboard(let titles): preview = .dashboard(titles)
        case .html(let html):
            await snapshots.acquire()
            let image = await ArtifactSnapshotJob().run(html: html, scripts: false)
            snapshots.release()
            preview = image.map(Preview.image) ?? .art
        case .scriptedPage(let html):
            await snapshots.acquire()
            let image = await ArtifactSnapshotJob().run(html: html, scripts: true)
            snapshots.release()
            preview = image.map(Preview.image) ?? .art
        case .art:
            preview = .art
        }
        cache.setObject(Box(preview), forKey: key as NSString)
        return preview
    }
}

/// A body reduced to what a card draws; built off the main thread, because a
/// body can be megabytes of JSON or base64.
private enum ArtifactCardContent: Sendable {
    case image(Data)
    case page([String])
    case sheet([[String]])
    case slide([String])
    case code([String])
    case dashboard([String])
    case html(String)
    /// Our own page around the record's text (mermaid), which needs scripts.
    case scriptedPage(String)
    case art

    static func decode(_ artifact: ArtifactV2) async -> ArtifactCardContent {
        let body = artifact.version.body
        let format = artifact.version.bodyFormat
        switch artifact.kind {
        case "image":
            guard let uri = ArtifactBodyDecoder.firstURI(body) else { return .art }
            if case .string("video")? = artifact.version.meta?["media"] { return await videoFrame(uri) }
            guard let data = try? await ArtifactMediaLoader.data(for: uri), let small = thumbnail(data) else { return .art }
            return .image(small)
        case "video":
            guard let uri = ArtifactBodyDecoder.videoURL(body: body, meta: artifact.version.meta) else { return .art }
            return await videoFrame(uri)
        case "doc":
            guard let markdown = ArtifactBodyDecoder.docMarkdown(body: body, format: format) else { return .art }
            return .page(plainLines(markdown))
        case "sheet":
            guard let grid = ArtifactBodyDecoder.sheetGrid(body: body, format: format) else { return .art }
            let rows = grid.columns.isEmpty ? grid.rows : [grid.columns] + grid.rows
            return .sheet(rows.prefix(12).map { Array($0.prefix(6)) })
        case "slides":
            return .slide(ArtifactBodyDecoder.firstSlideLines(body: body, format: format))
        case "design":
            return ArtifactBodyDecoder.designPreviewHTML(body: body, format: format).map(ArtifactCardContent.html) ?? .art
        case "page":
            if format == "text/markdown" { return .page(plainLines(body)) }
            let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines)
            // A hosted page is a link, not markup to draw.
            if format == "text/uri-list" || trimmed.isEmpty || trimmed.hasPrefix("https://") { return .art }
            return .html(body)
        case "card":
            return body.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("<") ? .html(body) : .art
        case "diagram":
            let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.hasPrefix("<svg") { return .html(svgPage(trimmed)) }
            return trimmed.isEmpty ? .art : .scriptedPage(ArtifactHTML.mermaid(trimmed))
        case "code":
            guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return .art }
            let lines = body.split(separator: "\n", maxSplits: 16, omittingEmptySubsequences: false).prefix(16)
            return .code(lines.map(String.init))
        case "dashboard", "ops":
            guard let titles = ArtifactBodyDecoder.dashboardTileTitles(body: body), !titles.isEmpty else { return .art }
            return .dashboard(titles)
        case "pdf":
            guard format == "application/pdf",
                  let data = Data(base64Encoded: body, options: .ignoreUnknownCharacters),
                  let page = PDFDocument(data: data)?.page(at: 0) else { return .art }
            let box = page.bounds(for: .mediaBox)
            guard box.width > 0, box.height > 0 else { return .art }
            let image = page.thumbnail(of: CGSize(width: 480, height: 480 * box.height / box.width), for: .mediaBox)
            return image.jpegData(compressionQuality: 0.8).map(ArtifactCardContent.image) ?? .art
        default:
            return .art
        }
    }

    /// Markdown reduced to the page's lines (plainFromMarkdown; embedded
    /// files are bytes, not text to preview).
    private static func plainLines(_ markdown: String) -> [String] {
        var text = markdown
        for (pattern, replacement) in [
            (#"data:[\w.+-]+/[\w.+-]+;base64,[A-Za-z0-9+/=]+"#, ""),
            (#"(?s)```.*?```"#, ""),
            (#"(?m)^#{1,6}\s*"#, ""),
            (#"!?\[([^\]]*)\]\([^)]*\)"#, "$1"),
            (#"[*_`>]"#, ""),
            (#"(?m)^[ \t]*[-+][ \t]+"#, "• "),
        ] {
            text = text.replacingOccurrences(of: pattern, with: replacement, options: .regularExpression)
        }
        return text
            .split(whereSeparator: \.isNewline)
            .map(String.init)
            .filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            .prefix(18)
            .map { $0 }
    }

    /// Scripts in an SVG never run: it's an image, in a page with scripts off.
    private static func svgPage(_ svg: String) -> String {
        let encoded = svg.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        return "<div style=\"display:flex;align-items:center;justify-content:center;width:1000px;height:625px;background:#fff\"><img alt=\"\" style=\"max-width:96%;max-height:96%\" src=\"data:image/svg+xml;charset=utf-8,\(encoded)\"></div>"
    }

    private static func thumbnail(_ data: Data) -> Data? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 640,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return UIImage(cgImage: image).pngData()
    }

    /// API-hosted footage needs the bearer, which a video request can't carry
    /// without downloading the whole file, so only open-web footage gets a frame.
    private static func videoFrame(_ uri: String) async -> ArtifactCardContent {
        guard let url = URL(string: uri), url.scheme?.lowercased() == "https" else { return .art }
        let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: 640, height: 640)
        guard let frame = try? await generator.image(at: CMTime(seconds: 1, preferredTimescale: 600)) else { return .art }
        return UIImage(cgImage: frame.image).jpegData(compressionQuality: 0.8).map(ArtifactCardContent.image) ?? .art
    }
}

// MARK: - HTML snapshot

/// One picture of a page. A record's own markup is script-free (HtmlSnapshot /
/// snapshotDoc): JavaScript off and a CSP that allows no scripts or frames.
/// The mermaid page is ours and runs the viewer's pinned script. Either way: no
/// navigation and its own empty data store. The web view sits behind the app's root view for the moment it
/// takes, because a view outside a window doesn't paint.
@MainActor
private final class ArtifactSnapshotJob: NSObject, WKNavigationDelegate {
    private static let size = CGSize(width: 1000, height: 625)
    private static let csp = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: https: blob:; style-src 'unsafe-inline' https:; font-src data: https:\">"

    private var webView: WKWebView?
    private var continuation: CheckedContinuation<UIImage?, Never>?

    private var settle = Duration.milliseconds(250)

    func run(html: String, scripts: Bool) async -> UIImage? {
        guard let window = Self.hostWindow() else { return nil }
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let preferences = WKWebpagePreferences()
        preferences.allowsContentJavaScript = scripts
        // Mermaid draws after the page has loaded.
        if scripts { settle = .milliseconds(1200) }
        configuration.defaultWebpagePreferences = preferences
        let view = WKWebView(frame: CGRect(origin: .zero, size: Self.size), configuration: configuration)
        view.navigationDelegate = self
        view.isUserInteractionEnabled = false
        view.accessibilityElementsHidden = true
        view.overrideUserInterfaceStyle = .light
        view.backgroundColor = .white
        view.scrollView.isScrollEnabled = false
        window.insertSubview(view, at: 0)
        webView = view
        return await withCheckedContinuation { continuation in
            self.continuation = continuation
            let page = scripts ? html : "\(Self.csp)<style>html,body{margin:0;overflow:hidden;background:#fff}</style>\(html)"
            view.loadHTMLString(page, baseURL: nil)
            // A page whose resources hang must not stall the queue.
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(8))
                self?.finish(nil)
            }
        }
    }

    private static func hostWindow() -> UIWindow? {
        let windows = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .filter { $0.activationState == .foregroundActive }
            .flatMap(\.windows)
        return windows.first(where: \.isKeyWindow) ?? windows.first
    }

    private func finish(_ image: UIImage?) {
        continuation?.resume(returning: image)
        continuation = nil
        webView?.stopLoading()
        webView?.navigationDelegate = nil
        webView?.removeFromSuperview()
        webView = nil
    }

    func webView(_ webView: WKWebView,
                 decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
        let scheme = navigationAction.request.url?.scheme?.lowercased()
        decisionHandler(scheme == nil || scheme == "about" ? .allow : .cancel)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Task { @MainActor in
            // Let the first paint land.
            try? await Task.sleep(for: self.settle)
            guard self.webView != nil else { return }
            let configuration = WKSnapshotConfiguration()
            configuration.rect = CGRect(origin: .zero, size: Self.size)
            configuration.snapshotWidth = 560
            let image = try? await webView.takeSnapshot(configuration: configuration)
            self.finish(image)
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        finish(nil)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        finish(nil)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        finish(nil)
    }
}
