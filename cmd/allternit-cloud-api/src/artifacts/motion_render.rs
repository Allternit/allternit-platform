//! Rules for the Motion cloud render (pure, so the unit tests need no
//! database or child process): what a job may ask for, how many may run and
//! queue, who may start one, and which paths a job may touch.
//!
//! The route, the queue and the child process live in
//! `routes::artifact_render`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ids;
use super::sharing::OrgSettings;

/// Renders running at once on the host (8 cores, one ffmpeg + one Node each).
pub const MAX_CONCURRENT: usize = 2;
/// Queued + running jobs one user may have.
pub const MAX_ACTIVE_PER_USER: i64 = 3;
/// Longest clip, in seconds.
pub const MAX_SECONDS: f64 = 120.0;
/// Most frames in a clip: 120 s at 30 fps. A 60 fps clip may run 60 s.
pub const MAX_FRAMES: u64 = 3600;
/// Largest frame: 1920×1080, in either orientation.
pub const MAX_LONG_SIDE: u64 = 1920;
pub const MAX_PIXELS: u64 = 1920 * 1080;
/// A job that runs longer than this is killed.
pub const JOB_TIMEOUT_SECS: u64 = 10 * 60;
/// How long a finished file stays downloadable.
pub const FILE_TTL_HOURS: i64 = 24;
/// Largest output file kept.
pub const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

pub const JOB_PREFIX: &str = "rj_";

/// A refusal the route turns into `{error, code, message}` with this status.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn refuse(status: u16, code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { status, code, message: message.into() }
}

/// What a motion body asks the renderer for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// Encoded size: H.264 needs even sides.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub seconds: f64,
    pub frames: u32,
}

/// Read size, frame rate and length from a motion body. The body was
/// validated when it was saved; this only guards the numbers the render
/// depends on, so a hand-edited or stale body fails here with a clear code
/// instead of inside ffmpeg.
pub fn inspect(body: &str) -> Result<Spec, Refusal> {
    let bad = || refuse(422, "invalid_motion_body", "This motion artifact can’t be read. Open it, fix the highlighted problem and try again.");
    let v: Value = serde_json::from_str(body).map_err(|_| bad())?;
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
    let (w, h, fps) = (num("width").ok_or_else(bad)?, num("height").ok_or_else(bad)?, num("fps").ok_or_else(bad)?);
    if w < 2.0 || h < 2.0 || w > 16384.0 || h > 16384.0 || !(1.0..=240.0).contains(&fps) {
        return Err(bad());
    }
    let scenes = v.get("scenes").and_then(Value::as_array).filter(|s| !s.is_empty()).ok_or_else(bad)?;
    let seconds: f64 = scenes
        .iter()
        .map(|s| s.get("duration").and_then(Value::as_f64).filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0))
        .sum();
    if seconds <= 0.0 {
        return Err(bad());
    }
    let (width, height, fps) = (w.round() as u32, h.round() as u32, fps.round() as u32);
    Ok(Spec {
        width: width + width % 2,
        height: height + height % 2,
        fps,
        seconds,
        frames: ((seconds * fps as f64).round() as u32).max(1),
    })
}

/// Size, length and frame-count caps. Messages say what to change.
pub fn check_limits(spec: &Spec) -> Result<(), Refusal> {
    let long = u64::from(spec.width.max(spec.height));
    let pixels = u64::from(spec.width) * u64::from(spec.height);
    if long > MAX_LONG_SIDE || pixels > MAX_PIXELS {
        return Err(refuse(
            422,
            "render_too_large",
            format!(
                "Cloud export is limited to 1920×1080. This video is {}×{}. Ask Gizzi for a smaller size, or export from Allternit Desktop.",
                spec.width, spec.height
            ),
        ));
    }
    if spec.seconds > MAX_SECONDS || u64::from(spec.frames) > MAX_FRAMES {
        return Err(refuse(
            422,
            "render_too_long",
            format!(
                "Cloud export is limited to {} seconds at 30 fps ({} frames). This video has {} frames. Shorten it or lower the frame rate (ask Gizzi), or export from Allternit Desktop.",
                MAX_SECONDS as u32, MAX_FRAMES, spec.frames
            ),
        ));
    }
    Ok(())
}

