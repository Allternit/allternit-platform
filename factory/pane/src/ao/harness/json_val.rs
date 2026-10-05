//! Order-preserving JSON value: strict parser + ECMAScript-compatible pretty
//! printer.
//!
//! Why this exists: the JS harness rewrites MCP config files with
//! `JSON.parse` → mutate → `JSON.stringify(obj, null, 2)`. `JSON.parse`
//! preserves object key order (file order; duplicate keys keep first position,
//! last value), and the pretty printer normalizes the WHOLE file. A global
//! `serde_json` `preserve_order` feature could match this, but it would also
//! change engine-internal JSON rewrites (`integration/claude_settings.rs`,
//! `integration/targets.rs`) — off-limits for P4. This module is scoped to the
//! harness and byte-replicates the JS pipeline:
//!
//!   - Strict parse (no comments, no trailing commas, no single quotes — the
//!     opencode `.jsonc` fragility is reproduced deliberately: a commented
//!     file fails to parse and is treated as missing, exactly like the JS).
//!   - Numbers keep their raw lexeme. JS *normalizes* number lexemes
//!     (`1e3` → `1000`, `1.50` → `1.5`); accepted deviation (documented in
//!     docs/programs/ao/AO_HARNESS_PORT_NOTES.md): lexemes are reprinted verbatim. No
//!     manifest target contains numeric values, and a lexeme that round-trips
//!     is unaffected.
//!   - String escaping matches `JSON.stringify`: `"` `\` `\b` `\f` `\n` `\r`
//!     `\t`, other C0 controls as `\u00xx` (lowercase hex), everything else
//!     (including non-ASCII) literal.
//!   - Pretty format matches `JSON.stringify(obj, null, 2)`: 2-space indent,
//!     `": "` after keys, empty containers inline as `{}` / `[]`.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JVal {
    Null,
    Bool(bool),
    /// Raw number lexeme, preserved verbatim.
    Num(String),
    Str(String),
    Arr(Vec<JVal>),
    /// Key-ordered object (insertion order preserved).
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    pub(crate) fn js_truthy(&self) -> bool {
        match self {
            JVal::Null => false,
            JVal::Bool(b) => *b,
            JVal::Num(lex) => lex.parse::<f64>().map(|v| v != 0.0).unwrap_or(true),
            JVal::Str(s) => !s.is_empty(),
            JVal::Arr(_) | JVal::Obj(_) => true,
        }
    }

    /// `obj[key]` — JS property lookup on objects; arrays and scalars yield
    /// undefined (None), matching `typeof [] === "object"` only superficially:
    /// `[]["mcpServers"]` is undefined in JS too.
    pub(crate) fn get<'a>(&'a self, key: &str) -> Option<&'a JVal> {
        match self {
            JVal::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `obj[key] = value`: replaces the value in place (first-position key
    /// kept, matching `JSON.parse` duplicate-key semantics) or appends a new
    /// key at the end (matching JS assignment order).
    pub(crate) fn set(&mut self, key: &str, value: JVal) {
        if let JVal::Obj(fields) = self {
            if let Some(slot) = fields.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                fields.push((key.to_string(), value));
            }
        }
    }

    /// `delete obj[key]`.
    pub(crate) fn remove(&mut self, key: &str) {
        if let JVal::Obj(fields) = self {
            fields.retain(|(k, _)| k != key);
        }
    }

    pub(crate) fn str(s: impl Into<String>) -> JVal {
        JVal::Str(s.into())
    }
}

/// Byte-exact `JSON.stringify(value, null, 2)`.
pub(crate) fn print_pretty(value: &JVal) -> String {
    let mut out = String::new();
    print_into(value, 0, &mut out);
    out
}

/// Compact `JSON.stringify(value)` (no indent) — used by the TOML block
/// renderer (`args = <JSON.stringify(args)>`).
pub(crate) fn print_compact(value: &JVal) -> String {
    let mut out = String::new();
    print_compact_into(value, &mut out);
    out
}

