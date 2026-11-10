//! Auto-drain a pane's mailbox when the pane goes idle.
//!
//! A send to a busy pane lands in its mailbox. Nothing used to empty it until
//! someone ran a drain by hand. This watcher looks at the live panes every
//! few seconds; when a pane that was working is now idle (or done), it drains
//! that session's mailbox, one message per idle edge, through the same
//! verified paste as a send (`backend::drain`: settle only after the paste is
//! read back). A message the pane did not take stays queued for the next edge.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use super::backend;

const EVERY: Duration = Duration::from_secs(4);

fn is_idle(status: Option<&str>) -> bool {
    matches!(status, Some("idle") | Some("done"))
}

/// Sessions that moved from not-idle to idle since `last`; updates `last`.
pub fn idle_edges(last: &mut HashMap<String, bool>, panes: &[(String, Option<String>)]) -> Vec<String> {
    let mut edges = Vec::new();
    for (session, status) in panes {
        let idle = is_idle(status.as_deref());
        // First sight of an idle pane counts as an edge too: its mailbox may be
        // holding mail from before the watcher started.
        if idle && !last.get(session).copied().unwrap_or(false) {
            edges.push(session.clone());
        }
        last.insert(session.clone(), idle);
    }
    last.retain(|s, _| panes.iter().any(|(p, _)| p == s));
    edges
}

/// Start the watcher for `root` (idempotent; needs a tokio runtime).
pub fn ensure(root: PathBuf) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut last: HashMap<String, bool> = HashMap::new();
        loop {
            tokio::time::sleep(EVERY).await;
            let Ok(live) = backend::blocking(|| backend::backend()?.list()).await else { continue };
            let panes: Vec<(String, Option<String>)> = live.into_iter().map(|p| (p.session, p.agent_status)).collect();
            for session in idle_edges(&mut last, &panes) {
                let root = root.clone();
                let _ = backend::blocking(move || backend::drain(&root, &session, false)).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_on_the_working_to_idle_edge_only() {
        let mut last = HashMap::new();
        let p = |s: &str| vec![("ao-a".to_string(), Some(s.to_string()))];
        assert_eq!(idle_edges(&mut last, &p("working")), Vec::<String>::new());
        assert_eq!(idle_edges(&mut last, &p("idle")), vec!["ao-a".to_string()]);
        assert!(idle_edges(&mut last, &p("idle")).is_empty());
        assert_eq!(idle_edges(&mut last, &p("done")).len(), 0);
    }
}
