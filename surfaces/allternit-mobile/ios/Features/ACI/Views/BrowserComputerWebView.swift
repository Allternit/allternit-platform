import SwiftUI
import WebKit

/// The "Browser computer": a small Linux VM that runs inside a WKWebView on
/// `https://box.allternit.com/` (single-CPU embedded mode, CONTRACT
/// colibri-unkernel section 6). The page needs JSPI (`WebAssembly.Suspending`),
/// which WebKit ships from iOS 27, so a capability probe runs first and the
/// computer never boots without it.
///
/// Bridge: the box page speaks the section 6.2 message format over
/// `window.postMessage` with a parent window. A native host has no parent, so
/// a document-start script gives the page a stand-in `window.parent` and hands
/// it messages as if they came from `https://ai.allternit.com` (one of the
/// origins the page already allows). Page replies go out through the
/// `allternitBox` script message handler. The wire shapes are unchanged.
///
/// The WKWebView uses the default (persistent) website data store, so the
/// disk in IndexedDB on the box origin survives app restarts.

// MARK: - Model

@MainActor
final class BrowserComputerModel: NSObject, ObservableObject {
    enum Phase: Equatable {
        case probing
        case unsupported
        case idle            // supported, not started
        case starting        // handshake, list or create
        case running         // VM state known, see `state`
        case stopped         // web content process was killed
        case failed(String)
    }

    struct Status: Equatable {
        var state: String = "idle"
        var computerId: String?
        var progress: (loaded: Int, total: Int)?
        var diskBytesUsed: Int?
        var persisted: Bool?
        var errorMessage: String?

        static func == (a: Status, b: Status) -> Bool {
            a.state == b.state && a.computerId == b.computerId
                && a.progress?.loaded == b.progress?.loaded && a.progress?.total == b.progress?.total
                && a.diskBytesUsed == b.diskBytesUsed && a.persisted == b.persisted
                && a.errorMessage == b.errorMessage
        }
    }

    static let boxURL = URL(string: "https://box.allternit.com/")!
    static let handlerName = "allternitBox"
    static let unsupportedLine = "The browser computer needs iOS 27 or later."
    static let memoryLine = "The computer stopped because the phone ran low on memory. Start it again."

    @Published private(set) var phase: Phase = .probing
    @Published private(set) var status = Status()
    @Published private(set) var output = ""
    @Published private(set) var isResetting = false

    private(set) var webView: WKWebView?
    private var pending: [String: CheckedContinuation<[String: Any], Error>] = [:]
    private var ptyId: String?
    private var computerId: String?

    struct RPCError: LocalizedError {
        let code: String
        let message: String
        var errorDescription: String? { message }
    }

    // MARK: Probe

    /// Runs `typeof WebAssembly.Suspending === 'function'` in a bare web view
    /// before anything from the box origin is loaded.
    func probe() async {
        guard phase == .probing else { return }
        let bare = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        let supported = (try? await bare.evaluateJavaScript("typeof WebAssembly !== 'undefined' && typeof WebAssembly.Suspending === 'function'")) as? Bool
        phase = supported == true ? .idle : .unsupported
    }

    // MARK: Start and stop

    func start() async {
        guard phase == .idle || phase == .stopped || isFailed else { return }
        phase = .starting
        output = ""
        status = Status()
        ptyId = nil
        let view = makeWebView()
        webView = view
        view.load(URLRequest(url: Self.boxURL))
        do {
            try await handshake()
            let record = try await ensureComputer()
            computerId = record
            phase = .running
            _ = try await call("boot", ["computer_id": record, "memory_mib": 256, "cpus": 1,
                                        "network": ["mode": "off", "custom_url": NSNull()]])
        } catch let e as RPCError {
            fail(Self.plain(e))
        } catch {
            fail("The computer did not start. Try again.")
        }
    }

    private var isFailed: Bool { if case .failed = phase { return true } else { return false } }

    func stop() {
        Task { _ = try? await call("halt", [:]) }
        tearDown()
        phase = .idle
        status = Status()
    }

    private func tearDown() {
        webView?.configuration.userContentController.removeScriptMessageHandler(forName: Self.handlerName)
        webView?.stopLoading()
        webView = nil
        for (_, c) in pending { c.resume(throwing: RPCError(code: "host_error", message: "Stopped.")) }
        pending.removeAll()
    }

    private func fail(_ message: String) {
        phase = .failed(message)
    }

