//! Node-output placeholders in node titles/descriptions.
//!
//! A node may reference a predecessor's recorded output with
//! `{{ <node_id>.output }}` (inline text) or `{{ <node_id>.output_path }}`
//! (path of the derived `.out.md` view). Placeholders are resolved at Gate 1
//! pickup into the WIH context; the DAG itself is never rewritten.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;

/// Which part of a node output a placeholder asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlaceholderField {
    Output,
    OutputPath,
}

impl PlaceholderField {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlaceholderField::Output => "output",
            PlaceholderField::OutputPath => "output_path",
        }
    }
}

/// One `{{ <node_id>.<field> }}` occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    pub node_id: String,
    pub field: PlaceholderField,
    /// The exact matched text, braces included.
    pub raw: String,
}

fn node_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{\s*([A-Za-z0-9_\-.]+?)\.(output_path|output)\s*\}\}")
            .expect("node ref regex")
    })
}

fn param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{\s*params\.([A-Za-z0-9_\-]+)\s*\}\}").expect("param regex")
    })
}

/// All node-output references in `text`, in order of appearance.
pub fn node_refs(text: &str) -> Vec<NodeRef> {
    node_ref_re()
        .captures_iter(text)
        .filter(|c| &c[1] != "params")
        .map(|c| NodeRef {
            node_id: c[1].to_string(),
            field: if &c[2] == "output_path" {
                PlaceholderField::OutputPath
            } else {
                PlaceholderField::Output
            },
            raw: c[0].to_string(),
        })
        .collect()
}

/// Distinct node ids referenced across several texts.
pub fn referenced_node_ids<'a>(texts: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    texts
        .into_iter()
        .flat_map(node_refs)
        .map(|r| r.node_id)
        .collect()
}

/// Replace every node reference using `resolve`. References `resolve`
/// returns `None` for are left untouched.
pub fn render_node_refs(text: &str, mut resolve: impl FnMut(&NodeRef) -> Option<String>) -> String {
    let re = node_ref_re();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let m = caps.get(0).expect("match");
        let node_id = caps[1].to_string();
        if node_id == "params" {
            continue;
        }
        let r = NodeRef {
            node_id,
            field: if &caps[2] == "output_path" {
                PlaceholderField::OutputPath
            } else {
                PlaceholderField::Output
            },
            raw: m.as_str().to_string(),
        };
        out.push_str(&text[last..m.start()]);
        match resolve(&r) {
            Some(v) => out.push_str(&v),
            None => out.push_str(m.as_str()),
        }
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Rewrite the node id inside node references (used when a template's step
/// ids are minted into DAG node ids). Ids `map` does not know are left as-is.
pub fn rewrite_node_ids(text: &str, map: &std::collections::HashMap<String, String>) -> String {
    render_node_refs(text, |r| {
        map.get(&r.node_id)
            .map(|id| format!("{{{{ {}.{} }}}}", id, r.field.as_str()))
    })
}

/// Param names referenced as `{{ params.<name> }}`.
pub fn param_refs(text: &str) -> BTreeSet<String> {
    param_re()
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect()
}

/// Substitute `{{ params.<name> }}`. Unknown names are left untouched (the
/// caller rejects missing required params before rendering).
pub fn render_params(text: &str, params: &std::collections::HashMap<String, String>) -> String {
    param_re()
        .replace_all(text, |c: &regex::Captures| {
            params
                .get(&c[1])
                .cloned()
                .unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn finds_output_and_output_path_refs() {
        let refs = node_refs("use {{ capture.output }} and {{capture.output_path}} not {{ params.x }}");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].node_id, "capture");
        assert_eq!(refs[0].field, PlaceholderField::Output);
        assert_eq!(refs[1].field, PlaceholderField::OutputPath);
    }

    #[test]
    fn node_ids_may_contain_dashes_and_dots() {
        let refs = node_refs("{{ cut-3fa2.output }} {{ a.b.output_path }}");
        assert_eq!(refs[0].node_id, "cut-3fa2");
        assert_eq!(refs[1].node_id, "a.b");
    }

    #[test]
    fn render_leaves_unresolved_refs() {
        let out = render_node_refs("x={{ a.output }} y={{ b.output }}", |r| {
            (r.node_id == "a").then(|| "A".to_string())
        });
        assert_eq!(out, "x=A y={{ b.output }}");
    }

    #[test]
    fn rewrite_maps_step_ids() {
        let mut map = HashMap::new();
        map.insert("capture".to_string(), "capture-00ff".to_string());
        assert_eq!(
            rewrite_node_ids("see {{capture.output_path}}", &map),
            "see {{ capture-00ff.output_path }}"
        );
    }

    #[test]
    fn params_render_and_list() {
        let mut p = HashMap::new();
        p.insert("topic".to_string(), "Projects".to_string());
        assert_eq!(render_params("promo: {{ params.topic }}", &p), "promo: Projects");
        assert!(param_refs("{{ params.topic }} {{params.len}}").contains("len"));
    }
}
