//! `GET /files/v2/items/:item_id/range`: a slice of a file's text, for agents
//! that read a large file a window at a time.
//!
//! - **Query.** `unit=lines` (default): `start` and `end` are 1-based line
//!   numbers, both inclusive. `unit=bytes`: 0-based byte offsets, both
//!   inclusive (like an HTTP `Range`), moved inward to whole UTF-8 characters.
//!   `start` defaults to the first line or byte, `end` to the cap.
//! - **Caps.** At most [`MAX_LINES`] lines and [`MAX_BYTES`] bytes of text per
//!   call. `next_start` is where the next call starts; it is absent at the end.
//! - **Text form.** PDF and Word files use their extracted text (the same
//!   reader as summaries); anything else is read as UTF-8. A file with NUL
//!   bytes or bad UTF-8 in its first 8 KiB, or a document with no text, gets
//!   415 `unsupported`.
//! - **Access.** The same check as `GET /items/:item_id/content`
//!   ([`Inner::guard_item`] for read, then the engine's `open_content`), so a
//!   guest link that may download may also read a range.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::routes::run;
use super::{intel, ApiError, Fail, FilesV2, Inner, Need, Req};

/// Lines per call.
pub(crate) const MAX_LINES: u64 = 2000;
/// Bytes of text per call.
pub(crate) const MAX_BYTES: usize = 256 * 1024;
/// How much of a plain file is checked for binary content.
const SNIFF_BYTES: usize = 8192;

#[derive(Deserialize, Default)]
pub(crate) struct RangeQuery {
    start: Option<u64>,
    end: Option<u64>,
    unit: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct RangeResponse {
    pub text: String,
    /// `lines` or `bytes`, echoing the query.
    pub unit: &'static str,
    /// First line (1-based) or byte (0-based) returned.
    pub start: u64,
    /// Last line or byte returned, inclusive. `start - 1` when `text` is empty
    /// (0 for an empty file).
    pub end: u64,
    pub total_lines: u64,
    pub total_bytes: u64,
    /// Where the next call starts, in the same unit. Absent at the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start: Option<u64>,
    /// True when one line was longer than the byte cap and was cut.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A window of lines from a reader (the pure part of the route).
#[derive(Debug, PartialEq)]
pub(crate) struct LineSlice {
    pub text: String,
    /// Last line kept; `start - 1` when none.
    pub end: u64,
    pub total_lines: u64,
    pub total_bytes: u64,
    pub truncated: bool,
}

/// Lines `start..=end` (1-based, `start >= 1`) of `r`, at most `max_bytes` of
/// text. Each line keeps its own line ending. When the byte cap is hit, the
/// window stops after the last whole line; if the first line alone is over
/// the cap, it is cut and `truncated` is set. The whole reader is scanned so
/// the totals are exact; lines outside the window are never held in memory.
pub(crate) fn slice_lines<R: BufRead>(mut r: R, start: u64, end: u64, max_bytes: usize) -> std::io::Result<LineSlice> {
    let start = start.max(1);
    let mut line: u64 = 1; // the line the next byte belongs to
    let mut total_bytes: u64 = 0;
    let mut last_was_nl = true; // the last byte read was '\n' (or nothing was read)
    let mut out: Vec<u8> = Vec::new();
    let mut line_begin = 0usize; // out.len() where the current line began
    let mut last = start - 1; // last whole line kept
    let mut full = false;
    let mut truncated = false;
    loop {
        let buf = r.fill_buf()?;
        let n = buf.len();
        if n == 0 {
            break;
        }
        let mut i = 0;
        while i < n {
            let nl = buf[i..].iter().position(|b| *b == b'\n');
            let seg_end = nl.map_or(n, |p| i + p + 1);
            if !full && line >= start && line <= end {
                let seg = &buf[i..seg_end];
                if out.len() + seg.len() > max_bytes {
                    full = true;
                    if line == start {
                        out.extend_from_slice(&seg[..max_bytes - out.len()]);
                        truncated = true;
                        last = start;
                    } else {
                        out.truncate(line_begin);
                    }
                } else {
                    out.extend_from_slice(seg);
                    if nl.is_some() {
                        last = line;
                        line_begin = out.len();
                    }
                }
            }
            if nl.is_some() {
                line += 1;
            }
            i = seg_end;
        }
        last_was_nl = buf[n - 1] == b'\n';
        total_bytes += n as u64;
        r.consume(n);
    }
    // A last line with no line ending, inside the window.
    if !full && !last_was_nl && line >= start && line <= end && out.len() > line_begin {
        last = line;
    }
    let total_lines = if last_was_nl { line - 1 } else { line };
    Ok(LineSlice { text: utf8_text(&out), end: last, total_lines, total_bytes, truncated })
}

/// Lines in `r` and its length in bytes.
fn count_lines<R: BufRead>(r: R) -> std::io::Result<(u64, u64)> {
    let s = slice_lines(r, u64::MAX, u64::MAX, 0)?;
    Ok((s.total_lines, s.total_bytes))
}

/// Text from bytes: an incomplete character at the end (a cut) is dropped,
/// any other bad byte becomes U+FFFD.
fn utf8_text(b: &[u8]) -> String {
    utf8_prefix(b).0
}

/// [`utf8_text`] and how many input bytes it used.
fn utf8_prefix(b: &[u8]) -> (String, usize) {
    match std::str::from_utf8(b) {
        Ok(s) => (s.to_string(), b.len()),
        Err(e) if e.error_len().is_none() => (String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned(), e.valid_up_to()),
        Err(_) => (String::from_utf8_lossy(b).into_owned(), b.len()),
    }
}

/// Bytes `start..=end` (0-based) of `r`, at most `max_bytes`, moved inward to
/// whole characters. Returns the text and the first and last offsets used
/// (`last = first - 1` when no whole character fit). `start <= end`.
fn slice_bytes<R: Read + Seek>(mut r: R, start: u64, end: u64, max_bytes: usize) -> std::io::Result<(String, u64, u64)> {
    let want = (end - start + 1).min(max_bytes as u64);
    r.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(want as usize);
    r.take(want).read_to_end(&mut buf)?;
    // Skip continuation bytes of a character that began before `start`.
    let lead = buf.iter().take(3).take_while(|b| (**b & 0xC0) == 0x80).count();
    let (text, used) = utf8_prefix(&buf[lead..]);
    let first = start + lead as u64;
    Ok((text, first, (first + used as u64).saturating_sub(1)))
}

fn unsupported_media(msg: &str) -> ApiError {
    let mut e = ApiError::unsupported(msg);
    e.status = StatusCode::UNSUPPORTED_MEDIA_TYPE;
    e
}

fn not_satisfiable(msg: String) -> ApiError {
    let mut e = ApiError::invalid(msg);
    e.status = StatusCode::RANGE_NOT_SATISFIABLE;
    e
}

/// The file's text form: extracted text for PDF and Word, else `None` for a
/// plain file that is read straight from disk.
enum Source {
    Extracted(String),
    Plain(std::fs::File),
}

fn open_text(v: &Inner, item_id: &str, name: &str, file: std::fs::File) -> Result<Source, Fail> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "pdf" | "docx") {
        let path = v.drive.local_path(item_id)?;
        return match intel::read_document(&path, name) {
            Some(text) => Ok(Source::Extracted(text)),
            None => Err(unsupported_media("This document has no text to read.").into()),
        };
    }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    (&file).take(SNIFF_BYTES as u64).read_to_end(&mut head).map_err(|e| Fail::Files(e.into()))?;
    let bad_utf8 = matches!(std::str::from_utf8(&head), Err(e) if e.error_len().is_some());
    if head.contains(&0) || bad_utf8 {
        return Err(unsupported_media("This is a binary file with no text form.").into());
    }
    (&file).seek(SeekFrom::Start(0)).map_err(|e| Fail::Files(e.into()))?;
    Ok(Source::Plain(file))
}

