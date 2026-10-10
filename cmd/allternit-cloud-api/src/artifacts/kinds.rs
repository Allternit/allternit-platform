//! The artifact kind list (contract: docs/design/artifacts-v2.md §1).
//!
//! The app keeps the same list in `allternit-ai/src/lib/artifacts/kinds.ts`;
//! change both together. The server only needs each kind's default body
//! format (used when a create omits `body_format`) and the formats the
//! contract names for it. Unknown kinds are accepted (a newer client may
//! know more kinds than this build); the app renders them as `page` when the
//! body is HTML and as code otherwise.

pub struct KindSpec {
    pub kind: &'static str,
    /// Used when the create request leaves `body_format` out.
    pub default_body_format: &'static str,
    /// Every format the contract lists for this kind (default first).
    pub body_formats: &'static [&'static str],
}

pub const DOC: &str = "doc";

pub const KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "doc",
        default_body_format: "application/vnd.allternit.doc+json",
        body_formats: &["application/vnd.allternit.doc+json", "text/markdown"],
    },
    KindSpec {
        kind: "sheet",
        default_body_format: "application/vnd.allternit.sheet+json",
        body_formats: &["application/vnd.allternit.sheet+json"],
    },
    KindSpec {
        kind: "slides",
        default_body_format: "application/vnd.allternit.slides+json",
        body_formats: &["application/vnd.allternit.slides+json"],
    },
    KindSpec {
        kind: "design",
        default_body_format: "application/vnd.allternit.design+json",
        body_formats: &["application/vnd.allternit.design+json", "text/html"],
    },
    KindSpec {
        kind: "dashboard",
        default_body_format: "application/vnd.allternit.dashboard+json",
        // `openui` stays accepted: Phase 1 dashboards were stored as OpenUI cards.
        body_formats: &["application/vnd.allternit.dashboard+json", "application/vnd.allternit.openui"],
    },
    KindSpec {
        kind: "motion",
        default_body_format: "application/vnd.allternit.motion+json",
        body_formats: &["application/vnd.allternit.motion+json"],
    },
    KindSpec {
        kind: "page",
        default_body_format: "text/html",
        body_formats: &["text/html", "text/markdown"],
    },
    KindSpec {
        kind: "card",
        default_body_format: "application/vnd.allternit.openui",
        body_formats: &["application/vnd.allternit.openui"],
    },
    KindSpec {
        kind: "diagram",
        default_body_format: "text/vnd.mermaid",
        body_formats: &["text/vnd.mermaid", "image/svg+xml"],
    },
    KindSpec {
        kind: "image",
        default_body_format: "text/uri-list",
        body_formats: &["text/uri-list"],
    },
    KindSpec {
        kind: "code",
        default_body_format: "text/plain",
        body_formats: &["text/plain"],
    },
    KindSpec {
        kind: "pdf",
        default_body_format: "application/pdf",
        body_formats: &["application/pdf"],
    },
];

pub fn spec(kind: &str) -> Option<&'static KindSpec> {
    KINDS.iter().find(|spec| spec.kind == kind)
}

/// Default body format for a kind; unknown kinds fall back to `text/plain`
/// (the app shows them as code).
pub fn default_body_format(kind: &str) -> &'static str {
    spec(kind)
        .map(|spec| spec.default_body_format)
        .unwrap_or("text/plain")
}

/// A kind name is lower-case ASCII, starts with a letter, at most 32 chars.
/// Unknown-but-well-formed kinds are allowed (see the module docs).
pub fn is_valid_kind_name(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 32
        && kind.as_bytes()[0].is_ascii_lowercase()
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A body format is a MIME-like token: `type/subtype[+suffix]`, no spaces.
pub fn is_valid_body_format(format: &str) -> bool {
    format.len() <= 128
        && format.contains('/')
        && format
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/.+-_".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_contract_kind_has_its_default_first() {
        let names: Vec<_> = KINDS.iter().map(|k| k.kind).collect();
        assert_eq!(
            names,
            [
                "doc", "sheet", "slides", "design", "dashboard", "motion", "page", "card",
                "diagram", "image", "code", "pdf"
            ]
        );
        for spec in KINDS {
            assert_eq!(spec.body_formats[0], spec.default_body_format);
            assert!(is_valid_body_format(spec.default_body_format));
        }
        assert_eq!(default_body_format("diagram"), "text/vnd.mermaid");
        assert_eq!(default_body_format("hologram"), "text/plain");
    }

    #[test]
    fn dashboards_default_to_tile_json_and_still_accept_openui() {
        assert_eq!(
            default_body_format("dashboard"),
            "application/vnd.allternit.dashboard+json"
        );
        let spec = KINDS.iter().find(|k| k.kind == "dashboard").unwrap();
        assert!(spec.body_formats.contains(&"application/vnd.allternit.openui"));
    }

    #[test]
    fn kind_names() {
        assert!(is_valid_kind_name("doc"));
        assert!(is_valid_kind_name("hologram_v2"));
        assert!(!is_valid_kind_name(""));
        assert!(!is_valid_kind_name("Doc"));
        assert!(!is_valid_kind_name("2doc"));
        assert!(!is_valid_kind_name("doc kind"));
    }
}

/// The `capabilities.connectors` a dashboard body declares: one entry per
/// connector with the tools its tiles call (`<connector>__<tool>`), sorted.
/// Mirrors `connectorCapabilities` in the app's dashboard schema. The server
/// derives it on every save so the link rule and viewer consents apply even
/// before the owner opens the dashboard. Returns None for a body that isn't
/// dashboard JSON or calls no tools.
pub fn dashboard_connectors(body: &str) -> Option<serde_json::Value> {
    use std::collections::{BTreeMap, BTreeSet};
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let tiles = parsed.get("tiles")?.as_array()?;
    let mut by_connector: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for tile in tiles {
        let Some(name) = tile.pointer("/query/tool").and_then(|t| t.as_str()).map(str::trim) else { continue };
        if name.is_empty() {
            continue;
        }
        let (connector, tool) = match name.find("__") {
            Some(i) if i > 0 => (&name[..i], &name[i + 2..]),
            _ => (name, name),
        };
        by_connector.entry(connector.to_string()).or_default().insert(tool.to_string());
    }
    if by_connector.is_empty() {
        return None;
    }
    Some(serde_json::Value::Array(
        by_connector
            .into_iter()
            .map(|(connector, tools)| serde_json::json!({ "connector": connector, "tools": tools.into_iter().collect::<Vec<_>>() }))
            .collect(),
    ))
}

#[cfg(test)]
mod dashboard_connector_tests {
    use super::dashboard_connectors;

    #[test]
    fn groups_tools_by_connector() {
        let body = r#"{"version":1,"tiles":[
            {"query":{"tool":"stripe__list_charges"}},
            {"query":{"tool":"stripe__list_customers"}},
            {"query":{"tool":"snowflake__run_sql","sql":"select 1"}},
            {"query":{"tool":"stripe__list_charges"}}]}"#;
        let got = dashboard_connectors(body).unwrap();
        assert_eq!(
            got,
            serde_json::json!([
                {"connector":"snowflake","tools":["run_sql"]},
                {"connector":"stripe","tools":["list_charges","list_customers"]}
            ])
        );
    }

    #[test]
    fn none_for_no_tools_or_bad_json() {
        assert!(dashboard_connectors(r#"{"version":1,"tiles":[]}"#).is_none());
        assert!(dashboard_connectors("not json").is_none());
    }
}
