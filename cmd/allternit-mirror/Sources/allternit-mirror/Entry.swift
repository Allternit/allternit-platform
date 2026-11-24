// allternit-mirror: newline-delimited JSON on stdin/stdout (protocol in
// MirrorCore/Protocol.swift). One command at a time, in order. Each command
// has a time limit: past it, the process answers `timeout` and exits, and
// Desktop starts a fresh one. The process exits when stdin closes.

import AppKit
import Foundation
import MirrorCore

enum Output {
    private static let lock = NSLock()

    static func line(_ s: String) {
        lock.lock()
        defer { lock.unlock() }
        FileHandle.standardOutput.write(Data((s + "\n").utf8))
    }
}

/// Abandons a command that runs past its time limit.
final class Watchdog: @unchecked Sendable {
    private let lock = NSLock()
    private var token = 0

    func arm(id: String?, seconds: Double) -> Int {
        lock.lock()
        token += 1
        let mine = token
        lock.unlock()
        DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { [self] in
            lock.lock()
            let stillRunning = token == mine
            lock.unlock()
            guard stillRunning else { return }
            Output.line(errorReply(id: id, MirrorError("timeout", "The command took longer than \(Int(seconds)) seconds and was stopped.")))
            exit(3)
        }
        return mine
    }

    func disarm(_ mine: Int) {
        lock.lock()
        if token == mine { token += 1 }
        lock.unlock()
    }
}

@main
enum Main {
    static func main() async {
        signal(SIGPIPE, SIG_IGN)
        let lines = AsyncStream<String> { continuation in
            let reader = Thread {
                while let line = readLine(strippingNewline: true) {
                    continuation.yield(line)
                }
                continuation.finish()
            }
            reader.name = "stdin"
            reader.start()
        }
        let watchdog = Watchdog()
        for await line in lines {
            if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
            let (id, parsed) = parseRequest(line)
            switch parsed {
            case .failure(let e):
                Output.line(errorReply(id: id, e))
            case .success(let cmd):
                let token = watchdog.arm(id: id, seconds: cmd.timeLimit)
                let reply: String
                do {
                    reply = encodeReply(id: id, try await run(cmd))
                } catch let e as MirrorError {
                    reply = errorReply(id: id, e)
                } catch {
                    reply = errorReply(id: id, MirrorError("failed", String(describing: error)))
                }
                watchdog.disarm(token)
                Output.line(reply)
            }
        }
        exit(0)
    }

    static func run(_ cmd: Command) async throws -> [String: Any] {
        switch cmd {
        case .version:
            return ["ok": true, "version": toolVersion, "protocol": protocolVersion]
        case .probe(let ocr):
            return await probe(ocr: ocr)
        case .capture(let png, let gray):
            try System.requireScreenRecording()
            let (_, window) = try System.theWindow()
            let image = try await Capture.image(window)
            var out: [String: Any] = [
                "ok": true, "width": image.width, "height": image.height, "scale": window.geometry.scale,
                "coordinate_space": "pixels", "window": windowJSON(window),
            ]
            if png {
                out["mime_type"] = "image/png"
                out["png_base64"] = try Capture.png(image).base64EncodedString()
            }
            if gray, let g = Capture.gray(image) {
                out["gray"] = ["width": g.width, "height": g.height, "base64": g.data.base64EncodedString()]
            }
            return out
        case .read:
            try System.requireScreenRecording()
            let (_, window) = try System.theWindow()
            let image = try await Capture.image(window)
            let lines = try Capture.text(image, fast: false)
            // Reading order: top to bottom, then left to right.
            var boxed: [(text: String, bounds: [Int], confidence: Double)] = []
            for line in lines {
                boxed.append((line.text, window.geometry.pixelBox(normalized: line.box), Double(line.confidence)))
            }
            boxed.sort { a, b in
                if a.bounds[1] != b.bounds[1] { return a.bounds[1] < b.bounds[1] }
                return a.bounds[0] < b.bounds[0]
            }
            var elements: [[String: Any]] = []
            for b in boxed {
                elements.append(["class": "Text", "label": b.text, "id": "", "bounds": b.bounds, "confidence": b.confidence])
            }
            let text = boxed.map { $0.text }.joined(separator: "\n")
            return [
                "ok": true, "elements": elements, "text": text,
                "width": image.width, "height": image.height, "coordinate_space": "pixels",
            ]
        case .readImage(let data):
            // An image Desktop already has: no window, no Screen Recording, nothing on screen is touched.
            let image = try Capture.decode(data)
            let lines = try Capture.text(image, fast: false)
            var boxed: [(text: String, bounds: [Int], confidence: Double)] = lines.map { line in
                (line.text, pixelBox(normalized: line.box, width: image.width, height: image.height), Double(line.confidence))
            }
            boxed.sort { a, b in a.bounds[1] != b.bounds[1] ? a.bounds[1] < b.bounds[1] : a.bounds[0] < b.bounds[0] }
            let elements: [[String: Any]] = boxed.map { ["class": "Text", "label": $0.text, "id": "", "bounds": $0.bounds, "confidence": $0.confidence] }
            return [
                "ok": true, "elements": elements, "text": boxed.map { $0.text }.joined(separator: "\n"),
                "width": image.width, "height": image.height, "coordinate_space": "pixels",
            ]
        default:
            return try await input(cmd)
        }
    }

