//! Skills over MCP (SEP-2640): a server publishes skills as `skill://`
//! resources. `list_skills` reads them out of `resources/list`; `read_skill`
//! returns the text of one skill file through `resources/read`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{McpClient, McpError, Result};

/// A skill a connected server offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub uri: String,
    pub name: String,
    pub description: String,
}

const SKILL_SCHEME: &str = "skill://";
const MAX_PAGES: usize = 10;

/// The skills inside one `resources/list` result: entries whose URI is
/// `skill://<path>/SKILL.md`. Supporting files of a skill (other names under
/// the same prefix) are not skills of their own. A missing name falls back to
/// the path segment before `SKILL.md`.
pub fn parse_skills(page: &Value) -> Vec<SkillInfo> {
    let Some(items) = page.get("resources").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|r| {
            let uri = r.get("uri")?.as_str()?;
            let rest = uri.strip_prefix(SKILL_SCHEME)?;
            let dir = rest.strip_suffix("/SKILL.md").or_else(|| rest.strip_suffix("/skill.md"))?;
            let name = r
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| dir.rsplit('/').next().unwrap_or(dir));
            Some(SkillInfo {
                uri: uri.to_string(),
                name: name.to_string(),
                description: r.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// The text of a `resources/read` result (all text parts, joined).
pub fn skill_text(result: &Value) -> Option<String> {
    let parts: Vec<&str> = result
        .get("contents")?
        .as_array()?
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

impl McpClient {
    /// `skill://` resources of the server, following `resources/list` pages.
    pub async fn list_skills(&self) -> Result<Vec<SkillInfo>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self.request("resources/list", cursor.as_ref().map(|c| json!({ "cursor": c }))).await?;
            out.extend(parse_skills(&page));
            cursor = page.get("nextCursor").and_then(Value::as_str).map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Read one skill file (`skill://…/SKILL.md`) as text.
    pub async fn read_skill(&self, uri: &str) -> Result<String> {
        if !uri.starts_with(SKILL_SCHEME) {
            return Err(McpError::Protocol(format!("not a skill URI: {uri}")));
        }
        let result = self.request("resources/read", Some(json!({ "uri": uri }))).await?;
        skill_text(&result).ok_or_else(|| McpError::Protocol(format!("skill has no text: {uri}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_list_parses() {
        let page = json!({ "resources": [
            { "uri": "skill://git/commit/SKILL.md", "name": "commit", "description": "Write commits" },
            { "uri": "skill://git/commit/helper.py", "name": "helper" },
            { "uri": "file:///x.txt", "name": "x" },
            { "uri": "skill://review/SKILL.md" }
        ]});
        let skills = parse_skills(&page);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0], SkillInfo { uri: "skill://git/commit/SKILL.md".into(), name: "commit".into(), description: "Write commits".into() });
        assert_eq!(skills[1].name, "review");
        assert_eq!(skills[1].description, "");
    }

    #[test]
    fn skill_text_joins_text_parts() {
        let r = json!({ "contents": [{ "uri": "u", "text": "a" }, { "uri": "u", "blob": "zz" }, { "uri": "u", "text": "b" }] });
        assert_eq!(skill_text(&r).as_deref(), Some("a\nb"));
        assert_eq!(skill_text(&json!({ "contents": [] })), None);
    }
}
