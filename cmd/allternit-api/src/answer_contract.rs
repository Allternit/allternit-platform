//! Data-only answer contract shared with the workspace and cloud artifact API.
//! Schema is generated from allternit-ai/src/lib/openui/answer-contract.ts.
use serde_json::Value;
use std::sync::OnceLock;

pub const BODY_FORMAT: &str = "application/vnd.allternit.answer+json";
pub const MAX_BYTES: usize = 65536;
pub const SCHEMA: &str = include_str!("answer_contract.schema.json");
pub const MANUAL: &str = include_str!("answer_contract.manual.md");

/// The persistence boundary knows the artifact kind even when the caller
/// deliberately sends the old MIME type or omits the template entirely.
pub fn validate_write(kind: &str, format: &str, body: &str) -> Result<(), String> {
    if kind == "card" || format == BODY_FORMAT || format == "application/vnd.allternit.openui" {
        if format != BODY_FORMAT {
            return Err(format!(
                "New cards require {BODY_FORMAT}; saved legacy cards remain readable."
            ));
        }
        validate(body)?;
    }
    Ok(())
}

pub fn validate(code: &str) -> Result<Value, String> {
    if code.len() > MAX_BYTES {
        return Err("Answer exceeds 64 KiB.".into());
    }
    let value: Value = serde_json::from_str(code)
        .map_err(|_| "Expected a versioned answer template with JSON data.")?;
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    let validator = VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(SCHEMA).expect("checked-in answer schema");
        jsonschema::options()
            .offline()
            .should_validate_formats(true)
            .build(&schema)
            .expect("valid offline answer schema")
    });
    let mut pending = vec![(&value, 0)];
    let mut count = 0;
    while let Some((v, depth)) = pending.pop() {
        count += 1;
        if count > 10000 || depth > 32 {
            return Err("Answer data is too deeply nested or too complex.".into());
        }
        match v {
            Value::Array(a) => {
                for child in a {
                    pending.push((child, depth + 1));
                }
            }
            Value::Object(o) => {
                for (key, child) in o {
                    if matches!(key.as_str(), "__proto__" | "prototype" | "constructor") {
                        return Err("Reserved data key.".into());
                    }
                    pending.push((child, depth + 1));
                }
            }
            _ => {}
        }
    }
    if !validator.is_valid(&value) {
        return Err("Answer data does not match a supported template and version.".into());
    }
    let template = value["template"].as_str().unwrap_or_default();
    let data = &value["data"];
    let array = |key: &str| data[key].as_array().expect("validated array");
    match template {
        "chart" => {
            if array("labels").len() != array("values").len() {
                return Err("Chart labels and values must have the same length.".into());
            }
            if data["kind"] == "pie"
                && array("values")
                    .iter()
                    .any(|v| v.as_f64().is_some_and(|n| n < 0.0))
            {
                return Err("Pie values must not be negative.".into());
            }
        }
        "table" => {
            if array("rows").iter().any(|row| {
                row.as_array()
                    .is_some_and(|r| r.len() != array("headers").len())
            }) {
                return Err("Every table row must match its headers.".into());
            }
        }
        "plan" => {
            if data["checked"]
                .as_array()
                .is_some_and(|checked| checked.iter().any(|v| !array("items").contains(v)))
            {
                return Err("Checked items must belong to the plan.".into());
            }
        }
        "form" | "calculator" => {
            let fields = array(if template == "form" {
                "fields"
            } else {
                "inputs"
            });
            let mut ids = std::collections::HashSet::new();
            for field in fields {
                if !ids.insert(field["id"].as_str().unwrap_or_default()) {
                    return Err("Input IDs must be unique.".into());
                }
                if let (Some(min), Some(max)) = (field["min"].as_f64(), field["max"].as_f64()) {
                    if min > max {
                        return Err("Minimum exceeds maximum.".into());
                    }
                }
                if let Some(n) = field["value"].as_f64() {
                    if field["min"].as_f64().is_some_and(|min| n < min)
                        || field["max"].as_f64().is_some_and(|max| n > max)
                    {
                        return Err("Input value is outside its bounds.".into());
                    }
                }
                if field["type"] == "select"
                    && !field["options"]
                        .as_array()
                        .expect("validated options")
                        .contains(&field["value"])
                {
                    return Err("Selected value is not an option.".into());
                }
            }
            if template == "calculator" {
                for result in array("results") {
                    let mut refs = vec![&result["value"]];
                    while let Some(v) = refs.pop() {
                        if v.get("op").is_some() {
                            refs.extend([&v["left"], &v["right"]]);
                        } else if v
                            .get("input")
                            .and_then(Value::as_str)
                            .is_some_and(|name| !ids.contains(name))
                        {
                            return Err("Calculation refers to an unknown input.".into());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(value)
}

/// A safe channel fallback. Never exposes query arguments or executes a tool.
pub fn as_text(code: &str) -> String {
    let Ok(answer) = validate(code) else {
        return "This card could not be shown because its data did not match a supported template."
            .into();
    };
    fn scalar(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Null => "—".into(),
            _ => v.to_string(),
        }
    }
    fn bullets(v: &Value) -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|v| format!("- {}", scalar(v)))
            .collect()
    }
    fn calc(v: &Value, inputs: &Value) -> Result<f64, &'static str> {
        if let Some(n) = v.as_f64() {
            return Ok(n);
        }
        if let Some(id) = v.get("input").and_then(Value::as_str) {
            return inputs
                .as_array()
                .into_iter()
                .flatten()
                .find(|i| i["id"].as_str() == Some(id))
                .and_then(|i| i["value"].as_f64())
                .ok_or("Unknown input");
        }
        let left = calc(&v["left"], inputs)?;
        let right = calc(&v["right"], inputs)?;
        let n = match v["op"].as_str().unwrap_or_default() {
            "add" => left + right,
            "subtract" => left - right,
            "multiply" => left * right,
            "divide" if right == 0.0 => return Err("Cannot divide by zero"),
            "divide" => left / right,
            _ => return Err("Invalid calculation"),
        };
        if n.is_finite() {
            Ok(n)
        } else {
            Err("Result is outside the supported numeric range")
        }
    }
    let d = &answer["data"];
    let mut out = vec![scalar(&answer["title"])];
    if let Some(s) = answer["summary"].as_str() {
        out.push(s.into());
    }
    match answer["template"].as_str().unwrap_or_default() {
        "quick-answer" => out.push(scalar(&d["text"])),
        "comparison" => {
            for column in d["columns"].as_array().into_iter().flatten() {
                out.push(scalar(&column["title"]));
                out.extend(bullets(&column["points"]));
            }
            if let Some(s) = d["recommendation"].as_str() {
                out.push(s.into());
            }
        }
        "decision" => {
            out.push(scalar(&d["question"]));
            out.extend(bullets(&d["options"]));
            if let Some(s) = d["recommendation"].as_str() {
                out.push(s.into());
            }
        }
        "plan" => {
            for item in d["items"].as_array().into_iter().flatten() {
                let checked = d["checked"]
                    .as_array()
                    .is_some_and(|items| items.contains(item));
                out.push(format!(
                    "[{}] {}",
                    if checked { "x" } else { " " },
                    scalar(item)
                ));
            }
        }
        "how-to" => {
            out.extend(bullets(&d["steps"]));
            out.push(format!("Verify: {}", scalar(&d["verification"])));
        }
        "troubleshooting" => {
            out.push(format!("Cause: {}", scalar(&d["cause"])));
            out.extend(bullets(&d["fix"]));
            out.push(format!("Verify: {}", scalar(&d["verify"])));
        }
        "status-report" => {
            for (key, label) in [
                ("done", "Done"),
                ("inProgress", "In progress"),
                ("blocked", "Blocked"),
                ("next", "Next"),
            ] {
                out.push(format!("{label}:"));
                let rows = bullets(&d[key]);
                if rows.is_empty() {
                    out.push("None reported.".into());
                } else {
                    out.extend(rows);
                }
            }
        }
        "research-brief" => {
            for finding in d["findings"].as_array().into_iter().flatten() {
                out.push(format!(
                    "- {} ({})",
                    scalar(&finding["text"]),
                    scalar(&finding["url"])
                ));
            }
            out.push("Open questions:".into());
            out.extend(bullets(&d["openQuestions"]));
        }
        "table" => {
            out.push(
                d["headers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(scalar)
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            for row in d["rows"].as_array().into_iter().flatten() {
                out.push(
                    row.as_array()
                        .into_iter()
                        .flatten()
                        .map(scalar)
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
            }
            if d.get("query").is_some() {
                out.push("Open in Allternit for current connected records.".into());
            }
            out.push(format!("Source: {}", scalar(&d["source"])));
        }
        "chart" => {
            for (label, value) in d["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(d["values"].as_array().into_iter().flatten())
            {
                out.push(format!("{}: {}", scalar(label), scalar(value)));
            }
            out.push(format!("Source: {}", scalar(&d["source"])));
        }
        "form" => {
            for field in d["fields"].as_array().into_iter().flatten() {
                out.push(format!(
                    "{}: {}",
                    scalar(&field["label"]),
                    scalar(&field["value"])
                ));
            }
            out.push("Open in Allternit to fill in this form.".into());
        }
        "calculator" => {
            for result in d["results"].as_array().into_iter().flatten() {
                let value = match calc(&result["value"], &d["inputs"]) {
                    Ok(n) => match result["format"].as_str().unwrap_or("number") {
                        "currency" => format!(
                            "{} {:.2}",
                            result["unit"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("USD"),
                            n
                        ),
                        "percent" => format!("{n}%"),
                        _ => format!(
                            "{n}{}",
                            result["unit"]
                                .as_str()
                                .map(|u| format!(" {u}"))
                                .unwrap_or_default()
                        ),
                    },
                    Err(reason) => reason.into(),
                };
                out.push(format!(
                    "{}: {} ({})",
                    scalar(&result["label"]),
                    value,
                    scalar(&result["note"])
                ));
            }
        }
        _ => {
            fn lines(value: &Value, prefix: &str, out: &mut Vec<String>) {
                match value {
                    Value::Object(map) => for (key, child) in map {
                        if key == "query" || key == "args" { continue; }
                        lines(child, &if prefix.is_empty() { key.clone() } else { format!("{prefix} · {key}") }, out);
                    },
                    Value::Array(items) => for (i, child) in items.iter().enumerate() { lines(child, &format!("{prefix} {}", i+1), out); },
                    _ => out.push(format!("{prefix}: {}", scalar(value))),
                }
            }
            lines(d, "", &mut out);
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_catalog_example_passes_server_validation_and_has_readable_channel_content() {
        let examples: Vec<Value> = serde_json::from_str(include_str!("answer_catalog.examples.json")).unwrap();
        for example in examples {
            let code = example.to_string();
            assert!(validate(&code).is_ok(), "{}", example["template"]);
            assert!(as_text(&code).lines().count() > 1, "{}", example["template"]);
        }
    }

    #[test]
    fn accepts_data_and_refuses_layouts_versions_and_bad_rows() {
        let good = json!({"version":1,"template":"table","title":"Records","data":{"headers":["Name"],"rows":[["A"]],"source":"User input"}});
        assert!(validate(&good.to_string()).is_ok());
        let mut bad = good.clone();
        bad["data"]["rows"] = json!([["A", "B"]]);
        assert!(validate(&bad.to_string()).is_err());
        let mut bad = good.clone();
        bad["version"] = json!(2);
        assert!(validate(&bad.to_string()).is_err());
        let mut bad = good;
        bad["html"] = json!("<script>danger()</script>");
        assert!(validate(&bad.to_string()).is_err());
        assert!(validate("root = Answer(\"Bypass\", [Text(\"x\")])").is_err());
    }

    #[test]
    fn refuses_bad_dimensions_references_and_bounds() {
        assert!(validate(&json!({"version":1,"template":"chart","title":"Chart","data":{"kind":"bar","labels":["A"],"values":[],"source":"User"}}).to_string()).is_err());
        let mut calc = json!({"version":1,"template":"calculator","title":"Calc","data":{"inputs":[{"id":"a","label":"A","value":1,"min":0,"max":2}],"results":[{"label":"Result","value":{"input":"missing"},"note":"Input"}]}});
        assert!(validate(&calc.to_string()).is_err());
        calc["data"]["results"][0]["value"] = json!({"input":"a"});
        assert!(validate(&calc.to_string()).is_ok());
        calc["data"]["inputs"][0]["min"] = json!(3);
        assert!(validate(&calc.to_string()).is_err());
    }

    #[test]
    fn fallback_hides_query_arguments() {
        let body = json!({"version":1,"template":"table","title":"Deals","data":{"headers":["Title"],"rows":[],"source":"CRM","query":{"tool":"crm__list","args":{"private":"do not show"}}}}).to_string();
        assert!(as_text(&body).contains("Deals"));
        assert!(!as_text(&body).contains("do not show"));
    }

    #[test]
    fn write_gate_cannot_be_bypassed_with_a_legacy_or_unrelated_mime() {
        assert!(validate_write("card", "text/plain", "bypass").is_err());
        assert!(validate_write(
            "card",
            "application/vnd.allternit.openui",
            "root = Answer()"
        )
        .is_err());
        assert!(validate_write("page", BODY_FORMAT, "not JSON").is_err());
        assert!(validate_write("page", "text/html", "<p>A requested page</p>").is_ok());
    }

    #[test]
    fn channel_calculators_report_the_result_and_never_fabricate_zero_on_division_errors() {
        let mut answer = json!({"version":1,"template":"calculator","title":"Bill","data":{"inputs":[{"id":"people","label":"People","value":4,"min":0,"max":12}],"results":[{"label":"Each pays","value":{"op":"divide","left":120,"right":{"input":"people"}},"format":"currency","unit":"USD","note":"Your inputs"}]}});
        assert!(as_text(&answer.to_string()).contains("Each pays: USD 30.00 (Your inputs)"));
        answer["data"]["inputs"][0]["value"] = json!(0);
        assert!(as_text(&answer.to_string()).contains("Cannot divide by zero"));
    }

    #[test]
    fn channel_forms_show_labelled_values_instead_of_schema_internals() {
        let answer = json!({"version":1,"template":"form","title":"Contact","data":{"fields":[{"id":"private_id","label":"Name","type":"text","value":"Eoj"}],"submitLabel":"Continue"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("Name: Eoj"));
        assert!(!text.contains("private_id"));
    }
}
