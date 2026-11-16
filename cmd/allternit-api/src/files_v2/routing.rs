//! Outputs routed into the Downloads and Meetings roots (SPEC section 3).
//!
//! - **Meetings.** The meeting bot posts the end-of-meeting transcript to
//!   this runtime (`POST /api/v1/meeting-turns/summary`,
//!   `meeting_turn_stream.rs`). After the summary is made, [`save_meeting`]
//!   writes `~/Allternit/Meetings/<YYYY-MM-DD HHmm> <title>.md` (summary,
//!   decisions, action items, then the transcript) and a
//!   `.provenance.json` sidecar (`source: meeting`, `ai_generated: true`).
//! - **Downloads.** The agent browser is the ACU gateway
//!   (`domains/computer-use/core/gateway/session_manager.py`). It saves every
//!   download into `~/Allternit/Downloads` when the drive is on (it reads the
//!   same `ALLTERNIT_FILES_DRIVE` switch, or `ALLTERNIT_FILES_DOWNLOADS_DIR`).
//!
//! Both write plain files; the drive's watcher indexes them. Nothing is
//! written while the drive is off.

use std::path::{Path, PathBuf};

use allternit_files_core::{Provenance, ProvenanceSource};
use serde_json::json;

use crate::meeting_tools::MeetingSummary;
use crate::meeting_turn_stream::{MeetingInfo, Segment};

/// `~/Allternit/<folder>` when the drive is on and there is a home folder.
pub fn home_folder(folder: &str) -> Option<PathBuf> {
    let on = std::env::var(super::FLAG_ENV).map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true")).unwrap_or(false);
    if !on {
        return None;
    }
    Some(dirs::home_dir()?.join("Allternit").join(folder))
}

/// A file name made from untrusted text (a meeting title): no path
/// separators, no control characters, no leading dot, at most 80 characters.
pub fn safe_file_name(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { ' ' } else { c })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim_start_matches(|c: char| c == '.' || c.is_whitespace()).trim();
    let name: String = trimmed.chars().take(80).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        fallback.to_string()
    } else {
        name
    }
}

/// `dir/<stem>.<ext>`, or `<stem> 2.<ext>`, `<stem> 3.<ext>`… when taken.
pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem} {n}.{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// The meeting note: summary first, then the transcript.
pub fn meeting_markdown(meeting: &MeetingInfo, transcript: &[Segment], summary: &MeetingSummary) -> String {
    let title = if meeting.title.trim().is_empty() { "Meeting" } else { meeting.title.trim() };
    let mut out = format!("# {title}\n\nSummary written by AI from the transcript below.\n\n## Summary\n\n{}\n", summary.summary.trim());
    if !summary.decisions.is_empty() {
        out.push_str("\n## Decisions\n\n");
        for d in &summary.decisions {
            out.push_str(&format!("- {d}\n"));
        }
    }
    if !summary.action_items.is_empty() {
        out.push_str("\n## Action items\n\n");
        for a in &summary.action_items {
            let owner = a.owner.as_deref().map(|o| format!(" ({o})")).unwrap_or_default();
            let due = a.due.as_deref().map(|d| format!(", due {d}")).unwrap_or_default();
            out.push_str(&format!("- {}{owner}{due}\n", a.text));
        }
    }
    out.push_str("\n## Transcript\n\n");
    for s in transcript.iter().filter(|s| !s.text.trim().is_empty()) {
        let speaker = s.speaker.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or("Unknown");
        out.push_str(&format!("[{}] {speaker}: {}\n", clock(s.start_ms), s.text.trim()));
    }
    out
}

/// Writes the meeting note and its provenance sidecar into `dir`. Returns
/// the note's path.
pub fn write_meeting(
    dir: &Path,
    session_id: &str,
    meeting: &MeetingInfo,
    transcript: &[Segment],
    summary: &MeetingSummary,
    local_now: chrono::DateTime<chrono::FixedOffset>,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = format!("{} {}", local_now.format("%Y-%m-%d %H%M"), safe_file_name(&meeting.title, "Meeting"));
    let path = free_path(dir, &stem, "md");
    std::fs::write(&path, meeting_markdown(meeting, transcript, summary))?;
    let provenance = Provenance {
        source: ProvenanceSource::Meeting,
        ai_generated: true,
        agent_id: None,
        task_id: None,
        input_item_ids: None,
    };
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let sidecar = json!({
        "allternit_provenance": 1,
        "provenance": provenance,
        "result": { "name": name },
        "meeting": { "session_id": session_id, "title": meeting.title, "platform": meeting.platform },
        "created_at": local_now.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let side = dir.join(format!("{name}{}", super::inbox::SIDECAR_SUFFIX));
    std::fs::write(side, serde_json::to_vec_pretty(&sidecar).unwrap_or_default())?;
    Ok(path)
}

/// The meeting summary route's hook: best effort, never fails the summary.
pub fn save_meeting(session_id: &str, meeting: &MeetingInfo, transcript: &[Segment], summary: &MeetingSummary) -> Option<PathBuf> {
    let dir = home_folder("Meetings")?;
    if transcript.iter().all(|s| s.text.trim().is_empty()) {
        return None;
    }
    match write_meeting(&dir, session_id, meeting, transcript, summary, chrono::Local::now().fixed_offset()) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "files v2: meeting note not saved to Meetings");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_tools::ActionItem;

    #[test]
    fn a_hostile_meeting_title_stays_inside_meetings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Meetings");
        let meeting = MeetingInfo { title: "../../.ssh/authorized_keys".into(), ..Default::default() };
        let transcript = vec![Segment { seq: 1, speaker: Some("Ann".into()), confidence: 0.9, start_ms: 61_000, end_ms: 62_000, text: "Ship Friday.".into() }];
        let summary = MeetingSummary {
            summary: "We agreed to ship.".into(),
            decisions: vec!["Ship Friday".into()],
            action_items: vec![ActionItem { text: "Tag the release".into(), owner: Some("Ann".into()), due: None }],
        };
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-15T10:42:00-06:00").unwrap();
        let path = write_meeting(&dir, "ms_1", &meeting, &transcript, &summary, at).unwrap();
        assert_eq!(path.parent().unwrap(), dir);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, "2026-11-15 1042 ssh authorized_keys.md");
        assert!(!tmp.path().join(".ssh").exists());
        let note = std::fs::read_to_string(&path).unwrap();
        assert!(note.contains("- Tag the release (Ann)") && note.contains("[00:01:01] Ann: Ship Friday."), "{note}");
        let side: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(format!("{name}.provenance.json"))).unwrap()).unwrap();
        assert_eq!(side["provenance"]["source"], "meeting");
        assert_eq!(side["provenance"]["ai_generated"], true);
        // A second note for the same minute and title does not overwrite the first.
        let again = write_meeting(&dir, "ms_2", &meeting, &transcript, &summary, at).unwrap();
        assert_ne!(again, path);
    }
}
