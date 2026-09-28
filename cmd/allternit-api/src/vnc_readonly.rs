//! RFB-aware read-only filter for the VNC-over-WebSocket proxy.
//!
//! Read-only viewers (e.g. the shipped embed viewer, whose tokens are always
//! read-only) must be able to complete the RFB handshake and receive display
//! traffic, while keyboard/mouse/clipboard input is blocked. A blanket drop of
//! every client->server byte breaks the handshake entirely (the server greets
//! with "RFB 003.889\n" but never sees the client's version), so instead the
//! proxy runs each binary frame through [`RfbReadOnlyFilter`] and forwards only
//! what the filter returns.
//!
//! Client->server direction state machine:
//!
//! - `Version`: expects 12 bytes starting with "RFB ". Forwarded verbatim.
//! - `SecurityChoice`: 1 byte. `1` (None) -> `ClientInit`; `2` (VNC auth) ->
//!   `AuthResponse`; `30` (Apple Remote Desktop) -> `ArdResponse`; anything
//!   else -> `Passthrough`.
//! - `AuthResponse`: 16 bytes of challenge response, then `ClientInit`.
//! - `ArdResponse`: 128 bytes of encrypted credentials plus the client's DH
//!   public key (the server's key length), then `ClientInit`. macOS Screen
//!   Sharing offers ARD first, so paired Macs and this Mac land here.
//!
//! The ARD response length depends on the server's DH key length, which only
//! the server sends, so the proxy also passes every server->client byte to
//! [`RfbReadOnlyFilter::observe_server`].
//! - `ClientInit`: 1 shared-flag byte, then `Normal`.
//! - `Normal`: parses typed client messages. SetPixelFormat (0, 20 bytes),
//!   SetEncodings (2, 4 + 4n), FramebufferUpdateRequest (3, 10 bytes), and
//!   EnableContinuousUpdates (150, 4 bytes) are forwarded; KeyEvent (4, 8
//!   bytes), PointerEvent (5, 6 bytes), and ClientCutText (6, 8 + len32le) are
//!   consumed without forwarding. Unknown types are forwarded as a single byte
//!   so the stream stays aligned.
//!
//! Degradation: when the client picks a security mechanism this filter does not
//! understand (anything but None/VNC auth/ARD), or sends bytes that do not look
//! like an RFB version string, the filter switches to `Passthrough` and sets
//! [`RfbReadOnlyFilter::unfilterable`]: read-only can no longer be enforced
//! for the rest of the connection. The proxy site logs this once.

/// Parse state for the client->server RFB direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RfbState {
    /// Expecting the 12-byte "RFB xxx.yyy\n" client version string.
    Version,
    /// Expecting the 1-byte security-type choice.
    SecurityChoice,
    /// Expecting the 16-byte VNC-auth challenge response.
    AuthResponse,
    /// Expecting the ARD response: 128 bytes of credentials + the client's
    /// DH public key (as long as the server's).
    ArdResponse,
    /// Expecting the 1-byte ClientInit shared flag.
    ClientInit,
    /// Handshake complete; parse typed client messages.
    Normal,
    /// Read-only can no longer be enforced; forward everything.
    Passthrough,
}

/// RFB read-only filter. Feed it each client->server frame; it returns the
/// bytes that may be written to the VNC server. Input messages (KeyEvent,
/// PointerEvent, ClientCutText) are silently consumed.
pub struct RfbReadOnlyFilter {
    state: RfbState,
    buf: Vec<u8>,
    unfilterable: bool,
    server: ServerHandshake,
}

/// What the filter has seen of the server's side of the handshake: the
/// version string, the security-type list, then (for ARD) the 2-byte DH
/// generator and 2-byte key length.
#[derive(Debug, Default)]
struct ServerHandshake {
    seen: Vec<u8>,
    done: bool,
}

impl ServerHandshake {
    /// Offset of the first byte after the security-type list, once known.
    fn types_end(&self) -> Option<usize> {
        let count = *self.seen.get(12)? as usize;
        (count > 0 && self.seen.len() >= 13 + count).then_some(13 + count)
    }

    /// The ARD DH key length (bytes 2..4 after the type list).
    fn ard_key_len(&self) -> Option<usize> {
        let at = self.types_end()?;
        let bytes = self.seen.get(at + 2..at + 4)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]) as usize)
    }
}