/// At most [`MAX_ACTIVE_PER_USER`] queued or running jobs per user.
pub fn check_queue(active_for_user: i64) -> Result<(), Refusal> {
    if active_for_user >= MAX_ACTIVE_PER_USER {
        return Err(refuse(
            429,
            "render_limit_reached",
            format!("You already have {MAX_ACTIVE_PER_USER} videos rendering. Wait for one to finish, or cancel one, then try again."),
        ));
    }
    Ok(())
}

/// Motion (and its cloud render) is on every plan (Eoj, 2026-10-10). Kept as
/// the one place a plan rule would go; the per-user concurrency limit and the
/// org switch still apply.
pub fn plan_allows(_plan_id: Option<&str>, _in_org: bool) -> Result<(), Refusal> {
    Ok(())
}

/// The artifact's org can switch artifacts, or Motion alone, off.
pub fn org_allows(settings: Option<&OrgSettings>) -> Result<(), Refusal> {
    if let Some(s) = settings {
        if !s.enabled || s.templates.get("motion").and_then(Value::as_bool) == Some(false) {
            return Err(refuse(403, "org_disabled", "Your org admin turned Motion off."));
        }
    }
    Ok(())
}

pub fn new_job_id() -> String {
    format!("{JOB_PREFIX}{}", ids::ulid())
}

/// `rj_` + 26 Crockford chars, nothing else: the only shape a job id can
/// take, so it is safe to use as a directory name and R2 key part.
pub fn valid_job_id(id: &str) -> bool {
    id.strip_prefix(JOB_PREFIX).is_some_and(|rest| {
        rest.len() == 26 && rest.bytes().all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !matches!(b, b'I' | b'L' | b'O' | b'U')))
    })
}

/// The job's working directory under `root`; `None` for any id that is not a
/// generated job id (no separators, no `..`).
pub fn job_dir(root: &Path, id: &str) -> Option<PathBuf> {
    valid_job_id(id).then(|| root.join(id))
}

/// The one output file name inside a job directory.
pub const OUTPUT_NAME: &str = "out.mp4";
pub const INPUT_NAME: &str = "composition.json";

/// R2 key for a finished video. Rejects user ids with anything outside
/// `[A-Za-z0-9_-]` so a key can never climb or split.
pub fn output_key(user_id: &str, job_id: &str) -> Option<String> {
    let safe_user = !user_id.is_empty()
        && user_id.len() <= 128
        && user_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (safe_user && valid_job_id(job_id)).then(|| format!("motion-render/{user_id}/{job_id}.mp4"))
}

