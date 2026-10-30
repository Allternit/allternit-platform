import SwiftUI
import UIKit
import WebKit

/// Runs a v2 artifact in the web's own viewer, signed in as the user.
///
/// Kinds the web draws with React and live runtimes — dashboards, ops
/// sheets, motion, cards, canvas designs, video projects and pages that use
/// stored data, AI or connectors — open here instead of in a native
/// renderer. There is no signed-in render URL on the server
/// (`/api/v2/artifacts/:id/render` is the motion video export), so this
/// hosts the web app's single-artifact route `ai.allternit.com/a/<id>`
/// (PublicArtifactPage: the kind's Viewer, nothing else) and hands it two
/// things before any page script runs:
///
/// 1. **The artifact.** The route reads `GET /api/v2/public/artifacts/:id`,
///    which only serves link-shared artifacts. The bridge answers that one
///    request with the record the app already fetched with the user's
///    bearer (`GET /api/v2/artifacts/:id`), in the public shape, with
///    `capabilities` folded into `meta` so PageViewer starts the page
///    runtime for pages that use storage/AI/connectors.
/// 2. **The sign-in.** `window.allternit.auth.getClerkToken()` is the
///    Desktop bridge the web's `hasCloudClerkSession` / `buildAuthHeaders`
///    already read, so the page runtime (`/api/v2/artifact-runtime/:id/*`:
///    storage, consents, AI billed to the viewer), dashboard/ops connector
///    calls and every other cloud call go out with the user's Clerk JWT.
///    `startLogin` is left out on purpose, so the web never switches into
///    Desktop-shell mode. `AuthenticatedWebBridge` also signs the page's
///    own-origin and cloud-API requests. Tokens come from the app on demand
///    over reply handlers, so they stay fresh past a Clerk JWT's ~60 s life.
///
/// The page runtime itself (sandbox-origin iframe + postMessage, the web's
/// PageRuntimeFrame) runs unchanged inside the web view. Links that leave
/// Allternit open in Safari.
///
/// Downloads and exports the web starts (MP4 export, CSV/XLSX/DOCX/PNG
/// files, `blob:` and `data:` links, server attachments) are caught —
/// `blob:`/`data:` by a script that reads the file and posts it back as
/// base64, http(s) by `WKDownload` — saved to a temp file and handed to the
/// share sheet, so the person can Save to Files or Photos. `window.print()`
/// (the web's "PDF" exports) opens the iOS print sheet, which saves a PDF. Data lives in an in-memory store shared by
/// these views for the app session, so nothing persists across launches.
///
/// With a `route`, it hosts the web's real editors instead of the viewer
/// (src/pages/AppEditorPage.tsx in allternit-ai): `ArtifactWindow` for a stored
/// artifact, or the Docs/Sheets/Slides and Design editors of a studio session.
/// Those pages have their own toolbar, so they call back to close the view,
/// and their "ask the chat" buttons call back with the text.
struct ArtifactLiveWebView: View {
    /// The viewer's record; nil when a `route` opens an editor.
    var artifact: ArtifactV2? = nil
    var route: ArtifactWebRoute? = nil
    var onClose: (() -> Void)? = nil
    var onChat: ((String) -> Void)? = nil
    @Environment(\.colorScheme) private var colorScheme

    enum Phase: Equatable {
        case loading
        case ready
        case failed(String)
    }

    /// What a download or export is doing.
    enum DownloadEvent {
        case started
        case finished(URL)
        case failed(String)
    }

    @State private var phase: Phase = .loading
    @State private var reloadToken = 0
    @State private var isExporting = false
    @State private var sharedFile: SharedArtifactFile?
    @State private var exportError: String?

    /// The web app that serves `/a/<id>` (DEFAULT_PUBLIC_BASE_URL in
    /// artifacts_v2.rs).
    static let webOrigin: URL = {
        #if DEBUG
        // `-web-url <url>` (DEBUG only): a local or preview build of the web app.
        if let override = UserDefaults.standard.string(forKey: "web-url"), let url = URL(string: override) {
            return url
        }
        #endif
        return URL(string: "https://ai.allternit.com")!
    }()

