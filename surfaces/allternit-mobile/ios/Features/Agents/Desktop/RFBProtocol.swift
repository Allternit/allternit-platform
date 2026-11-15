import Foundation

// -----------------------------------------------------------------------------
// RFB 3.8 protocol primitives for the native bot-desktop viewer: wire
// messages, the incremental stream parser, typed errors, and the X11 keysym
// table.
//
// Wire grounding: the server is a blind proxy
// (cmd/allternit-api/src/bot_desktop_stream.rs) that shuttles BINARY
// WebSocket frames to a sandbox `x11vnc -nopw` over 16 KB TCP reads, so RFB
// bytes arrive fragmented at ARBITRARY boundaries — `RFBStreamParser` buffers
// and only emits complete messages. The web reference client is noVNC
// (surfaces/ai.allternit.com/src/views/bots/BotDesktopView.tsx).
//
// Sandbox x11vnc runs with `-nopw` → Security Type None (1). A paired
// computer's own screen sharing may ask for a sign-in instead: VNC password
// (2) or, on a Mac, Apple's user name + password (30) — see RFBAuth.swift. All multi-byte integer fields are big-endian on the
// wire (RFB 3.8 §6/§7).
// -----------------------------------------------------------------------------

/// RFB encoding values (RFB 3.8 §7.7) the client requests via SetEncodings.
enum RFBEncoding {
    static let raw: Int32 = 0
    static let copyRect: Int32 = 1
    static let zrle: Int32 = 16
}

/// Typed failures — a decode problem throws one of these instead of
/// rendering garbage (repo rule: no silent catch-alls).
enum RFBError: LocalizedError {
    /// `ws_url` could not be absolutized against the API origin.
    case invalidURL(String)
    /// Server greeting wasn't an RFB version line.
    case invalidProtocolVersion(String)
    /// Server speaks a version older than 3.8 (we answer 3.8 only).
    case unsupportedProtocolVersion(String)
    /// Security handshake: server sent a failure reason (count-0 reply).
    case securityHandshakeFailed(String)
    /// Server offered no Security Type None (the sandbox runs `x11vnc -nopw`,
    /// so anything else is a proxy/config bug worth surfacing).
    case noSupportedSecurityType([UInt8])
    /// ServerInit advertised an empty framebuffer.
    case invalidFramebufferSize(Int, Int)
    /// A message arrived out of handshake order, or a handshake message
    /// arrived after ServerInit.
    case unexpectedMessage(String)
    /// Server→client message type we don't implement.
    case unknownServerMessage(UInt8)
    /// Rectangle encoding we never requested (SetEncodings sends 0/1/16).
    case unsupportedEncoding(Int32)
    /// Rectangle extends outside the negotiated framebuffer.
    case rectangleOutOfBounds
    /// Payload shorter/longer than the encoding requires.
    case malformedRectanglePayload
    /// ZRLE tile subencoding outside the defined ranges.
    case invalidZRLESubencoding(UInt8)
    /// ZRLE tile wrote more pixels than its tile has (corrupt stream).
    case malformedZRLETile
    /// zlib stream ended while a ZRLE rectangle still needed bytes.
    case zrleUnderflow
    /// libz inflate returned an error status.
    case zrleInflateFailed(Int32)
    /// An operation was attempted before the WebSocket connected.
    case notConnected
    /// The computer's screen sharing asks for a sign-in the viewer doesn't
    /// have yet (Apple's needs the user name too).
    case credentialsRequired(needsUsername: Bool)
    /// SecurityResult said no: wrong name or password, or access refused.
    case authenticationFailed(String)