/// `true` when `child` is inside `root` once both are lexically normalised.
/// Used before deleting a path read back from the database.
pub fn is_inside(root: &Path, child: &Path) -> bool {
    use std::path::Component;
    if !child.starts_with(root) {
        return false;
    }
    !child.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(w: u32, h: u32, fps: u32, durations: &[f64]) -> String {
        let scenes: Vec<Value> = durations.iter().map(|d| json!({ "type": "title", "duration": d })).collect();
        json!({ "version": 1, "width": w, "height": h, "fps": fps, "scenes": scenes }).to_string()
    }

    #[test]
    fn inspect_reads_size_fps_and_length() {
        let s = inspect(&body(1920, 1080, 30, &[3.0, 4.5])).unwrap();
        assert_eq!((s.width, s.height, s.fps, s.frames), (1920, 1080, 30, 225));
        assert!((s.seconds - 7.5).abs() < 1e-9);
    }

    #[test]
    fn inspect_rounds_odd_sizes_up_to_even() {
        let s = inspect(&body(1281, 721, 24, &[1.0])).unwrap();
        assert_eq!((s.width, s.height), (1282, 722));
    }

    #[test]
    fn inspect_refuses_unreadable_bodies() {
        let bad: Vec<String> = vec![
            String::new(),
            "not json".into(),
            "[]".into(),
            "{}".into(),
            body(1920, 1080, 30, &[]),
            body(1920, 1080, 0, &[2.0]),
            body(1920, 1080, 30, &[0.0]),
        ];
        for b in &bad {
            assert_eq!(inspect(b).unwrap_err().code, "invalid_motion_body", "{b}");
        }
    }

    #[test]
    fn limits_allow_the_caps_and_refuse_beyond() {
        assert!(check_limits(&inspect(&body(1920, 1080, 30, &[120.0])).unwrap()).is_ok());
        assert!(check_limits(&inspect(&body(1080, 1920, 30, &[10.0])).unwrap()).is_ok());
        assert!(check_limits(&inspect(&body(1280, 720, 60, &[60.0])).unwrap()).is_ok());
        assert_eq!(check_limits(&inspect(&body(1920, 1080, 30, &[121.0])).unwrap()).unwrap_err().code, "render_too_long");
        // 60 fps for 61 s is over the 3600-frame cap even though it is under 120 s.
        assert_eq!(check_limits(&inspect(&body(1280, 720, 60, &[61.0])).unwrap()).unwrap_err().code, "render_too_long");
        assert_eq!(check_limits(&inspect(&body(3840, 2160, 30, &[10.0])).unwrap()).unwrap_err().code, "render_too_large");
        assert_eq!(check_limits(&inspect(&body(2560, 720, 30, &[10.0])).unwrap()).unwrap_err().code, "render_too_large");
        // Under the long-side cap but over the pixel cap.
        assert_eq!(check_limits(&inspect(&body(1920, 1200, 30, &[10.0])).unwrap()).unwrap_err().code, "render_too_large");
    }

    #[test]
    fn queue_holds_three_per_user() {
        assert!(check_queue(0).is_ok());
        assert!(check_queue(2).is_ok());
        let r = check_queue(3).unwrap_err();
        assert_eq!((r.status, r.code), (429, "render_limit_reached"));
    }

    #[test]
    fn two_renders_at_once() {
        assert_eq!(MAX_CONCURRENT, 2);
    }

    #[test]
    fn plans_follow_motion() {
        assert!(plan_allows(Some("super"), false).is_ok());
        assert!(plan_allows(Some("ultra"), false).is_ok());
        assert!(plan_allows(Some("team"), false).is_ok());
        assert!(plan_allows(None, true).is_ok());
        assert!(plan_allows(Some("plus"), false).is_ok());
        assert!(plan_allows(None, false).is_ok(), "free plans can render too");
    }

    #[test]
    fn org_switches_apply() {
        assert!(org_allows(None).is_ok());
        let mut s = OrgSettings::default();
        assert!(org_allows(Some(&s)).is_ok());
        s.templates = json!({ "motion": false });
        assert_eq!(org_allows(Some(&s)).unwrap_err().code, "org_disabled");
        s.templates = json!({ "motion": true });
        s.enabled = false;
        assert_eq!(org_allows(Some(&s)).unwrap_err().code, "org_disabled");
    }

    #[test]
    fn job_ids_are_generated_shapes_only() {
        let id = new_job_id();
        assert!(valid_job_id(&id), "{id}");
        for bad in ["", "rj_", "rj_short", "art_01HZ", "../etc/passwd", "rj_../../../../etc/passwd0000", "rj_0123456789ABCDEFGHJKMNPQRS/", "rj_0123456789abcdefghjkmnpqrs", "rj_0123456789ABCDEFGHJKMNPQRSI"] {
            assert!(!valid_job_id(bad), "{bad}");
        }
    }

    #[test]
    fn job_dirs_stay_under_the_root() {
        let root = Path::new("/tmp/allternit-motion-render");
        let id = new_job_id();
        assert_eq!(job_dir(root, &id), Some(root.join(&id)));
        assert_eq!(job_dir(root, "../x"), None);
        assert_eq!(job_dir(root, "/etc"), None);
        assert!(is_inside(root, &root.join(&id).join(OUTPUT_NAME)));
        assert!(!is_inside(root, &root.join("..").join("x")));
        assert!(!is_inside(root, Path::new("/etc/passwd")));
    }

    #[test]
    fn r2_keys_refuse_odd_users() {
        let id = new_job_id();
        assert_eq!(output_key("user_2abc", &id), Some(format!("motion-render/user_2abc/{id}.mp4")));
        assert_eq!(output_key("user/../x", &id), None);
        assert_eq!(output_key("", &id), None);
        assert_eq!(output_key("user_1", "../bad"), None);
    }
}
