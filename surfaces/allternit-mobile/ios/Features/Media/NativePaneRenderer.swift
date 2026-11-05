import AVFoundation
import UIKit
import WebKit

/// Runs the session agent's pane_render requests on the phone, the way the
/// web app does (allternit-ai lib/media/native-render, native-video and
/// lib/artifacts/check):
///
/// - image: SVG, HTML or a canvas script drawn in an off-screen web view with
///   no network, returned as a PNG.
/// - video: a canvas scene drawn frame by frame (seeded random, sub-frame
///   motion blur) and an optional OfflineAudioContext score, encoded here to
///   H.264/AAC MP4 with AVAssetWriter, stored on the chat's computer, and
///   answered with its URL and a six-still contact sheet.
/// - check: an artifact rendered at each width the way the phone's preview
///   shows it (ArtifactPageWebView), with script errors, sideways scroll, a
///   blank-screen test and a native snapshot; with a reference picture, the
///   closest-shaped width is compared on a 48-px grid.
///
/// Web views live behind the app's root view (inserted at the bottom of the
/// key window) so WebKit keeps rendering them, and are removed when done.
@MainActor
final class NativePaneRenderer {
    static let shared = NativePaneRenderer()

    /// Request ids already taken on this phone: two chat screens on the same
    /// session must not render it twice.
    private var taken: Set<String> = []

    func take(_ requestId: String) -> Bool { taken.insert(requestId).inserted }

    func run(_ request: PaneRenderRequest, client: AgentChatClient) async -> PaneRenderResult {
        switch request.kind {
        case .image: return await renderImage(request)
        case .video: return await renderVideo(request, client: client)
        case .check: return await check(request)
        }
    }

    // MARK: - Off-screen web view

    static let frameCSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'"

