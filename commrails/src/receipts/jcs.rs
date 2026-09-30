//! RFC 8785 JSON Canonicalization Scheme (JCS) and `sha256:<hex>` hashing.
//!
//! Implemented locally (no extra dependency). Supported subset: everything
//! serde_json can represent, except that numbers must be exactly representable
//! as ECMAScript doubles: integers with |n| <= 2^53 and finite floats in the
//! range where ES6 `Number::toString` never uses exponent notation. Anything
//! else is rejected (fail closed) rather than canonicalized ambiguously.

use anyhow::{bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_SAFE: u64 = 1 << 53;

pub fn canonicalize(v: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(v, &mut out)?;
    Ok(out)
}

pub fn sha256_tagged(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// `sha256:<hex>` of the JCS form of `v`.
pub fn hash_value(v: &Value) -> Result<String> {
    Ok(sha256_tagged(canonicalize(v)?.as_bytes()))
}

fn write_value(v: &Value, out: &mut String) -> Result<()> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(x, out)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            // RFC 8785 §3.2.3: sort by UTF-16 code units.
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(&m[*k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(n: &serde_json::Number, out: &mut String) -> Result<()> {
    if let Some(u) = n.as_u64() {
        if u > MAX_SAFE {
            bail!("JCS: integer {u} exceeds 2^53");
        }
        out.push_str(&u.to_string());
    } else if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_SAFE {
            bail!("JCS: integer {i} exceeds 2^53");
        }
        out.push_str(&i.to_string());
    } else {
        let f = n.as_f64().unwrap_or(f64::NAN);
        if !f.is_finite() {
            bail!("JCS: non-finite number");
        }
        if f == 0.0 {
            out.push('0'); // also covers -0
            return Ok(());
        }
        let a = f.abs();
        if !(1e-6..1e21).contains(&a) {
            bail!("JCS: float {f} needs exponent notation (unsupported)");
        }
        // Rust's Display is shortest-round-trip and never uses exponents,
        // matching ES6 in this range; integral floats print without ".0".
        out.push_str(&format!("{f}"));
    }
    Ok(())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{09}' => out.push_str("\\t"),
            '\u{0a}' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{0d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rfc8785_sorting_and_literals() {
        // RFC 8785 §3.2.3 example (keys sorted by UTF-16 code units).
        let v: Value = serde_json::from_str(
            r#"{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh","1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis"}"#,
        )
        .unwrap();
        let c = canonicalize(&v).unwrap();
        let order: Vec<&str> = ["\"\\r\"", "\"1\"", "\"\u{80}\"", "\"\u{f6}\"", "\"\u{20ac}\"", "\"\u{1f600}\"", "\"\u{fb33}\""].to_vec();
        let mut last = 0;
        for k in order {
            let p = c.find(k).unwrap_or_else(|| panic!("missing {k}"));
            assert!(p >= last, "key order wrong at {k}: {c}");
            last = p;
        }
    }

    #[test]
    fn rfc8785_structure_example() {
        // RFC 8785 §3.2.2 example.
        let v: Value = serde_json::from_str(
            r#"{"numbers":[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001],"string":"\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/","literals":[null,true,false]}"#,
        )
        .unwrap();
        // 1E30 and 1e-27 need exponent notation: we fail closed.
        assert!(canonicalize(&v).is_err());
        // serde_json's default float parser is not correctly rounded, so build the
        // RFC's 333333333.33333329 from an exact rustc literal instead of parsing it.
        let mut ok: Value = serde_json::from_str(
            r#"{"numbers":[0,4.50,2e-3],"string":"\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/","literals":[null,true,false]}"#,
        )
        .unwrap();
        ok["numbers"][0] = Value::from(333333333.33333329f64);
        assert_eq!(
            canonicalize(&ok).unwrap(),
            "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,4.5,0.002],\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
        );
    }

    #[test]
    fn numbers_and_hash_format() {
        assert_eq!(canonicalize(&json!([1, -2, 0, 1.0, 10.5])).unwrap(), "[1,-2,0,1,10.5]");
        assert!(canonicalize(&json!(9007199254740993u64)).is_err());
        let h = hash_value(&json!({"b":1,"a":2})).unwrap();
        assert_eq!(h, sha256_tagged(b"{\"a\":2,\"b\":1}"));
        assert!(h.starts_with("sha256:") && h.len() == 71);
    }
}
