//! Length-prefixed JSON frames for the Unix socket transport (CONTRACT.md 2.0): a 4-byte unsigned
//! big-endian length `N`, then `N` bytes of UTF-8 JSON. Max 1 MiB. One request frame gets exactly
//! one response frame, in order. std only.
use crate::error::AuthorityError;
use crate::types::ErrorCode;
use crate::MAX_FRAME_BYTES;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Read, Write};

/// Why a frame could not be encoded or decoded.
#[derive(Debug)]
pub enum FrameError {
    /// Body longer than [`MAX_FRAME_BYTES`]. The connection is closed with `frame_too_large`.
    TooLarge(usize),
    /// The buffer ended before the whole frame arrived.
    Incomplete,
    /// The body was not valid JSON for the target type.
    Json(serde_json::Error),
    Io(io::Error),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes exceeds {MAX_FRAME_BYTES}"),
            FrameError::Incomplete => f.write_str("incomplete frame"),
            FrameError::Json(e) => write!(f, "bad frame json: {e}"),
            FrameError::Io(e) => write!(f, "frame io: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<FrameError> for AuthorityError {
    fn from(e: FrameError) -> Self {
        let code = match e {
            FrameError::TooLarge(_) => ErrorCode::FrameTooLarge,
            _ => ErrorCode::BadRequest,
        };
        AuthorityError::without_receipt(code, e.to_string())
    }
}

/// Encode one value as a frame: 4-byte big-endian length, then the JSON.
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(value).map_err(FrameError::Json)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode the first frame in `buf`. Returns the value and the bytes consumed, so a caller can
/// advance past it. A declared length over the max fails before the body is read.
pub fn decode_frame<T: DeserializeOwned>(buf: &[u8]) -> Result<(T, usize), FrameError> {
    let head: [u8; 4] = buf
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or(FrameError::Incomplete)?;
    let len = u32::from_be_bytes(head) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let body = buf.get(4..4 + len).ok_or(FrameError::Incomplete)?;
    let value = serde_json::from_slice(body).map_err(FrameError::Json)?;
    Ok((value, 4 + len))
}

/// Write one frame to a stream.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), FrameError> {
    let bytes = encode_frame(value)?;
    w.write_all(&bytes).map_err(FrameError::Io)?;
    w.flush().map_err(FrameError::Io)
}

/// Read one frame from a stream. The length is checked before the body is allocated.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, FrameError> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head).map_err(FrameError::Io)?;
    let len = u32::from_be_bytes(head) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(FrameError::Io)?;
    serde_json::from_slice(&body).map_err(FrameError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calls::{KernelRequest, Request};
    use crate::types::{ApprovalRedeemBody, Caller, CallerKind};

    fn sample() -> KernelRequest {
        KernelRequest {
            v: 1,
            id: "r1".into(),
            caller: Caller {
                kind: CallerKind::Run,
                principal: "run_01".into(),
            },
            request: Request::ApprovalRedeem(ApprovalRedeemBody {
                token: "tok".into(),
                action_hash: format!("sha256:{}", "a".repeat(64)),
            }),
        }
    }

    #[test]
    fn round_trip_and_wire_shape() {
        let bytes = encode_frame(&sample()).unwrap();
        let n = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(n, bytes.len() - 4);
        let v: serde_json::Value = serde_json::from_slice(&bytes[4..]).unwrap();
        assert_eq!(v["call"], "approval.redeem");
        assert_eq!(v["body"]["token"], "tok");
        let (back, used): (KernelRequest, usize) = decode_frame(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(back.id, "r1");
        assert!(matches!(back.request, Request::ApprovalRedeem(_)));
    }

    #[test]
    fn two_frames_back_to_back_and_stream() {
        let mut buf = encode_frame(&sample()).unwrap();
        buf.extend(encode_frame(&sample()).unwrap());
        let (_, used): (KernelRequest, usize) = decode_frame(&buf).unwrap();
        let (_, used2): (KernelRequest, usize) = decode_frame(&buf[used..]).unwrap();
        assert_eq!(used + used2, buf.len());
        let mut cur = std::io::Cursor::new(buf);
        let _: KernelRequest = read_frame(&mut cur).unwrap();
        let _: KernelRequest = read_frame(&mut cur).unwrap();
    }

    #[test]
    fn oversize_and_truncated_frames_fail() {
        let mut big = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec();
        big.extend_from_slice(b"{}");
        assert!(matches!(
            decode_frame::<serde_json::Value>(&big),
            Err(FrameError::TooLarge(_))
        ));
        let ok = encode_frame(&sample()).unwrap();
        assert!(matches!(
            decode_frame::<KernelRequest>(&ok[..ok.len() - 1]),
            Err(FrameError::Incomplete)
        ));
        assert!(matches!(
            decode_frame::<KernelRequest>(&ok[..2]),
            Err(FrameError::Incomplete)
        ));
    }
}