    @MainActor
    private final class Loader: NSObject, WKNavigationDelegate {
        var done: CheckedContinuation<Void, Never>?
        func finish() { done?.resume(); done = nil }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { finish() }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finish() }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finish() }
        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
            // The first load only; a page can't navigate the hidden view away.
            decisionHandler(navigationAction.navigationType == .other ? .allow : .cancel)
        }
    }

    /// A hidden web view of `width`×`height` points with `html` loaded, or nil when there's no window to host it.
    /// Base URL for loaded documents: a real (reserved, unreachable) origin,
    /// so script errors in the page keep their message instead of WebKit's
    /// cross-origin "Script error.".
    static let documentBase = URL(string: "https://render.allternit.invalid/")!

    private func loadView(html: String, width: Int, height: Int, userScript: String? = nil, timeout: Double = 25) async -> (WKWebView, Loader)? {
        guard let window = UIApplication.shared.connectedScenes
            .compactMap({ ($0 as? UIWindowScene)?.keyWindow }).first else { return nil }
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let preferences = WKWebpagePreferences()
        preferences.allowsContentJavaScript = true
        configuration.defaultWebpagePreferences = preferences
        if let userScript {
            configuration.userContentController.addUserScript(
                WKUserScript(source: userScript, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        }
        let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: width, height: height), configuration: configuration)
        webView.isUserInteractionEnabled = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.accessibilityElementsHidden = true
        let loader = Loader()
        webView.navigationDelegate = loader
        window.insertSubview(webView, at: 0)
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            loader.done = c
            webView.loadHTMLString(html, baseURL: Self.documentBase)
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(timeout))
                loader.finish()
            }
        }
        return (webView, loader)
    }

    private func snapshot(_ webView: WKWebView, width: Int, height: Int, outputWidth: Int) async -> UIImage? {
        let config = WKSnapshotConfiguration()
        config.rect = CGRect(x: 0, y: 0, width: width, height: height)
        config.afterScreenUpdates = true
        config.snapshotWidth = NSNumber(value: Double(outputWidth) / Double(UIScreen.main.scale))
        guard let shot = try? await webView.takeSnapshot(configuration: config) else { return nil }
        return Self.resized(shot, width: outputWidth, height: Int((Double(outputWidth) * Double(height) / Double(width)).rounded()))
    }

    /// The image drawn at exactly `width`×`height` pixels.
    static func resized(_ image: UIImage, width: Int, height: Int) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        return UIGraphicsImageRenderer(size: CGSize(width: width, height: height), format: format).image { _ in
            image.draw(in: CGRect(x: 0, y: 0, width: width, height: height))
        }
    }

    static func dataURL(png image: UIImage) -> String? {
        image.pngData().map { "data:image/png;base64,\($0.base64EncodedString())" }
    }

    static func dataURL(jpeg image: UIImage, quality: CGFloat = 0.82) -> String? {
        image.jpegData(compressionQuality: quality).map { "data:image/jpeg;base64,\($0.base64EncodedString())" }
    }

    static func image(fromDataURL url: String) -> UIImage? {
        guard let comma = url.firstIndex(of: ","), let data = Data(base64Encoded: String(url[url.index(after: comma)...])) else { return nil }
        return UIImage(data: data)
    }

    private static func scriptSafe(_ code: String) -> String {
        code.replacingOccurrences(of: "</script", with: "<\\/script", options: .caseInsensitive)
            .replacingOccurrences(of: "<!--", with: "<\\!--")
    }

    // MARK: - Image

    private func renderImage(_ r: PaneRenderRequest) async -> PaneRenderResult {
        let head = "<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=\(r.width), initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"\(Self.frameCSP)\">"
        switch r.format {
        case "canvas":
            let html = """
            <!doctype html><html><head>\(head)</head><body style="margin:0">
            <canvas id="c" width="\(r.width)" height="\(r.height)"></canvas>
            <script>window.__err = null; window.onerror = function (m) { window.__err = String(m); };</script>
            <script>function __draw(ctx, width, height) {
            \(Self.scriptSafe(r.code))
            }</script>
            <script>try { if (typeof __draw !== 'function') throw new Error('The canvas code did not parse.');
              var c = document.getElementById('c'); __draw(c.getContext('2d'), \(r.width), \(r.height)); window.__png = c.toDataURL('image/png');
            } catch (e) { window.__err = String((e && e.message) || e); }</script></body></html>
            """
            guard let (view, _) = await loadView(html: html, width: min(r.width, 2048), height: min(r.height, 2048)) else { return .failed("The app has no window to render in.") }
            defer { view.removeFromSuperview() }
            let out = try? await view.callAsyncJavaScript("return { png: window.__png || null, err: window.__err || null }", contentWorld: .page) as? [String: Any]
            if let png = out?["png"] as? String { return PaneRenderResult(ok: true, image: png, width: r.width, height: r.height) }
            return .failed((out?["err"] as? String).map { "The canvas code threw: \($0)" } ?? "The canvas didn't render.")
        default:
            // SVG and HTML: laid out at the requested size and pictured as shown.
            let body = r.format == "svg"
                ? "<div style=\"width:\(r.width)px;height:\(r.height)px\">\(r.code)</div><style>svg{width:100%;height:100%;display:block}</style>"
                : r.code
            let html = r.format == "svg"
                ? "<!doctype html><html><head>\(head)</head><body style=\"margin:0\">\(body)</body></html>"
                : Self.withHead(head, in: body)
            guard let (view, _) = await loadView(html: html, width: r.width, height: r.height) else { return .failed("The app has no window to render in.") }
            defer { view.removeFromSuperview() }
            try? await Task.sleep(for: .milliseconds(150))
            guard let shot = await snapshot(view, width: r.width, height: r.height, outputWidth: r.width),
                  let png = Self.dataURL(png: shot) else { return .failed("The \(r.format) did not render (check it is well-formed).") }
            return PaneRenderResult(ok: true, image: png, width: r.width, height: r.height)
        }
    }

    /// `head` markup placed first in the document's head (made if missing).
    static func withHead(_ head: String, in html: String) -> String {
        if let range = html.range(of: "<head[^>]*>", options: [.regularExpression, .caseInsensitive]) {
            return html.replacingCharacters(in: range, with: html[range] + head)
        }
        if let range = html.range(of: "<html[^>]*>", options: [.regularExpression, .caseInsensitive]) {
            return html.replacingCharacters(in: range, with: html[range] + "<head>\(head)</head>")
        }
        return "<head>\(head)</head>" + html
    }

    // MARK: - Video

    static let audioSampleRate = 48_000

    /// Frame indices on the contact sheet: evenly spread, mid-interval (native-video contactSheetFrames).
    static func contactSheetFrames(_ total: Int, count: Int = 6) -> [Int] {
        Array(Set((0..<count).map { min(total - 1, Int((Double($0) + 0.5) / Double(count) * Double(total))) })).sorted()
    }

    private func videoDocument(_ r: PaneRenderRequest, samples: Int) -> String {
        let score = (r.audio?.isEmpty == false) ? "<script>function __score(ctx, duration) {\n\(Self.scriptSafe(r.audio!))\n}</script>" : ""
        return """
        <!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="\(Self.frameCSP)"></head><body style="margin:0">
        <script>var __errors = []; window.onerror = function (m) { __errors.push(String(m)); };</script>
        <script>function __scene(ctx, t, width, height) {
        \(Self.scriptSafe(r.code))
        }</script>
        \(score)
        <script>
        (function () {
          var W = \(r.width), H = \(r.height), FPS = \(r.fps ?? 30), DUR = \(r.duration ?? 1), SAMPLES = \(samples), SHUTTER = 0.5, SR = \(Self.audioSampleRate);
          var random = Math.random;
          function reseed() {
            var s = 0x2f6b1a9d;
            Math.random = function () {
              s = (s + 0x6d2b79f5) | 0;
              var t = Math.imul(s ^ (s >>> 15), 1 | s);
              t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
              return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
            };
          }
          function el() { var c = document.createElement('canvas'); c.width = W; c.height = H; return c; }
          var work = el(), acc = el(), wctx = work.getContext('2d'), actx = acc.getContext('2d');
          function drawAt(t) {
            wctx.setTransform(1, 0, 0, 1, 0, 0); wctx.globalAlpha = 1; wctx.globalCompositeOperation = 'source-over';
            wctx.fillStyle = '#000'; wctx.fillRect(0, 0, W, H);
            reseed(); wctx.save(); __scene(wctx, t, W, H); wctx.restore();
          }
          window.__ready = function () {
            return typeof __scene === 'function' ? null : 'The scene code did not parse: ' + (__errors.join('; ') || 'syntax error');
          };
          window.__frame = function (i) {
            try {
              for (var k = 0; k < SAMPLES; k++) {
                var t = SAMPLES === 1 ? i / FPS : (i + ((k + 0.5) / SAMPLES - 0.5) * SHUTTER) / FPS;
                drawAt(Math.max(0, Math.min(DUR, t)));
                actx.globalAlpha = 1 / (k + 1); actx.drawImage(work, 0, 0);
              }
              actx.globalAlpha = 1;
              return { jpeg: acc.toDataURL('image/jpeg', 0.92) };
            } catch (e) { return { error: 'The scene threw at t=' + (i / FPS).toFixed(2) + 's: ' + String((e && e.message) || e) }; }
          };
          function b64(f32) {
            var bytes = new Uint8Array(f32.buffer), out = '', CH = 0x8000;
            for (var i = 0; i < bytes.length; i += CH) out += String.fromCharCode.apply(null, bytes.subarray(i, i + CH));
            return btoa(out);
          }
          window.__audio = function () {
            if (typeof __score !== 'function') return Promise.resolve(__errors.length && \(r.audio?.isEmpty == false ? "true" : "false") ? { error: 'The audio code did not parse: ' + __errors.join('; ') } : null);
            try {
              Math.random = random; reseed();
              var ctx = new OfflineAudioContext(2, Math.max(1, Math.ceil(DUR * SR)), SR);
              __score(ctx, DUR);
              return ctx.startRendering().then(function (buf) {
                return { sampleRate: SR, l: b64(buf.getChannelData(0)), r: b64(buf.getChannelData(buf.numberOfChannels > 1 ? 1 : 0)) };
              }, function (e) { return { error: 'The audio failed to render: ' + String((e && e.message) || e) }; });
            } catch (e) { return Promise.resolve({ error: 'The audio code threw: ' + String((e && e.message) || e) }); }
          };
        })();
        </script></body></html>
        """
    }

    private func renderVideo(_ r: PaneRenderRequest, client: AgentChatClient) async -> PaneRenderResult {
        let fps = r.fps ?? 30
        let duration = r.duration ?? 1
        let total = max(1, Int((duration * Double(fps)).rounded()))
        let samples = max(1, min(16, r.motionBlur ?? 1))
        let deadline = Date().addingTimeInterval((r.timeoutMs ?? 600_000) / 1000 - 5)
        guard let (view, _) = await loadView(html: videoDocument(r, samples: samples), width: 16, height: 16) else {
            return .failed("The app has no window to render in.")
        }
        defer { view.removeFromSuperview() }
        if let problem = try? await view.callAsyncJavaScript("return window.__ready ? window.__ready() : 'The scene page did not load.'", contentWorld: .page) as? String {
            return .failed(problem)
        }

        // The score first: the writer interleaves, so audio goes in alongside the frames.
        var pcm: (left: [Float], right: [Float])? = nil
        if r.audio?.isEmpty == false {
            let out = try? await view.callAsyncJavaScript("return await window.__audio()", contentWorld: .page) as? [String: Any]
            if let error = out?["error"] as? String { return .failed(error) }
            if let l = (out?["l"] as? String).flatMap({ Data(base64Encoded: $0) }), let rr = (out?["r"] as? String).flatMap({ Data(base64Encoded: $0) }) {
                pcm = (Self.floats(l), Self.floats(rr))
            }
        }

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("render-\(r.requestId).mp4")
        try? FileManager.default.removeItem(at: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let encoder: VideoEncoder
        do { encoder = try VideoEncoder(url: url, width: r.width, height: r.height, fps: fps, audio: pcm != nil) }
        catch { return .failed("The phone couldn't start the video encoder: \(error.localizedDescription)") }

        let sheetPicks = Set(Self.contactSheetFrames(total))
        var stills: [(Int, UIImage)] = []
        var audioCursor = 0
        for i in 0..<total {
            if Date() > deadline { encoder.cancel(); return .failed("The render ran out of time at frame \(i) of \(total).") }
            let out = try? await view.callAsyncJavaScript("return window.__frame(i)", arguments: ["i": i], contentWorld: .page) as? [String: Any]
            if let error = out?["error"] as? String { encoder.cancel(); return .failed(error) }
            guard let jpeg = out?["jpeg"] as? String, let frame = Self.image(fromDataURL: jpeg)?.cgImage else {
                encoder.cancel(); return .failed("Frame \(i) didn't draw.")
            }
            if let pcm {
                let until = min(pcm.left.count, Int(Double(i + 1) / Double(fps) * Double(Self.audioSampleRate)))
                if until > audioCursor {
                    await encoder.appendAudio(left: pcm.left[audioCursor..<until], right: pcm.right[audioCursor..<until], at: audioCursor)
                    audioCursor = until
                }
            }
            guard await encoder.appendFrame(frame, index: i) else { encoder.cancel(); return .failed("The encoder stopped at frame \(i).") }
            if sheetPicks.contains(i) { stills.append((i, UIImage(cgImage: frame))) }
        }
        if let pcm, audioCursor < pcm.left.count {
            await encoder.appendAudio(left: pcm.left[audioCursor...], right: pcm.right[audioCursor...], at: audioCursor)
        }
        guard await encoder.finish(), let data = try? Data(contentsOf: url) else {
            return .failed("The video couldn't be encoded: \(encoder.errorText ?? "unknown error").")
        }
        let stored: String
        do { stored = try await client.uploadMediaArtifact(data, mime: "video/mp4") }
        catch { return .failed("The video rendered but couldn't be saved: \(error.localizedDescription)") }
        return PaneRenderResult(
            ok: true,
            image: Self.contactSheet(stills, fps: fps, width: r.width, height: r.height).flatMap { Self.dataURL(png: $0) },
            video: PaneRenderVideo(url: stored, mime: "video/mp4", bytes: data.count, duration: duration, fps: fps, codec: "avc1", audio: pcm != nil),
            width: r.width, height: r.height
        )
    }

    private static func floats(_ data: Data) -> [Float] {
        data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    }

    /// Six stills in a 3×2 grid, each labelled with its time.
    static func contactSheet(_ stills: [(Int, UIImage)], fps: Int, width: Int, height: Int) -> UIImage? {
        guard !stills.isEmpty else { return nil }
        let cellW = 480.0, cellH = (480.0 * Double(height) / Double(width)).rounded(), cols = 3
        let rows = Int(ceil(Double(stills.count) / Double(cols)))
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: CGSize(width: cellW * Double(cols), height: cellH * Double(rows)), format: format).image { ctx in
            UIColor.black.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: cellW * Double(cols), height: cellH * Double(rows)))
            for (k, still) in stills.enumerated() {
                let rect = CGRect(x: Double(k % cols) * cellW, y: Double(k / cols) * cellH, width: cellW, height: cellH)
                still.1.draw(in: rect)
                let seconds = Double(still.0) / Double(fps)
                let label = String(format: "%d:%04.1f", Int(seconds) / 60, seconds.truncatingRemainder(dividingBy: 60)) as NSString
                let attrs: [NSAttributedString.Key: Any] = [.font: UIFont.monospacedDigitSystemFont(ofSize: 16, weight: .semibold), .foregroundColor: UIColor.white]
                let size = label.size(withAttributes: attrs)
                UIColor.black.withAlphaComponent(0.6).setFill()
                ctx.fill(CGRect(x: rect.minX + 8, y: rect.minY + 8, width: size.width + 12, height: size.height + 6))
                label.draw(at: CGPoint(x: rect.minX + 14, y: rect.minY + 11), withAttributes: attrs)
            }
        }
    }

    // MARK: - Check

    static let checkWidths = [390, 768, 1440]
    static let blankSpread = 2.5
    static let regionMismatch = 0.18
    static let grid = 48

    static func heightFor(_ width: Int) -> Int { width < 600 ? 844 : width < 1000 ? 1024 : 900 }

    /// Runs first in the page: collects script errors, failed loads and console.error lines.
    static let probeScript = """
    (function () {
      var errors = window.__checkErrors = [];
      function note(m) { if (errors.length < 5) errors.push(String(m).slice(0, 300)); }
      addEventListener('error', function (e) {
        var t = e.target;
        if (t && t !== window && (t.src || t.href)) note('Could not load ' + (t.src || t.href));
        else note(e.message || 'Script error');
      }, true);
      addEventListener('unhandledrejection', function (e) { var r = e.reason; note('Unhandled rejection: ' + ((r && r.message) || r)); });
      var ce = console.error;
      console.error = function () { note([].map.call(arguments, function (a) { return (a && a.message) || String(a); }).join(' ')); return ce.apply(console, arguments); };
    })();
    """

    /// The artifact as one page, or why the phone can't check it.
    static func checkableHTML(kind: String?, format: String?, body: String) -> Result<String, CheckRefusal> {
        let format = format ?? "text/html"
        switch kind ?? "page" {
        case "design":
            if let html = ArtifactBodyDecoder.designHTML(body: body, format: format) { return .success(html) }
            return .failure(CheckRefusal("This design is a canvas, which the phone app doesn't draw yet; check it from Desktop or ai.allternit.com."))
        case "page":
            if format == "text/uri-list" { return .failure(CheckRefusal("This page is a published link, not HTML in the artifact; open the link to look at it.")) }
            if format == "text/markdown" { return .failure(CheckRefusal("This page is Markdown, which renders as plain text; there is no layout to check.")) }
            if body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return .failure(CheckRefusal("The artifact is empty.")) }
            return .success(body)
        case let other:
            return .failure(CheckRefusal("artifact_check looks at pages and designs; a \(other) artifact can't be checked yet."))
        }
    }

    struct CheckRefusal: Error { let message: String; init(_ m: String) { message = m } }

    private func check(_ r: PaneRenderRequest) async -> PaneRenderResult {
        var html: String
        switch Self.checkableHTML(kind: r.artifactKind, format: r.bodyFormat, body: r.code) {
        case .success(let h): html = h
        case .failure(let refusal): return .failed(refusal.message)
        }
        // A page without a viewport tag lays out at WebKit's 980px default on
        // any width; the web check (an iframe of that width) never does, so
        // the window width is the layout width here too.
        if html.range(of: "<meta[^>]+name=[\"']?viewport", options: [.regularExpression, .caseInsensitive]) == nil {
            html = Self.withHead("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">", in: html)
        }
        var checks: [PaneRenderCheckWidth] = []
        for width in r.widths.isEmpty ? Self.checkWidths : r.widths {
            if Task.isCancelled { return .failed("Cancelled.") }
            let height = Self.heightFor(width)
            guard let (view, _) = await loadView(html: html, width: width, height: height, userScript: Self.probeScript) else {
                return .failed("The app has no window to render in.")
            }
            let measured = try? await view.callAsyncJavaScript("""
                await Promise.race([document.fonts ? document.fonts.ready : Promise.resolve(), new Promise(function (r) { setTimeout(r, 3000); })]);
                await new Promise(function (r) { setTimeout(r, 1200); });
                var de = document.documentElement;
                return { overflowX: Math.max(0, de.scrollWidth - innerWidth), text: ((document.body && document.body.innerText) || '').trim().length, errors: window.__checkErrors || [] };
                """, contentWorld: .page) as? [String: Any]
            let shot = await snapshot(view, width: width, height: height, outputWidth: min(width, 1000))
            view.removeFromSuperview()
            var errors = (measured?["errors"] as? [String]) ?? []
            if measured == nil { errors.append("(check) The page didn't finish loading.") }
            if shot == nil { errors.append("(check) The phone couldn't take the picture.") }
            let text = (measured?["text"] as? NSNumber)?.intValue ?? 0
            var blank = text == 0 && shot == nil
            if let cg = shot?.cgImage, let px = Self.pixels(cg, cols: Self.grid, rows: max(8, Self.grid * height / width)) {
                blank = Self.brightnessSpread(px) < Self.blankSpread
            }
            checks.append(PaneRenderCheckWidth(
                width: width, height: height,
                image: shot.flatMap { Self.dataURL(jpeg: $0) },
                blank: blank,
                overflowX: Int(((measured?["overflowX"] as? NSNumber)?.doubleValue ?? 0).rounded()),
                errors: Array(errors.prefix(6))
            ))
        }
        var note: String? = nil
        if let reference = r.reference {
            if let ref = Self.image(fromDataURL: reference)?.cgImage {
                let at = Self.closestWidth(checks.map { ($0.width, $0.height) }, ratio: Double(ref.width) / Double(max(1, ref.height)))
                if let image = checks[at].image.flatMap(Self.image(fromDataURL:))?.cgImage {
                    let rows = max(8, Self.grid * checks[at].height / checks[at].width)
                    if let a = Self.pixels(image, cols: Self.grid, rows: rows), let b = Self.pixels(ref, cols: Self.grid, rows: rows) {
                        let out = Self.compareGrids(a, b, cols: Self.grid, rows: rows)
                        checks[at].similarity = out.similarity
                        checks[at].mismatch = out.mismatch
                    }
                }
            } else {
                note = "The reference picture couldn't be compared: it could not be read."
            }
        }
        return PaneRenderResult(ok: true, checks: checks, error: note)
    }

    /// RGBA pixels of an image squeezed to cols×rows.
    static func pixels(_ image: CGImage, cols: Int, rows: Int) -> [UInt8]? {
        var data = [UInt8](repeating: 0, count: cols * rows * 4)
        let ok = data.withUnsafeMutableBytes { buf -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: cols, height: rows, bitsPerComponent: 8, bytesPerRow: cols * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            ctx.interpolationQuality = .high
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: cols, height: rows))
            return true
        }
        return ok ? data : nil
    }

    /// Spread (standard deviation) of brightness, 0–255.
    static func brightnessSpread(_ data: [UInt8]) -> Double {
        var sum = 0.0, sq = 0.0
        let n = Double(data.count / 4)
        for i in stride(from: 0, to: data.count, by: 4) {
            let l = 0.299 * Double(data[i]) + 0.587 * Double(data[i + 1]) + 0.114 * Double(data[i + 2])
            sum += l; sq += l * l
        }
        let mean = sum / n
        return max(0, sq / n - mean * mean).squareRoot()
    }

    /// How alike two same-size grids are (0–1) and the 3×3 regions that differ (lib/artifacts/check compareGrids).
    static func compareGrids(_ a: [UInt8], _ b: [UInt8], cols: Int, rows: Int) -> (similarity: Double, mismatch: [String]) {
        var total = 0.0
        var region = Array(repeating: (diff: 0.0, la: 0.0, lb: 0.0, n: 0.0), count: 9)
        for y in 0..<rows {
            for x in 0..<cols {
                let i = (y * cols + x) * 4
                let d = (abs(Double(a[i]) - Double(b[i])) + abs(Double(a[i + 1]) - Double(b[i + 1])) + abs(Double(a[i + 2]) - Double(b[i + 2]))) / (3 * 255)
                total += d
                let k = min(2, y * 3 / rows) * 3 + min(2, x * 3 / cols)
                region[k].diff += d
                region[k].la += 0.299 * Double(a[i]) + 0.587 * Double(a[i + 1]) + 0.114 * Double(a[i + 2])
                region[k].lb += 0.299 * Double(b[i]) + 0.587 * Double(b[i + 1]) + 0.114 * Double(b[i + 2])
                region[k].n += 1
            }
        }
        let rowsNames = ["top", "middle", "bottom"], colsNames = ["left", "centre", "right"]
        let mismatch = region.enumerated()
            .map { (k: $0.offset, diff: $0.element.n > 0 ? $0.element.diff / $0.element.n : 0, delta: ($0.element.la - $0.element.lb) / max(1, $0.element.n)) }
            .filter { $0.diff > regionMismatch }
            .sorted { $0.diff > $1.diff }
            .map { r -> String in
                let how = r.delta > 25 ? "lighter than the reference" : r.delta < -25 ? "darker than the reference" : "different colours or shapes"
                return "\(rowsNames[r.k / 3]) \(colsNames[r.k % 3]): \(how)"
            }
        return (max(0, min(1, 1 - total / Double(cols * rows))), mismatch)
    }

    /// The width whose window shape is closest to the reference picture's.
    static func closestWidth(_ widths: [(Int, Int)], ratio: Double) -> Int {
        var best = 0
        func off(_ i: Int) -> Double { abs(log(Double(widths[i].0) / Double(widths[i].1) / ratio)) }
        for i in widths.indices.dropFirst() where off(i) < off(best) { best = i }
        return best
    }
}