    var errorDescription: String? {
        switch self {
        case .invalidURL(let path):
            return "Couldn't build a WebSocket URL from \(path)."
        case .invalidProtocolVersion(let line):
            return "Server didn't send an RFB version line (got \(line))."
        case .unsupportedProtocolVersion(let line):
            return "Server speaks \(line); the viewer requires RFB 3.8."
        case .securityHandshakeFailed(let reason):
            return "VNC security handshake failed: \(reason)"
        case .noSupportedSecurityType(let types):
            return "This computer's screen sharing uses a sign-in the app doesn't support (offered: \(types))."
        case .invalidFramebufferSize(let width, let height):
            return "Server advertised an invalid framebuffer (\(width)×\(height))."
        case .unexpectedMessage(let detail):
            return "Unexpected RFB message: \(detail)"
        case .unknownServerMessage(let type):
            return "Unknown RFB server message type \(type)."
        case .unsupportedEncoding(let encoding):
            return "Server used rectangle encoding \(encoding), which was never requested."
        case .rectangleOutOfBounds:
            return "A framebuffer rectangle extends outside the negotiated screen."
        case .malformedRectanglePayload:
            return "A framebuffer rectangle's payload doesn't match its encoding."
        case .invalidZRLESubencoding(let subencoding):
            return "Invalid ZRLE tile subencoding \(subencoding)."
        case .malformedZRLETile:
            return "A ZRLE tile produced more pixels than it contains."
        case .zrleUnderflow:
            return "The ZRLE stream ended mid-rectangle."
        case .zrleInflateFailed(let status):
            return "ZRLE decompression failed (zlib status \(status))."
        case .notConnected:
            return "The desktop stream isn't connected."
        case .credentialsRequired:
            return "This computer's screen sharing asks for a password."
        case .authenticationFailed(let reason):
            return reason
        }
    }
}

/// ServerInit payload (RFB 3.8 §7.3.2) — the negotiated framebuffer geometry
/// plus the desktop name. The server's own pixel format is carried in the
/// message but unused: the client immediately overrides it with
/// SetPixelFormat (see `RFBClientMessage.setPixelFormat()`).
struct RFBServerInit: Sendable, Equatable {
    let width: Int
    let height: Int
    let name: String
}

/// One FramebufferUpdate rectangle with its (still encoded) payload.
/// For CopyRect the payload is the 4-byte src-x/src-y pair; for ZRLE the
/// payload is this rectangle's segment of the persistent zlib stream (the
/// u32 length prefix already stripped by the parser); for Raw the raw
/// BGRX bytes.
struct RFBRectangle: Sendable, Equatable {
    let x: UInt16
    let y: UInt16
    let width: UInt16
    let height: UInt16
    let encoding: Int32
    let payload: Data
}

/// Complete server→client messages emitted by `RFBStreamParser`.
enum RFBServerMessage: Equatable {
    /// The 12-byte version line, e.g. "RFB 003.008\n".
    case protocolVersion(String)
    /// Security handshake answer carrying offered type bytes.
    case securityTypes([UInt8])
    /// Security handshake refusal (type count 0) with the server's reason.
    case securityFailure(String)
    case serverInit(RFBServerInit)
    case framebufferUpdate([RFBRectangle])
    /// Parsed-and-skipped (true color is negotiated, palettes never apply).
    case setColourMapEntries
    case bell
    /// Content skipped — the iOS viewer doesn't sync the remote clipboard.
    case serverCutText
}

/// Incremental parser for the server→client half of RFB. Bytes are appended
/// as WebSocket frames arrive (arbitrary fragmentation — the proxy reads the
/// sandbox TCP socket in 16 KB chunks, bot_desktop_stream.rs:172-188) and
/// `nextMessage()` returns nil until a whole message is buffered; nothing is
/// consumed for a partial message, so a parse that runs out of bytes simply
/// resumes where it stopped.
final class RFBStreamParser {
    /// Handshake phases advance automatically as messages are emitted — the
    /// exchange order is fixed (version → security → ServerInit → messages),
    /// with the client's replies interleaved by the caller.
    enum Phase: Equatable {
        case version
        case security
        case serverInit
        case messages
    }

    private(set) var phase: Phase = .version

    private var buffer: [UInt8] = []
    private var offset = 0

    /// Bytes buffered but not yet consumed (diagnostics).
    var bufferedByteCount: Int { buffer.count - offset }

    func append(_ data: Data) {
        // Compact first so the buffer can't grow unboundedly between updates.
        if offset > 0 {
            buffer.removeFirst(offset)
            offset = 0
        }
        buffer.append(contentsOf: data)
    }

    /// The next complete message, or nil when the buffer holds only a
    /// partial one. Throws on bytes that violate the protocol — the stream
    /// is unrecoverable at that point and the client tears it down.
    func nextMessage() throws -> RFBServerMessage? {
        switch phase {
        case .version: return try parseVersion()
        case .security: return try parseSecurity()
        case .serverInit: return try parseServerInit()
        case .messages: return try parseServerToClientMessage()
        }
    }

    // MARK: - Handshake

