// Ported from iCTRL (https://github.com/Allternit/allternit-ictrl, iCTRLKit/LiveScreenPacket.swift).
// Copyright iCTRL authors. Licensed under the Apache License, Version 2.0;
// see http://www.apache.org/licenses/LICENSE-2.0. Modified for Allternit:
// the extension now sends these packets over an outbound WebSocket (one
// packet per sealed binary frame) instead of loopback TCP.

import Foundation

/// Wire framing for live screen video and app sound: the AllternitScreen
/// broadcast extension seals one of these per WebSocket frame, and the node
/// fans the opened packets out to viewers unchanged (the same format the
/// iCTRL live screen uses, so viewers decode both alike).
///
///     u8 type | u32 big-endian length | payload[length]
///
/// Compiled into the app and the AllternitScreen extension — pure Foundation
/// on purpose, so the extension stays small.
enum LiveScreenPacket {
    enum Kind: UInt8 {
        /// `{"broadcasting":bool}`. Also the 2 s keepalive on /video, so a
        /// client can tell a stalled socket from a screen that isn't changing
        /// (ReplayKit sends no frames while nothing moves).
        case status = 0
        /// `{"width","height","scale","rotation"}` — the encoded pixel size,
        /// pixels per UIKit point, and 0/90/180/270.
        case config = 1
        /// One H.264 access unit in Annex-B. Every IDR carries SPS+PPS in-band.
        case frame = 2
        /// `{"reason":string}` — the broadcast stopped. The /video connection
        /// stays open; a new CONFIG follows if it restarts.
        case ended = 3
        /// `{"codec":"aac","sampleRate":int,"channels":int,"asc":"<base64>"}` —
        /// the app-sound format; `asc` is the bare AudioSpecificConfig (the
        /// WebCodecs `mp4a.40.2` description). Resent on resume/keyframe
        /// requests and whenever the format changes.
        case audioConfig = 4
        /// One raw AAC-LC access unit (no ADTS header). On /video, only after
        /// the client has had its first IDR, so sound never runs ahead of picture.
        case audio = 5
    }

    static let headerLength = 5

    static func encode(_ kind: Kind, _ payload: Data) -> Data {
        var out = Data(capacity: headerLength + payload.count)
        out.append(kind.rawValue)
        let n = UInt32(payload.count)
        out.append(contentsOf: [UInt8(n >> 24 & 0xff), UInt8(n >> 16 & 0xff), UInt8(n >> 8 & 0xff), UInt8(n & 0xff)])
        out.append(payload)
        return out
    }

    static func json(_ kind: Kind, _ object: [String: Any]) -> Data {
        let body = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data("{}".utf8)
        return encode(kind, body)
    }
}
