// allternit-bthid: newline-delimited JSON on stdin/stdout (protocol in
// BTHIDTool/Protocol.swift). One command at a time, in order. Each command
// has a time limit; every wait inside is bounded below it, so the limit is a
// last resort: past it the process answers `timeout` and exits, and Desktop
// starts a fresh one. The process lets go of every key and button, removes
// the keyboard and exits when stdin closes (and so when Desktop quits).

import AllternitBTHID
import BTHIDTool
import Foundation

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
            Output.line(errorReply(id: id, BTHIDError("timeout", "The command took longer than \(Int(seconds)) seconds and was stopped.")))
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

let localName = "Allternit Keyboard"

@main
enum Main {
    static let hid = HIDPeripheral(localName: localName)
    /// Keys and buttons held right now. Commands run one at a time, so only the main loop touches it.
    nonisolated(unsafe) static var input = HIDInputState()

    static func main() async {
        signal(SIGPIPE, SIG_IGN)
        hid.onChange = { status in
            var event = stateJSON(status, localName: localName)
            event["event"] = "state"
            event.removeValue(forKey: "ok")
            Output.line(encodeReply(id: nil, event))
        }
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
                } catch let e as BTHIDError {
                    reply = errorReply(id: id, e)
                } catch {
                    reply = errorReply(id: id, BTHIDError("failed", String(describing: error)))
                }
                watchdog.disarm(token)
                Output.line(reply)
            }
        }
        // stdin closed: Desktop is gone or done with us.
        try? await hid.send(input.releaseAll(), timeout: 0.5)
        hid.stop()
        exit(0)
    }

    static func run(_ cmd: Command) async throws -> [String: Any] {
        switch cmd {
        case .version:
            return ["ok": true, "version": toolVersion, "protocol": protocolVersion]
        case .state:
            return stateJSON(hid.currentStatus(), localName: localName)
        case .pair:
            try await hid.start()
            return stateJSON(hid.currentStatus(), localName: localName)
        case .unpair:
            try? await hid.send(input.releaseAll(), timeout: 0.5)
            hid.stop()
            return stateJSON(hid.currentStatus(), localName: localName)
        case .key(let usage, let modifiers, let down):
            do {
                try await hid.send(try input.key(usage: usage, modifiers: modifiers, down: down))
            } catch let e as HIDInputError {
                throw BTHIDError("bad_request", e.message)
            }
            return ["ok": true]
        case .type(let text):
            guard let reports = input.type(text) else { throw BTHIDError("bad_request", "The keyboard can’t type that text.") }
            try await hid.send(reports)
            return ["ok": true, "sent": text.count]
        case .move(let dx, let dy):
            try await hid.send(input.move(dx: dx, dy: dy))
            return ["ok": true]
        case .button(let b, let down, let up):
            try await hid.send(input.button(b, down: down, up: up))
            return ["ok": true]
        case .scroll(let lines):
            try await hid.send(input.scroll(lines: lines))
            return ["ok": true]
        case .reset:
            try await hid.send(input.cornerReset())
            return ["ok": true]
        case .release:
            try await hid.send(input.releaseAll())
            return ["ok": true]
        }
    }
}