    private func parseVersion() throws -> RFBServerMessage? {
        guard available >= 12 else { return nil }
        let lineBytes = bytes(at: 0, count: 12)
        guard let line = String(bytes: lineBytes, encoding: .ascii),
              line.hasPrefix("RFB ") else {
            throw RFBError.invalidProtocolVersion(String(bytes: lineBytes, encoding: .ascii) ?? "<non-ascii>")
        }
        commit(12)
        phase = .security
        return .protocolVersion(line)
    }

    private func parseSecurity() throws -> RFBServerMessage? {
        guard available >= 1 else { return nil }
        let count = Int(readUInt8(at: 0))
        if count == 0 {
            // Failure: u32 reason length + reason string (RFB 3.8 §7.1.2).
            guard available >= 5 else { return nil }
            let reasonLength = Int(readUInt32(at: 1))
            guard available >= 5 + reasonLength else { return nil }
            let reason = String(bytes: bytes(at: 5, count: reasonLength), encoding: .utf8)
                ?? "unknown reason"
            commit(5 + reasonLength)
            return .securityFailure(reason)
        }
        guard available >= 1 + count else { return nil }
        let types = bytes(at: 1, count: count)
        commit(1 + count)
        phase = .serverInit
        return .securityTypes(types)
    }

    private func parseServerInit() throws -> RFBServerMessage? {
        guard available >= 24 else { return nil }
        let width = Int(readUInt16(at: 0))
        let height = Int(readUInt16(at: 2))
        // Bytes 4..<20 are the server's pixel format — unused here (the
        // client overrides it with SetPixelFormat right after ServerInit).
        let nameLength = Int(readUInt32(at: 20))
        guard available >= 24 + nameLength else { return nil }
        let name = String(bytes: bytes(at: 24, count: nameLength), encoding: .utf8) ?? ""
        commit(24 + nameLength)
        phase = .messages
        return .serverInit(RFBServerInit(width: width, height: height, name: name))
    }

    // MARK: - Server→client messages (RFB 3.8 §7.6)

    private func parseServerToClientMessage() throws -> RFBServerMessage? {
        guard available >= 1 else { return nil }
        let type = readUInt8(at: 0)
        switch type {
        case 0: return try parseFramebufferUpdate()
        case 1: return parseSetColourMapEntries()
        case 2:
            commit(1)
            return .bell
        case 3: return parseServerCutText()
        default:
            throw RFBError.unknownServerMessage(type)
        }
    }

    /// FramebufferUpdate: u8 type, u8 pad, u16 rect-count, then per rect a
    /// 12-byte header (x/y/w/h u16 + i32 encoding) plus its payload. The
    /// whole update is emitted atomically — a partially buffered update
    /// leaves everything unconsumed so the next append resumes cleanly.
    private func parseFramebufferUpdate() throws -> RFBServerMessage? {
        guard available >= 4 else { return nil }
        let rectCount = Int(readUInt16(at: 2))
        var pos = 4
        var rects: [RFBRectangle] = []
        rects.reserveCapacity(rectCount)
        for _ in 0..<rectCount {
            guard available >= pos + 12 else { return nil }
            let x = readUInt16(at: pos)
            let y = readUInt16(at: pos + 2)
            let width = readUInt16(at: pos + 4)
            let height = readUInt16(at: pos + 6)
            let encoding = Int32(bitPattern: readUInt32(at: pos + 8))
            pos += 12
            let payload: [UInt8]
            switch encoding {
            case RFBEncoding.raw:
                let length = Int(width) * Int(height) * 4
                guard available >= pos + length else { return nil }
                payload = bytes(at: pos, count: length)
                pos += length
            case RFBEncoding.copyRect:
                guard available >= pos + 4 else { return nil }
                payload = bytes(at: pos, count: 4)
                pos += 4
            case RFBEncoding.zrle:
                guard available >= pos + 4 else { return nil }
                let length = Int(readUInt32(at: pos))
                guard available >= pos + 4 + length else { return nil }
                payload = bytes(at: pos + 4, count: length)
                pos += 4 + length
            default:
                throw RFBError.unsupportedEncoding(encoding)
            }
            rects.append(RFBRectangle(
                x: x, y: y, width: width, height: height,
                encoding: encoding, payload: Data(payload)
            ))
        }
        commit(pos)
        return .framebufferUpdate(rects)
    }