fn read_range(v: &Inner, req: &Req, item_id: &str, q: &RangeQuery) -> Result<RangeResponse, Fail> {
    let bytes = match q.unit.as_deref().map(str::trim) {
        None | Some("") | Some("lines") => false,
        Some("bytes") => true,
        Some(_) => return Err(ApiError::invalid("unit is lines or bytes.").into()),
    };
    // The same access rule as the normal file read.
    v.guard_item(req, item_id, Need::Read)?;
    let (rec, file) = v.drive.open_content(item_id)?;
    let io = |e: std::io::Error| Fail::Files(e.into());
    let source = open_text(v, item_id, &rec.name, file)?;
    if bytes {
        let start = q.start.unwrap_or(0);
        let end = q.end.unwrap_or(start.saturating_add(MAX_BYTES as u64 - 1));
        if end < start {
            return Err(ApiError::invalid("end comes before start.").into());
        }
        let (total_lines, total_bytes) = match &source {
            Source::Extracted(t) => count_lines(t.as_bytes()),
            Source::Plain(f) => count_lines(BufReader::new(f)),
        }
        .map_err(io)?;
        if total_bytes == 0 {
            if start > 0 {
                return Err(not_satisfiable("start is past the end of the file (0 bytes).".into()).into());
            }
            return Ok(RangeResponse { text: String::new(), unit: "bytes", start: 0, end: 0, total_lines, total_bytes, next_start: None, truncated: false });
        }
        if start >= total_bytes {
            return Err(not_satisfiable(format!("start is past the end of the file ({total_bytes} bytes).")).into());
        }
        let stop = end.min(total_bytes - 1);
        let (text, first, last) = match &source {
            Source::Extracted(t) => slice_bytes(std::io::Cursor::new(t.as_bytes()), start, stop, MAX_BYTES),
            Source::Plain(f) => slice_bytes(f, start, stop, MAX_BYTES),
        }
        .map_err(io)?;
        // `last >= first` unless no whole character fit; then go on past any
        // skipped continuation bytes.
        let next = if last + 1 >= first { last + 1 } else { first };
        let next_start = (next > start && next < total_bytes && next <= q.end.unwrap_or(u64::MAX)).then_some(next);
        return Ok(RangeResponse { text, unit: "bytes", start: first, end: last, total_lines, total_bytes, next_start, truncated: false });
    }
    let start = q.start.unwrap_or(1);
    if start == 0 {
        return Err(ApiError::invalid("Lines start at 1.").into());
    }
    let cap_end = start.saturating_add(MAX_LINES - 1);
    let end = q.end.unwrap_or(cap_end);
    if end < start {
        return Err(ApiError::invalid("end comes before start.").into());
    }
    let end = end.min(cap_end);
    let s = match source {
        Source::Extracted(t) => slice_lines(t.as_bytes(), start, end, MAX_BYTES),
        Source::Plain(f) => slice_lines(BufReader::new(f), start, end, MAX_BYTES),
    }
    .map_err(io)?;
    if start > s.total_lines && !(start == 1 && s.total_lines == 0) {
        return Err(not_satisfiable(format!("start is past the end of the file ({} lines).", s.total_lines)).into());
    }
    let asked_end = q.end.unwrap_or(u64::MAX);
    let next_start = if s.truncated {
        (start < s.total_lines && start < asked_end).then_some(start + 1)
    } else {
        (s.end >= start && s.end < s.total_lines && s.end < asked_end).then_some(s.end + 1)
    };
    Ok(RangeResponse {
        text: s.text,
        unit: "lines",
        start,
        end: s.end,
        total_lines: s.total_lines,
        total_bytes: s.total_bytes,
        next_start,
        truncated: s.truncated,
    })
}

