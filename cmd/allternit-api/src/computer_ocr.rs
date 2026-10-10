//! Server-side OCR for screenshot redaction (driver spec D5, task K4 gap 1).
//!
//! Screenshot redaction locates personal data by OCR of the exact image. On a
//! Mac with the Allternit Driver that OCR runs through the driver's `ocr`
//! method (Apple Vision). A cloud-hosted allternit-api has no driver, so this
//! module is the fallback: a pure-Rust OCR engine ([ocrs], MIT/Apache-2.0,
//! inference with [rten]) that answers the same `{lines: [{text, box, words:
//! [{start, end, box}]}]}` shape the driver replies, so `pii_boxes` and
//! `black_out` treat both sources identically. The models download once into
//! the API's data dir (`<data_dir>/ocr-models`), pinned by URL and verified
//! against a SHA-256 digest when one is pinned; a missing model, a failed
//! download or a digest mismatch fails closed and the caller's redaction
//! setting decides what happens to the screenshot (computer_safety::redact_png).
//!
//! OCR runs in `spawn_blocking` off the async runtime, with a size cap (the
//! driver's 40 MB PNG cap and a 4096x4096 pixel cap) and a timeout. When guest
//! computers gain the Allternit Driver, the guest's own OCR is preferred for
//! its screenshots; this engine stays the fallback behind
//! `computer_safety::ocr_lines`.
//!
//! [ocrs]: https://github.com/robertknight/ocrs
//! [rten]: https://github.com/robertknight/rten

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextChar, TextItem, TextLine};
use once_cell::sync::Lazy;
use rten::Model;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};

/// One OCR model file: where it comes from and how its integrity is checked.
struct ModelFile {
    file_name: &'static str,
    url: &'static str,
    /// Expected SHA-256 of the file, hex-encoded. Verified after every
    /// download and on every process start against the cached file; a
    /// mismatch deletes the cache and re-fetches. `None` skips the digest
    /// check (the file still has to parse as an rten model); pin the digest
    /// with `shasum -a 256 <file>` of the pinned upstream model release.
    sha256: Option<&'static str>,
}

/// The upstream ocrs model releases. These are the files the ocrs CLI itself
/// downloads (see ocrs-cli's models.rs); both are required.
const MODELS: [ModelFile; 2] = [
    ModelFile {
        file_name: "text-detection.rten",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten",
        sha256: Some("f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca"),
    },
    ModelFile {
        file_name: "text-recognition.rten",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten",
        sha256: Some("e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e"),
    },
];

/// The driver's OCR refuses PNGs above 40 MB; the server engine keeps the
/// same cap so behaviour matches on every host.
const MAX_PNG_BYTES: usize = 40 * 1024 * 1024;
/// Decoded-image cap: 4096x4096 px. Recognition quality degrades and latency
/// explodes far above a 4K-class screenshot, which redaction never needs.
const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// Wall-clock budget for one image through the engine.
const OCR_TIMEOUT: Duration = Duration::from_secs(20);
/// Wall-clock budget for one model download.
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// The engine is built once per process (model files are a few MB to parse);
/// OCR calls share it through an `Arc`.
static ENGINE: OnceCell<Arc<OcrEngine>> = OnceCell::const_new();
/// Serializes first-use model fetch + engine build so concurrent screenshots
/// don't download the models twice.
static ENGINE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ocr-models")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Fetch one model file to `dest` (via a temp file + rename so a crash mid
/// download can't leave a truncated file that looks cached), checking the
/// pinned digest when there is one.
async fn fetch_model(m: &ModelFile, dest: &Path) -> Result<(), String> {
    let bytes = reqwest::Client::new()
        .get(m.url)
        .timeout(MODEL_DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("couldn't download the OCR model {}: {e}", m.file_name))?
        .error_for_status()
        .map_err(|e| format!("couldn't download the OCR model {}: {e}", m.file_name))?
        .bytes()
        .await
        .map_err(|e| format!("couldn't read the OCR model {}: {e}", m.file_name))?;
    match m.sha256 {
        Some(expected) => {
            let got = sha256_hex(&bytes);
            if got != expected {
                return Err(format!("the OCR model {} failed its integrity check (sha256 {got}, expected {expected})", m.file_name));
            }
        }
        None => tracing::warn!(
            file = m.file_name,
            "OCR model sha256 not pinned in computer_ocr.rs; fill it in from `shasum -a 256` of the pinned model release"
        ),
    }
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("couldn't save the OCR model {}: {e}", m.file_name))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("couldn't install the OCR model {}: {e}", m.file_name))?;
    Ok(())
}

