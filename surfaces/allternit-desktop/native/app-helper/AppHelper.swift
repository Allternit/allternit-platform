// Allternit app helper (plan v3 lane 38, decision E1 = B).
//
// One tiny native app per installed Allternit app, so each app gets its own
// macOS Dock tile, Cmd-Tab entry and window, under its own bundle id
// (com.allternit.app.<slug>). Desktop's app-launchers.ts copies this binary
// into ~/Applications/Allternit Apps/<Name>.app with an Info.plist and icon.
//
// Arguments (all optional; a Dock or Spotlight launch passes none):
//   --url <app url>        the app shell URL (https, or http on loopback)
//   --token <one-time>     loaded as <url>?handoff=<token>; the app shell
//                          swaps it for a session (D15) and drops it
//   --menubar              keep a menu bar item, so the app keeps running
//                          after its window closes (only when the app's
//                          permissions include it)
// Without arguments the Info.plist keys AllternitAppURL and AllternitMenuBar
// are used. The window title is CFBundleName.
//
// Links that leave the app (anything but the app's own origin and the
// Allternit sign-in origins) open in the default browser.
import AppKit
import WebKit

struct LaunchOptions {
  var url: URL?
  var token: String?
  var menuBar = false
}

func parseOptions() -> LaunchOptions {
  var opts = LaunchOptions()
  let info = Bundle.main.infoDictionary ?? [:]
  if let s = info["AllternitAppURL"] as? String { opts.url = URL(string: s) }
  opts.menuBar = (info["AllternitMenuBar"] as? Bool) ?? false
  var args = Array(CommandLine.arguments.dropFirst())
  while !args.isEmpty {
    let a = args.removeFirst()
    switch a {
    case "--url": if let v = args.first { opts.url = URL(string: v); args.removeFirst() }
    case "--token": if let v = args.first { opts.token = v; args.removeFirst() }
    case "--menubar": opts.menuBar = true
    default: break  // ignore -psn_… and anything unknown
    }
  }
  return opts
}

/// https anywhere, http only on loopback (dev builds).
func isAllowedAppURL(_ url: URL) -> Bool {
  guard let scheme = url.scheme?.lowercased(), let host = url.host?.lowercased() else { return false }
  if scheme == "https" { return true }
  return scheme == "http" && (host == "127.0.0.1" || host == "localhost" || host == "::1")
}

func launchURL(_ opts: LaunchOptions) -> URL? {
  guard let base = opts.url, isAllowedAppURL(base) else { return nil }
  guard let token = opts.token, !token.isEmpty,
        var comps = URLComponents(url: base, resolvingAgainstBaseURL: false) else { return base }
  var items = comps.queryItems ?? []
  items.removeAll { $0.name == "handoff" }
  items.append(URLQueryItem(name: "handoff", value: token))
  comps.queryItems = items
  return comps.url ?? base
}