/// H.264 + AAC MP4 writer fed one frame (and its share of audio) at a time.
@MainActor
private final class VideoEncoder {
    private let writer: AVAssetWriter
    private let video: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private let audio: AVAssetWriterInput?
    private let fps: Int
    private let width: Int
    private let height: Int
    var errorText: String? { writer.error?.localizedDescription }

    init(url: URL, width: Int, height: Int, fps: Int, audio hasAudio: Bool) throws {
        self.width = width
        self.height = height
        self.fps = fps
        writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
        video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: max(2_000_000, width * height * fps / 5)],
        ])
        video.expectsMediaDataInRealTime = false
        adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: width,
            kCVPixelBufferHeightKey as String: height,
        ])
        writer.add(video)
        if hasAudio {
            let input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: NativePaneRenderer.audioSampleRate,
                AVNumberOfChannelsKey: 2,
                AVEncoderBitRateKey: 192_000,
            ])
            input.expectsMediaDataInRealTime = false
            writer.add(input)
            audio = input
        } else {
            audio = nil
        }
        guard writer.startWriting() else { throw writer.error ?? CocoaError(.fileWriteUnknown) }
        writer.startSession(atSourceTime: .zero)
    }

    private func waitReady(_ input: AVAssetWriterInput) async -> Bool {
        var waited = 0
        while !input.isReadyForMoreMediaData {
            if writer.status != .writing || waited > 2_000 { return false }
            try? await Task.sleep(for: .milliseconds(5))
            waited += 1
        }
        return true
    }

    func appendFrame(_ image: CGImage, index: Int) async -> Bool {
        guard await waitReady(video), let pool = adaptor.pixelBufferPool else { return false }
        var buffer: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &buffer)
        guard let buffer else { return false }
        CVPixelBufferLockBaseAddress(buffer, [])
        let ctx = CGContext(data: CVPixelBufferGetBaseAddress(buffer), width: width, height: height, bitsPerComponent: 8,
                            bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        ctx?.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        CVPixelBufferUnlockBaseAddress(buffer, [])
        return adaptor.append(buffer, withPresentationTime: CMTime(value: CMTimeValue(index), timescale: CMTimeScale(fps)))
    }

    func appendAudio(left: ArraySlice<Float>, right: ArraySlice<Float>, at start: Int) async {
        guard let audio else { return }
        let chunk = 4_096
        var offset = 0
        let count = min(left.count, right.count)
        while offset < count {
            let n = min(chunk, count - offset)
            var interleaved = [Float](repeating: 0, count: n * 2)
            for j in 0..<n {
                interleaved[2 * j] = left[left.startIndex + offset + j]
                interleaved[2 * j + 1] = right[right.startIndex + offset + j]
            }
            guard await waitReady(audio), let sample = Self.sampleBuffer(interleaved, frames: n, at: start + offset) else { return }
            audio.append(sample)
            offset += n
        }
    }

    private static func sampleBuffer(_ samples: [Float], frames: Int, at start: Int) -> CMSampleBuffer? {
        var asbd = AudioStreamBasicDescription(
            mSampleRate: Double(NativePaneRenderer.audioSampleRate), mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked, mBytesPerPacket: 8, mFramesPerPacket: 1,
            mBytesPerFrame: 8, mChannelsPerFrame: 2, mBitsPerChannel: 32, mReserved: 0)
        var format: CMAudioFormatDescription?
        guard CMAudioFormatDescriptionCreate(allocator: nil, asbd: &asbd, layoutSize: 0, layout: nil, magicCookieSize: 0,
                                             magicCookie: nil, extensions: nil, formatDescriptionOut: &format) == noErr, let format else { return nil }
        let bytes = samples.count * MemoryLayout<Float>.size
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(allocator: nil, memoryBlock: nil, blockLength: bytes, blockAllocator: nil,
                                                 customBlockSource: nil, offsetToData: 0, dataLength: bytes, flags: 0, blockBufferOut: &block) == noErr,
              let block else { return nil }
        let copied = samples.withUnsafeBytes { CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: bytes) }
        guard copied == noErr else { return nil }
        var sample: CMSampleBuffer?
        guard CMAudioSampleBufferCreateReadyWithPacketDescriptions(
            allocator: nil, dataBuffer: block, formatDescription: format, sampleCount: frames,
            presentationTimeStamp: CMTime(value: CMTimeValue(start), timescale: CMTimeScale(NativePaneRenderer.audioSampleRate)),
            packetDescriptions: nil, sampleBufferOut: &sample) == noErr else { return nil }
        return sample
    }

    func finish() async -> Bool {
        video.markAsFinished()
        audio?.markAsFinished()
        await writer.finishWriting()
        return writer.status == .completed
    }

    func cancel() { writer.cancelWriting() }
}