fn print_compact_into(value: &JVal, out: &mut String) {
    match value {
        JVal::Null => out.push_str("null"),
        JVal::Bool(true) => out.push_str("true"),
        JVal::Bool(false) => out.push_str("false"),
        JVal::Num(lex) => out.push_str(lex),
        JVal::Str(s) => print_string(s, out),
        JVal::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                print_compact_into(item, out);
            }
            out.push(']');
        }
        JVal::Obj(fields) => {
            out.push('{');
            for (i, (key, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                print_string(key, out);
                out.push(':');
                print_compact_into(val, out);
            }
            out.push('}');
        }
    }
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn print_into(value: &JVal, level: usize, out: &mut String) {
    match value {
        JVal::Null => out.push_str("null"),
        JVal::Bool(true) => out.push_str("true"),
        JVal::Bool(false) => out.push_str("false"),
        JVal::Num(lex) => out.push_str(lex),
        JVal::Str(s) => print_string(s, out),
        JVal::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, level + 1);
                print_into(item, level + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, level);
            out.push(']');
        }
        JVal::Obj(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (key, val)) in fields.iter().enumerate() {
                indent(out, level + 1);
                print_string(key, out);
                out.push_str(": ");
                print_into(val, level + 1, out);
                if i + 1 < fields.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, level);
            out.push('}');
        }
    }
}

