//! Byte ranges for FETCH_DATA.
//!
//! cfapi accepts a TRANSFER_DATA only when its offset is 4096-aligned and its
//! length is a multiple of 4096, except for the last range of the file, which may
//! end at end-of-file. The relay takes at most 4 MiB per request (SPEC section 5),
//! so each range is also at most 4 MiB.

/// cfapi transfer alignment.
pub const ALIGN: u64 = 4096;
/// Largest range read in one request (4 MiB, a multiple of [`ALIGN`]).
pub const MAX_RANGE: u64 = 4 * 1024 * 1024;

/// The ranges `(offset, length)` that cover `required_offset .. +required_length`,
/// widened to the cfapi alignment and split at [`MAX_RANGE`].
pub fn plan(required_offset: u64, required_length: u64, file_size: u64) -> Vec<(u64, u64)> {
    if file_size == 0 || required_offset >= file_size || required_length == 0 {
        return Vec::new();
    }
    let (start, end) = covering(required_offset, required_length, file_size);
    let mut out = Vec::new();
    let mut off = start;
    while off < end {
        let len = (end - off).min(MAX_RANGE);
        out.push((off, len));
        off += len;
    }
    out
}

/// The aligned span `[start, end)` that covers the request (end clamped to EOF).
pub fn covering(required_offset: u64, required_length: u64, file_size: u64) -> (u64, u64) {
    let start = required_offset / ALIGN * ALIGN;
    let want_end = required_offset.saturating_add(required_length).min(file_size);
    let end = want_end.div_ceil(ALIGN).saturating_mul(ALIGN).min(file_size);
    (start, end.max(start))
}

/// True when cfapi accepts a transfer of `len` bytes at `off`.
pub fn is_legal(off: u64, len: u64, file_size: u64) -> bool {
    len > 0 && off % ALIGN == 0 && (len % ALIGN == 0 || off + len == file_size) && off + len <= file_size
}

/// `Range` header value for an inclusive byte range.
pub fn range_header(off: u64, len: u64) -> String {
    format!("bytes={}-{}", off, off + len - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every planned range must be legal for cfapi, contiguous, within 4 MiB, and
    // together cover the requested bytes. The unaligned sizes are the cases the
    // spike hit (10_000 bytes) and a file bigger than one relay request.
    #[test]
    fn ranges_are_aligned_bounded_and_cover_the_request() {
        let cases = [
            (0u64, 10_000u64, 10_000u64),
            (0, 9 * 1024 * 1024 + 123, 9 * 1024 * 1024 + 123),
            (5000, 1, 10_000),
            (4096 * 3 + 7, 4 * 1024 * 1024, 20 * 1024 * 1024),
            (MAX_RANGE - 1, 2, 3 * MAX_RANGE + 1),
        ];
        for (ro, rl, size) in cases {
            let p = plan(ro, rl, size);
            assert!(!p.is_empty(), "{ro} {rl} {size}");
            assert!(p[0].0 <= ro);
            let last = p.last().unwrap();
            assert!(last.0 + last.1 >= (ro + rl).min(size));
            for w in p.windows(2) {
                assert_eq!(w[0].0 + w[0].1, w[1].0, "contiguous");
            }
            for &(o, l) in &p {
                assert!(l <= MAX_RANGE);
                assert!(is_legal(o, l, size), "illegal {o}+{l} of {size}");
            }
        }
    }

    #[test]
    fn empty_and_past_eof_requests_plan_nothing() {
        assert!(plan(0, 10, 0).is_empty());
        assert!(plan(10, 10, 10).is_empty());
        assert_eq!(range_header(4096, 4096), "bytes=4096-8191");
    }
}
