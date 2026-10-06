//! Tool bridge for integrating MCP with Allternit tool registry

/// Format an MCP tool name with server prefix
///
/// Format: "mcp__{server_name}__{tool_name}"
pub fn format_tool_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_name(server_name),
        sanitize_name(tool_name)
    )
}

/// Parse a prefixed tool name
///
/// Returns (server_name, tool_name) if valid
pub fn parse_tool_name(prefixed: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = prefixed.splitn(3, "__").collect();
    if parts.len() == 3 && parts[0] == "mcp" {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tool_name() {
        assert_eq!(
            format_tool_name("my-server", "my-tool"),
            "mcp__my_server__my_tool"
        );
    }

    #[test]
    fn test_parse_tool_name() {
        let result = parse_tool_name("mcp__my_server__my_tool");
        assert_eq!(result, Some(("my_server".to_string(), "my_tool".to_string())));
    }

    #[test]
    fn test_parse_invalid_tool_name() {
        assert_eq!(parse_tool_name("invalid"), None);
        assert_eq!(parse_tool_name("not__mcp__format"), None);
    }
}
