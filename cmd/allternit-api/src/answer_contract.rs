//! Data-only answer contract shared with the workspace and cloud artifact API.
//! Schema is generated from allternit-ai/src/lib/openui/answer-contract.ts.
use serde_json::Value;
use std::sync::OnceLock;

pub const BODY_FORMAT: &str = "application/vnd.allternit.answer+json";
pub const MAX_BYTES: usize = 65536;
pub const SCHEMA: &str = include_str!("answer_contract.schema.json");
pub const MANUAL: &str = include_str!("answer_contract.manual.md");
/// Where the catalog part of MANUAL starts (allternit-ai `CATALOG_MANUAL_HEADING`).
const CATALOG_HEADING: &str = "\n### Catalog templates";

/// The manual for one gateway turn: the envelope, core templates and rules
/// always; from the catalog only the line for the template the policy picked.
/// When the manual has no catalog heading, or the picked template has no line
/// in either part, the whole manual goes (never a turn missing its template).
pub fn manual_for(template: Option<&str>) -> std::borrow::Cow<'static, str> {
    let Some((core, catalog)) = MANUAL.split_once(CATALOG_HEADING) else {
        return MANUAL.into();
    };
    let core = core.trim_end();
    let Some(id) = template else {
        return core.into();
    };
    let mut lines = catalog.lines();
    let heading = lines.next().unwrap_or_default();
    let prefix = format!("{id}: ");
    match lines.find(|line| line.starts_with(&prefix)) {
        Some(line) => format!("{core}\n\n{}{heading}\n{line}", CATALOG_HEADING.trim_start()).into(),
        None if core.contains(&format!("{id}: {{")) => core.into(),
        None => MANUAL.into(),
    }
}

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
    /// A number as people write it: `21`, not `21.0`.
    fn num(v: &Value) -> String {
        match v.as_f64() {
            Some(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", n as i64),
            Some(n) => format!("{n}"),
            None => scalar(v),
        }
    }
    /// `Credit: …`, with the credit link when the card has one.
    fn credit(d: &Value) -> Option<String> {
        let text = d["credit"].as_str()?;
        Some(match d["creditUrl"].as_str() {
            Some(url) => format!("Credit: {text} ({url})"),
            None => format!("Credit: {text}"),
        })
    }
    /// The present string fields of `v`, in order, joined with ` · `.
    fn joined(v: &Value, keys: &[&str]) -> String {
        keys.iter()
            .filter_map(|k| v[*k].as_str().filter(|s| !s.is_empty()))
            .collect::<Vec<_>>()
            .join(" · ")
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
        "weather" => {
            let unit = d["unit"].as_str().unwrap_or("C");
            out.push(format!("{} · {}", scalar(&d["location"]), scalar(&d["observedAt"])));
            out.push(format!("{}°{unit}, {}", num(&d["temperature"]), scalar(&d["conditions"])));
            out.push(format!("Humidity {}% · Wind {}", num(&d["humidity"]), scalar(&d["wind"])));
            let forecast = d["forecast"].as_array().into_iter().flatten().collect::<Vec<_>>();
            if !forecast.is_empty() {
                out.push("Forecast:".into());
                for day in forecast {
                    let mut row = format!(
                        "- {}: {}°/{}°{unit}, {}",
                        scalar(&day["day"]),
                        num(&day["high"]),
                        num(&day["low"]),
                        scalar(&day["conditions"])
                    );
                    if let Some(p) = day["precipitation"].as_str() {
                        row.push_str(&format!(", {p}"));
                    }
                    out.push(row);
                }
            }
            out.push(format!("Source: {}", scalar(&d["source"])));
            out.extend(credit(d));
        }
        "image-gallery" => {
            for image in d["images"].as_array().into_iter().flatten() {
                let label = image["caption"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| image["alt"].as_str().unwrap_or_default());
                out.push(format!("- {label}: {}", scalar(&image["url"])));
                let mut about = Vec::new();
                if let Some(c) = image["credit"].as_str() {
                    about.push(format!("Credit: {c}"));
                }
                if let Some(l) = image["license"].as_str() {
                    about.push(format!("License: {l}"));
                }
                if let Some(u) = image["sourceUrl"].as_str() {
                    about.push(format!("Source: {u}"));
                }
                if !about.is_empty() {
                    out.push(format!("  {}", about.join(" · ")));
                }
            }
        }
        "time-zones" => {
            out.push(scalar(&d["reference"]));
            for zone in d["zones"].as_array().into_iter().flatten() {
                out.push(format!(
                    "- {} ({}): {}, {} · {}",
                    scalar(&zone["place"]),
                    scalar(&zone["zone"]),
                    scalar(&zone["time"]),
                    scalar(&zone["date"]),
                    scalar(&zone["offset"])
                ));
            }
            out.extend(credit(d));
        }
        "conversion" => {
            let (from, to) = (&d["from"], &d["to"]);
            out.push(format!(
                "{} {} = {} {}",
                num(&from["value"]),
                scalar(&from["unit"]),
                num(&to["value"]),
                scalar(&to["unit"])
            ));
            let mut rate = Vec::new();
            if d.get("rate").is_some() {
                rate.push(format!(
                    "Rate: 1 {} = {} {}",
                    scalar(&from["unit"]),
                    num(&d["rate"]),
                    scalar(&to["unit"])
                ));
            }
            if let Some(as_of) = d["asOf"].as_str() {
                rate.push(format!("As of {as_of}"));
            }
            if !rate.is_empty() {
                out.push(rate.join(" · "));
            }
            let also = d["also"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| format!("{} {}", num(&v["value"]), scalar(&v["unit"])))
                .collect::<Vec<_>>();
            if !also.is_empty() {
                out.push(format!("Also: {}", also.join(", ")));
            }
            out.extend(credit(d));
        }
        "link-preview" => {
            let head = joined(d, &["title", "siteName"]);
            out.push(head);
            if let Some(desc) = d["description"].as_str().filter(|s| !s.trim().is_empty()) {
                out.push(desc.into());
            }
            out.push(scalar(&d["url"]));
            if let Some(p) = d["published"].as_str() {
                out.push(format!("Published: {p}"));
            }
            out.extend(credit(d));
        }
        "places" => {
            let near = d["near"].as_str().map(|n| format!(" near {n}")).unwrap_or_default();
            out.push(format!("{}{near}", scalar(&d["query"])));
            let places = d["places"].as_array().into_iter().flatten().collect::<Vec<_>>();
            if places.is_empty() {
                out.push("No places found.".into());
            }
            for place in places {
                let mut row = format!("- {}", joined(place, &["name", "address", "category", "distance"]));
                if let Some(url) = place["url"].as_str() {
                    row.push_str(&format!(" ({url})"));
                }
                out.push(row);
            }
            out.extend(credit(d));
        }
        // A draft to read and copy. It never tells the reader to send anything:
        // the card has no send action, and neither does its channel text.
        "writing" => {
            if let Some(to) = d["to"].as_str() {
                out.push(format!("To: {to}"));
            }
            if let Some(subject) = d["subject"].as_str() {
                out.push(format!("Subject: {subject}"));
            }
            out.push(String::new());
            out.push(scalar(&d["body"]));
            for alt in d["alternatives"].as_array().into_iter().flatten() {
                out.push(String::new());
                out.push(format!("{}:", scalar(&alt["label"])));
                out.push(scalar(&alt["body"]));
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
    fn gateway_manual_carries_only_the_picked_template() {
        let core = manual_for(None);
        assert!(core.starts_with("## Enforced answer templates"));
        assert!(core.contains("tagged `openui`"));
        assert!(core.contains("calculator: {inputs:"));
        assert!(!core.contains("### Catalog templates"));
        assert!(core.len() < MANUAL.len() / 2);
        assert_eq!(manual_for(Some("comparison")), core);
        let weather = manual_for(Some("weather"));
        assert!(weather.starts_with(&*core));
        assert!(weather.contains("\nweather: "));
        assert!(!weather.contains("\nfaq: "));
        // Unknown template: the whole manual, never a turn without its template.
        assert_eq!(manual_for(Some("no-such-template")), MANUAL);
        // Every catalog template has its own line (the split never falls back for a real one).
        let examples: Vec<Value> = serde_json::from_str(include_str!("answer_catalog.examples.json")).unwrap();
        for example in examples {
            let id = example["template"].as_str().unwrap();
            assert_ne!(manual_for(Some(id)), MANUAL, "{id}");
        }
    }

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

    #[test]
    fn channel_weather_shows_the_forecast_and_credit() {
        let answer = json!({"version":1,"template":"weather","title":"Chicago now","data":{"location":"Chicago, IL","observedAt":"2:00 PM","temperature":21,"unit":"C","conditions":"Partly cloudy","humidity":46,"wind":"NE 11 km/h","source":"MET Norway","forecast":[{"day":"Mon","high":23,"low":14,"conditions":"Sunny","precipitation":"0 mm"},{"day":"Tue","high":19.5,"low":12,"conditions":"Showers"}],"credit":"MET Norway (CC BY 4.0)","creditUrl":"https://api.met.no/"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("21°C, Partly cloudy"), "{text}");
        assert!(text.contains("Humidity 46% · Wind NE 11 km/h"), "{text}");
        assert!(text.contains("- Mon: 23°/14°C, Sunny, 0 mm"), "{text}");
        assert!(text.contains("- Tue: 19.5°/12°C, Showers"), "{text}");
        assert!(text.contains("Credit: MET Norway (CC BY 4.0) (https://api.met.no/)"), "{text}");
        // Without a forecast there is no empty "Forecast:" heading.
        let mut bare = answer.clone();
        bare["data"].as_object_mut().unwrap().remove("forecast");
        assert!(!as_text(&bare.to_string()).contains("Forecast:"));
    }

    #[test]
    fn channel_image_gallery_keeps_credit_license_and_source() {
        let answer = json!({"version":1,"template":"image-gallery","title":"Lake","data":{"images":[{"url":"https://img.example/1.jpg","alt":"A pier","caption":"Morning pier","credit":"Jane Doe","license":"CC BY 4.0","sourceUrl":"https://commons.example/1"},{"url":"https://img.example/2.jpg","alt":"A skyline","caption":""}]}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("- Morning pier: https://img.example/1.jpg"), "{text}");
        assert!(text.contains("  Credit: Jane Doe · License: CC BY 4.0 · Source: https://commons.example/1"), "{text}");
        assert!(text.contains("- A skyline: https://img.example/2.jpg"), "{text}");
    }

    #[test]
    fn channel_time_zones_list_every_zone() {
        let answer = json!({"version":1,"template":"time-zones","title":"Your call","data":{"reference":"When it is 9:00 AM in Chicago","zones":[{"place":"Chicago","zone":"America/Chicago","time":"9:00 AM","date":"Tue, Nov 17","offset":"UTC-06:00"},{"place":"Tokyo","zone":"Asia/Tokyo","time":"12:00 AM","date":"Wed, Nov 18","offset":"UTC+09:00"}],"credit":"IANA time zone database"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("When it is 9:00 AM in Chicago"), "{text}");
        assert!(text.contains("- Tokyo (Asia/Tokyo): 12:00 AM, Wed, Nov 18 · UTC+09:00"), "{text}");
        assert!(text.contains("Credit: IANA time zone database"), "{text}");
    }

    #[test]
    fn channel_conversion_states_the_result_rate_and_credit() {
        let answer = json!({"version":1,"template":"conversion","title":"USD to EUR","data":{"kind":"currency","from":{"value":100,"unit":"USD"},"to":{"value":92.4,"unit":"EUR"},"rate":0.924,"asOf":"Nov 16, 2026","also":[{"value":78.9,"unit":"GBP"},{"value":15410,"unit":"JPY"}],"credit":"Frankfurter (ECB)"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("100 USD = 92.4 EUR"), "{text}");
        assert!(text.contains("Rate: 1 USD = 0.924 EUR · As of Nov 16, 2026"), "{text}");
        assert!(text.contains("Also: 78.9 GBP, 15410 JPY"), "{text}");
        assert!(text.contains("Credit: Frankfurter (ECB)"), "{text}");
        let unit = json!({"version":1,"template":"conversion","title":"Miles","data":{"kind":"unit","from":{"value":5,"unit":"mi"},"to":{"value":8.04672,"unit":"km"},"credit":"Local maths"}});
        let text = as_text(&unit.to_string());
        assert!(text.contains("5 mi = 8.04672 km") && !text.contains("Rate:"), "{text}");
    }

    #[test]
    fn channel_link_preview_shows_the_page_and_its_url() {
        let answer = json!({"version":1,"template":"link-preview","title":"Shared article","data":{"url":"https://example.com/notes","title":"Field notes","siteName":"Example Journal","description":"A calm summary.","published":"Nov 12, 2026","credit":"Preview from example.com"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("Field notes · Example Journal"), "{text}");
        assert!(text.contains("A calm summary."), "{text}");
        assert!(text.contains("https://example.com/notes"), "{text}");
        assert!(text.contains("Published: Nov 12, 2026"), "{text}");
        assert!(text.contains("Credit: Preview from example.com"), "{text}");
    }

    #[test]
    fn channel_places_list_name_address_and_link() {
        let answer = json!({"version":1,"template":"places","title":"Coffee","data":{"query":"Coffee","near":"Riverwalk","places":[{"name":"Harbor Beans","address":"120 Example Ave","category":"Cafe","distance":"250 m","url":"https://example.com/map/1"},{"name":"Alder","address":"9 Alder Ct"}],"credit":"© OpenStreetMap contributors","creditUrl":"https://www.openstreetmap.org/copyright"}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("Coffee near Riverwalk"), "{text}");
        assert!(text.contains("- Harbor Beans · 120 Example Ave · Cafe · 250 m (https://example.com/map/1)"), "{text}");
        assert!(text.contains("- Alder · 9 Alder Ct"), "{text}");
        assert!(text.contains("Credit: © OpenStreetMap contributors (https://www.openstreetmap.org/copyright)"), "{text}");
        let empty = json!({"version":1,"template":"places","title":"Coffee","data":{"query":"Coffee","places":[],"credit":"OSM"}});
        assert!(as_text(&empty.to_string()).contains("No places found."));
    }

    #[test]
    fn channel_writing_is_the_draft_with_subject_and_to_and_never_a_send_instruction() {
        let answer = json!({"version":1,"template":"writing","title":"Reply to Maya","data":{"variant":"email","to":"Maya Chen","subject":"Re: Friday","body":"Hi Maya,\n\nFriday at 2 PM works.\n\nThanks,\nJordan","alternatives":[{"label":"Shorter","body":"Friday at 2 works."}]}});
        let text = as_text(&answer.to_string());
        assert!(text.contains("To: Maya Chen\nSubject: Re: Friday\n\nHi Maya,\n\nFriday at 2 PM works."), "{text}");
        assert!(text.contains("Shorter:\nFriday at 2 works."), "{text}");
        assert!(!text.to_lowercase().contains("send"), "{text}");
        let note = json!({"version":1,"template":"writing","title":"Note","data":{"variant":"note","body":"Buy milk."}});
        let text = as_text(&note.to_string());
        assert!(text.contains("Buy milk.") && !text.contains("To:") && !text.contains("Subject:"), "{text}");
        assert!(!text.to_lowercase().contains("send"), "{text}");
    }
}
