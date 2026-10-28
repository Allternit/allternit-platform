import SwiftUI
import UIKit
import WebKit

/// Lets the owner's bots drive the app's own ACI browser over the control
/// socket (`web.*`, SPEC-iphone-app-screen §2 control / §3 2b).
///
/// The page is read and acted on by one fixed user script that runs in its
/// own `WKContentWorld` ("allternit-aci"), so page scripts can't see or
/// tamper with it, and bots never send JavaScript: they get an element list
/// with refs from `web.snapshot` and name a ref in `web.click` / `web.type`.
/// Refs are only valid for the snapshot that issued them.
@MainActor
final class ACIBrowserBridge: ObservableObject {
    static let shared = ACIBrowserBridge()

    static let world = WKContentWorld.world(name: "allternit-aci")

    /// A bot called a `web.*` method in the last 20 s (a model thinks between
    /// steps, so a shorter window flickers) — drives the
    /// "Bot is using the browser" banner.
    @Published private(set) var isBotActive = false
    /// The person pressed Stop: `web.*` is refused until they start a new
    /// run or reopen the browser.
    @Published private(set) var isStoppedByUser = false
    /// Asks ACITabView to open the browser at this URL (a bot called
    /// `web.open` while the browser was closed).
    @Published var requestedURL: URL?
    /// The ACI run this browser is serving, stopped along with the bot.
    var activeRunId: String?

    private weak var controller: ACIBrowserController?
    private var idleTask: Task<Void, Never>?
    private static let activeWindow: Duration = .seconds(20)

    var isBrowserOpen: Bool { controller?.webView != nil }

    // MARK: - Registration (ACIWebBrowserView)

    func attach(_ controller: ACIBrowserController) {
        self.controller = controller
        isStoppedByUser = false
        AppPhoneService.shared.browserStateChanged()
    }

    func detach(_ controller: ACIBrowserController) {
        guard self.controller === controller else { return }
        self.controller = nil
        isBotActive = false
        AppPhoneService.shared.browserStateChanged()
    }

    /// The person started a run from the browser: bots may drive it again.
    func allowBot() {
        isStoppedByUser = false
    }

    /// Stop from the banner: refuse further `web.*` calls and stop the run.
    func stopBot() {
        isStoppedByUser = true
        isBotActive = false
        idleTask?.cancel()
        if let runId = activeRunId {
            activeRunId = nil
            Task { try? await ACIAgentClient().stop(runId: runId) }
        }
    }

    // MARK: - web.*

    func handle(method: String, params: [String: Any]) async throws -> [String: Any] {
        guard !isStoppedByUser else {
            throw AppPhoneRPCError(code: "stopped_by_user", message: "The person stopped the bot in the Allternit app.")
        }
        markActive()
        switch method {
        case "web.open":
            guard let raw = params["url"] as? String, let url = URL(string: raw),
                  let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http" else {
                throw AppPhoneRPCError(code: "bad_params", message: "web.open needs an http(s) url")
            }
            let controller = try await openBrowser(at: url)
            await waitForLoad(controller)
            return pageInfo(controller)
        case "web.snapshot":
            let webView = try requireWebView()
            guard var result = try await call("return window.__allternitAci.snapshot();", in: webView) as? [String: Any] else {
                throw AppPhoneRPCError(code: "internal", message: "Snapshot returned nothing")
            }
            result["jpegBase64"] = await jpeg(of: webView)?.base64EncodedString() ?? ""
            return result
        case "web.click":
            let ref = try string(params, "ref")
            let webView = try requireWebView()
            _ = try await call("return window.__allternitAci.click(ref);", in: webView, ["ref": ref])
            try? await Task.sleep(for: .milliseconds(300))
            return pageInfo(controller)
        case "web.type":
            let ref = try string(params, "ref")
            let text = try string(params, "text")
            let submit = params["submit"] as? Bool ?? false
            let webView = try requireWebView()
            _ = try await call("return window.__allternitAci.type(ref, text, submit);", in: webView,
                               ["ref": ref, "text": text, "submit": submit])
            if submit { try? await Task.sleep(for: .milliseconds(300)) }
            return pageInfo(controller)
        case "web.scroll":
            let dy = (params["dy"] as? NSNumber)?.doubleValue ?? 0
            let webView = try requireWebView()
            return try await call("return window.__allternitAci.scroll(dy);", in: webView, ["dy": dy]) as? [String: Any] ?? ["ok": true]
        case "web.back":
            guard let controller, controller.webView != nil else { throw notOpen }
            let went = controller.navigation.canGoBack
            controller.goBack()
            if went { await waitForLoad(controller) }
            return pageInfo(controller).merging(["wentBack": went]) { $1 }
        case "web.wait":
            let ms = min(max((params["ms"] as? NSNumber)?.intValue ?? 0, 0), 10_000)
            try? await Task.sleep(for: .milliseconds(ms))
            return pageInfo(controller)
        default:
            throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
    }

