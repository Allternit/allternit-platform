import Foundation

/// One dispatched server-sent-events frame.
struct SSEFrame: Equatable {
    var id: String?
    var event: String?
    var data: String
}

/// Incremental SSE parser (WHATWG event-stream rules). Fed raw bytes because
/// `URLSession.AsyncBytes.lines` drops the blank lines that end a frame.
/// Comment lines (`: keepalive`) are ignored. `lastEventID` survives across
/// frames so a reconnect can send it as `Last-Event-ID`.
struct SSEParser {
    private var buffer = Data()
    private var pendingID: String?
    private var pendingEvent: String?
    private var pendingData: [String] = []
    private(set) var lastEventID: String?

    init(lastEventID: String? = nil) { self.lastEventID = lastEventID }

    /// Feed bytes; returns every frame completed by them.
    mutating func feed(_ bytes: some Sequence<UInt8>) -> [SSEFrame] {
        var frames: [SSEFrame] = []
        for byte in bytes {
            if byte == 0x0A {
                var line = buffer
                buffer.removeAll(keepingCapacity: true)
                if line.last == 0x0D { line.removeLast() }
                if let frame = handle(line: String(decoding: line, as: UTF8.self)) {
                    frames.append(frame)
                }
            } else {
                buffer.append(byte)
            }
        }
        return frames
    }


    private mutating func handle(line: String) -> SSEFrame? {
        if line.isEmpty { return dispatch() }
        if line.hasPrefix(":") { return nil }
        let field: String
        var value: String
        if let colon = line.firstIndex(of: ":") {
            field = String(line[..<colon])
            value = String(line[line.index(after: colon)...])
            if value.hasPrefix(" ") { value.removeFirst() }
        } else {
            field = line
            value = ""
        }
        switch field {
        case "id": if !value.contains("\0") { pendingID = value }
        case "event": pendingEvent = value
        case "data": pendingData.append(value)
        default: break // retry and unknown fields are ignored
        }
        return nil
    }

    private mutating func dispatch() -> SSEFrame? {
        defer { pendingEvent = nil; pendingData = []; pendingID = nil }
        if let id = pendingID { lastEventID = id }
        guard !pendingData.isEmpty else { return nil }
        return SSEFrame(id: pendingID, event: pendingEvent, data: pendingData.joined(separator: "\n"))
    }
}
