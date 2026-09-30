// ax-bridge: minimal macOS Accessibility (AXUIElement) helper. JSON-lines over stdio, one request per line:
//   {"id":1,"cmd":"trust"}                              -> {"id":1,"ok":true,"trusted":true}
//   {"id":2,"cmd":"attach","bundleId":"com.x"}          -> sets AXManualAccessibility (Electron), pins the target
//   {"id":3,"cmd":"snapshot","maxDepth":12}             -> {"root":{role,subrole,title,description,value,identifier,frame,actions,path,children}}
//   {"id":4,"cmd":"find","role":"AXButton","labelRegex":"^send"}
//   {"id":5,"cmd":"press"|"focus"|"setValue","path":[0,2],"value":"text"}
//   {"id":6,"cmd":"observe","notifications":["AXValueChanged"]} -> ack, then {"event":"AXValueChanged",...} lines
//   {"id":7,"cmd":"detach"}
// It never prompts for the Accessibility permission (AXIsProcessTrusted, not the prompting variant) and never
// attaches to anything until asked to. Responses are one JSON object per line on stdout; diagnostics go to stderr.
import ApplicationServices
import AppKit
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)
let outLock = NSLock()
func emit(_ obj: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: obj, options: []),
          let s = String(data: data, encoding: .utf8) else { return }
    outLock.lock(); print(s); outLock.unlock()
}

func attr(_ el: AXUIElement, _ name: String) -> AnyObject? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(el, name as CFString, &v) == .success ? (v as AnyObject?) : nil
}
func str(_ el: AXUIElement, _ name: String) -> String? {
    guard let v = attr(el, name) else { return nil }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    return nil
}
func frame(_ el: AXUIElement) -> [String: Double]? {
    var out: [String: Double] = [:]
    if let p = attr(el, kAXPositionAttribute as String), CFGetTypeID(p) == AXValueGetTypeID() {
        var pt = CGPoint.zero
        if AXValueGetValue(p as! AXValue, .cgPoint, &pt) { out["x"] = Double(pt.x); out["y"] = Double(pt.y) }
    }
    if let s = attr(el, kAXSizeAttribute as String), CFGetTypeID(s) == AXValueGetTypeID() {
        var sz = CGSize.zero
        if AXValueGetValue(s as! AXValue, .cgSize, &sz) { out["w"] = Double(sz.width); out["h"] = Double(sz.height) }
    }
    return out.isEmpty ? nil : out
}
func children(_ el: AXUIElement) -> [AXUIElement] {
    (attr(el, kAXChildrenAttribute as String) as? [AXUIElement]) ?? []
}
func actions(_ el: AXUIElement) -> [String] {
    var a: CFArray?
    return AXUIElementCopyActionNames(el, &a) == .success ? ((a as? [String]) ?? []) : []
}

func node(_ el: AXUIElement, path: [Int], depth: Int, maxDepth: Int, budget: inout Int) -> [String: Any] {
    budget -= 1
    var n: [String: Any] = ["path": path, "role": str(el, kAXRoleAttribute as String) ?? "AXUnknown"]
    if let v = str(el, kAXSubroleAttribute as String) { n["subrole"] = v }
    if let v = str(el, kAXTitleAttribute as String), !v.isEmpty { n["title"] = v }
    if let v = str(el, kAXDescriptionAttribute as String), !v.isEmpty { n["description"] = v }
    if let v = str(el, kAXValueAttribute as String) { n["value"] = String(v.prefix(20000)) }
    if let v = str(el, "AXIdentifier"), !v.isEmpty { n["identifier"] = v }
    if let f = frame(el) { n["frame"] = f }
    let acts = actions(el); if !acts.isEmpty { n["actions"] = acts }
    if depth < maxDepth && budget > 0 {
        var kids: [[String: Any]] = []
        for (i, c) in children(el).enumerated() {
            if budget <= 0 { break }
            kids.append(node(c, path: path + [i], depth: depth + 1, maxDepth: maxDepth, budget: &budget))
        }
        if !kids.isEmpty { n["children"] = kids }
    }
    return n
}
func resolve(_ root: AXUIElement, _ path: [Int]) -> AXUIElement? {
    var cur = root
    for i in path { let k = children(cur); guard i >= 0 && i < k.count else { return nil }; cur = k[i] }
    return cur
}
func label(_ n: [String: Any]) -> String {
    [n["title"], n["description"], n["value"]].compactMap { $0 as? String }.joined(separator: " ")
}
func collect(_ n: [String: Any], role: String?, re: NSRegularExpression?, into out: inout [[String: Any]]) {
    var hit = true
    if let r = role, (n["role"] as? String) != r { hit = false }
    if hit, let re = re {
        let l = label(n); hit = re.firstMatch(in: l, range: NSRange(l.startIndex..., in: l)) != nil
    }
    if hit { var c = n; c["children"] = nil; out.append(c) }
    for k in (n["children"] as? [[String: Any]]) ?? [] { collect(k, role: role, re: re, into: &out) }
}

var app: AXUIElement?
var pid: pid_t = 0
var observer: AXObserver?