    /// SetColourMapEntries: u8 pad, u16 first-colour, u16 colour-count, then
    /// 6 bytes per colour. Skipped wholesale (true color is negotiated).
    private func parseSetColourMapEntries() -> RFBServerMessage? {
        guard available >= 6 else { return nil }
        let colourCount = Int(readUInt16(at: 4))
        guard available >= 6 + 6 * colourCount else { return nil }
        commit(6 + 6 * colourCount)
        return .setColourMapEntries
    }

    /// ServerCutText: 3 pad bytes, u32 length, latin-1 bytes. Skipped.
    private func parseServerCutText() -> RFBServerMessage? {
        guard available >= 8 else { return nil }
        let length = Int(readUInt32(at: 4))
        guard available >= 8 + length else { return nil }
        commit(8 + length)
        return .serverCutText
    }

    // MARK: - Buffer primitives

    private var available: Int { buffer.count - offset }

    /// Exactly `count` raw bytes, or nil (nothing consumed) until they are
    /// buffered — for the security exchange, whose shape depends on the type
    /// the client picked (challenges, Diffie-Hellman keys, SecurityResult).
    func takeBytes(_ count: Int) -> [UInt8]? {
        guard available >= count else { return nil }
        let result = bytes(at: 0, count: count)
        commit(count)
        return result
    }

    private func commit(_ count: Int) {
        offset += count
    }

    private func readUInt8(at pos: Int) -> UInt8 {
        buffer[offset + pos]
    }

    /// All wire integers are big-endian (RFB 3.8 §6).
    private func readUInt16(at pos: Int) -> UInt16 {
        UInt16(buffer[offset + pos]) << 8 | UInt16(buffer[offset + pos + 1])
    }

    private func readUInt32(at pos: Int) -> UInt32 {
        UInt32(buffer[offset + pos]) << 24
            | UInt32(buffer[offset + pos + 1]) << 16
            | UInt32(buffer[offset + pos + 2]) << 8
            | UInt32(buffer[offset + pos + 3])
    }

    private func bytes(at pos: Int, count: Int) -> [UInt8] {
        Array(buffer[(offset + pos)..<(offset + pos + count)])
    }
}

// MARK: - Client→server messages (RFB 3.8 §7.5)

/// Byte builders for the client half of the protocol. Each builder returns
/// exactly one RFB message per WebSocket binary frame — the proxy writes
/// each frame to the TCP socket verbatim (bot_desktop_stream.rs:152-156),
/// so message boundaries are preserved.
enum RFBClientMessage {
    /// ClientProtocolVersion: the only version this client speaks. The
    /// sandbox's x11vnc greets with "RFB 003.008\n"; an older greeting is
    /// rejected by the client rather than downgraded.
    static func protocolVersion() -> Data {
        Data("RFB 003.008\n".utf8)
    }

    /// Security type selection: None (1) — x11vnc runs with `-nopw`.
    static func securitySelectionNone() -> Data {
        Data([1])
    }

    /// Security type selection by number (2 = VNC password, 30 = Apple).
    static func securitySelection(_ type: UInt8) -> Data {
        Data([type])
    }

    /// ClientInit: shared-flag 1 (the bot keeps its own session alive).
    static func clientInit() -> Data {
        Data([1])
    }

    /// SetPixelFormat (message type 0): 32 bpp, depth 24, true-color,
    /// big-endian-flag 0, maxima 255/255/255, shifts R16/G8/B0.
    ///
    /// A pixel value is therefore 0xXXRRGGBB; with little-endian byte order
    /// the wire bytes are [B, G, R, pad] — exactly the BGRA memory layout
    /// `RFBFramebuffer` renders (CGBitmapInfo.byteOrder32Little +
    /// noneSkipFirst), so Raw rectangles are straight row copies. ZRLE
    /// CPIXELs (3 bytes, least-significant first) arrive as [B, G, R].
    static func setPixelFormat() -> Data {
        var data = Data()
        data.appendUInt8(0) // message type
        data.appendUInt8(0); data.appendUInt8(0); data.appendUInt8(0) // padding
        data.appendUInt8(32) // bits-per-pixel
        data.appendUInt8(24) // depth
        data.appendUInt8(0) // big-endian-flag: little
        data.appendUInt8(1) // true-colour-flag
        data.appendUInt16(255) // red-max
        data.appendUInt16(255) // green-max
        data.appendUInt16(255) // blue-max
        data.appendUInt8(16) // red-shift
        data.appendUInt8(8) // green-shift
        data.appendUInt8(0) // blue-shift
        data.appendUInt8(0); data.appendUInt8(0); data.appendUInt8(0) // padding
        return data
    }