pub(crate) async fn range(
    Extension(files): Extension<std::sync::Arc<FilesV2>>,
    req: Req,
    Path(item_id): Path<String>,
    q: Result<Query<RangeQuery>, QueryRejection>,
) -> Result<Json<RangeResponse>, ApiError> {
    let q = q.map(|Query(v)| v)?;
    run(&files, move |v| read_range(v, &req, &item_id, &q).map(Json)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, start: u64, end: u64, max: usize) -> LineSlice {
        slice_lines(text.as_bytes(), start, end, max).unwrap()
    }

    #[test]
    fn slice_lines_windows_caps_and_totals() {
        let t = "a\nbb\nccc\ndddd";
        let s = lines(t, 2, 3, 1024);
        assert_eq!((s.text.as_str(), s.end, s.total_lines, s.total_bytes, s.truncated), ("bb\nccc\n", 3, 4, 13, false));
        // A last line with no newline is a line.
        let s = lines(t, 4, 9, 1024);
        assert_eq!((s.text.as_str(), s.end), ("dddd", 4));
        // The byte cap stops after the last whole line.
        let s = lines(t, 1, 4, 6);
        assert_eq!((s.text.as_str(), s.end, s.truncated), ("a\nbb\n", 2, false));
        // A first line over the cap is cut.
        let s = lines("abcdef\nx\n", 1, 2, 3);
        assert_eq!((s.text.as_str(), s.end, s.total_lines, s.truncated), ("abc", 1, 2, true));
        // Past the end: nothing, exact totals.
        let s = lines("a\nb\n", 5, 9, 1024);
        assert_eq!((s.text.as_str(), s.end, s.total_lines), ("", 4, 2));
        // Empty input.
        let s = lines("", 1, 10, 1024);
        assert_eq!((s.text.as_str(), s.end, s.total_lines, s.total_bytes), ("", 0, 0, 0));
        // Lines that span reader chunks.
        let s = slice_lines(BufReader::with_capacity(2, "one\ntwo\nthree\n".as_bytes()), 2, 3, 1024).unwrap();
        assert_eq!((s.text.as_str(), s.end, s.total_lines), ("two\nthree\n", 3, 3));
    }

    #[test]
    fn slice_bytes_keeps_whole_characters() {
        let t = "aé€b"; // a(1) é(2) €(3) b(1)
        let (text, first, last) = slice_bytes(std::io::Cursor::new(t.as_bytes()), 2, 4, 1024).unwrap();
        // Byte 2 is inside é: skipped. Bytes 3..=4 are a cut €: dropped.
        assert_eq!((text.as_str(), first, last), ("", 3, 2));
        let (text, first, last) = slice_bytes(std::io::Cursor::new(t.as_bytes()), 1, 5, 1024).unwrap();
        assert_eq!((text.as_str(), first, last), ("é€", 1, 5));
    }
}