func fail(_ id: Any?, _ code: String, _ msg: String) { emit(["id": id ?? NSNull(), "ok": false, "error": ["code": code, "message": msg]]) }

let observerCb: AXObserverCallback = { _, element, notification, _ in
    var ev: [String: Any] = ["event": notification as String, "role": str(element, kAXRoleAttribute as String) ?? "AXUnknown"]
    if let v = str(element, kAXValueAttribute as String) { ev["value"] = String(v.prefix(20000)) }
    if let v = str(element, kAXTitleAttribute as String) { ev["title"] = v }
    if let v = str(element, kAXDescriptionAttribute as String) { ev["description"] = v }
    ev["ts"] = Date().timeIntervalSince1970 * 1000
    emit(ev)
}

func handle(_ req: [String: Any]) {
    let id = req["id"]
    guard let cmd = req["cmd"] as? String else { return fail(id, "BAD_REQUEST", "missing cmd") }
    switch cmd {
    case "trust":
        emit(["id": id ?? NSNull(), "ok": true, "trusted": AXIsProcessTrusted()])
    case "attach":
        guard AXIsProcessTrusted() else { return fail(id, "NOT_TRUSTED", "Accessibility permission not granted") }
        guard let bid = req["bundleId"] as? String else { return fail(id, "BAD_REQUEST", "bundleId required") }
        guard let running = NSRunningApplication.runningApplications(withBundleIdentifier: bid).first else { return fail(id, "NOT_RUNNING", "\(bid) is not running") }
        pid = running.processIdentifier
        let el = AXUIElementCreateApplication(pid)
        // Electron/Chromium build the web accessibility tree only when an assistive client asks.
        let r = AXUIElementSetAttributeValue(el, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        app = el
        emit(["id": id ?? NSNull(), "ok": true, "pid": Int(pid), "manualAccessibility": r == .success])
    case "detach":
        app = nil; observer = nil
        emit(["id": id ?? NSNull(), "ok": true])
    case "snapshot", "find", "press", "focus", "setValue", "observe":
        guard AXIsProcessTrusted() else { return fail(id, "NOT_TRUSTED", "Accessibility permission not granted") }
        guard let root = app else { return fail(id, "NOT_ATTACHED", "attach first") }
        if cmd == "snapshot" || cmd == "find" {
            var budget = (req["maxNodes"] as? Int) ?? 4000
            let tree = node(root, path: [], depth: 0, maxDepth: (req["maxDepth"] as? Int) ?? 12, budget: &budget)
            if cmd == "snapshot" { return emit(["id": id ?? NSNull(), "ok": true, "root": tree, "truncated": budget <= 0, "capturedAt": Date().timeIntervalSince1970 * 1000]) }
            let re = (req["labelRegex"] as? String).flatMap { try? NSRegularExpression(pattern: $0, options: [.caseInsensitive]) }
            var hits: [[String: Any]] = []
            collect(tree, role: req["role"] as? String, re: re, into: &hits)
            return emit(["id": id ?? NSNull(), "ok": true, "matches": hits])
        }
        if cmd == "observe" {
            var obs: AXObserver?
            guard AXObserverCreate(pid, observerCb, &obs) == .success, let o = obs else { return fail(id, "OBSERVER_FAILED", "AXObserverCreate failed") }
            let names = (req["notifications"] as? [String]) ?? ["AXValueChanged", "AXUIElementCreated", "AXFocusedUIElementChanged"]
            for nname in names { AXObserverAddNotification(o, root, nname as CFString, nil) }
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(o), .defaultMode)
            observer = o
            return emit(["id": id ?? NSNull(), "ok": true, "observing": names])
        }
        guard let path = req["path"] as? [Int], let el = resolve(root, path) else { return fail(id, "NOT_FOUND", "no element at path") }
        switch cmd {
        case "press":
            let r = AXUIElementPerformAction(el, kAXPressAction as CFString)
            emit(["id": id ?? NSNull(), "ok": r == .success, "error": r == .success ? NSNull() : ["code": "ACTION_FAILED", "message": "AXPress \(r.rawValue)"]])
        case "focus":
            let r = AXUIElementSetAttributeValue(el, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            emit(["id": id ?? NSNull(), "ok": r == .success])
        default:
            let r = AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, (req["value"] as? String ?? "") as CFString)
            emit(["id": id ?? NSNull(), "ok": r == .success, "error": r == .success ? NSNull() : ["code": "ACTION_FAILED", "message": "setValue \(r.rawValue)"]])
        }
    default:
        fail(id, "BAD_REQUEST", "unknown cmd \(cmd)")
    }
}

// stdin reader on a background thread; all AX work is marshalled to the main run loop (observers need it).
let reader = Thread {
    while let line = readLine(strippingNewline: true) {
        guard !line.isEmpty, let d = line.data(using: .utf8), let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else {
            if !line.isEmpty { fail(nil, "BAD_REQUEST", "invalid JSON") }
            continue
        }
        DispatchQueue.main.async { handle(obj) }
    }
    DispatchQueue.main.async { exit(0) }
}
reader.start()
RunLoop.main.run()
