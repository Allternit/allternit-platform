use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;

use crate::core::ids::create_event_id;
use crate::core::io::{append_json_line, list_jsonl_sorted, read_json_lines};
use crate::core::types::{AllternitEvent, LedgerQuery};
use crate::core::EventSink;

#[derive(Debug, Clone)]
pub struct LedgerOptions {
    pub root_dir: Option<PathBuf>,
    pub ledger_dir: Option<PathBuf>,
}

pub struct Ledger {
    events_dir: PathBuf,
}

impl Ledger {
    pub fn new(opts: LedgerOptions) -> Self {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let ledger_dir = opts
            .ledger_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/ledger"));
        let base = if ledger_dir.is_absolute() {
            ledger_dir
        } else {
            root_dir.join(ledger_dir)
        };
        let events_dir = if base.ends_with("events") {
            base
        } else {
            base.join("events")
        };

        Self { events_dir }
    }

    pub async fn append(&self, mut event: AllternitEvent) -> Result<String> {
        if event.event_id.is_empty() {
            event.event_id = create_event_id();
        }
        if event.ts.is_empty() {
            event.ts = Utc::now().to_rfc3339();
        }
        let date_file = event.ts.get(0..10).unwrap_or("unknown");
        let path = self.events_dir.join(format!("{}.jsonl", date_file));
        append_json_line(&path, &event)?;
        Ok(event.event_id)
    }

    pub async fn query(&self, q: LedgerQuery) -> Result<Vec<AllternitEvent>> {
        let mut events = self.read_all_events()?;
        if let Some(scope) = q.scope {
            events = events
                .into_iter()
                .filter(|evt| matches_scope(evt, &scope))
                .collect();
        }
        if let Some(t) = q.r#type {
            events = events.into_iter().filter(|evt| evt.r#type == t).collect();
        }
        if let Some(types) = q.types {
            let set: std::collections::HashSet<String> = types.into_iter().collect();
            events = events
                .into_iter()
                .filter(|evt| set.contains(&evt.r#type))
                .collect();
        }
        if let Some(since) = q.since {
            let since_ts = chrono::DateTime::parse_from_rfc3339(&since)
                .ok()
                .map(|d| d.timestamp());
            if let Some(since_ts) = since_ts {
                events = events
                    .into_iter()
                    .filter(|evt| {
                        chrono::DateTime::parse_from_rfc3339(&evt.ts)
                            .map(|d| d.timestamp() >= since_ts)
                            .unwrap_or(false)
                    })
                    .collect();
            }
        }
        if let Some(until) = q.until {
            let until_ts = chrono::DateTime::parse_from_rfc3339(&until)
                .ok()
                .map(|d| d.timestamp());
            if let Some(until_ts) = until_ts {
                events = events
                    .into_iter()
                    .filter(|evt| {
                        chrono::DateTime::parse_from_rfc3339(&evt.ts)
                            .map(|d| d.timestamp() <= until_ts)
                            .unwrap_or(false)
                    })
                    .collect();
            }
        }
        if let Some(limit) = q.limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    pub async fn tail(&self, n: usize) -> Result<Vec<AllternitEvent>> {
        let mut events = self.read_all_events()?;
        if n == 0 {
            return Ok(Vec::new());
        }
        if events.len() > n {
            events = events.split_off(events.len() - n);
        }
        Ok(events)
    }

    pub async fn verify_chain(&self) -> Result<(bool, Vec<String>)> {
        Ok((true, Vec::new()))
    }

    fn read_all_events(&self) -> Result<Vec<AllternitEvent>> {
        let mut all = Vec::new();
        for file in list_jsonl_sorted(&self.events_dir)? {
            let mut batch: Vec<AllternitEvent> = read_json_lines(&file)?;
            all.append(&mut batch);
        }
        Ok(all)
    }

    #[allow(dead_code)]
    fn events_dir(&self) -> &Path {
        &self.events_dir
    }
}

fn matches_scope(event: &AllternitEvent, scope: &crate::core::types::EventScope) -> bool {
    let evt_scope = event.scope.as_ref();
    match evt_scope {
        None => false,
        Some(s) => {
            if let Some(project_id) = &scope.project_id {
                if s.project_id.as_ref() != Some(project_id) {
                    return false;
                }
            }
            if let Some(dag_id) = &scope.dag_id {
                if s.dag_id.as_ref() != Some(dag_id) {
                    return false;
                }
            }
            if let Some(node_id) = &scope.node_id {
                if s.node_id.as_ref() != Some(node_id) {
                    return false;
                }
            }
            if let Some(wih_id) = &scope.wih_id {
                if s.wih_id.as_ref() != Some(wih_id) {
                    return false;
                }
            }
            if let Some(run_id) = &scope.run_id {
                if s.run_id.as_ref() != Some(run_id) {
                    return false;
                }
            }
            true
        }
    }
}

#[async_trait::async_trait]
impl EventSink for Ledger {
    async fn append(&self, event: AllternitEvent) -> anyhow::Result<String> {
        Ledger::append(self, event).await
    }
}