/// Make sure both model files are in the data dir and match their pinned
/// digests; re-fetch on mismatch.
async fn ensure_model_files(data_dir: &Path) -> Result<(), String> {
    let dir = model_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create the OCR model directory: {e}"))?;
    for m in &MODELS {
        let dest = dir.join(m.file_name);
        let stale = match (dest.exists(), m.sha256) {
            (true, Some(expected)) => match std::fs::read(&dest) {
                Ok(bytes) => sha256_hex(&bytes) != expected,
                Err(_) => true,
            },
            (exists, _) => !exists,
        };
        if stale {
            tracing::info!(file = m.file_name, "fetching the OCR model into the data dir");
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            fetch_model(m, &dest).await?;
        }
    }
    Ok(())
}

/// The shared engine, built on first use.
async fn engine(data_dir: &Path) -> Result<Arc<OcrEngine>, String> {
    let _guard = ENGINE_LOCK.lock().await;
    ENGINE
        .get_or_try_init(|| async {
            ensure_model_files(data_dir).await?;
            let dir = model_dir(data_dir);
            let detection = Model::load_file(dir.join(MODELS[0].file_name))
                .map_err(|e| format!("couldn't load the OCR detection model: {e}"))?;
            let recognition = Model::load_file(dir.join(MODELS[1].file_name))
                .map_err(|e| format!("couldn't load the OCR recognition model: {e}"))?;
            OcrEngine::new(OcrEngineParams {
                detection_model: Some(detection),
                recognition_model: Some(recognition),
                ..Default::default()
            })
            .map(Arc::new)
            .map_err(|e| format!("couldn't start the OCR engine: {e}"))
        })
        .await
        .cloned()
}

fn within_pixel_cap(width: u32, height: u32) -> bool {
    width as u64 * height as u64 <= MAX_IMAGE_PIXELS
}

/// Decode a screenshot to RGB8 for the engine. Accepts any format the `image`
/// crate reads (screenshots are PNG; browser captures can be JPEG).
fn decode_rgb(image_bytes: &[u8]) -> Result<(Vec<u8>, (u32, u32)), String> {
    let img = image::load_from_memory(image_bytes).map_err(|e| format!("couldn't decode the screenshot for OCR: {e}"))?;
    let rgb = img.into_rgb8();
    let dims = rgb.dimensions();
    Ok((rgb.into_raw(), dims))
}

/// One recognized line as the driver's `ocr` replies it: the line text, its
/// box and one box per word with character offsets, so `pii_boxes` finds the
/// same boxes it would from Apple Vision.
fn line_value(line: &TextLine) -> Value {
    let chars: &[TextChar] = line.chars();
    let text: String = chars.iter().map(|c| c.char).collect();
    let rect = line.bounding_rect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].char.is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !chars[i].char.is_whitespace() {
            i += 1;
        }
        if i <= start {
            continue;
        }
        let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for c in &chars[start..i] {
            left = left.min(c.rect.left());
            top = top.min(c.rect.top());
            right = right.max(c.rect.right());
            bottom = bottom.max(c.rect.bottom());
        }
        words.push(json!({ "start": start, "end": i, "box": [left, top, right, bottom] }));
    }
    json!({
        "text": text,
        "box": [rect.left(), rect.top(), rect.right(), rect.bottom()],
        "words": words,
    })
}

/// The blocking half of `ocr_png`: prepare, detect, group, recognize. Runs on
/// the blocking pool; the caller wraps it in the timeout.
fn run_ocr(engine: &OcrEngine, rgb: Vec<u8>, dims: (u32, u32)) -> Result<Value, String> {
    let source = ImageSource::from_bytes(&rgb, dims).map_err(|e| format!("couldn't prepare the screenshot for OCR: {e}"))?;
    let input = engine.prepare_input(source).map_err(|e| format!("couldn't prepare the screenshot for OCR: {e}"))?;
    let words = engine.detect_words(&input).map_err(|e| format!("OCR text detection failed: {e}"))?;
    let line_rects = engine.find_text_lines(&input, &words);
    let lines = engine.recognize_text(&input, &line_rects).map_err(|e| format!("OCR text recognition failed: {e}"))?;
    Ok(json!({
        "engine": "server",
        "lines": lines.iter().flatten().map(line_value).collect::<Vec<_>>(),
    }))
}