    private var noun: String { artifact?.knownKind?.noun.lowercased() ?? "artifact" }

    var body: some View {
        ZStack {
            ArtifactLiveWebContainer(artifact: artifact, route: route, dark: colorScheme == .dark, onClose: onClose, onChat: onChat, reloadToken: reloadToken, onPhase: { next in
                phase = next
            }, onDownload: { event in
                switch event {
                case .started:
                    isExporting = true
                case .finished(let url):
                    isExporting = false
                    sharedFile = SharedArtifactFile(url: url)
                case .failed(let message):
                    isExporting = false
                    exportError = message
                }
            })
            .opacity(phase == .ready ? 1 : 0.001)

            if isExporting {
                VStack {
                    Spacer()
                    ArtifactExportProgressPill(label: "Preparing download…")
                        .padding(.bottom, 24)
                }
                .transition(.opacity)
            }

            switch phase {
            case .loading:
                ProgressView("Opening \(noun)…")
            case .ready:
                EmptyView()
            case .failed(let message):
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't open this \(noun)",
                    message: message,
                    actionTitle: "Retry",
                    action: {
                        phase = .loading
                        reloadToken += 1
                    }
                )
                .background(Color("BgPrimary"))
            }
        }
        .animation(.easeOut(duration: 0.2), value: isExporting)
        .sheet(item: $sharedFile) { file in
            ArtifactShareSheet(items: [file.url])
                .presentationDetents([.medium, .large])
        }
        .alert("Couldn't save the file", isPresented: Binding(get: { exportError != nil }, set: { if !$0 { exportError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(exportError ?? "")
        }
    }

    /// Whether a page body needs the page runtime (port of the web's
    /// `pageCapabilities` + `hasCapabilities` checks in PageViewer).
    static func pageNeedsRuntime(_ a: ArtifactV2) -> Bool {
        if hasCapabilities(a.capabilities) { return true }
        if let meta = a.version.meta, hasCapabilities(meta["capabilities"]) { return true }
        let body = a.version.body
        if body.range(of: #"<meta\b[^>]*\bname\s*=\s*["']allternit:capabilities["']"#, options: [.regularExpression, .caseInsensitive]) != nil {
            return true
        }
        if body.range(of: #"\ballternit\s*\.\s*storage\s*\.\s*(get|set|delete|list|getJSON|setJSON)\b"#, options: .regularExpression) != nil {
            return true
        }
        return body.range(of: #"\ballternit\s*\.\s*ai\s*\.\s*complete\b"#, options: .regularExpression) != nil
    }

    private static func hasCapabilities(_ value: JSONValue?) -> Bool {
        guard case .object(let caps)? = value else { return false }
        if case .bool(true)? = caps["storage"] { return true }
        if case .bool(true)? = caps["ai"] { return true }
        if case .array(let list)? = caps["connectors"], !list.isEmpty { return true }
        return false
    }
}

/// The record in the public route's shape (`public_artifact` in
/// artifacts_v2.rs / PublicArtifact in PublicArtifactPage.tsx).
private struct ArtifactLiveSeed: Encodable {
    let id: String
    let kind: String
    let title: String
    let icon: String?
    let body: String
    let bodyFormat: String
    let meta: [String: JSONValue]
    let version: Int
    let updatedAt: String
    let runtimeVersion: Int?
    let ownerName: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, title, icon, body, meta, version
        case bodyFormat = "body_format"
        case updatedAt = "updated_at"
        case runtimeVersion = "runtime_version"
        case ownerName = "owner_name"
    }

    init(_ a: ArtifactV2) {
        id = a.id
        kind = a.kind
        title = a.title
        icon = a.icon
        body = a.version.body
        bodyFormat = a.version.bodyFormat
        var meta = a.version.meta ?? [:]
        if meta["capabilities"] == nil, let caps = a.capabilities, caps != .null {
            meta["capabilities"] = caps
        }
        self.meta = meta
        version = a.version.version
        updatedAt = a.updatedAt
        runtimeVersion = a.runtimeVersion
        ownerName = a.owner.name
    }
}

/// A web editor page (allternit-ai src/routes.tsx).
enum ArtifactWebRoute: Equatable {
    case window(id: String)
    case office(product: String, artifactId: String)
    case design(projectId: String, title: String)

    /// Identity for a pane that swaps pages.
    var paneKey: String {
        switch self {
        case .window(let id): return "window:\(id)"
        case .office(_, let artifactId): return "office:\(artifactId)"
        case .design(let projectId, _): return "design:\(projectId)"
        }
    }

    func url(dark: Bool) -> URL {
        // The sample workspace is answered inside the page on the /embed path.
        let sample = SampleWorkspace.isEnabled
        func escaped(_ value: String) -> String {
            value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? value
        }
        let path: String
        var items = [URLQueryItem(name: "theme", value: dark ? "dark" : "light")]
        switch self {
        case .window(let id):
            path = sample ? "/embed/editor/a/\(escaped(id))" : "/a/\(escaped(id))/edit"
        case .office(let product, let artifactId):
            path = "\(sample ? "/embed" : "")/editor/office/\(escaped(product))/\(escaped(artifactId))"
        case .design(let projectId, let title):
            path = "\(sample ? "/embed" : "")/editor/design/\(escaped(projectId))"
            items.append(URLQueryItem(name: "title", value: title))
        }
        var components = URLComponents(url: ArtifactLiveWebView.webOrigin, resolvingAgainstBaseURL: false)!
        components.path = path
        components.queryItems = items
        return components.url!
    }
}

private struct ArtifactLiveWebContainer: UIViewRepresentable {
    let artifact: ArtifactV2?
    let route: ArtifactWebRoute?
    let dark: Bool
    let onClose: (() -> Void)?
    let onChat: ((String) -> Void)?
    let reloadToken: Int
    let onPhase: (ArtifactLiveWebView.Phase) -> Void
    let onDownload: (ArtifactLiveWebView.DownloadEvent) -> Void

    static let clerkHandlerName = "allternitClerk"
    static let downloadHandlerName = "allternitDownload"
    static let editorHandlerName = "allternitEditor"

    /// One in-memory store for every live artifact view this launch: the web
    /// bundle stays cached between opens, and nothing outlives the process.
    @MainActor private static let dataStore = WKWebsiteDataStore.nonPersistent()

    private var pageURL: URL {
        if let route { return route.url(dark: dark) }
        return artifact.map(Self.pageURL(for:)) ?? ArtifactLiveWebView.webOrigin
    }

    func makeCoordinator() -> Coordinator { Coordinator(onPhase: onPhase, onDownload: onDownload) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // Editors keep drafts and Design canvases in the page's storage, as on the web.
        configuration.websiteDataStore = route == nil ? Self.dataStore : .default()
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        let preferences = WKWebpagePreferences()
        preferences.allowsContentJavaScript = true
        configuration.defaultWebpagePreferences = preferences

        let controller = configuration.userContentController
        AuthenticatedWebBridge.install(
            into: controller,
            handler: context.coordinator.authHandler,
            trustedOrigins: [ArtifactLiveWebView.webOrigin, AppConfig.cloudAPIBaseURL, AppConfig.apiBaseURL]
        )
        controller.addScriptMessageHandler(context.coordinator.clerkHandler, contentWorld: .page, name: Self.clerkHandlerName)
        controller.addUserScript(WKUserScript(
            source: artifact.map(Self.seedScript(for:)) ?? Self.signInScript,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true,
            in: .page
        ))
        controller.add(context.coordinator.downloadHandler, contentWorld: .page, name: Self.downloadHandlerName)
        if route != nil {
            context.coordinator.editorHandler.onClose = onClose
            context.coordinator.editorHandler.onChat = onChat
            controller.add(context.coordinator.editorHandler, contentWorld: .page, name: Self.editorHandlerName)
        }
        controller.addUserScript(WKUserScript(
            source: Self.downloadScript,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: false,
            in: .page
        ))

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        context.coordinator.reloadToken = reloadToken
        context.coordinator.load(pageURL, into: webView)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.onPhase = onPhase
        context.coordinator.onDownload = onDownload
        context.coordinator.editorHandler.onClose = onClose
        context.coordinator.editorHandler.onChat = onChat
        if context.coordinator.reloadToken != reloadToken {
            context.coordinator.reloadToken = reloadToken
            context.coordinator.load(pageURL, into: webView)
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: AuthenticatedWebBridge.handlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: clerkHandlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: downloadHandlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: editorHandlerName, contentWorld: .page)
        controller.removeAllUserScripts()
    }

    static func pageURL(for a: ArtifactV2) -> URL {
        let escaped = a.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? a.id
        return URL(string: "\(ArtifactLiveWebView.webOrigin.absoluteString)/a/\(escaped)")!
    }

    /// Runs before an editor page: the Desktop-style Clerk getter alone.
    static let signInScript = """
    (function () {
      var handlers = window.webkit && window.webkit.messageHandlers;
      var clerk = handlers && handlers.\(clerkHandlerName);
      if (!clerk) return;
      window.allternit = window.allternit || {};
      if (!window.allternit.auth) {
        window.allternit.auth = {
          getClerkToken: function () {
            return clerk.postMessage(null).then(function (t) { return t || null; }, function () { return null; });
          }
        };
      }
    })();
    """

    /// Runs before the web bundle: the Desktop-style Clerk getter, the
    /// answer to the route's public fetch, and the in-app chrome tweaks.
    static func seedScript(for a: ArtifactV2) -> String {
        let seedJSON = (try? JSONEncoder().encode(ArtifactLiveSeed(a))).map { String(decoding: $0, as: UTF8.self) } ?? "null"
        let pathJSON = ArtifactHTML.jsString("/api/v2/public/artifacts/" + (a.id.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-_.~"))) ?? a.id))
        return """
        (function () {
          var handlers = window.webkit && window.webkit.messageHandlers;
          var clerk = handlers && handlers.\(clerkHandlerName);
          if (!clerk) return;
          var seed = \(seedJSON);
          var seedPath = \(pathJSON);
          window.allternit = window.allternit || {};
          if (!window.allternit.auth) {
            window.allternit.auth = {
              getClerkToken: function () {
                return clerk.postMessage(null).then(function (t) { return t || null; }, function () { return null; });
              }
            };
          }
          var origFetch = window.fetch;
          window.fetch = function (input, init) {
            try {
              var raw = typeof input === 'string' ? input : (input && input.url) || String(input);
              var u = new URL(raw, location.href);
              if (seed && u.pathname === seedPath) {
                return Promise.resolve(new Response(JSON.stringify(seed), {
                  status: 200, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }
                }));
              }
            } catch (e) {}
            return origFetch.apply(this, arguments);
          };
          // The app's navigation bar names the artifact; drop the public
          // page's own title strip ("user-generated" notice + "Made with").
          function hideHeader() {
            var s = document.createElement('style');
            s.textContent = 'main > header:first-child{display:none!important}';
            (document.head || document.documentElement).appendChild(s);
          }
          if (document.documentElement) hideHeader(); else document.addEventListener('DOMContentLoaded', hideHeader);
        })();
        """
    }

    /// Runs in every frame: `blob:`/`data:` download links (clicked, or
    /// `a.click()` on a detached anchor) are read in the page and posted to
    /// the app as base64; `window.print()` asks the app for the print sheet,
    /// with the frame's HTML when a hidden frame prints (printHtml).
    static let downloadScript = """
    (function () {
      if (window.__allternitDownloads) return;
      window.__allternitDownloads = true;
      var handlers = window.webkit && window.webkit.messageHandlers;
      var h = handlers && handlers.\(downloadHandlerName);
      if (!h) return;
      var MAX = \(ArtifactFileShare.maxBytes);
      function post(m) { try { h.postMessage(m); } catch (e) {} }
      function save(href, name) {
        post({ stage: 'start' });
        fetch(href).then(function (r) { return r.blob(); }).then(function (b) {
          if (b.size > MAX) { post({ error: 'tooLarge' }); return; }
          var fr = new FileReader();
          fr.onload = function () {
            var s = String(fr.result || '');
            var i = s.indexOf(',');
            post({ name: name || '', mime: b.type || '', data: i >= 0 ? s.slice(i + 1) : '' });
          };
          fr.onerror = function () { post({ error: 'read' }); };
          fr.readAsDataURL(b);
        }).catch(function () { post({ error: 'read' }); });
      }
      window.__allternitSaveURL = save;
      function intercept(a) {
        var href = (a && a.href) || '';
        if (!/^(blob|data):/i.test(href)) return false;
        save(href, a.getAttribute('download') || '');
        return true;
      }
      document.addEventListener('click', function (e) {
        var t = e.target;
        var a = t && t.closest ? t.closest('a[href]') : null;
        if (a && intercept(a)) e.preventDefault();
      }, true);
      var nativeClick = HTMLAnchorElement.prototype.click;
      HTMLAnchorElement.prototype.click = function () {
        if (!this.isConnected && intercept(this)) return;
        return nativeClick.apply(this, arguments);
      };
      window.print = function () {
        post(window === window.top ? { kind: 'print' } : { kind: 'print', html: document.documentElement.outerHTML });
      };
      try {
        var desc = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow');
        if (desc && desc.get) {
          Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
            configurable: true,
            enumerable: desc.enumerable,
            get: function () {
              var w = desc.get.call(this);
              try {
                if (w && w.document && !w.__allternitPrint) {
                  w.__allternitPrint = true;
                  w.print = function () { post({ kind: 'print', html: w.document.documentElement.outerHTML }); };
                }
              } catch (e) {}
              return w;
            }
          });
        }
      } catch (e) {}
    })();
    """

    /// What an editor page tells the app (AppEditorPage's `tellApp`).
    @MainActor
    final class EditorHandler: NSObject, WKScriptMessageHandler {
        var onClose: (() -> Void)?
        var onChat: ((String) -> Void)?

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any] else { return }
            switch body["type"] as? String {
            case "close": onClose?()
            case "chat": onChat?(body["text"] as? String ?? "")
            default: break
            }
        }
    }

    /// Files posted by `downloadScript`.
    @MainActor
    final class DownloadHandler: NSObject, WKScriptMessageHandler {
        weak var coordinator: Coordinator?

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any] else { return }
            coordinator?.handleScriptMessage(body, isMainFrame: message.frameInfo.isMainFrame, webView: message.webView)
        }
    }