    static func input(_ cmd: Command) async throws -> [String: Any] {
        try System.requireAccessibility()
        switch cmd {
        case .tap(let x, let y):
            let (_, points) = try await System.reach([(x, y)])
            Input.click(points[0])
        case .swipe(let x1, let y1, let x2, let y2, let ms):
            try await swipe((x1, y1, x2, y2), ms)
        case .swipeDirection(let dir, let ms):
            let (_, window) = try System.theWindow()
            try await swipe(window.geometry.swipe(dir), ms)
        case .back:
            let (_, window) = try System.theWindow()
            try await swipe(window.geometry.backSwipe, 250)
        case .type(let text):
            let (app, window) = try System.theWindow()
            System.bringForward(app)
            Input.type(text, pid: window.pid)
        case .key(let combo):
            try press(combo)
        case .home:
            try press("home")
        case .appSwitcher:
            try press("app_switcher")
        case .spotlight:
            try press("spotlight")
        default:
            throw MirrorError("unknown_command", "Not an input command.")
        }
        return ["ok": true]
    }

    static func swipe(_ s: (Double, Double, Double, Double), _ ms: Int) async throws {
        // Start, middle and end must all be on the window: a straight line between them stays inside it.
        let mid = ((s.0 + s.2) / 2, (s.1 + s.3) / 2)
        let (_, points) = try await System.reach([(s.0, s.1), mid, (s.2, s.3)])
        Input.drag(points[0], points[2], durationMs: ms)
    }

    static func press(_ combo: String) throws {
        let stroke = try parseCombo(combo).get()
        let (app, window) = try System.theWindow()
        System.bringForward(app)
        Input.key(stroke, pid: window.pid)
    }

    static func windowJSON(_ w: MirrorWindow) -> [String: Any] {
        let b = w.geometry.bounds
        return [
            "x": Double(b.minX), "y": Double(b.minY), "width": Double(b.width), "height": Double(b.height),
            "scale": w.geometry.scale, "pixel_width": w.geometry.pixelWidth, "pixel_height": w.geometry.pixelHeight,
        ]
    }

    /// Everything Desktop needs for the route's health. Never asks for a permission.
    static func probe(ocr: Bool) async -> [String: Any] {
        let installed = System.installed()
        let app = System.runningApp()
        let windows = app.map { System.windows(of: $0.processIdentifier) } ?? []
        let session = System.session()
        let perms = System.permissions()
        var facts = ProbeFacts(
            installed: installed, running: app != nil, windowCount: windows.count,
            macLocked: session.locked, onConsole: session.onConsole,
            screenRecording: perms.screenRecording, accessibility: perms.accessibility
        )
        var out: [String: Any] = [
            "ok": true, "installed": installed, "running": app != nil, "window_count": windows.count,
            "mac_locked": session.locked, "on_console": session.onConsole,
            "permissions": ["screen_recording": perms.screenRecording, "accessibility": perms.accessibility],
        ]
        var h = health(facts)
        if h == .healthy {
            guard let found = try? System.theWindow() else {
                // The window went between the two looks: report what the user sees.
                out["health"] = Health.windowHidden.rawValue
                return out
            }
            let window = found.1
            out["window"] = windowJSON(window)
            if ocr {
                do {
                    let lines = try Capture.text(try await Capture.image(window), fast: true)
                    facts.phoneInUse = saysPhoneInUse(lines.map(\.text))
                    out["phone_in_use"] = facts.phoneInUse
                    h = health(facts)
                } catch {
                    // Can't see the window: not healthy, whatever else says so.
                    out["read_error"] = (error as? MirrorError)?.message ?? String(describing: error)
                    h = .windowHidden
                }
            }
        }
        out["health"] = h.rawValue
        if let pane = settingsPane(for: h) {
            out["needs_you"] = ["permission": pane.permission, "pane": pane.pane, "settings_url": pane.url]
        }
        return out
    }
}