/// OCR one screenshot with the built-in engine, returning the driver-shaped
/// reply. `Err` means no OCR is available for this image; the caller's
/// redaction setting decides whether the screenshot is withheld.
pub async fn ocr_png(image_bytes: &[u8], data_dir: &Path) -> Result<Value, String> {
    if image_bytes.len() > MAX_PNG_BYTES {
        return Err(format!("the screenshot is too large for OCR ({} bytes)", image_bytes.len()));
    }
    let (rgb, dims) = decode_rgb(image_bytes)?;
    if !within_pixel_cap(dims.0, dims.1) {
        return Err(format!("the screenshot ({}x{}) is too large for OCR", dims.0, dims.1));
    }
    let engine = engine(data_dir).await?;
    let task = move || run_ocr(&engine, rgb, dims);
    tokio::time::timeout(OCR_TIMEOUT, tokio::task::spawn_blocking(task))
        .await
        .map_err(|_| "OCR of the screenshot timed out".to_string())?
        .map_err(|e| format!("OCR task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rten_imageproc::Rect;

    fn chars_at(text: &str, left: i32, width: i32, top: i32, height: i32) -> Vec<TextChar> {
        text.chars()
            .enumerate()
            .map(|(i, c)| TextChar {
                char: c,
                rect: Rect::from_tlhw(top, left + i as i32 * width, height, width),
            })
            .collect()
    }

    #[test]
    fn server_ocr_line_value_matches_the_driver_shape() {
        // "Email ada@example.com": word 0 = chars 0..5, word 1 = chars 6..21.
        let line = TextLine::new(chars_at("Email ada@example.com", 0, 10, 10, 20));
        let v = line_value(&line);
        assert_eq!(v["text"], "Email ada@example.com");
        assert_eq!(v["box"], json!([0, 10, 210, 30]));
        let words = v["words"].as_array().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0]["start"], 0);
        assert_eq!(words[0]["end"], 5);
        assert_eq!(words[0]["box"], json!([0, 10, 50, 30]));
        assert_eq!(words[1]["start"], 6);
        assert_eq!(words[1]["end"], 21);
        assert_eq!(words[1]["box"], json!([60, 10, 210, 30]));
        // The line box spans both words.
        assert_eq!(v["box"], json!([0, 10, 210, 30]));
    }

    #[test]
    fn server_ocr_output_feeds_the_shared_redaction() {
        // A fixture PNG generated in code (nothing downloaded): white 200x40.
        let mut img = image::RgbaImage::from_pixel(200, 40, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(150, 20, image::Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageOutputFormat::Png).unwrap();
        // The server engine's reply for that image, as line_value shapes it.
        let line = TextLine::new(chars_at("Email ada@example.com", 0, 10, 10, 20));
        let ocr = json!({ "engine": "server", "lines": [line_value(&line)] });
        let (boxes, kinds) = crate::computer_safety::pii_boxes(&ocr, &["email", "card", "phone", "ssn", "secret"], &[]);
        assert_eq!(boxes, vec![[60, 10, 210, 30]]);
        assert_eq!(kinds, vec!["email"]);
        let out = crate::computer_safety::black_out(&png, &boxes).unwrap();
        let redacted = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(redacted.get_pixel(150, 20).0, [0, 0, 0, 255], "the email word is blacked out");
        assert_eq!(redacted.get_pixel(20, 20).0, [255, 255, 255, 255], "the label stays");
    }

    #[test]
    fn pixel_cap_and_sha256() {
        assert!(within_pixel_cap(4096, 4096));
        assert!(!within_pixel_cap(4097, 4096));
        assert!(!within_pixel_cap(5000, 5000));
        // Well-known digest vector (SHA-256 of the empty string).
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn decode_accepts_a_generated_png() {
        let img = image::RgbaImage::from_pixel(12, 8, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageOutputFormat::Png).unwrap();
        let (rgb, dims) = decode_rgb(&png).unwrap();
        assert_eq!(dims, (12, 8));
        assert_eq!(rgb.len(), 12 * 8 * 3);
    }
}