    /// Bearer for `AuthenticatedWebBridge` (Clerk first, then the runtime
    /// device token, like every other v2 call).
    @MainActor
    final class AuthHandler: NSObject, WKScriptMessageHandlerWithReply {
        @MainActor
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            (await ArtifactsV2Client.bearerToken(), nil)
        }
    }

    /// Clerk session JWT only — what the web's Desktop bridge returns. A
    /// device token is not a session, so a device-only app reads as signed
    /// out to the page and its AI/storage calls say so.
    @MainActor
    final class ClerkHandler: NSObject, WKScriptMessageHandlerWithReply {
        @MainActor
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            let token: String? = (try? await AuthManager.shared.getToken()) ?? nil
            let value: Any? = (token?.isEmpty == false) ? token : nil
            return (value, nil)
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKDownloadDelegate {
        let authHandler = AuthHandler()
        let clerkHandler = ClerkHandler()
        let downloadHandler = DownloadHandler()
        let editorHandler = EditorHandler()
        var onPhase: (ArtifactLiveWebView.Phase) -> Void
        var onDownload: (ArtifactLiveWebView.DownloadEvent) -> Void
        var reloadToken = 0
        private var pageURL: URL?
        /// Where each running WKDownload is being written.
        private var downloadDestinations: [ObjectIdentifier: URL] = [:]
        private var isPrinting = false

        init(onPhase: @escaping (ArtifactLiveWebView.Phase) -> Void,
             onDownload: @escaping (ArtifactLiveWebView.DownloadEvent) -> Void) {
            self.onPhase = onPhase
            self.onDownload = onDownload
            super.init()
            downloadHandler.coordinator = self
        }

        // MARK: Downloads from the page script

        func handleScriptMessage(_ body: [String: Any], isMainFrame: Bool, webView: WKWebView?) {
            if body["kind"] as? String == "print" {
                presentPrint(html: body["html"] as? String, webView: webView)
                return
            }
            if body["stage"] as? String == "start" {
                onDownload(.started)
                return
            }
            if let error = body["error"] as? String {
                onDownload(.failed(error == "tooLarge"
                    ? (ArtifactFileShare.ShareError.tooLarge.errorDescription ?? "")
                    : "The file couldn't be read. Try the export again."))
                return
            }
            guard let base64 = body["data"] as? String else { return }
            let name = body["name"] as? String
            let mime = body["mime"] as? String
            Task {
                let result: Result<URL, Error> = await Task.detached(priority: .userInitiated) {
                    guard let data = Data(base64Encoded: base64, options: .ignoreUnknownCharacters) else {
                        return .failure(ArtifactFileShare.ShareError.empty)
                    }
                    return Result { try ArtifactFileShare.write(data, suggestedName: name, mimeType: mime) }
                }.value
                switch result {
                case .success(let url): self.onDownload(.finished(url))
                case .failure(let error): self.onDownload(.failed(error.localizedDescription))
                }
            }
        }

        /// Reads a `blob:`/`data:` URL inside the page (only the page can).
        private func saveInPage(_ url: URL, webView: WKWebView) {
            let js = "window.__allternitSaveURL && window.__allternitSaveURL(\(ArtifactHTML.jsString(url.absoluteString)), '')"
            webView.evaluateJavaScript(js, completionHandler: nil)
        }

        /// The iOS print sheet: prints the live page, or a hidden frame's
        /// HTML. Its Share button saves the PDF to Files.
        private func presentPrint(html: String?, webView: WKWebView?) {
            guard !isPrinting else { return }
            let controller = UIPrintInteractionController.shared
            let info = UIPrintInfo(dictionary: nil)
            info.outputType = .general
            info.jobName = webView?.title.flatMap { $0.isEmpty ? nil : $0 } ?? "Artifact"
            controller.printInfo = info
            if let html, !html.isEmpty {
                controller.printFormatter = UIMarkupTextPrintFormatter(markupText: html)
            } else if let webView {
                controller.printFormatter = webView.viewPrintFormatter()
            } else {
                return
            }
            isPrinting = true
            controller.present(animated: true) { [weak self] _, _, _ in
                Task { @MainActor in self?.isPrinting = false }
            }
        }

        // MARK: WKDownload (http(s) attachments and anything WebKit downloads)

        func webView(_ webView: WKWebView, navigationAction: WKNavigationAction, didBecome download: WKDownload) {
            download.delegate = self
            onDownload(.started)
        }

        func webView(_ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload) {
            download.delegate = self
            onDownload(.started)
        }

        func download(_ download: WKDownload, decideDestinationUsing response: URLResponse, suggestedFilename: String) async -> URL? {
            if response.expectedContentLength > Int64(ArtifactFileShare.maxBytes) {
                onDownload(.failed(ArtifactFileShare.ShareError.tooLarge.errorDescription ?? ""))
                return nil
            }
            let name = ArtifactFileShare.fileName(suggestedFilename, mimeType: response.mimeType)
            guard let url = try? ArtifactFileShare.destination(named: name) else {
                onDownload(.failed("The file couldn't be saved on this phone."))
                return nil
            }
            downloadDestinations[ObjectIdentifier(download)] = url
            return url
        }

        func downloadDidFinish(_ download: WKDownload) {
            if let url = downloadDestinations.removeValue(forKey: ObjectIdentifier(download)) {
                onDownload(.finished(url))
            }
        }

        func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
            let hadDestination = downloadDestinations.removeValue(forKey: ObjectIdentifier(download)) != nil
            let nsError = error as NSError
            if nsError.domain == NSURLErrorDomain, nsError.code == NSURLErrorCancelled, !hadDestination { return }
            onDownload(.failed(FriendlyErrorMessage.from(error.localizedDescription)))
        }

        func webView(_ webView: WKWebView,
                     decidePolicyFor navigationResponse: WKNavigationResponse,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationResponsePolicy) -> Void) {
            let disposition = (navigationResponse.response as? HTTPURLResponse)?
                .value(forHTTPHeaderField: "Content-Disposition")?.lowercased() ?? ""
            if !navigationResponse.canShowMIMEType || disposition.hasPrefix("attachment") {
                decisionHandler(.download)
            } else {
                decisionHandler(.allow)
            }
        }

        func load(_ url: URL, into webView: WKWebView) {
            pageURL = url
            onPhase(.loading)
            webView.load(URLRequest(url: url, cachePolicy: .useProtocolCachePolicy, timeoutInterval: 30))
        }

        private func isAppPage(_ url: URL) -> Bool {
            url.scheme?.lowercased() == "https" && url.host == ArtifactLiveWebView.webOrigin.host
                && url.path.hasPrefix("/a/")
        }

        func webView(_ webView: WKWebView,
                     decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
            if navigationAction.shouldPerformDownload {
                if let url = navigationAction.request.url, let scheme = url.scheme?.lowercased(), ["blob", "data"].contains(scheme) {
                    saveInPage(url, webView: webView)
                    decisionHandler(.cancel)
                } else {
                    decisionHandler(.download)
                }
                return
            }
            // Frames (the page runtime's sandbox origin, embeds) load freely;
            // the main frame stays on the artifact route.
            guard navigationAction.targetFrame?.isMainFrame ?? false, let url = navigationAction.request.url else {
                decisionHandler(.allow)
                return
            }
            if isAppPage(url) || url.absoluteString == "about:blank" {
                decisionHandler(.allow)
                return
            }
            if let scheme = url.scheme?.lowercased(), ["blob", "data"].contains(scheme) {
                // `location = blobURL` / opening a data URL: save it.
                saveInPage(url, webView: webView)
            } else if let scheme = url.scheme?.lowercased(), ["http", "https", "mailto", "tel"].contains(scheme) {
                UIApplication.shared.open(url)
            }
            decisionHandler(.cancel)
        }

        func webView(_ webView: WKWebView,
                     createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction,
                     windowFeatures: WKWindowFeatures) -> WKWebView? {
            if let url = navigationAction.request.url, let scheme = url.scheme?.lowercased() {
                if ["blob", "data"].contains(scheme) {
                    // `window.open(blobURL)` for an export: save it.
                    saveInPage(url, webView: webView)
                } else if ["http", "https"].contains(scheme) {
                    UIApplication.shared.open(url)
                }
            }
            return nil
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            // The route paints its own skeleton while the viewer chunk
            // loads; show the web view as soon as the document is in.
            onPhase(.ready)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            fail(error)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            fail(error)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            if let pageURL { load(pageURL, into: webView) }
        }

        private func fail(_ error: Error) {
            let nsError = error as NSError
            // A cancelled navigation (our own policy, or a quick reload) isn't a failure.
            if nsError.domain == NSURLErrorDomain, nsError.code == NSURLErrorCancelled { return }
            if nsError.domain == "WebKitErrorDomain", nsError.code == 102 { return }
            onPhase(.failed(FriendlyErrorMessage.from(error.localizedDescription)))
        }
    }
}