impl Default for RfbReadOnlyFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl RfbReadOnlyFilter {
    pub fn new() -> Self {
        Self {
            state: RfbState::Version,
            buf: Vec::new(),
            unfilterable: false,
            server: ServerHandshake::default(),
        }
    }

    /// Record server->client bytes. Only the handshake prefix is kept (enough
    /// to read the ARD key length); after that this is a no-op.
    pub fn observe_server(&mut self, data: &[u8]) {
        if self.server.done {
            return;
        }
        self.server.seen.extend_from_slice(data);
        if self.server.ard_key_len().is_some() || self.server.seen.len() > 512 {
            self.server.done = true;
            self.server.seen.truncate(512);
        }
    }

    /// True once the filter has degraded to unfiltered passthrough because it
    /// could not understand the client's security choice or version string.
    /// Read-only is NOT enforced from this point on.
    pub fn unfilterable(&self) -> bool {
        self.unfilterable
    }

    /// Feed one received binary frame; returns the bytes to forward to the
    /// TCP socket (possibly empty when the frame held only input messages or
    /// a partial message that is still being buffered).
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if self.state == RfbState::Passthrough {
            return data.to_vec();
        }
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            match self.state {
                RfbState::Version => {
                    if self.buf.len() < 4 {
                        break;
                    }
                    if &self.buf[..4] != b"RFB " {
                        self.enter_passthrough(&mut out);
                        break;
                    }
                    if self.buf.len() < 12 {
                        break;
                    }
                    out.extend_from_slice(&self.buf[..12]);
                    self.buf.drain(..12);
                    self.state = RfbState::SecurityChoice;
                }
                RfbState::SecurityChoice => {
                    if self.buf.is_empty() {
                        break;
                    }
                    let v = self.buf[0];
                    out.push(v);
                    self.buf.remove(0);
                    self.state = match v {
                        1 => RfbState::ClientInit,
                        2 => RfbState::AuthResponse,
                        30 => RfbState::ArdResponse,
                        _ => {
                            self.enter_passthrough(&mut out);
                            break;
                        }
                    };
                }
                RfbState::AuthResponse => {
                    if self.buf.len() < 16 {
                        break;
                    }
                    out.extend_from_slice(&self.buf[..16]);
                    self.buf.drain(..16);
                    self.state = RfbState::ClientInit;
                }
                RfbState::ArdResponse => {
                    // The server's DH parameters precede the client's
                    // response, so the key length is known by the time
                    // the response arrives; until then, hold the bytes.
                    let Some(key_len) = self.server.ard_key_len() else {
                        break;
                    };
                    let len = 128 + key_len;
                    if self.buf.len() < len {
                        break;
                    }
                    out.extend_from_slice(&self.buf[..len]);
                    self.buf.drain(..len);
                    self.state = RfbState::ClientInit;
                }
                RfbState::ClientInit => {
                    if self.buf.is_empty() {
                        break;
                    }
                    out.push(self.buf[0]);
                    self.buf.remove(0);
                    self.state = RfbState::Normal;
                }
                RfbState::Normal => {
                    if self.buf.is_empty() {
                        break;
                    }
                    let msg_len = match self.buf[0] {
                        // SetPixelFormat
                        0 => Some(20usize),
                        // SetEncodings: type + pad + num(u16) + 4 bytes each.
                        2 => {
                            if self.buf.len() < 4 {
                                None
                            } else {
                                let n = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
                                Some(4 + 4 * n)
                            }
                        }
                        // FramebufferUpdateRequest
                        3 => Some(10),
                        // EnableContinuousUpdates: type + enable + u16 pad.
                        150 => Some(4),
                        // KeyEvent — input, dropped.
                        4 => Some(8),
                        // PointerEvent — input, dropped.
                        5 => Some(6),
                        // ClientCutText: 8-byte header + len32le payload — input, dropped.
                        6 => {
                            if self.buf.len() < 8 {
                                None
                            } else {
                                let len = u32::from_le_bytes([
                                    self.buf[4],
                                    self.buf[5],
                                    self.buf[6],
                                    self.buf[7],
                                ]) as usize;
                                Some(8 + len)
                            }
                        }
                        // Unknown type: forward the type byte alone so the
                        // stream stays aligned.
                        _ => Some(1),
                    };
                    let Some(len) = msg_len else { break };
                    if self.buf.len() < len {
                        break;
                    }
                    let is_input = matches!(self.buf[0], 4 | 5 | 6);
                    if !is_input {
                        out.extend_from_slice(&self.buf[..len]);
                    }
                    self.buf.drain(..len);
                }
                RfbState::Passthrough => {
                    out.append(&mut self.buf);
                    break;
                }
            }
        }
        out
    }

    /// Degrade to forwarding everything. Read-only cannot be enforced for
    /// security mechanisms this filter does not understand.
    fn enter_passthrough(&mut self, out: &mut Vec<u8>) {
        self.state = RfbState::Passthrough;
        self.unfilterable = true;
        out.append(&mut self.buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &[u8] = b"RFB 003.008\n";

    fn handshake_none(filter: &mut RfbReadOnlyFilter) -> Vec<u8> {
        let mut out = filter.feed(VERSION);
        out.extend_from_slice(&filter.feed(&[1]));
        out.extend_from_slice(&filter.feed(&[1]));
        out
    }

    #[test]
    fn full_handshake_security_none_then_input_dropped_display_flows() {
        let mut f = RfbReadOnlyFilter::new();
        assert_eq!(handshake_none(&mut f), [VERSION, &[1u8], &[1u8]].concat());

        // SetPixelFormat (type 0, 20 bytes) flows.
        let set_pixel_format: Vec<u8> = {
            let mut m = vec![0u8; 20];
            m[0] = 0;
            m
        };
        assert_eq!(f.feed(&set_pixel_format), set_pixel_format);

        // FramebufferUpdateRequest (type 3, 10 bytes) flows.
        let fur: Vec<u8> = {
            let mut m = vec![0u8; 10];
            m[0] = 3;
            m
        };
        assert_eq!(f.feed(&fur), fur);

        // KeyEvent (type 4, 8 bytes) is dropped.
        let key: Vec<u8> = {
            let mut m = vec![0u8; 8];
            m[0] = 4;
            m
        };
        assert!(f.feed(&key).is_empty());
        assert!(!f.unfilterable());
    }

    #[test]
    fn security_vnc_auth_path_completes_handshake() {
        let mut f = RfbReadOnlyFilter::new();
        let mut out = f.feed(VERSION);
        out.extend_from_slice(&f.feed(&[2])); // VNC auth chosen.
        let auth = [0xAAu8; 16];
        out.extend_from_slice(&f.feed(&auth));
        out.extend_from_slice(&f.feed(&[0])); // shared flag.
        assert_eq!(out, [VERSION, &[2u8], &auth[..], &[0u8]].concat());
        assert!(!f.unfilterable());

        // Post-handshake display request still flows.
        let fur = [3u8, 0, 0, 0, 0, 0, 10, 0, 10, 0];
        assert_eq!(f.feed(&fur), fur);
    }

    #[test]
    fn pointer_and_client_cut_text_dropped_with_length_consumed() {
        let mut f = RfbReadOnlyFilter::new();
        handshake_none(&mut f);

        // PointerEvent (type 5, 6 bytes) dropped.
        assert!(f.feed(&[5, 0, 0, 0, 0, 0]).is_empty());

        // ClientCutText (type 6) with an 11-byte payload dropped wholesale.
        let mut cut = vec![6u8, 0, 0, 0, 11, 0, 0, 0];
        cut.extend_from_slice(b"hello world");
        assert!(f.feed(&cut).is_empty());

        // Stream stays aligned: the next display request flows untouched.
        let fur = [3u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(f.feed(&fur), fur);
    }

    #[test]
    fn partial_frames_across_feeds_are_buffered() {
        let mut f = RfbReadOnlyFilter::new();
        assert!(f.feed(b"RF").is_empty());
        assert!(f.feed(b"B 003.00").is_empty());
        assert_eq!(f.feed(b"8\n"), VERSION);
        assert!(f.feed(&[]).is_empty());
        // Security choice byte arrives split from ClientInit.
        assert_eq!(f.feed(&[1]), vec![1u8]);
        assert_eq!(f.feed(&[0]), vec![0u8]);
        assert!(!f.unfilterable());
    }

    #[test]
    fn unknown_security_type_degrades_to_passthrough() {
        let mut f = RfbReadOnlyFilter::new();
        let mut out = f.feed(VERSION);
        out.extend_from_slice(&f.feed(&[19])); // e.g. VeNCrypt — not filterable.
        assert_eq!(out, [VERSION, &[19u8]].concat());
        assert!(f.unfilterable());

        // Everything afterwards flows unfiltered, including input messages.
        let key = [4u8; 8];
        assert_eq!(f.feed(&key), key);
        let cut = [6u8, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(f.feed(&cut), cut);
    }

    /// macOS Screen Sharing: server offers ARD (30), client answers with
    /// 128 bytes of credentials + its DH public key.
    #[test]
    fn apple_remote_desktop_auth_stays_filtered() {
        let mut f = RfbReadOnlyFilter::new();
        let key_len = 128usize;
        let mut server = b"RFB 003.889\n".to_vec();
        server.extend_from_slice(&[2, 30, 2]); // two types: ARD, VNC auth
        f.observe_server(&server);

        let mut out = f.feed(b"RFB 003.008\n");
        out.extend_from_slice(&f.feed(&[30]));
        // DH params: generator (2), key length (2), prime, server public key.
        let mut params = vec![0, 2];
        params.extend_from_slice(&(key_len as u16).to_be_bytes());
        params.extend(std::iter::repeat(7u8).take(key_len * 2));
        // The response can arrive split, and before the params are read.
        let response = vec![0xABu8; 128 + key_len];
        assert!(f.feed(&response[..100]).is_empty());
        f.observe_server(&params[..3]);
        assert!(f.feed(&[]).is_empty());
        f.observe_server(&params[3..]);
        out.extend_from_slice(&f.feed(&response[100..]));
        out.extend_from_slice(&f.feed(&[1])); // shared flag
        assert_eq!(
            out,
            [&b"RFB 003.008\n"[..], &[30u8], &response[..], &[1u8]].concat()
        );
        assert!(!f.unfilterable());

        // Input is dropped; display requests flow.
        assert!(f.feed(&[5, 0, 0, 0, 0, 0]).is_empty());
        assert!(f.feed(&[4, 1, 0, 0, 0, 0, 0, 97]).is_empty());
        let fur = [3u8, 0, 0, 0, 0, 0, 10, 0, 10, 0];
        assert_eq!(f.feed(&fur), fur);
    }

    #[test]
    fn non_rfb_bytes_degrade_to_passthrough() {
        let mut f = RfbReadOnlyFilter::new();
        let junk = [0x01u8; 12];
        assert_eq!(f.feed(&junk), junk);
        assert!(f.unfilterable());
    }

    #[test]
    fn key_event_split_across_feeds_is_dropped() {
        let mut f = RfbReadOnlyFilter::new();
        handshake_none(&mut f);

        let key: Vec<u8> = {
            let mut m = vec![0u8; 8];
            m[0] = 4;
            m
        };
        // First 3 bytes: type 4 is known to be 8 bytes, so nothing forwards.
        assert!(f.feed(&key[..3]).is_empty());
        // Remaining 5 bytes complete the message; it is consumed, not forwarded.
        assert!(f.feed(&key[3..]).is_empty());

        // The display request after it is unaffected.
        let fur = [3u8, 0, 0, 0, 0, 0, 0, 4, 0, 4];
        assert_eq!(f.feed(&fur), fur);
    }

    #[test]
    fn set_encodings_with_encodings_forwarded() {
        let mut f = RfbReadOnlyFilter::new();
        handshake_none(&mut f);

        // SetEncodings (type 2) with 3 encodings: 4 + 4*3 = 16 bytes.
        let mut m = vec![2u8, 0, 0, 3];
        m.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2, 255, 255, 255, 33]);
        assert_eq!(f.feed(&m), m);

        // EnableContinuousUpdates (type 150, 4 bytes) forwarded.
        let ecu = [150u8, 1, 0, 0];
        assert_eq!(f.feed(&ecu), ecu);
    }

    #[test]
    fn unknown_message_type_forwards_single_byte_and_stays_aligned() {
        let mut f = RfbReadOnlyFilter::new();
        handshake_none(&mut f);

        // Type 7 (Bell-ish/unknown) forwards only its type byte.
        assert_eq!(f.feed(&[7]), vec![7u8]);
        let fur = [3u8, 9, 9, 9, 9, 9, 9, 9, 9, 9];
        assert_eq!(f.feed(&fur), fur);
    }
}