    // MARK: - Helpers

    private var notOpen: AppPhoneRPCError {
        AppPhoneRPCError(code: "browser_not_open",
                         message: "The ACI browser isn't open in the Allternit app. Call web.open first.")
    }

    private func requireWebView() throws -> WKWebView {
        guard let webView = controller?.webView else { throw notOpen }
        return webView
    }

    private func string(_ params: [String: Any], _ key: String) throws -> String {
        guard let value = params[key] as? String else {
            throw AppPhoneRPCError(code: "bad_params", message: "Missing \(key)")
        }
        return value
    }

    private func pageInfo(_ controller: ACIBrowserController?) -> [String: Any] {
        ["url": controller?.webView?.url?.absoluteString ?? "",
         "title": controller?.webView?.title ?? ""]
    }

    private func markActive() {
        isBotActive = true
        idleTask?.cancel()
        idleTask = Task { [weak self] in
            try? await Task.sleep(for: Self.activeWindow)
            guard !Task.isCancelled else { return }
            self?.isBotActive = false
        }
    }

    /// Opens (or reuses) the browser and loads `url`. When the browser is
    /// closed the app navigates to it and waits for the view to attach.
    private func openBrowser(at url: URL) async throws -> ACIBrowserController {
        if let controller, controller.webView != nil {
            controller.load(url)
            return controller
        }
        requestedURL = url
        for _ in 0..<50 {
            try? await Task.sleep(for: .milliseconds(100))
            if let controller, controller.webView != nil { return controller }
        }
        requestedURL = nil
        throw AppPhoneRPCError(code: "browser_not_open",
                               message: "Couldn't open the ACI browser. Ask the person to open the Browser tab.")
    }

    /// Up to 15 s for the page to finish (a slow page still answers).
    private func waitForLoad(_ controller: ACIBrowserController) async {
        try? await Task.sleep(for: .milliseconds(150))
        for _ in 0..<60 where controller.navigation.isLoading {
            try? await Task.sleep(for: .milliseconds(250))
        }
    }

    /// Runs one of the script's own entry points in the isolated world,
    /// installing the script first if the page loaded before it was added.
    private func call(_ body: String, in webView: WKWebView, _ arguments: [String: Any] = [:]) async throws -> Any? {
        do {
            return try await webView.callAsyncJavaScript(
                "if (!window.__allternitAci) { \(Self.script) }\n" + body,
                arguments: arguments, in: nil, contentWorld: Self.world)
        } catch {
            let message = (error as NSError).userInfo["WKJavaScriptExceptionMessage"] as? String
                ?? error.localizedDescription
            if message.contains("stale_ref") {
                throw AppPhoneRPCError(code: "stale_ref", message: "That ref is from an older snapshot or left the page. Take a new web.snapshot.")
            }
            if message.contains("not_editable") {
                throw AppPhoneRPCError(code: "not_editable", message: "That element doesn't take text.")
            }
            throw AppPhoneRPCError(code: "script_error", message: message)
        }
    }

    /// The visible page as a JPEG at 1 pixel per point, so element bounds
    /// (CSS pixels at the default zoom) line up with image pixels.
    private func jpeg(of webView: WKWebView) async -> Data? {
        let config = WKSnapshotConfiguration()
        config.afterScreenUpdates = false
        let image: UIImage? = await withCheckedContinuation { continuation in
            webView.takeSnapshot(with: config) { image, _ in continuation.resume(returning: image) }
        }
        guard let image else { return nil }
        let width = min(image.size.width, 1024)
        let size = CGSize(width: width, height: image.size.height * width / max(image.size.width, 1))
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let scaled = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        return scaled.jpegData(compressionQuality: 0.6)
    }

    // MARK: - User script

    /// Installed into every ACI browser web view (ACIBrowserWebView).
    static var userScript: WKUserScript {
        WKUserScript(source: script, injectionTime: .atDocumentEnd, forMainFrameOnly: true, in: world)
    }