/// `JSON.stringify` string escaping.
fn print_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub(crate) fn parse(text: &str) -> Result<JVal, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("trailing characters at offset {}", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected {} at offset {}", b as char, self.pos))
        }
    }

    fn parse_value(&mut self) -> Result<JVal, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(JVal::Str(self.parse_string()?)),
            Some(b't') => self.parse_lit("true", JVal::Bool(true)),
            Some(b'f') => self.parse_lit("false", JVal::Bool(false)),
            Some(b'n') => self.parse_lit("null", JVal::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(format!("unexpected character at offset {}", self.pos)),
        }
    }

    fn parse_lit(&mut self, lit: &str, value: JVal) -> Result<JVal, String> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(value)
        } else {
            Err(format!("invalid literal at offset {}", self.pos))
        }
    }

    fn parse_number(&mut self) -> Result<JVal, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // Integer part: 0 | [1-9][0-9]*
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(format!("invalid number at offset {start}")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(format!("invalid number at offset {start}"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(format!("invalid number at offset {start}"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let lexeme = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "invalid utf-8 in number".to_string())?;
        Ok(JVal::Num(lexeme.to_string()))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{08}'),
                        Some(b'f') => out.push('\u{0C}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let cp = self.parse_hex4()?;
                            // Surrogate pair handling, matching JSON.parse.
                            if (0xD800..0xDC00).contains(&cp) {
                                if self.peek() == Some(b'\\')
                                    && self.bytes.get(self.pos + 1) == Some(&b'u')
                                {
                                    self.pos += 1;
                                    let low = self.parse_hex4()?;
                                    if !(0xDC00..0xE000).contains(&low) {
                                        return Err("invalid low surrogate".to_string());
                                    }
                                    let combined = 0x10000
                                        + ((cp - 0xD800) << 10)
                                        + (low - 0xDC00);
                                    out.push(
                                        char::from_u32(combined)
                                            .ok_or("invalid surrogate pair")?,
                                    );
                                } else {
                                    return Err("lone high surrogate".to_string());
                                }
                            } else if (0xDC00..0xE000).contains(&cp) {
                                return Err("lone low surrogate".to_string());
                            } else {
                                out.push(char::from_u32(cp).ok_or("invalid codepoint")?);
                            }
                            continue;
                        }
                        _ => return Err(format!("invalid escape at offset {}", self.pos)),
                    }
                    self.pos += 1;
                }
                // Raw control characters are rejected by JSON.parse.
                Some(c) if c < 0x20 => {
                    return Err(format!("raw control character at offset {}", self.pos));
                }
                Some(_) => {
                    // Copy one UTF-8 encoded char.
                    let rest = &self.bytes[self.pos..];
                    let len = utf8_len(rest[0]);
                    let end = (self.pos + len).min(self.bytes.len());
                    let chunk = std::str::from_utf8(&self.bytes[self.pos..end])
                        .map_err(|_| "invalid utf-8 in string".to_string())?;
                    out.push_str(chunk);
                    self.pos = end;
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        // self.pos is on 'u'
        let hex = std::str::from_utf8(
            self.bytes
                .get(self.pos + 1..self.pos + 5)
                .ok_or("truncated \\u escape")?,
        )
        .map_err(|_| "invalid \\u escape")?;
        u32::from_str_radix(hex, 16).map_err(|_| "invalid \\u escape".to_string())
    }

    fn parse_array(&mut self) -> Result<JVal, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JVal::Arr(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JVal::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at offset {}", self.pos)),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JVal, String> {
        self.expect(b'{')?;
        let mut fields: Vec<(String, JVal)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JVal::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value()?;
            // JSON.parse: duplicate keys keep first position, last value.
            if let Some(slot) = fields.iter_mut().find(|(k, _)| *k == key) {
                slot.1 = value;
            } else {
                fields.push((key, value));
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JVal::Obj(fields));
                }
                _ => return Err(format!("expected ',' or '}}' at offset {}", self.pos)),
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_print_matches_json_stringify() {
        // Shapes captured live from node JSON.stringify(obj, null, 2).
        let obj = JVal::Obj(vec![
            ("version".to_string(), JVal::Num("1".to_string())),
            ("syncedAt".to_string(), JVal::str("2026-09-10T00:00:00.000Z")),
            (
                "skills".to_string(),
                JVal::Obj(vec![
                    ("deploy".to_string(), JVal::str("ab12")),
                    ("invoice".to_string(), JVal::str("cd34")),
                ]),
            ),
        ]);
        let expected = "{\n  \"version\": 1,\n  \"syncedAt\": \"2026-09-10T00:00:00.000Z\",\n  \"skills\": {\n    \"deploy\": \"ab12\",\n    \"invoice\": \"cd34\"\n  }\n}";
        assert_eq!(print_pretty(&obj), expected);
    }

    #[test]
    fn empty_containers_inline() {
        assert_eq!(print_pretty(&JVal::Obj(vec![])), "{}");
        assert_eq!(print_pretty(&JVal::Arr(vec![])), "[]");
        let nested = JVal::Obj(vec![
            ("a".to_string(), JVal::Obj(vec![])),
            ("b".to_string(), JVal::Arr(vec![])),
        ]);
        assert_eq!(print_pretty(&nested), "{\n  \"a\": {},\n  \"b\": []\n}");
    }

    #[test]
    fn string_escaping_matches_json_stringify() {
        let s = JVal::str("quote\" back\\ \u{08}\u{0C}\n\r\t\u{01}/ é");
        assert_eq!(
            print_pretty(&s),
            "\"quote\\\" back\\\\ \\b\\f\\n\\r\\t\\u0001/ é\""
        );
    }

    #[test]
    fn parses_and_preserves_key_order() {
        let text = "{\"b\": 1, \"a\": {\"y\": true, \"x\": null}, \"c\": [1, \"two\"]}";
        let value = parse(text).unwrap();
        let printed = print_pretty(&value);
        assert_eq!(
            printed,
            "{\n  \"b\": 1,\n  \"a\": {\n    \"y\": true,\n    \"x\": null\n  },\n  \"c\": [\n    1,\n    \"two\"\n  ]\n}"
        );
    }

    #[test]
    fn duplicate_keys_keep_position_last_value() {
        let value = parse("{\"a\": 1, \"b\": 2, \"a\": 3}").unwrap();
        assert_eq!(print_pretty(&value), "{\n  \"a\": 3,\n  \"b\": 2\n}");
    }

    #[test]
    fn rejects_non_strict_json() {
        for bad in [
            "{\"a\": 1,}",
            "{'a': 1}",
            "{\"a\": /* c */ 1}",
            "{\"a\": 01}",
            "{\"a\": +1}",
            "[1,]",
            "{\"a\": \"raw\nnewline\"}",
            "not json",
            "{\"a\": 1} extra",
        ] {
            assert!(parse(bad).is_err(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn number_lexemes_preserved() {
        let value = parse("{\"a\": 1e3, \"b\": 1.50, \"c\": -0.25}").unwrap();
        assert_eq!(print_pretty(&value), "{\n  \"a\": 1e3,\n  \"b\": 1.50,\n  \"c\": -0.25\n}");
    }

    #[test]
    fn set_appends_at_end_and_replaces_in_place() {
        let mut value = JVal::Obj(vec![("a".to_string(), JVal::Num("1".to_string()))]);
        value.set("b", JVal::Bool(true));
        value.set("a", JVal::Num("2".to_string()));
        assert_eq!(
            print_pretty(&value),
            "{\n  \"a\": 2,\n  \"b\": true\n}"
        );
    }
}
