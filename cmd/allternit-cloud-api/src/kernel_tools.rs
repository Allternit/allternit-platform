//! Tool to action-class table for the tools cloud-api runs itself (plan step 18 follow-up).
//!
//! Mirrors allternit-api's `authority/check.rs` where tools overlap (shell and file writes ask,
//! reads allow). A tool in no table is **ask, never allow** (`known: false`). A connector write
//! that sends something to another person is `client_message`; anything that moves money is
//! `money`. Both are human-only classes.

use allternit_authority::ActionClass::{self, *};

/// A classified tool: its class and whether the table knew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Class {
    pub class: ActionClass,
    pub known: bool,
}

fn known(class: ActionClass) -> Class {
    Class { class, known: true }
}

/// Not in any table: recorded as `file_write` with its rule forced to `ask`.
fn unknown() -> Class {
    Class { class: FileWrite, known: false }
}

/// Words in a connector tool name that mean it sends something to another person.
const SENDS: [&str; 10] = ["send", "reply", "post", "message", "email", "mail", "sms", "text", "dial", "call"];
/// Words that mean it moves money.
const PAYS: [&str; 8] = ["pay", "payment", "charge", "refund", "invoice", "checkout", "transfer", "payout"];
/// Words that only read.
const READS: [&str; 8] = ["read", "list", "get", "search", "find", "fetch", "view", "status"];
/// Words that cancel a read-looking name.
const NOT_READS: [&str; 7] = ["delete", "remove", "write", "create", "update", "otp", "secret"];

/// A tool on the owner's connectors MCP server (`meeting_pod`), named by the connector itself.
/// Money first, then sends, then reads; anything else asks.
pub fn connector_class(tool: &str) -> Class {
    let w: Vec<String> = tool.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase).collect();
    let has = |set: &[&str]| w.iter().any(|x| set.contains(&x.as_str()));
    if has(&PAYS) {
        known(Money)
    } else if has(&SENDS) {
        known(ClientMessage)
    } else if has(&READS) && !has(&NOT_READS) {
        known(Read)
    } else {
        unknown()
    }
}

/// The platform agent tools served by `platform_v1/agent_tools`.
pub fn agent_tool_class(tool: &str) -> Class {
    match tool {
        "knowledge_search" => known(Read),
        // Posts to a customer's connected channel and texts or calls a person.
        "channel_post" | "send_text" | "call" => known(ClientMessage),
        _ => unknown(),
    }
}

/// The edge tools a project key runs server-side in `mcp_edge`: `decide` / `respond`, the phone
/// tools and the mail tools. `phone_tool` is classified by its inner `name`.
pub fn edge_class(tool: &str, args: &serde_json::Value) -> Class {
    match tool {
        "decide" => known(Read),
        // Runs a model or an agent: spends the project's inference and may drive the agent's tools.
        "respond" => known(RunControl),
        "phones_list" | "phone_screenshot" | "phone_maps" | "phone_history" | "phone_run_status" | "phone_live_url" => known(Read),
        "phone_tap" | "phone_type" | "phone_open_app" | "phone_task" | "phone_skill_run" => known(FileWrite),
        "phone_tool" => match args.get("name").and_then(|v| v.as_str()) {
            Some("phone.sms.send" | "phone.call.dial") => known(ClientMessage),
            Some("phone.read_ui" | "phone.sms.read" | "phone.screenshot") => known(Read),
            _ => unknown(),
        },
        "mailbox_list" | "mail_list" | "mail_read" => known(Read),
        "mailbox_create" => known(FileWrite),
        // `mail_read_otp` reads a one-time code, a secret: it asks like any unlisted tool.
        _ => unknown(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sends_are_client_messages_payments_are_money_unknown_asks() {
        assert_eq!(connector_class("gmail.send_email"), known(ClientMessage));
        assert_eq!(connector_class("slack_post_message"), known(ClientMessage));
        assert_eq!(connector_class("stripe.create_refund"), known(Money));
        assert_eq!(connector_class("calendar.list_events"), known(Read));
        assert!(!connector_class("drive.share").known);
        assert!(!connector_class("gmail.read_otp").known);
        assert_eq!(agent_tool_class("send_text"), known(ClientMessage));
        assert!(!agent_tool_class("novel").known);
        assert_eq!(edge_class("phone_tool", &json!({ "name": "phone.sms.send" })), known(ClientMessage));
        assert!(!edge_class("phone_tool", &json!({ "name": "phone.swipe" })).known);
        assert!(!edge_class("mail_read_otp", &json!({})).known);
    }
}