    static func plain(_ e: RPCError) -> String {
        switch e.code {
        case "no_jspi": return unsupportedLine
        case "busy_owner": return "This computer is open in another tab."
        case "image_unavailable": return "The computer image is not available right now. Try again later."
        case "storage_full": return "The phone has no room left for the computer's disk."
        default: return e.message.isEmpty ? "The computer did not start. Try again." : e.message
        }
    }

    // MARK: Web view

    private func makeWebView() -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        let controller = configuration.userContentController
        controller.add(WeakHandler(self), name: Self.handlerName)
        controller.addUserScript(WKUserScript(source: Self.shim, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = self
        view.isInspectable = false
        return view
    }

    /// Stand-in parent window for the box page (see the type comment).
    private static let shim = """
    (function () {
      var NAME = '\(handlerName)', ORIGIN = 'https://ai.allternit.com';
      var handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers[NAME];
      if (!handler) return;
      var fakeParent = { postMessage: function (msg) { handler.postMessage(JSON.stringify(msg)); } };
      try { Object.defineProperty(window, 'parent', { value: fakeParent, configurable: true }); } catch (e) {}
      var listeners = [];
      var add = window.addEventListener.bind(window), rem = window.removeEventListener.bind(window);
      window.addEventListener = function (type, fn, opts) {
        if (type === 'message' && fn) listeners.push(fn);
        return add(type, fn, opts);
      };
      window.removeEventListener = function (type, fn, opts) {
        if (type === 'message') listeners = listeners.filter(function (l) { return l !== fn; });
        return rem(type, fn, opts);
      };
      window.__allternitHostDeliver = function (data) {
        var ev = { data: data, origin: ORIGIN, source: fakeParent, type: 'message' };
        listeners.slice().forEach(function (l) {
          try { typeof l === 'function' ? l.call(window, ev) : l.handleEvent(ev); } catch (e) {}
        });
      };
    })();
    """

    // MARK: RPC

    @discardableResult
    func call(_ method: String, _ params: [String: Any], timeout: TimeInterval = 20) async throws -> [String: Any] {
        guard let view = webView else { throw RPCError(code: "not_ready", message: "The computer is not running.") }
        let id = UUID().uuidString
        let message: [String: Any] = ["v": 1, "kind": "req", "id": id, "method": method, "params": params]
        let data = try JSONSerialization.data(withJSONObject: message)
        let json = String(decoding: data, as: UTF8.self)
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            view.evaluateJavaScript("window.__allternitHostDeliver && window.__allternitHostDeliver(\(json))") { [weak self] _, error in
                if error != nil { Task { @MainActor in self?.resolve(id, .failure(RPCError(code: "not_ready", message: "The page is not ready."))) } }
            }
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
                self?.resolve(id, .failure(RPCError(code: "timeout", message: "The computer did not answer.")))
            }
        }
    }

    private func resolve(_ id: String, _ result: Result<[String: Any], Error>) {
        guard let c = pending.removeValue(forKey: id) else { return }
        c.resume(with: result)
    }

    /// `hello` every 250 ms until answered or 10 s pass (section 6.3).
    private func handshake() async throws {
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if let r = try? await call("hello", ["app": "ai.allternit.com"], timeout: 0.5) {
                if (r["jspi"] as? Bool) == false { throw RPCError(code: "no_jspi", message: Self.unsupportedLine) }
                return
            }
            try? await Task.sleep(nanoseconds: 250_000_000)
        }
        throw RPCError(code: "not_ready", message: "The computer page did not load. Check your connection and try again.")
    }

    /// The signed-in user's computer: the first record they own, else a new one.
    private func ensureComputer() async throws -> String {
        let owner = AuthManager.shared.currentUserId ?? ""
        let list = try await call("computers.list", [:])
        let records = (list["computers"] as? [[String: Any]]) ?? []
        if let mine = records.first(where: { ($0["owner_user_id"] as? String) == owner }), let id = mine["id"] as? String {
            return id
        }
        let created = try await call("computers.create",
                                     ["name": "Browser computer", "image": "alpine", "owner_user_id": owner])
        guard let id = (created["computer"] as? [String: Any])?["id"] as? String else {
            throw RPCError(code: "host_error", message: "The computer could not be created.")
        }
        return id
    }

    // MARK: Terminal

    func openTerminal() async {
        guard ptyId == nil else { return }
        if let r = try? await call("spawn", ["cmd": "/bin/sh", "args": ["-l"], "cols": 60, "rows": 24, "env": [String: String]()]) {
            ptyId = r["pty_id"] as? String
        }
    }

    func send(_ line: String) async {
        guard let ptyId, let data = (line + "\n").data(using: .utf8) else { return }
        _ = try? await call("pty.write", ["pty_id": ptyId, "data_b64": data.base64EncodedString()])
    }

    // MARK: Reset

    /// Deletes the disk. The caller has already shown the confirm.
    func resetDisk() async {
        guard let id = computerId ?? status.computerId else { return }
        isResetting = true
        defer { isResetting = false }
        if webView == nil {
            // Reset needs the box page; load it without booting.
            let view = makeWebView()
            webView = view
            view.load(URLRequest(url: Self.boxURL))
            try? await handshake()
        } else {
            _ = try? await call("halt", [:])
        }
        ptyId = nil
        output = ""
        do {
            _ = try await call("reset", ["computer_id": id, "confirm": true], timeout: 60)
            tearDown()
            phase = .idle
            status = Status()
        } catch let e as RPCError {
            fail(Self.plain(e))
        } catch {
            fail("The disk could not be reset. Try again.")
        }
    }

    // MARK: Incoming

    fileprivate func receive(_ raw: String) {
        guard let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              (obj["v"] as? Int) == 1 else { return }
        switch obj["kind"] as? String {
        case "res":
            guard let id = obj["id"] as? String else { return }
            if (obj["ok"] as? Bool) == true {
                resolve(id, .success((obj["result"] as? [String: Any]) ?? [:]))
            } else {
                let e = obj["error"] as? [String: Any]
                resolve(id, .failure(RPCError(code: e?["code"] as? String ?? "host_error",
                                              message: e?["message"] as? String ?? "")))
            }
        case "evt":
            handleEvent(obj["event"] as? String ?? "", (obj["data"] as? [String: Any]) ?? [:])
        default:
            break
        }
    }

    private func handleEvent(_ event: String, _ d: [String: Any]) {
        switch event {
        case "status":
            var s = Status()
            s.state = d["state"] as? String ?? "idle"
            s.computerId = d["computer_id"] as? String ?? computerId
            if let p = d["progress"] as? [String: Any], let l = p["loaded"] as? Int, let t = p["total"] as? Int { s.progress = (l, t) }
            s.diskBytesUsed = d["disk_bytes_used"] as? Int
            s.persisted = d["persisted"] as? Bool
            s.errorMessage = (d["error"] as? [String: Any])?["message"] as? String
            let becameReady = s.state == "ready" && status.state != "ready"
            status = s
            if becameReady { Task { await openTerminal() } }
        case "pty.data":
            if let b64 = d["data_b64"] as? String, let bytes = Data(base64Encoded: b64) {
                output += Self.clean(String(decoding: bytes, as: UTF8.self))
                if output.count > 40_000 { output = String(output.suffix(30_000)) }
            }
        case "pty.exit":
            ptyId = nil
            if status.state == "ready" { Task { await openTerminal() } }
        default:
            break // net.token_needed: network mode is off in this build
        }
    }

    /// Drops terminal escape sequences and carriage returns for the plain text view.
    private static func clean(_ s: String) -> String {
        s.replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[ -/]*[@-~]", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "")
    }
}

