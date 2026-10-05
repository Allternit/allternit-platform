use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::core::io::{append_json_line, ensure_dir, read_json_lines};
use crate::core::types::AllternitEvent;

pub fn append_thread_event(root_dir: &PathBuf, event: &AllternitEvent) -> Result<()> {
    let thread_id = event.payload.get("thread_id").and_then(|v| v.as_str());
    let Some(thread_id) = thread_id else {
        return Ok(());
    };
    let dir = root_dir.join(".allternit/mail/threads");
    ensure_dir(&dir)?;
    let path = dir.join(format!("{}.jsonl", thread_id));
    append_json_line(&path, event)?;
    append_digest_line(&digest_path(root_dir, thread_id), event)?;
    Ok(())
}

pub fn rebuild_threads(root_dir: &PathBuf, events: &[AllternitEvent]) -> Result<usize> {
    let mut buckets: std::collections::HashMap<String, Vec<AllternitEvent>> =
        std::collections::HashMap::new();
    for event in events {
        if let Some(thread_id) = event.payload.get("thread_id").and_then(|v| v.as_str()) {
            buckets
                .entry(thread_id.to_string())
                .or_default()
                .push(event.clone());
        }
    }
    let dir = root_dir.join(".allternit/mail/threads");
    ensure_dir(&dir)?;
    for (thread_id, events) in buckets.iter() {
        let path = dir.join(format!("{}.jsonl", thread_id));
        std::fs::write(&path, "")?;
        for event in events {
            append_json_line(&path, event)?;
        }
    }
    Ok(buckets.len())
}

// ---------------------------------------------------------------------------
// Thread digests (E2-R2)
// ---------------------------------------------------------------------------

/// Rebuild a thread's digest file exactly from its thread JSONL. Deleting
/// the digest and regenerating is a fixed-point: N events produce exactly N
/// one-line entries in event order, byte-identical on every regeneration.
pub fn regenerate_digest(root_dir: &Path, thread_id: &str) -> Result<usize> {
    let events: Vec<AllternitEvent> =
        read_json_lines(&root_dir.join(format!(".allternit/mail/threads/{}.jsonl", thread_id)))?;
    let path = digest_path(root_dir, thread_id);
    ensure_dir(path.parent().unwrap())?;
    let mut out = String::new();
    for event in &events {
        out.push_str(&digest_line(event));
        out.push('\n');
    }
    std::fs::write(&path, out)?;
    Ok(events.len())
}

fn digest_path(root_dir: &Path, thread_id: &str) -> PathBuf {
    root_dir.join(format!(".allternit/mail/threads/{}.digest.md", thread_id))
}

fn append_digest_line(path: &Path, event: &AllternitEvent) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(digest_line(event).as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// One digest line per thread event:
/// `<ts> <actor> <event_type> <subject-or-first-80-chars>`.
fn digest_line(event: &AllternitEvent) -> String {
    format!(
        "{} {} {} {}",
        event.ts,
        event.actor.id,
        event.r#type,
        digest_subject(event)
    )
}

/// Subject for typed envelopes; otherwise the first 80 chars of the most
/// descriptive payload field, flattened to a single line.
fn digest_subject(event: &AllternitEvent) -> String {
    let payload = &event.payload;
    let raw = ["subject", "body_ref", "note", "asset_ref", "decision", "topic"]
        .iter()
        .find_map(|key| payload.get(key).and_then(|v| v.as_str()))
        .unwrap_or("");
    let flat: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(80).collect()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Actor, ActorType};
    use serde_json::json;
    use tempfile::TempDir;

    fn event(id: &str, ts: &str, actor: &str, event_type: &str, payload: serde_json::Value) -> AllternitEvent {
        AllternitEvent {
            event_id: id.to_string(),
            ts: ts.to_string(),
            actor: Actor {
                r#type: ActorType::Agent,
                id: actor.to_string(),
            },
            scope: None,
            r#type: event_type.to_string(),
            payload,
            provenance: None,
        }
    }

    fn digest_text(tmp: &TempDir, thread_id: &str) -> String {
        std::fs::read_to_string(
            tmp.path()
                .join(format!(".allternit/mail/threads/{}.digest.md", thread_id)),
        )
        .unwrap()
    }

    #[test]
    fn digest_appends_in_event_order() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let events = vec![
            event(
                "e1",
                "2026-08-01T00:00:01Z",
                "api",
                "ThreadCreated",
                json!({ "thread_id": "mail:general", "topic": "mail:general" }),
            ),
            event(
                "e2",
                "2026-08-01T00:00:02Z",
                "alpha",
                "MessageSent",
                json!({ "thread_id": "mail:general", "subject": "deploy window Friday", "body_path": "x" }),
            ),
            event(
                "e3",
                "2026-08-01T00:00:03Z",
                "executor",
                "MailAssetShared",
                json!({ "thread_id": "mail:general", "asset_ref": "outputs/probe.txt" }),
            ),
        ];
        for e in &events {
            append_thread_event(&root, e).unwrap();
        }

        let text = digest_text(&tmp, "mail:general");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        // Order matches event order; format is `<ts> <actor> <type> <subject>`.
        assert_eq!(
            lines[0],
            "2026-08-01T00:00:01Z api ThreadCreated mail:general"
        );
        assert_eq!(
            lines[1],
            "2026-08-01T00:00:02Z alpha MessageSent deploy window Friday"
        );
        assert_eq!(
            lines[2],
            "2026-08-01T00:00:03Z executor MailAssetShared outputs/probe.txt"
        );
    }

    #[test]
    fn digest_subject_falls_back_and_truncates() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let long_body = "word ".repeat(40); // 200 chars
        let e = event(
            "e1",
            "2026-08-01T00:00:01Z",
            "test",
            "MessageSent",
            json!({ "thread_id": "mail:general", "body_ref": long_body }),
        );
        append_thread_event(&root, &e).unwrap();
        let line = digest_text(&tmp, "mail:general").lines().next().unwrap().to_string();
        // No subject -> first 80 chars of body_ref, single line.
        let subject_part = line.splitn(4, ' ').nth(3).unwrap();
        assert_eq!(subject_part.chars().count(), 80);
        assert!(!subject_part.contains('\n'));
    }

    #[test]
    fn regenerate_digest_is_exact_and_idempotent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        for i in 0..5 {
            append_thread_event(
                &root,
                &event(
                    &format!("e{}", i),
                    &format!("2026-08-01T00:00:0{}Z", i),
                    "alpha",
                    "MessageSent",
                    json!({ "thread_id": "mail:general", "subject": format!("s{}", i) }),
                ),
            )
            .unwrap();
        }

        // Delete + regenerate from the thread JSONL: exactly 5 lines in order.
        std::fs::remove_file(
            tmp.path()
                .join(".allternit/mail/threads/mail:general.digest.md"),
        )
        .unwrap();
        let n = regenerate_digest(&root, "mail:general").unwrap();
        assert_eq!(n, 5);
        let first = digest_text(&tmp, "mail:general");
        assert_eq!(first.lines().count(), 5);
        assert!(first.lines().next().unwrap().contains("s0"));
        assert!(first.lines().nth(4).unwrap().contains("s4"));

        // Byte-identical on second regeneration.
        let n = regenerate_digest(&root, "mail:general").unwrap();
        assert_eq!(n, 5);
        assert_eq!(digest_text(&tmp, "mail:general"), first);
    }
}