    /// Builds the element list (role, accessible name, text, value, viewport
    /// bounds, ref) and performs click / type / scroll on a ref. Passwords'
    /// values are never read. Defined once per page; guarded so a second
    /// install is a no-op.
    static let script = #"""
    (function () {
      if (window.__allternitAci) return;
      var MAX = 300;
      var gen = 0;
      var refs = new Map();
      var SEL = 'a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=checkbox],' +
        '[role=radio],[role=tab],[role=menuitem],[role=option],[role=switch],[role=combobox],[role=searchbox],' +
        '[role=textbox],[contenteditable=""],[contenteditable=true],[tabindex]:not([tabindex="-1"]),h1,h2,h3';
      function clean(s, n) { return (s || '').replace(/\s+/g, ' ').trim().slice(0, n || 120); }
      function shown(el, r) {
        if (r.width < 1 || r.height < 1) return false;
        var s = getComputedStyle(el);
        return s.visibility !== 'hidden' && s.display !== 'none' && parseFloat(s.opacity || '1') > 0.05;
      }
      function near(r) { return r.bottom > 0 && r.right > 0 && r.top < innerHeight * 2 && r.left < innerWidth; }
      function role(el) {
        var r = el.getAttribute('role');
        if (r) return r;
        var t = el.tagName.toLowerCase();
        if (t === 'a') return 'link';
        if (t === 'button' || t === 'summary') return 'button';
        if (t === 'select') return 'combobox';
        if (t === 'textarea') return 'textbox';
        if (t === 'input') {
          var ty = (el.type || 'text').toLowerCase();
          if (['button', 'submit', 'reset', 'image'].indexOf(ty) >= 0) return 'button';
          if (ty === 'checkbox' || ty === 'radio') return ty;
          if (ty === 'search') return 'searchbox';
          return 'textbox';
        }
        if (/^h[1-6]$/.test(t)) return 'heading';
        if (el.isContentEditable) return 'textbox';
        return 'generic';
      }
      function name(el) {
        var a = el.getAttribute('aria-label');
        if (a) return clean(a);
        var by = el.getAttribute('aria-labelledby');
        if (by) {
          var t = by.split(/\s+/).map(function (id) { var e = document.getElementById(id); return e ? e.textContent : ''; }).join(' ');
          if (clean(t)) return clean(t);
        }
        if (el.labels && el.labels.length) return clean(el.labels[0].textContent);
        return clean(el.getAttribute('alt') || el.getAttribute('title') || el.getAttribute('placeholder') ||
          (el.tagName === 'INPUT' ? (el.type === 'submit' || el.type === 'button' ? el.value : '') : el.innerText));
      }
      function snapshot() {
        gen += 1;
        refs = new Map();
        var out = [];
        var all = document.querySelectorAll(SEL);
        for (var i = 0; i < all.length && out.length < MAX; i++) {
          var el = all[i];
          var r = el.getBoundingClientRect();
          if (!shown(el, r) || !near(r)) continue;
          var ref = 'g' + gen + 'e' + (out.length + 1);
          refs.set(ref, el);
          var item = {
            ref: ref, role: role(el), name: name(el), text: clean(el.innerText || el.textContent, 200),
            bounds: { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) }
          };
          if (typeof el.value === 'string' && el.type !== 'password') item.value = clean(el.value, 200);
          out.push(item);
        }
        return { url: location.href, title: document.title, viewport: { w: innerWidth, h: innerHeight }, elements: out };
      }
      function get(ref) {
        var el = refs.get(ref);
        if (!el || !el.isConnected) throw new Error('stale_ref');
        return el;
      }
      function click(ref) {
        var el = get(ref);
        el.scrollIntoView({ block: 'center', inline: 'center' });
        var r = el.getBoundingClientRect();
        var o = { bubbles: true, cancelable: true, composed: true, view: window,
                  clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
        ['pointerdown', 'mousedown', 'pointerup', 'mouseup'].forEach(function (t) {
          var E = t.indexOf('pointer') === 0 && window.PointerEvent ? PointerEvent : MouseEvent;
          el.dispatchEvent(new E(t, o));
        });
        if (el.focus) el.focus();
        el.click();
        return { ok: true };
      }
      function setValue(el, v) {
        var proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
          : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
        var d = Object.getOwnPropertyDescriptor(proto, 'value');
        if (d && d.set) d.set.call(el, v); else el.value = v;
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      function type(ref, text, submit) {
        var el = get(ref);
        el.scrollIntoView({ block: 'center' });
        if (el.focus) el.focus();
        if (el.isContentEditable) {
          document.execCommand('selectAll', false);
          document.execCommand('insertText', false, text);
        } else if (typeof el.value === 'string') {
          setValue(el, text);
        } else {
          throw new Error('not_editable');
        }
        if (submit) {
          var k = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true };
          var go = el.dispatchEvent(new KeyboardEvent('keydown', k));
          el.dispatchEvent(new KeyboardEvent('keyup', k));
          if (go && el.form) { if (el.form.requestSubmit) el.form.requestSubmit(); else el.form.submit(); }
        }
        return { ok: true };
      }
      function scroll(dy) {
        window.scrollBy(0, dy);
        return { ok: true, scrollY: Math.round(window.scrollY) };
      }
      window.__allternitAci = { snapshot: snapshot, click: click, type: type, scroll: scroll };
    })();
    """#
}
