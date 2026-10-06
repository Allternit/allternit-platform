use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::types::AllternitEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptDelta {
    pub delta_id: String,
    pub recorded_at: String,
    pub valid_at: String,
    pub author: String,
    pub category: String,
    pub text: String,
    pub linked_mutations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTimeline {
    pub prompt_id: String,
    pub created_at: String,
    pub source: String,
    pub raw_text: String,
    pub intent_tags: Vec<String>,
    pub deltas: Vec<PromptDelta>,
    pub work_links: Vec<Value>,
}

pub fn project_prompt(events: &[AllternitEvent], prompt_id: &str) -> Option<PromptTimeline> {
    let mut prompt: Option<PromptTimeline> = None;
    let mut deltas: Vec<PromptDelta> = Vec::new();
    let mut links: Vec<Value> = Vec::new();

    for evt in events {
        if evt.payload.get("prompt_id").and_then(|v| v.as_str()) != Some(prompt_id) {
            continue;
        }
        match evt.r#type.as_str() {
            "PromptCreated" => {
                let source = evt
                    .payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let raw_text = evt
                    .payload
                    .get("raw_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let intent_tags = evt
                    .payload
                    .get("intent_tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(Vec::new);
                prompt = Some(PromptTimeline {
                    prompt_id: prompt_id.to_string(),
                    created_at: evt.ts.clone(),
                    source,
                    raw_text,
                    intent_tags,
                    deltas: Vec::new(),
                    work_links: Vec::new(),
                });
            }
            "PromptDeltaAppended" => {
                let delta_id = evt
                    .payload
                    .get("delta_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let author = evt
                    .payload
                    .get("author")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let category = evt
                    .payload
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let text = evt
                    .payload
                    .get("delta_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let valid_at = evt
                    .payload
                    .get("valid_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&evt.ts)
                    .to_string();
                let linked_mutations = evt
                    .payload
                    .get("links")
                    .and_then(|v| v.get("mutations"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(Vec::new);
                deltas.push(PromptDelta {
                    delta_id,
                    recorded_at: evt.ts.clone(),
                    valid_at,
                    author,
                    category,
                    text,
                    linked_mutations,
                });
            }
            "PromptLinkedToWork" => {
                links.push(evt.payload.clone());
            }
            _ => {}
        }
    }

    if let Some(mut timeline) = prompt {
        deltas.sort_by(|a, b| a.recorded_at.cmp(&b.recorded_at));
        timeline.deltas = deltas;
        timeline.work_links = links;
        Some(timeline)
    } else {
        None
    }
}