    /// SetEncodings (message type 2): Raw + CopyRect + ZRLE.
    static func setEncodings() -> Data {
        var data = Data()
        data.appendUInt8(2)
        data.appendUInt8(0) // padding
        data.appendUInt16(3)
        data.appendInt32(RFBEncoding.raw)
        data.appendInt32(RFBEncoding.copyRect)
        data.appendInt32(RFBEncoding.zrle)
        return data
    }

    /// FramebufferUpdateRequest (message type 3). The initial request is
    /// non-incremental (full repaint); afterwards each processed update is
    /// answered with an incremental request for the full screen.
    static func framebufferUpdateRequest(incremental: Bool, width: UInt16, height: UInt16) -> Data {
        var data = Data()
        data.appendUInt8(3)
        data.appendUInt8(incremental ? 1 : 0)
        data.appendUInt16(0) // x
        data.appendUInt16(0) // y
        data.appendUInt16(width)
        data.appendUInt16(height)
        return data
    }

    /// KeyEvent (message type 4): down-flag, 2 pad bytes, u32 keysym.
    static func keyEvent(keysym: UInt32, down: Bool) -> Data {
        var data = Data()
        data.appendUInt8(4)
        data.appendUInt8(down ? 1 : 0)
        data.appendUInt16(0) // padding
        data.appendUInt32(keysym)
        return data
    }

    /// PointerEvent (message type 5): button mask (bit 0 left, 1 middle,
    /// 2 right) + position.
    static func pointerEvent(buttonMask: UInt8, x: UInt16, y: UInt16) -> Data {
        var data = Data()
        data.appendUInt8(5)
        data.appendUInt8(buttonMask)
        data.appendUInt16(x)
        data.appendUInt16(y)
        return data
    }
}

extension Data {
    /// Big-endian appends for the RFB wire format.
    fileprivate mutating func appendUInt8(_ value: UInt8) { append(value) }
    fileprivate mutating func appendUInt16(_ value: UInt16) {
        append(UInt8(value >> 8)); append(UInt8(value & 0xFF))
    }
    fileprivate mutating func appendUInt32(_ value: UInt32) {
        append(UInt8(value >> 24)); append(UInt8((value >> 16) & 0xFF))
        append(UInt8((value >> 8) & 0xFF)); append(UInt8(value & 0xFF))
    }
    fileprivate mutating func appendInt32(_ value: Int32) {
        appendUInt32(UInt32(bitPattern: value))
    }
}

// MARK: - X11 keysyms (RFB 3.8 §7.5.4)

/// The keysym subset the viewer sends. Printable Latin-1 maps 1:1
/// (U+20…U+7E and U+A0…U+FF equal their keysyms per X11 keysymdef.h);
/// everything structural is listed explicitly.
enum RFBKeysym {
    static let backSpace: UInt32 = 0xFF08
    static let tab: UInt32 = 0xFF09
    static let `return`: UInt32 = 0xFF0D
    static let escape: UInt32 = 0xFF1B
    static let delete: UInt32 = 0xFFFF
    static let left: UInt32 = 0xFF51
    static let up: UInt32 = 0xFF52
    static let right: UInt32 = 0xFF53
    static let down: UInt32 = 0xFF54
    static let shiftLeft: UInt32 = 0xFFE1
    static let shiftRight: UInt32 = 0xFFE2
    static let controlLeft: UInt32 = 0xFFE3
    static let controlRight: UInt32 = 0xFFE4
    static let altLeft: UInt32 = 0xFFE9
    /// Hardware ⌘ maps to Super_L — the X11 desktop has no Command key.
    static let superLeft: UInt32 = 0xFFEB
    static let superRight: UInt32 = 0xFFEC

    /// Keysym for a printable character, or nil when it has no Latin-1
    /// keysym (emoji, CJK, …) — unmappable input is dropped by design, the
    /// remote side has no keysym for it either.
    static func forScalar(_ scalar: Unicode.Scalar) -> UInt32? {
        switch scalar.value {
        case 0x20...0x7E, 0xA0...0xFF:
            return scalar.value
        default:
            return nil
        }
    }
}
