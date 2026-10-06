pub mod projection;
pub mod types;

use std::collections::HashSet;

use crate::core::types::AllternitEvent;

pub use projection::project_wih;
pub use types::WihState;

pub fn active_wihs(events: &[AllternitEvent]) -> Vec<WihState> {
    let mut wih_ids: HashSet<String> = HashSet::new();
    for evt in events {
        if matches!(evt.r#type.as_str(), "WIHCreated" | "WIHPickedUp") {
            if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                wih_ids.insert(wih_id.to_string());
            }
        }
    }
    let mut out: Vec<WihState> = wih_ids
        .into_iter()
        .filter_map(|wih_id| project_wih(events, &wih_id))
        .filter(|state| !matches!(state.status.as_str(), "CLOSED" | "FAILED" | "VAULTED"))
        .collect();
    out.sort_by(|a, b| a.wih_id.cmp(&b.wih_id));
    out
}