final class HelperDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
  var window: NSWindow!
  var webView: WKWebView!
  var statusItem: NSStatusItem?
  let opts = parseOptions()
  lazy var appName: String =
    (Bundle.main.infoDictionary?["CFBundleName"] as? String) ?? "Allternit App"

  func applicationDidFinishLaunching(_ n: Notification) {
    buildMainMenu()
    let config = WKWebViewConfiguration()
    config.websiteDataStore = .default()  // per bundle id, so per app
    webView = WKWebView(frame: .zero, configuration: config)
    webView.navigationDelegate = self
    webView.uiDelegate = self
    webView.allowsBackForwardNavigationGestures = true

    window = NSWindow(
      contentRect: NSRect(x: 200, y: 200, width: 1100, height: 760),
      styleMask: [.titled, .closable, .resizable, .miniaturizable],
      backing: .buffered, defer: false)
    window.contentView = webView
    window.title = appName
    window.delegate = self
    window.isReleasedWhenClosed = false
    window.minSize = NSSize(width: 480, height: 360)
    window.setFrameAutosaveName("AllternitAppWindow")
    window.makeKeyAndOrderFront(nil)

    if let url = launchURL(opts) {
      webView.load(URLRequest(url: url))
    } else {
      webView.loadHTMLString(
        "<html><body style='font:15px -apple-system;padding:40px;color:#444'>"
          + "<h2>This app can't open</h2><p>Open it again from Allternit.</p></body></html>",
        baseURL: nil)
    }

    if opts.menuBar {
      let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
      if let icon = NSApp.applicationIconImage?.copy() as? NSImage {
        icon.size = NSSize(width: 18, height: 18)
        item.button?.image = icon
      } else {
        item.button?.title = String(appName.prefix(3))
      }
      item.button?.toolTip = appName
      let m = NSMenu()
      let show = NSMenuItem(title: "Show \(appName)", action: #selector(showWindow), keyEquivalent: "")
      show.target = self
      m.addItem(show)
      m.addItem(.separator())
      m.addItem(withTitle: "Quit \(appName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
      item.menu = m
      statusItem = item
    }
  }

  // Edit menu is what makes copy, paste and undo work inside the web view.
  func buildMainMenu() {
    let main = NSMenu()
    let appItem = NSMenuItem(); main.addItem(appItem)
    let appMenu = NSMenu()
    appMenu.addItem(withTitle: "Hide \(appName)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
    appMenu.addItem(.separator())
    appMenu.addItem(withTitle: "Quit \(appName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    appItem.submenu = appMenu

    let editItem = NSMenuItem(); main.addItem(editItem)
    let edit = NSMenu(title: "Edit")
    edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
    edit.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
    edit.addItem(.separator())
    edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editItem.submenu = edit

    let viewItem = NSMenuItem(); main.addItem(viewItem)
    let view = NSMenu(title: "View")
    let reload = NSMenuItem(title: "Reload", action: #selector(reloadPage), keyEquivalent: "r")
    reload.target = self
    view.addItem(reload)
    viewItem.submenu = view

    let winItem = NSMenuItem(); main.addItem(winItem)
    let win = NSMenu(title: "Window")
    win.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
    win.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
    winItem.submenu = win
    NSApp.mainMenu = main
    NSApp.windowsMenu = win
  }

  @objc func showWindow() {
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
  }

  @objc func reloadPage() { webView.reload() }

  // Dock tile click with the window closed brings it back.
  func applicationShouldHandleReopen(_ s: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
    if !flag { showWindow() }
    return true
  }

  // With a menu bar item the app keeps running after its window closes.
  func applicationShouldTerminateAfterLastWindowClosed(_ s: NSApplication) -> Bool {
    statusItem == nil
  }

  // MARK: navigation

  func isInApp(_ url: URL) -> Bool {
    guard let host = url.host?.lowercased() else { return url.scheme == "about" }
    if let appHost = opts.url?.host?.lowercased(), host == appHost { return true }
    // Sign-in round trip through the Allternit origins (D15) stays inside.
    let inside = ["allternit.com", "allternitugc.app", "clerk.accounts.dev"]
    return inside.contains { host == $0 || host.hasSuffix("." + $0) }
  }

  func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
               decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
    guard let url = action.request.url else { return decisionHandler(.cancel) }
    if url.scheme == "about" || url.scheme == "blob" || url.scheme == "data" || isInApp(url) {
      return decisionHandler(.allow)
    }
    if url.scheme == "https" || url.scheme == "http" || url.scheme == "mailto" {
      NSWorkspace.shared.open(url)
    }
    decisionHandler(.cancel)
  }

  // target=_blank and window.open go to the default browser.
  func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
               for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
    if let url = action.request.url, url.scheme == "https" || url.scheme == "http" {
      NSWorkspace.shared.open(url)
    }
    return nil
  }
}

let app = NSApplication.shared
let delegate = HelperDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
