import SwiftUI
import ImageIO
import UniformTypeIdentifiers
import CryptoKit
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

    func run(_ request: PaneRenderRequest, client: AgentChatClient, browserConfirmed: Bool = false, browserLocal: Bool = false) async -> PaneRenderResult {
        switch request.kind {
        case .browserImage:
            guard browserConfirmed else { return PaneRenderResult(ok: false, error: "Confirm this exact image prompt in the originating chat first.", code: "confirmation_required") }
            return await ACIBrowserImageJob.shared.run(request, client: client, local: browserLocal)
        case .image: return await renderImage(request, client: client)
        case .video: return await renderVideo(request, client: client)
        case .gif: return await renderGif(request, client: client)
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

    /// A progress preview: at most 320 px wide, a small JPEG data URL (the
    /// card shows it blurred or as a frame thumbnail, never the final art).
    static func thumbnailURL(_ image: UIImage) -> String? {
        let w = 320.0
        let size = image.size
        guard size.width > 0, size.height > 0 else { return nil }
        let scale = min(1, w / size.width)
        let small = Self.resized(image, width: max(1, Int((size.width * scale).rounded())), height: max(1, Int((size.height * scale).rounded())))
        return dataURL(jpeg: small, quality: 0.6)
    }

    static func thumbnailURL(fromDataURL url: String) -> String? {
        image(fromDataURL: url).flatMap { thumbnailURL($0) }
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

    private func renderImage(_ r: PaneRenderRequest, client: AgentChatClient) async -> PaneRenderResult {
        let progress = PaneRenderProgressReporter(requestId: r.requestId, client: client)
        progress.report(stage: .drawing) { .init(lane: .picture, stage: .drawing, label: "Drawing the picture") }
        let result = await drawImage(r)
        if result.ok, let png = result.image {
            progress.report(stage: .saving) {
                .init(lane: .picture, stage: .saving, label: "Saving", fraction: 1, preview: Self.thumbnailURL(fromDataURL: png))
            }
        }
        return result
    }

    private func drawImage(_ r: PaneRenderRequest) async -> PaneRenderResult {
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
        <script>function __scene(ctx, t, width, height, source) {
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
            reseed(); wctx.save(); __scene(wctx, t, W, H, __source); wctx.restore();
          }
          var __source, SOURCE = \(r.source.map { "\"" + $0 + "\"" } ?? "null");
          window.__ready = function () {
            if (typeof __scene !== 'function') return 'The scene code did not parse: ' + (__errors.join('; ') || 'syntax error');
            if (!SOURCE) return null;
            return new Promise(function (res) {
              var im = new Image();
              im.onload = function () { __source = im; res(null); };
              im.onerror = function () { res('The source picture did not load.'); };
              im.src = SOURCE;
            });
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
        let progress = PaneRenderProgressReporter(requestId: r.requestId, client: client)
        progress.report(stage: .drawing) { .init(lane: .motion, stage: .drawing, label: "Setting up the scene") }
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
            progress.report(stage: .drawing) { .init(lane: .motion, stage: .drawing, label: "Scoring the audio") }
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
            progress.report(stage: .rendering) {
                .init(lane: .motion, stage: .rendering, label: "Frame \(i + 1) of \(total)", frame: i + 1, frames: total,
                      fraction: Double(i + 1) / Double(total), preview: Self.thumbnailURL(UIImage(cgImage: frame)))
            }
            if sheetPicks.contains(i) { stills.append((i, UIImage(cgImage: frame))) }
        }
        if let pcm, audioCursor < pcm.left.count {
            await encoder.appendAudio(left: pcm.left[audioCursor...], right: pcm.right[audioCursor...], at: audioCursor)
        }
        progress.report(stage: .encoding) { .init(lane: .motion, stage: .encoding, label: "Encoding the video") }
        guard await encoder.finish(), let data = try? Data(contentsOf: url) else {
            return .failed("The video couldn't be encoded: \(encoder.errorText ?? "unknown error").")
        }
        progress.report(stage: .saving) { .init(lane: .motion, stage: .saving, label: "Saving the video", fraction: 1) }
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

    /// A code-animated GIF: the scene draws like a video's, frames are encoded
    /// with ImageIO (loop forever). Limits are clamped in PaneRenderRequest.
    @MainActor
    private func renderGif(_ r: PaneRenderRequest, client: AgentChatClient) async -> PaneRenderResult {
        let fps = r.fps ?? 12
        let total = max(1, min(120, Int(((r.duration ?? 3) * Double(fps)).rounded())))
        let deadline = Date().addingTimeInterval((r.timeoutMs ?? 120_000) / 1000 - 5)
        let progress = PaneRenderProgressReporter(requestId: r.requestId, client: client)
        progress.report(stage: .drawing) { .init(lane: .picture, stage: .drawing, label: "Setting up the animation") }
        guard let (view, _) = await loadView(html: videoDocument(r, samples: 1), width: 16, height: 16) else {
            return .failed("The app has no window to render in.")
        }
        defer { view.removeFromSuperview() }
        if let problem = try? await view.callAsyncJavaScript("return window.__ready ? await window.__ready() : 'The scene page did not load.'", contentWorld: .page) as? String {
            return .failed(problem)
        }
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, UTType.gif.identifier as CFString, total, nil) else {
            return .failed("The phone couldn't start the GIF encoder.")
        }
        CGImageDestinationSetProperties(dest, [kCGImagePropertyGIFDictionary as String: [kCGImagePropertyGIFLoopCount as String: 0]] as CFDictionary)
        let frameProps = [kCGImagePropertyGIFDictionary as String: [kCGImagePropertyGIFDelayTime as String: 1.0 / Double(fps)]] as CFDictionary
        for i in 0..<total {
            if Date() > deadline { return .failed("The render ran out of time at frame \(i) of \(total).") }
            if Task.isCancelled { return .failed("Cancelled.") }
            let out = try? await view.callAsyncJavaScript("return window.__frame(i)", arguments: ["i": i], contentWorld: .page) as? [String: Any]
            if let error = out?["error"] as? String { return .failed(error) }
            guard let jpeg = out?["jpeg"] as? String, let frame = Self.image(fromDataURL: jpeg)?.cgImage else {
                return .failed("Frame \(i) didn't draw.")
            }
            CGImageDestinationAddImage(dest, frame, frameProps)
            progress.report(stage: .rendering) {
                .init(lane: .picture, stage: .rendering, label: "Frame \(i + 1) of \(total)", frame: i + 1, frames: total,
                      fraction: Double(i + 1) / Double(total), preview: Self.thumbnailURL(UIImage(cgImage: frame)))
            }
        }
        progress.report(stage: .encoding) { .init(lane: .picture, stage: .encoding, label: "Encoding the GIF") }
        guard CGImageDestinationFinalize(dest) else { return .failed("The GIF couldn't be encoded.") }
        guard data.length <= 8 * 1024 * 1024 else {
            return .failed("The GIF is larger than 8 MB. Make it shorter or smaller.")
        }
        return PaneRenderResult(ok: true, image: "data:image/gif;base64," + (data as Data).base64EncodedString(), width: r.width, height: r.height)
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

/// Durable submission fence. A replay can never click Send again, including after a crash.
/// Only identity hashes are stored here; website credentials remain inside WebKit.
struct ACIBrowserImageLedger {
    let defaults: UserDefaults
    private let key = "aci.browser-image.submissions.v1"
    func reserve(_ request: PaneRenderRequest) -> Bool {
        var jobs = defaults.dictionary(forKey: key) as? [String: String] ?? [:]
        guard jobs[request.requestId] == nil else { return false }
        let identity = request.sessionId + "\n" + (request.prompt ?? "")
        jobs[request.requestId] = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        defaults.set(jobs, forKey: key)
        return defaults.synchronize()
    }
    func recordConversation(_ url: URL, requestId: String) {
        guard url.scheme == "https", url.host == "chatgpt.com", url.absoluteString.count <= 2048 else { return }
        var conversations = defaults.dictionary(forKey: key + ".conversations") as? [String: String] ?? [:]
        conversations[requestId] = url.absoluteString
        defaults.set(conversations, forKey: key + ".conversations")
        defaults.synchronize()
    }
    func conversation(requestId: String) -> URL? {
        (defaults.dictionary(forKey: key + ".conversations")?[requestId] as? String).flatMap(URL.init(string:))
    }
}

/// Fixed ChatGPT adapter, never a page-to-native message bridge. The browser owns its
/// private website session. OAuth used for text chat does not authorize this session.
@MainActor
final class ACIBrowserImageJob: ObservableObject {
    static let shared = ACIBrowserImageJob()
    static var websiteDataStore = WKWebsiteDataStore.nonPersistent()
    static let sourceLimit = 20 * 1024 * 1024
    static let pngLimit = 8 * 1024 * 1024
    private let world = WKContentWorld.world(name: "allternit-image-job")
    @Published private(set) var isBusy = false
    @Published private(set) var needsResume = false
    @Published private(set) var resumeLabel = "Resume image job"
    private var resumeRequested = false
    private(set) var hasSubmitted = false
    func resume() { resumeRequested = true }
    private(set) var epoch = 0
    private var activeRequestId: String?

    func cancel() {
        epoch += 1
        guard isBusy, let web = ACIBrowserController.shared.webView, let id = activeRequestId else { return }
        Task {
            _ = try? await call(web, "if(window.__aciImage?.id === id) { window.__aciImage.abort?.abort(); document.querySelector('button[data-testid=\"stop-button\"]')?.click(); } return {stopped:true};", ["id": id])
        }
    }
    func signOut() {
        ACILocalImageStore.shared.resetForAccountChange()
        cancel()
        ACIBrowserBridge.shared.stopBot()
        ACIBrowserController.shared.tearDown()
        Self.websiteDataStore = .nonPersistent()
    }

    enum Failure: LocalizedError {
        case stopped, unsupported, expired, replay, image
        var errorDescription: String? {
            switch self {
            case .stopped: return "Image job stopped. Check the ChatGPT conversation before starting another image."
            case .unsupported: return "ChatGPT's image controls are unavailable. In the ACI browser, sign in and open a regular empty New chat with Create image available, then start a new image action. No prompt was automatically retried."
            case .expired: return "Image job timed out. Check the image in ChatGPT before starting another image action."
            case .replay: return "This image request was already handled or may have been submitted. Check the ACI ChatGPT conversation. It will not be sent again."
            case .image: return "ChatGPT did not return a supported image within the download and image size limits."
            }
        }
        var code: String {
            switch self {
            case .stopped: return "cancelled"
            case .unsupported: return "needs_user"
            case .expired: return "timeout"
            case .replay: return "ambiguous_submission"
            case .image: return "invalid_image"
            }
        }
    }

    /// Inspect dimensions before full pixel allocation. Final PNG must fit the relay envelope.
    static func normalize(_ data: Data, mime: String) throws -> PaneRenderResult {
        guard ["image/png", "image/jpeg", "image/webp", "image/heic"].contains(mime.lowercased()),
              !data.isEmpty, data.count <= sourceLimit,
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(source) == 1,
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int,
              let height = properties[kCGImagePropertyPixelHeight] as? Int,
              (1...4096).contains(width), (1...4096).contains(height), width * height <= 16_000_000,
              let decoded = UIImage(data: data) else { throw Failure.image }
        // UIKit drawing applies EXIF orientation and removes provider metadata.
        let rotated = decoded.imageOrientation == .left || decoded.imageOrientation == .right || decoded.imageOrientation == .leftMirrored || decoded.imageOrientation == .rightMirrored
        let size = CGSize(width: rotated ? height : width, height: rotated ? width : height)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let clean = UIGraphicsImageRenderer(size: size, format: format).image { _ in decoded.draw(in: CGRect(origin: .zero, size: size)) }
        guard let png = clean.pngData(), png.count <= pngLimit else { throw Failure.image }
        return PaneRenderResult(ok: true, image: "data:image/png;base64," + png.base64EncodedString(), width: Int(size.width), height: Int(size.height))
    }

    private func call(_ web: WKWebView, _ script: String, _ args: [String: Any] = [:]) async throws -> [String: Any] {
        guard web.url?.scheme == "https", web.url?.host == "chatgpt.com" else { throw Failure.unsupported }
        let value = try await web.callAsyncJavaScript(script, arguments: args, in: nil, contentWorld: world)
        guard let result = value as? [String: Any] else { throw Failure.unsupported }
        return result
    }

    func run(_ request: PaneRenderRequest, client: AgentChatClient, local: Bool = false) async -> PaneRenderResult {
        let executorId = UUID().uuidString
        let generation = epoch
        let owner = ACILocalImageStore.shared.owner
        // Claim before every actionable refusal; a conflict must never produce a reply.
        if !local {
            do { try await client.claimBrowserImage(requestId: request.requestId, executorId: executorId) }
            catch { return PaneRenderResult(ok: false, error: "Image job is unavailable or already owned.", code: "executor_conflict") }
        }
        func refused(_ text: String, _ code: String) -> PaneRenderResult {
            PaneRenderResult(ok: false, error: text, code: code, executorId: local ? nil : executorId)
        }
        guard generation == epoch, owner == ACILocalImageStore.shared.owner else { return refused(Failure.stopped.localizedDescription, Failure.stopped.code) }
        guard !isBusy else { return refused("Another image job owns the ACI browser.", "browser_busy") }
        guard request.kind == .browserImage, request.engine == "chatgpt-aci", let prompt = request.prompt,
              let time = request.time, let timeout = request.timeoutMs,
              time.isFinite, timeout.isFinite, timeout > 0, timeout <= 600_000,
              Date().timeIntervalSince1970 * 1000 - time >= -30_000,
              Date().timeIntervalSince1970 * 1000 - time <= timeout else {
            return refused("Unsupported or expired browser image request.", "unsupported")
        }
        guard ACIBrowserImageLedger(defaults: .standard).reserve(request) else {
            return refused(Failure.replay.localizedDescription, Failure.replay.code)
        }
        isBusy = true
        hasSubmitted = false
        activeRequestId = request.requestId
        let deadline = Date(timeIntervalSince1970: (time + timeout) / 1000)
        defer {
            isBusy = false
            hasSubmitted = false
            needsResume = false
            resumeRequested = false
            activeRequestId = nil
            if let web = ACIBrowserController.shared.webView {
                Task { _ = try? await call(web, "if(window.__aciImage?.id === id) { window.__aciImage.abort?.abort(); window.__aciImage.download = null; window.__aciImage.active = false; } return {cleared:true};", ["id": request.requestId]) }
            }
        }
        func check() throws {
            guard epoch == generation, owner == ACILocalImageStore.shared.owner, !Task.isCancelled else { throw Failure.stopped }
            guard Date() < deadline else { throw Failure.expired }
        }
        var throttle = GenerationProgressThrottle()
        var observedPreview: String?
        func report(_ stage: GenerationStage, _ label: String, bytes: Int? = nil, total: Int? = nil, preview: String? = nil) async throws {
            try check()
            guard preview != nil || throttle.shouldSend(stage: stage, now: ProcessInfo.processInfo.systemUptime) else { return }
            if let preview { observedPreview = preview }
            let detail = GenerationProgressDetail(lane: .picture, stage: stage, label: label, preview: observedPreview, bytes: bytes, totalBytes: total)
            GenerationProgressStore.shared.set(request.toolCall ?? request.requestId, detail: detail)
            if !local, await client.postPaneRenderProgress(requestId: request.requestId, detail: detail, executorId: executorId) == false { cancel(); throw Failure.stopped }
        }
        do {
            try await report(.queued, "Opening ChatGPT in the ACI browser")
            // The confirmed chat action presents the browser. Do not route to a remote machine.
            for _ in 0..<50 {
                try check()
                if let web = ACIBrowserController.shared.webView, web.url != nil { break }
                try await Task.sleep(for: .milliseconds(100))
            }
            guard let web = ACIBrowserController.shared.webView else { throw Failure.unsupported }
            // An already retained page/draft is the person's property. Never navigate it away.
            guard web.url?.host == "chatgpt.com" else { throw Failure.unsupported }
            func pauseForPerson(_ label: String) async throws {
                resumeLabel = label
                needsResume = true
                resumeRequested = false
                defer { needsResume = false; resumeRequested = false }
                while !resumeRequested || UIApplication.shared.applicationState != .active || web.url?.host != "chatgpt.com" || web.isLoading {
                    try await report(.queued, "Needs you: " + label)
                    try await Task.sleep(for: .seconds(1))
                }
                try check()
            }
            var ready = false
            while !ready {
                try check()
                guard UIApplication.shared.applicationState == .active else {
                    try await report(.queued, "Paused while the app is in the background")
                    try await Task.sleep(for: .seconds(1)); continue
                }
                if !web.isLoading, web.url?.host == "chatgpt.com" {
                    let state = try await call(web, Self.prepareScript, ["prompt": prompt, "id": request.requestId])
                    if state["ready"] as? Bool == true { ready = true; break }
                    if let label = state["needsUser"] as? String { try await pauseForPerson(label) }
                    else if state["working"] as? Bool != true { throw Failure.unsupported }
                }
                try await Task.sleep(for: .seconds(1))
            }
            try check()
            // Save both the app chat identity and current website conversation before Send.
            if let url = web.url { ACIBrowserImageLedger(defaults: .standard).recordConversation(url, requestId: request.requestId) }
            // The durable fence above is already on disk before the only Send operation.
            hasSubmitted = true // Lock navigation even if Send's acknowledgement is ambiguous.
            let submitted = try await call(web, Self.submitScript, ["prompt": prompt, "id": request.requestId])
            guard submitted["sent"] as? Bool == true else { throw Failure.unsupported }
            try await report(.drawing, "Waiting for ChatGPT to generate this image")
            while true {
                try check()
                guard UIApplication.shared.applicationState == .active else {
                    try await report(.queued, "Paused; reconnecting when the app returns")
                    try await Task.sleep(for: .seconds(1)); continue
                }
                let state = try await call(web, Self.resultScript)
                if let url = web.url { ACIBrowserImageLedger(defaults: .standard).recordConversation(url, requestId: request.requestId) }
                if let label = state["needsUser"] as? String {
                    try await pauseForPerson(label)
                    continue
                }
                if state["unsupported"] as? Bool == true { throw Failure.unsupported }
                if let preview = state["preview"] as? String, preview.hasPrefix("data:image/jpeg;base64,"), preview.utf8.count <= 180_000 {
                    try await report(.rendering, "ChatGPT image preview", preview: preview)
                }
                if state["ready"] as? Bool == true { break }
                try await report(observedPreview == nil ? .drawing : .rendering, "Waiting for this ChatGPT turn's finished image")
                try await Task.sleep(for: .seconds(1))
            }
            // Fetch in the authenticated WebKit context. Only the adapter's current-turn
            // image is eligible. No URL supplied by native RPC or by page messages is accepted.
            _ = try await call(web, Self.downloadScript)
            var data = Data()
            var mime = ""
            while true {
                try check()
                guard UIApplication.shared.applicationState == .active else {
                    try await report(.queued, "Download paused; reconnecting when the app returns")
                    try await Task.sleep(for: .seconds(1)); continue
                }
                let state = try await call(web, Self.bytesScript)
                if state["error"] as? Bool == true { throw Failure.image }
                let received = state["bytes"] as? Int ?? 0
                try await report(.downloading, "Downloading the generated image", bytes: received, total: state["total"] as? Int)
                if let chunk = state["chunk"] as? String {
                    guard chunk.count <= 90_000, let decoded = Data(base64Encoded: chunk), data.count + decoded.count <= Self.sourceLimit else { throw Failure.image }
                    data.append(decoded)
                }
                if state["done"] as? Bool == true { mime = state["mime"] as? String ?? ""; break }
                try await Task.sleep(for: .milliseconds(100))
            }
            try await report(.saving, "Validating and saving the image", bytes: data.count, total: data.count)
            var result = try Self.normalize(data, mime: mime)
            result.executorId = executorId
            if let image = result.image.flatMap({ NativePaneRenderer.image(fromDataURL: $0) }) {
                let ratio = min(1, 320 / max(image.size.width, image.size.height))
                let size = CGSize(width: image.size.width * ratio, height: image.size.height * ratio)
                let format = UIGraphicsImageRendererFormat(); format.scale = 1
                let thumb = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
                if let jpeg = thumb.jpegData(compressionQuality: 0.65), jpeg.count <= 128 * 1024 {
                    try await report(.saving, "Saving the generated image", preview: "data:image/jpeg;base64," + jpeg.base64EncodedString())
                }
            }
            try check()
            return result
        } catch is CancellationError {
            return PaneRenderResult(ok: false, error: Failure.stopped.localizedDescription, code: Failure.stopped.code, executorId: executorId)
        } catch let failure as Failure {
            return PaneRenderResult(ok: false, error: failure.localizedDescription, code: failure.code, executorId: executorId)
        } catch {
            return PaneRenderResult(ok: false, error: "The ACI image job was interrupted. Check ChatGPT before starting another image action.", code: "ambiguous_submission", executorId: executorId)
        }
    }

    // All scripts are fixed native resources in an isolated content world. Provider DOM
    // differences fail closed. No general eval, message handler, screenshot or token bridge.
    static let prepareScript = #"""
    const visible = e => !!e && e.getClientRects().length > 0;
    const buttons = () => [...document.querySelectorAll('button,[role="button"],[role="menuitem"]')].filter(visible);
    const name = e => (e.getAttribute('aria-label') || e.innerText || '').trim();
    const challenge = document.querySelector('iframe[src*="challenge"], #challenge-running') || /verify you are human|checking your browser/i.test(document.body.innerText);
    if (challenge) return {needsUser:'Finish verification, then tap Resume'};
    if (buttons().some(e => /^(log in|sign in)$/i.test(name(e)))) return {needsUser:'Sign in to ChatGPT, then tap Resume'};
    const c = document.querySelector('#prompt-textarea[contenteditable="true"], textarea#prompt-textarea');
    if (!visible(c)) return {unsupported:true};
    const turns = document.querySelectorAll('[data-conversation-role], [data-message-author-role], [data-user-message-bubble]');
    if (location.pathname !== '/' || new URL(location.href).searchParams.get('temporary-chat') === 'true' ||
        /temporary chat/i.test(document.title) || buttons().some(e => /temporary/i.test(name(e)) && e.getAttribute('aria-pressed') === 'true') || turns.length) {
      return {needsUser:'Open a regular empty New chat (not Temporary Chat), then tap Resume'};
    }
    if (document.querySelector('button[data-testid="stop-button"], button[aria-label="Stop generating"]')) return {unsupported:true};
    const text = () => c instanceof HTMLTextAreaElement ? c.value : c.innerText;
    // Never replace a draft, even if it happens to match this job's prompt.
    let p = window.__aciPrepare;
    if (!p || p.id !== id) {
      if (text().trim()) return {needsUser:'Keep or save your draft and open an empty New chat, then tap Resume'};
      p = window.__aciPrepare = {id, mode:false, filled:false, menuOpened:false};
    }
    if (!p.mode) {
      const create = buttons().find(e => /^create image$/i.test(name(e)));
      if (create) { create.click(); p.mode = true; return {ready:false, working:true}; }
      const tools = buttons().find(e => /^(tools|view all tools|add files and more|add photos & files)$/i.test(name(e)));
      if (!p.menuOpened && tools) { tools.click(); p.menuOpened = true; return {ready:false, working:true}; }
      return {unsupported:true};
    }
    // Confirm the explicit image mode remains selected before editing or Send.
    const modeShown = buttons().some(e => /^(create image|image|remove create image|remove image)$/i.test(name(e))) ||
      [...document.querySelectorAll('[data-testid*="composer"]')].some(e => /create image/i.test(e.innerText));
    if (!modeShown) return {unsupported:true};
    if (!p.filled) {
      if (text().trim()) return {unsupported:true};
      c.focus();
      if (c instanceof HTMLTextAreaElement) {
        const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        setter.call(c, prompt);
        c.dispatchEvent(new InputEvent('input', {bubbles:true, inputType:'insertText', data:prompt}));
      } else {
        const selection = window.getSelection(), range = document.createRange();
        range.selectNodeContents(c); selection.removeAllRanges(); selection.addRange(range);
        if (!document.execCommand('insertText', false, prompt)) return {unsupported:true};
      }
      p.filled = true;
      return {ready:false, working:true};
    }
    const send = buttons().find(e => e.matches('button[data-testid="send-button"]') || /^send prompt$/i.test(name(e)));
    if (text() !== prompt || !send) return {unsupported:true};
    return {ready:!send.disabled, working:true};
    """#
    static let submitScript = #"""
    const c = document.querySelector('#prompt-textarea[contenteditable="true"], textarea#prompt-textarea');
    const b = [...document.querySelectorAll('button')].find(e => e.getClientRects().length &&
      (e.dataset.testid === 'send-button' || /^send prompt$/i.test(e.getAttribute('aria-label') || '')));
    const modeShown = [...document.querySelectorAll('button,[role="button"]')].some(e =>
      /^(create image|image|remove create image|remove image)$/i.test((e.getAttribute('aria-label') || e.innerText || '').trim())) ||
      [...document.querySelectorAll('[data-testid*="composer"]')].some(e => /create image/i.test(e.innerText));
    if (!modeShown || !window.__aciPrepare?.mode || window.__aciPrepare.id !== id || !c ||
        (c instanceof HTMLTextAreaElement ? c.value : c.innerText) !== prompt || !b || b.disabled ||
        location.pathname !== '/' || document.querySelector('[data-conversation-role], [data-message-author-role], [data-user-message-bubble]')) return {sent:false};
    const selector = '[data-conversation-role="user"], [data-message-author-role="user"], [data-user-message-bubble]';
    window.__aciImage = {id, prompt, users:new Set(document.querySelectorAll(selector)), turn:null, image:null, active:true};
    const job = window.__aciImage;
    // Guard provider SPA links as well as the native navigation controls. Authentication
    // and verification controls remain usable; New chat/history wait until completion.
    const navigationGuard = event => {
      if (!job.active) { document.removeEventListener('click', navigationGuard, true); return; }
      const target = event.target instanceof Element ? event.target.closest('a,button,[role="button"]') : null;
      if (!target) return;
      const label = (target.getAttribute('aria-label') || target.innerText || '').trim();
      const href = target.getAttribute('href');
      let leavesConversation = false;
      if (href) {
        try {
          const url = new URL(href, location.href);
          leavesConversation = url.origin === location.origin && (url.pathname === '/' || /^\/(c|g)\//.test(url.pathname));
        } catch { /* Non-navigation control. */ }
      }
      if (leavesConversation || /^(new chat|temporary chat)$/i.test(label)) {
        event.preventDefault(); event.stopImmediatePropagation();
      }
    };
    document.addEventListener('click', navigationGuard, true);
    // The one Send operation. Observation never calls this script again.
    b.click(); return {sent:true};
    """#
    static let resultScript = #"""
    const j = window.__aciImage;
    if (!j) return {unsupported:true};
    if (document.querySelector('iframe[src*="challenge"], #challenge-running') || /verify you are human|checking your browser/i.test(document.body.innerText))
      return {needsUser:'Finish verification without leaving this conversation, then tap Resume'};
    if ([...document.querySelectorAll('button')].some(e => /^(log in|sign in)$/i.test((e.getAttribute('aria-label') || e.innerText).trim())))
      return {needsUser:'Sign in without leaving this conversation, then tap Resume'};
    const selector = '[data-conversation-role="user"], [data-conversation-role="assistant"], [data-message-author-role="user"], [data-message-author-role="assistant"], [data-user-message-bubble]';
    const all = [...document.querySelectorAll(selector)].filter(e => !e.parentElement?.closest(selector));
    const role = e => e.dataset.conversationRole || e.dataset.messageAuthorRole || 'user';
    const users = all.filter(e => role(e) === 'user' && !j.users.has(e));
    const userText = e => (e.matches('[data-user-message-bubble]') ? e : e.querySelector('[data-user-message-bubble]') || e).innerText;
    if (users.length > 1 || (users.length === 1 && userText(users[0]).trim() !== j.prompt.trim())) return {unsupported:true};
    if (j.turn && !j.turn.isConnected) return {unsupported:true};
    if (!users.length) return {ready:false};
    if (j.turn && j.turn !== users[0]) return {unsupported:true};
    j.turn = users[0];
    const replies = all.slice(all.indexOf(j.turn)+1).filter(e => role(e) === 'assistant');
    if (!replies.length) return {ready:false};
    const reply = replies[replies.length-1];
    const allowed = i => {
      try {
        const u = new URL(i.currentSrc || i.src, location.href);
        return (u.protocol === 'https:' && u.hostname === 'chatgpt.com' && /^\/backend-api\/(estuary\/content|files\/)/.test(u.pathname)) ||
          (u.protocol === 'blob:' && u.origin === location.origin);
      } catch { return false; }
    };
    const images = [...reply.querySelectorAll('img')].filter(i => i.complete && i.naturalWidth >= 256 && i.naturalHeight >= 256 && allowed(i));
    if (images.length > 1) return {unsupported:true};
    const busy = !!document.querySelector('button[data-testid="stop-button"], button[aria-label="Stop generating"]') ||
      reply.getAttribute('aria-busy') === 'true' || !!reply.querySelector('[aria-busy="true"], [data-is-streaming="true"]');
    if (!images.length) {
      if (!busy) {
        j.noImageSince ??= Date.now();
        if (Date.now() - j.noImageSince > 30000) return {unsupported:true};
      }
      return {ready:false};
    }
    j.image = images[0];
    // Only real current-turn pixels. A tainted canvas leaves BlockPulse in place.
    let preview;
    if (j.previewSource !== j.image.currentSrc || j.previewWidth !== j.image.naturalWidth) {
      try {
        const canvas = document.createElement('canvas');
        const ratio = Math.min(1, 320 / Math.max(j.image.naturalWidth, j.image.naturalHeight));
        canvas.width = Math.max(1, Math.round(j.image.naturalWidth * ratio));
        canvas.height = Math.max(1, Math.round(j.image.naturalHeight * ratio));
        canvas.getContext('2d').drawImage(j.image, 0, 0, canvas.width, canvas.height);
        const data = canvas.toDataURL('image/jpeg', 0.65);
        if (data.startsWith('data:image/jpeg;base64,') && data.length <= 180000) preview = data;
        j.previewSource = j.image.currentSrc; j.previewWidth = j.image.naturalWidth;
      } catch { /* No fake preview or provider-private URL. */ }
    }
    const finished = /image created|created image/i.test(reply.innerText) ||
      [...reply.querySelectorAll('button,a,[role="button"]')].some(e =>
        /^(download( this image| image)?|save image|copy image)$/i.test((e.getAttribute('aria-label') || e.getAttribute('title') || e.innerText || '').trim()));
    if (!busy && !finished) {
      j.finishWaitSince ??= Date.now();
      if (Date.now() - j.finishWaitSince > 30000) return {unsupported:true};
    }
    return {ready:!busy && finished, preview};
    """#
    static let downloadScript = #"""
    const j = window.__aciImage;
    if (!j?.image?.isConnected) return {error:true};
    const url = new URL(j.image.currentSrc || j.image.src, location.href);
    if (!(url.protocol === 'https:' && url.hostname === 'chatgpt.com' && /^\/backend-api\/(estuary\/content|files\/)/.test(url.pathname)) && !(url.protocol === 'blob:' && url.origin === location.origin)) return {error:true};
    j.download = {bytes:0, chunks:[], done:false, error:false, mime:''};
    const d = j.download, abort = new AbortController(); j.abort = abort;
    const timer = setTimeout(() => abort.abort(), 60000);
    (async () => {
      try {
        const response = await fetch(url.href, {credentials:'same-origin', signal:abort.signal});
        if (!response.ok) throw Error('download');
        if (url.protocol !== 'blob:') {
          const finalURL = new URL(response.url);
          if (finalURL.protocol !== 'https:' || finalURL.hostname !== 'chatgpt.com' ||
              !/^\/backend-api\/(estuary\/content|files\/)/.test(finalURL.pathname)) throw Error('source');
        }
        d.mime = (response.headers.get('content-type') || '').split(';')[0].toLowerCase();
        if (!['image/png','image/jpeg','image/webp','image/heic'].includes(d.mime)) throw Error('mime');
        const total = Number(response.headers.get('content-length'));
        if (total > 20*1024*1024) throw Error('size');
        if (total > 0) d.total = total;
        const reader = response.body.getReader();
        for (;;) {
          const {done,value} = await reader.read(); if (done) break;
          d.bytes += value.length; if (d.bytes > 20*1024*1024) { await reader.cancel(); throw Error('size'); }
          for (let offset=0; offset<value.length; offset+=65536) {
            const chunk=value.subarray(offset,offset+65536); let binary='';
            for (const byte of chunk) binary += String.fromCharCode(byte);
            d.chunks.push(btoa(binary));
          }
        }
        d.done=true;
      } catch { d.error=true; } finally {clearTimeout(timer);}
    })();
    return {started:true};
    """#
    static let bytesScript = #"""
    const d = window.__aciImage?.download;
    if (!d) return {error:true};
    const chunk = d.chunks.shift();
    return {bytes:d.bytes,total:d.total,mime:d.mime,error:d.error,chunk,done:d.done && !d.chunks.length};
    """#
}

/// App-owned history and Outputs for phone-only image actions. No runtime resolution,
/// upload, OAuth credential, or website-private URL is needed to save or reopen a result.
@MainActor
final class ACILocalImageStore: ObservableObject {
    static let shared = ACILocalImageStore()
    static let prefix = "aci_local_"
    static func isLocal(_ id: String?) -> Bool { id?.hasPrefix(prefix) == true }
    struct Turn: Codable, Identifiable {
        let id: String
        let messageId: String
        let prompt: String
        let created: Date
        var filename: String?
        var error: String?
        var pending: Bool
    }
    struct TextTurn: Codable {
        let id: String
        let role: String
        let content: String
        let created: Date
    }
    struct Thread: Codable, Identifiable {
        let id: String
        var title: String
        let created: Date
        var updated: Date
        var prelude: [TextTurn]
        var turns: [Turn]
        var archived: Bool = false
    }
    @Published private(set) var threads: [String: Thread] = [:]
    private let directory: URL?
    private let ownerOverride: String?
    private var loadedOwner: String?
    var owner: String { ownerOverride ?? AuthManager.shared.currentUserId ?? "device" }
    init(directory: URL? = nil, owner: String? = nil) {
        self.directory = directory
        self.ownerOverride = owner
    }
    private var root: URL {
        let base = directory ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("ACIImages", isDirectory: true)
        let hash = SHA256.hash(data: Data(owner.utf8)).map { String(format: "%02x", $0) }.joined()
        return base.appendingPathComponent(hash, isDirectory: true)
    }
    func reloadIfNeeded() {
        guard loadedOwner != owner else { return }
        loadedOwner = owner
        let saved = try? Data(contentsOf: root.appendingPathComponent("history.json"))
        var loaded = saved.flatMap { try? JSONDecoder().decode([String: Thread].self, from: $0) } ?? [:]
        for key in Array(loaded.keys) {
            for index in loaded[key]!.turns.indices where loaded[key]!.turns[index].pending {
                loaded[key]!.turns[index].pending = false
                loaded[key]!.turns[index].error = "This image may have been submitted before the app closed. Check the ACI ChatGPT conversation. It will not be sent again."
            }
        }
        threads = loaded
    }
    func resetForAccountChange() { loadedOwner = nil; threads = [:] }
    private func save() throws {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try JSONEncoder().encode(threads).write(to: root.appendingPathComponent("history.json"), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    func createChat(history: [MessageRecord]) throws -> String {
        reloadIfNeeded()
        let id = Self.prefix + UUID().uuidString
        let before = threads
        threads[id] = Thread(id: id, title: "ChatGPT images", created: Date(), updated: Date(),
            prelude: history.filter { !$0.isStreaming }.map { TextTurn(id: $0.id, role: $0.role, content: $0.content, created: $0.createdAt) }, turns: [])
        do { try save() } catch { threads = before; throw error }
        return id
    }
    func begin(sessionId: String, id: String, messageId: String, prompt: String, history: [MessageRecord] = []) throws {
        reloadIfNeeded()
        let before = threads
        if threads[sessionId] == nil {
            threads[sessionId] = Thread(id: sessionId, title: String(prompt.prefix(60)), created: Date(), updated: Date(),
                prelude: history.filter { !$0.isStreaming }.map { TextTurn(id: $0.id, role: $0.role, content: $0.content, created: $0.createdAt) }, turns: [])
        }
        guard threads[sessionId]?.turns.contains(where: { $0.id == id }) == false else { throw ACIBrowserImageJob.Failure.replay }
        threads[sessionId]?.turns.append(Turn(id: id, messageId: messageId, prompt: prompt, created: Date(), pending: true))
        threads[sessionId]?.updated = Date()
        do { try save() } catch { threads = before; throw error }
        NotificationCenter.default.post(name: .historyMutated, object: nil)
    }
    func finish(sessionId: String, id: String, owner expectedOwner: String, result: PaneRenderResult) throws {
        guard owner == expectedOwner else { throw ACIBrowserImageJob.Failure.stopped }
        reloadIfNeeded()
        guard let index = threads[sessionId]?.turns.firstIndex(where: { $0.id == id }) else { throw ACIBrowserImageJob.Failure.stopped }
        if result.ok {
            guard let image = result.image, image.hasPrefix("data:image/png;base64,"),
                  let data = Data(base64Encoded: String(image.dropFirst("data:image/png;base64,".count))),
                  data.count <= ACIBrowserImageJob.pngLimit else { throw ACIBrowserImageJob.Failure.image }
            let filename = SHA256.hash(data: Data(id.utf8)).map { String(format: "%02x", $0) }.joined() + ".png"
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            try data.write(to: root.appendingPathComponent(filename), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            threads[sessionId]?.turns[index].filename = filename
        }
        threads[sessionId]?.turns[index].pending = false
        threads[sessionId]?.turns[index].error = result.error
        threads[sessionId]?.updated = Date()
        try save()
        NotificationCenter.default.post(name: .historyMutated, object: nil)
    }
    func file(_ turn: Turn) -> URL? {
        guard let filename = turn.filename,
              filename == SHA256.hash(data: Data(turn.id.utf8)).map({ String(format: "%02x", $0) }).joined() + ".png" else { return nil }
        let url = root.appendingPathComponent(filename)
        return FileManager.default.fileExists(atPath: url.path) ? url : nil
    }
    func entry(messageId: String) -> Turn? {
        threads.values.lazy.flatMap(\.turns).first { $0.messageId == messageId }
    }
    func messages(sessionId: String) -> [MessageRecord] {
        reloadIfNeeded()
        guard let thread = threads[sessionId] else { return [] }
        var messages = thread.prelude.map { MessageRecord(id: $0.id, role: $0.role, content: $0.content, createdAt: $0.created) }
        for turn in thread.turns {
            messages.append(MessageRecord(id: turn.id + "-user", role: "user", content: turn.prompt, createdAt: turn.created))
            messages.append(MessageRecord(id: turn.messageId, role: "assistant", content: "", isStreaming: turn.pending,
                toolCalls: [ToolCallRecord(id: turn.id, toolName: "media_generate", state: turn.pending ? .running : (turn.error == nil ? .done : .failed),
                    input: ["kind": "image", "engine": "chatgpt-aci", "prompt": turn.prompt], error: turn.error, startedAt: turn.created)], createdAt: turn.created.addingTimeInterval(0.001)))
        }
        return messages.sorted { $0.createdAt < $1.createdAt }
    }
    func appendText(sessionId: String, message: MessageRecord) throws {
        reloadIfNeeded()
        guard threads[sessionId] != nil else { return }
        threads[sessionId]?.prelude.removeAll { $0.id == message.id }
        threads[sessionId]?.prelude.append(TextTurn(id: message.id, role: message.role, content: message.content, created: message.createdAt))
        threads[sessionId]?.updated = Date()
        try save()
    }
    func sessions() -> [AgentSession] {
        reloadIfNeeded()
        let formatter = ISO8601DateFormatter()
        return threads.values.filter { Self.isLocal($0.id) }.compactMap { thread in
            let object: [String: Any] = ["id": thread.id, "name": thread.title, "created_at": formatter.string(from: thread.created),
                "updated_at": formatter.string(from: thread.updated), "message_count": thread.prelude.count + thread.turns.count * 2,
                "active": !thread.archived, "metadata": ["originSurface": "chat", "archived": thread.archived] as [String: Any]]
            guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
            return try? JSONDecoder().decode(AgentSession.self, from: data)
        }
    }
    func archive(_ id: String, archived: Bool) throws { reloadIfNeeded(); threads[id]?.archived = archived; try save() }
    func rename(_ id: String, title: String) throws { reloadIfNeeded(); threads[id]?.title = title; try save() }
    func delete(_ id: String) throws {
        reloadIfNeeded()
        if threads[id]?.turns.contains(where: { $0.pending }) == true { ACIBrowserImageJob.shared.cancel() }
        for turn in threads[id]?.turns ?? [] { if let url = file(turn) { try FileManager.default.removeItem(at: url) } }
        threads[id] = nil
        try save()
    }
}

/// The existing generation card owns progress; this is the finished local image and its
/// file actions. ShareLink's Download action exposes Save to Files without a server upload.
struct ACILocalImageTurnView: View {
    let messageId: String
    @ObservedObject private var store = ACILocalImageStore.shared
    @State private var opened = false
    var body: some View {
        if let turn = store.entry(messageId: messageId), let file = store.file(turn),
           let image = UIImage(contentsOfFile: file.path) {
            VStack(alignment: .leading, spacing: 10) {
                Image(uiImage: image).resizable().scaledToFit().clipShape(RoundedRectangle(cornerRadius: 12))
                HStack {
                    Button("Open", systemImage: "arrow.up.right.square") { opened = true }
                    ShareLink(item: file) { Label("Download", systemImage: "arrow.down.to.line") }
                    Button("Copy prompt", systemImage: "doc.on.doc") { UIPasteboard.general.string = turn.prompt }
                }
                .font(.footnote)
            }
            .sheet(isPresented: $opened) {
                NavigationStack {
                    ScrollView { Image(uiImage: image).resizable().scaledToFit() }
                        .navigationTitle("ChatGPT image")
                        .toolbar { Button("Done") { opened = false }; ShareLink(item: file) }
                }
            }
        }
    }
}

struct ACILocalImageOutputs: View {
    let sessionId: String?
    @ObservedObject private var store = ACILocalImageStore.shared
    @State private var opened = false
    var body: some View {
        if let sessionId, let thread = store.threads[sessionId], thread.turns.contains(where: { $0.filename != nil }) {
            Button("Outputs", systemImage: "photo.on.rectangle") { opened = true }
                .font(.footnote)
                .sheet(isPresented: $opened) {
                    NavigationStack {
                        ScrollView {
                            LazyVStack(spacing: 20) {
                                ForEach(thread.turns.filter { $0.filename != nil }) { turn in
                                    ACILocalImageTurnView(messageId: turn.messageId)
                                }
                            }.padding()
                        }
                        .navigationTitle("Outputs")
                        .toolbar { Button("Done") { opened = false } }
                    }
                }
        }
    }
}
