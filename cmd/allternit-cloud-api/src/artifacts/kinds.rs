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
        default_body_format: "application/vnd.allternit.openui",
        body_formats: &["application/vnd.allternit.openui"],
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
                "diagram", "image", "code"
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
    fn kind_names() {
        assert!(is_valid_kind_name("doc"));
        assert!(is_valid_kind_name("hologram_v2"));
        assert!(!is_valid_kind_name(""));
        assert!(!is_valid_kind_name("Doc"));
        assert!(!is_valid_kind_name("2doc"));
        assert!(!is_valid_kind_name("doc kind"));
    }
}
