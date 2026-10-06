//! Agent rules enforced in the spawn hook (resolved by the API at run
//! creation and carried in the WIH's JudgePolicy; the hook never calls out).

use super::HookRequest;
use crate::judge::policy::{CustomRule, RuleAction, RuleSet};

/// What a rule produced: ask (attention) or deny, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Ask(String),
    Deny(String),
}

fn glob(p: &[u8], t: &[u8]) -> bool {
    match p.split_first() {
        None => t.is_empty(),
        Some((b'*', rest)) => (0..=t.len()).any(|i| glob(rest, &t[i..])),
        Some((b'?', rest)) => !t.is_empty() && glob(rest, &t[1..]),
        Some((c, rest)) => t.first() == Some(c) && glob(rest, &t[1..]),
    }
}

/// `when` matches anywhere in `field`: a glob (`*`, `?`) or plain substring,
/// case-insensitive.
pub fn when_matches(when: &str, field: &str) -> bool {
    let w = when.trim().to_ascii_lowercase();
    let f = field.to_ascii_lowercase();
    if w.is_empty() {
        return false;
    }
    if w.contains(['*', '?']) {
        glob(format!("*{w}*").as_bytes(), f.as_bytes())
    } else {
        f.contains(&w)
    }
}

pub fn custom_hit(rules: &[CustomRule], tool: &str, command: Option<&str>, paths: &[String]) -> Option<Hit> {
    let mut ask = None;
    for r in rules {
        let hit = when_matches(&r.when, tool)
            || command.is_some_and(|c| when_matches(&r.when, c))
            || paths.iter().any(|p| when_matches(&r.when, p));
        if !hit {
            continue;
        }
        let why = format!("custom rule {} ({}): {}", r.id, r.when, r.text);
        match r.action {
            RuleAction::Deny => return Some(Hit::Deny(why)),
            RuleAction::Ask => {
                ask.get_or_insert(Hit::Ask(why));
            }
        }
    }
    ask
}

/// Private-network ask plus custom rules for one call (deny beats ask).
pub fn evaluate(rs: &RuleSet, req: &HookRequest, paths: &[String]) -> Option<Hit> {
    let command = req.command();
    let custom = custom_hit(&rs.custom, &req.tool_name, command.as_deref(), paths);
    if matches!(custom, Some(Hit::Deny(_))) {
        return custom;
    }
    evaluate_network(rs, req).or(custom)
}

pub fn evaluate_network(rs: &RuleSet, req: &HookRequest) -> Option<Hit> {
    if !rs.ask_private_network {
        return None;
    }
    let mut texts: Vec<String> = req.command().into_iter().collect();
    for key in ["url", "uri"] {
        if let Some(u) = req.tool_input.get(key).and_then(|v| v.as_str()) {
            texts.push(u.to_string());
        }
    }
    texts
        .iter()
        .find_map(|t| super::blocklist::check_private_egress(t))
        .map(|r| Hit::Ask(format!("private network: {r}")))
}