extension BrowserComputerModel: WKNavigationDelegate {
    /// The system killed the web content process, usually for memory.
    nonisolated func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        Task { @MainActor in
            self.tearDown()
            self.ptyId = nil
            self.status = Status()
            self.phase = .stopped
        }
    }
}

/// Avoids the retain cycle between the web view's controller and the model.
private final class WeakHandler: NSObject, WKScriptMessageHandler {
    weak var model: BrowserComputerModel?
    init(_ model: BrowserComputerModel) { self.model = model }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let raw = message.body as? String else { return }
        // Only the box origin may talk to the app.
        guard message.frameInfo.isMainFrame, message.frameInfo.securityOrigin.host == "box.allternit.com" else { return }
        Task { @MainActor in self.model?.receive(raw) }
    }
}

// MARK: - SwiftUI host

/// Puts the model's web view in the hierarchy. It is not shown: the app draws
/// the status and terminal itself. It still has to be attached to a window or
/// iOS pauses its timers.
struct BrowserComputerWebView: UIViewRepresentable {
    @ObservedObject var model: BrowserComputerModel

    func makeUIView(context: Context) -> UIView { UIView(frame: .zero) }

    func updateUIView(_ host: UIView, context: Context) {
        guard let web = model.webView else {
            host.subviews.forEach { $0.removeFromSuperview() }
            return
        }
        if web.superview !== host {
            host.subviews.forEach { $0.removeFromSuperview() }
            web.frame = host.bounds
            web.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            host.addSubview(web)
        }
    }
}
