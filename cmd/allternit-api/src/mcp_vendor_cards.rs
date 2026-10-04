//! MCP App cards for the vendor-bot connector (`mcp_vendor_bots`): thread, text/call result,
//! email draft and channel post cards, rendered by the host (Claude, ChatGPT, ...) from
//! `ui://allternit/vendor-*.v1.html` resources. One HTML template serves all four; the kind is
//! substituted when the resource is read, with the vendor-pack accent table embedded
//! (`assets/vendor-bot-card-packs.json`, the accents of the allternit-ai look packs).
//!
//! Cards render `structuredContent`; the model still reads the plain `content` text. A refused
//! tool still gets a card (`refused: true`) so the person sees why nothing went out.

use serde_json::{json, Value};

use crate::mcp_agents::MCP_APP_MIME;

const TEMPLATE: &str = include_str!("../assets/vendor-bot-card.v1.html");
const PACKS: &str = include_str!("../assets/vendor-bot-card-packs.json");

pub const THREAD_URI: &str = "ui://allternit/vendor-thread.v1.html";
pub const RESULT_URI: &str = "ui://allternit/vendor-result.v1.html";
pub const EMAIL_URI: &str = "ui://allternit/vendor-email.v1.html";
pub const POST_URI: &str = "ui://allternit/vendor-post.v1.html";

/// `(uri, kind in the template, resource title)`.
const CARDS: [(&str, &str, &str); 4] = [
    (THREAD_URI, "thread", "Thread card"),
    (RESULT_URI, "result", "Text or call result card"),
    (EMAIL_URI, "email", "Email draft card"),
    (POST_URI, "post", "Channel post card"),
];

/// The card a tool renders in, if it has one.
pub fn card_uri(tool: &str) -> Option<&'static str> {
    match tool {
        "list_threads" | "read_thread" => Some(THREAD_URI),
        "send_text" | "start_call" => Some(RESULT_URI),
        "send_email" => Some(EMAIL_URI),
        "post_message" => Some(POST_URI),
        _ => None,
    }
}

/// `_meta` for a tool descriptor.
pub fn tool_meta(uri: &str) -> Value {
    json!({ "ui": { "resourceUri": uri, "visibility": ["model", "app"] }, "ui/resourceUri": uri })
}

pub fn resource_descriptors() -> Vec<Value> {
    CARDS
        .iter()
        .map(|(uri, kind, title)| json!({ "uri": uri, "name": format!("vendor-{kind}"), "title": title, "description": "Allternit vendor bot card.", "mimeType": MCP_APP_MIME }))
        .collect()
}

pub fn read_resource(uri: &str) -> Option<Value> {
    let (_, kind, _) = CARDS.iter().find(|(u, _, _)| *u == uri)?;
    let html = TEMPLATE.replace("__KIND__", kind).replace("__PACKS__", PACKS.trim());
    Some(json!({
        "contents": [{
            "uri": uri,
            "mimeType": MCP_APP_MIME,
            "text": html,
            "_meta": { "ui": { "csp": { "connectDomains": [], "resourceDomains": [] }, "prefersBorder": true } }
        }]
    }))
}

/// What the card shows for one finished call: the tool's own answer, the request echoed back
/// (the host already has it), and, for a refusal, `refused` plus the sentence the person reads.
pub fn structured(tool: &str, args: &Value, outcome: &Result<Value, String>) -> Value {
    let pick = |keys: &[&str]| -> Value {
        let mut m = serde_json::Map::new();
        for k in keys {
            if let Some(v) = args.get(*k).filter(|v| !v.is_null()) {
                m.insert((*k).into(), v.clone());
            }
        }
        Value::Object(m)
    };
    let mut out = match outcome {
        Ok(v) if v.is_object() => v.clone(),
        Ok(v) => json!({ "result": v }),
        Err(message) => json!({ "refused": true, "message": message }),
    };
    let request = match tool {
        "send_text" => Some(pick(&["to", "text"])),
        "start_call" => Some(pick(&["to", "purpose"])),
        "send_email" => Some(pick(&["to", "subject", "body"])),
        "post_message" => Some(pick(&["provider", "target", "text"])),
        _ => None,
    };
    if let (Some(request), Some(o)) = (request, out.as_object_mut()) {
        o.insert("request".into(), request);
        o.insert("tool".into(), json!(tool));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_card_resource_reads_with_its_kind_and_the_pack_table() {
        for (uri, kind, _) in CARDS {
            let r = read_resource(uri).unwrap();
            let c = &r["contents"][0];
            assert_eq!(c["mimeType"], MCP_APP_MIME);
            let html = c["text"].as_str().unwrap();
            assert!(html.contains(&format!("var KIND = \"{kind}\";")));
            assert!(!html.contains("__KIND__") && !html.contains("__PACKS__"));
            assert!(html.contains("\"claude\"") && html.contains("#cc785c"));
            assert_eq!(c["_meta"]["ui"]["csp"]["connectDomains"], json!([]));
        }
        assert!(read_resource("ui://allternit/other.html").is_none());
        assert_eq!(resource_descriptors().len(), 4);
    }

    #[test]
    fn the_pack_table_is_valid_json_with_a_default_and_four_hosts() {
        let v: Value = serde_json::from_str(PACKS).unwrap();
        assert!(v["default"]["accent"].is_string());
        for host in ["claude", "chatgpt", "gemini", "hermes"] {
            assert!(v["packs"][host]["accent"].as_str().is_some_and(|a| a.starts_with('#')), "{host}");
            assert!(v["packs"][host]["match"].as_array().is_some_and(|m| !m.is_empty()));
        }
    }

    #[test]
    fn only_the_card_tools_have_a_card() {
        for t in ["list_threads", "read_thread", "send_text", "start_call", "send_email", "post_message"] {
            assert!(card_uri(t).is_some(), "{t}");
        }
        for t in ["ask_bot", "get_ticket", "post_result", "list_open_tickets"] {
            assert!(card_uri(t).is_none(), "{t}");
        }
    }

    #[test]
    fn a_refusal_still_has_card_data_and_a_success_echoes_the_request() {
        let args = json!({ "to": "+14155550123", "text": "hi", "extra": "x" });
        let refused = structured("send_text", &args, &Err("They haven't contacted this number.".into()));
        assert_eq!(refused["refused"], true);
        assert_eq!(refused["message"], "They haven't contacted this number.");
        assert_eq!(refused["request"], json!({ "to": "+14155550123", "text": "hi" }));
        let ok = structured("send_email", &json!({ "to": "a@b.co", "subject": "s", "body": "b" }), &Ok(json!({ "queued": true, "status": "pending_approval" })));
        assert_eq!(ok["status"], "pending_approval");
        assert_eq!(ok["request"]["subject"], "s");
        assert_eq!(ok["tool"], "send_email");
        let threads = structured("list_threads", &json!({}), &Ok(json!({ "threads": [] })));
        assert!(threads.get("request").is_none());
    }

    #[test]
    fn the_card_script_handles_every_kind_it_can_be_given() {
        for k in ["thread", "result", "email", "post"] {
            assert!(TEMPLATE.contains(&format!("{k}: render")), "{k}");
        }
    }
}
