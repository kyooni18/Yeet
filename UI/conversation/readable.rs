//! Readable fallback for structured tools without a specialized rich projection.
//! Preserve recorded values literally; JSON syntax belongs in raw inspection.
use super::types::DetailSection;
use serde_json::Value;

fn label(key: &str) -> String {
    let mut out = String::new();
    for (i, ch) in key.chars().enumerate() {
        if ch == '_' || ch == '-' {
            out.push(' ');
        } else {
            if ch.is_uppercase() && i > 0 {
                out.push(' ');
            }
            if i == 0 {
                out.extend(ch.to_uppercase());
            } else {
                out.push(ch);
            }
        }
    }
    out
}
fn encoded(value: &Value) -> Option<Value> {
    let text = value.as_str()?.trim();
    if !text.starts_with('{') && !text.starts_with('[') {
        return None;
    }
    serde_json::from_str(text)
        .ok()
        .filter(|value: &Value| value.is_object() || value.is_array())
}
fn scalar(value: &Value) -> Option<String> {
    if encoded(value).is_some() {
        return None;
    }
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(if *value { "Yes" } else { "No" }.into()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null => Some("Not reported".into()),
        _ => None,
    }
}
fn push(out: &mut Vec<DetailSection>, title: &str, content: String) {
    if !content.is_empty() && out.len() < 80 {
        out.push(DetailSection {
            title: title.into(),
            content,
            monospaced: false,
            is_error: false,
            has_background: false,
        });
    }
}
fn walk(value: &Value, title: &str, depth: usize, out: &mut Vec<DetailSection>) {
    if out.len() >= 79 || depth >= 8 {
        push(
            out,
            "Preview limit",
            "More structured content is available in raw input and output.".into(),
        );
        return;
    }
    if let Some(decoded) = encoded(value) {
        walk(&decoded, title, depth + 1, out);
        return;
    }
    match value {
        Value::Object(fields) => {
            let body = fields
                .iter()
                .filter_map(|(key, value)| {
                    scalar(value).map(|value| format!("{}: {}", label(key), value))
                })
                .collect::<Vec<_>>()
                .join("\n");
            push(out, title, body);
            for (key, value) in fields.iter().filter(|(_, value)| {
                value.is_object() || value.is_array() || encoded(value).is_some()
            }) {
                walk(value, &format!("{title} · {}", label(key)), depth + 1, out);
                if out.len() >= 80 {
                    break;
                }
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                push(out, title, "No items".into());
            }
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{title} · {}", i + 1), depth + 1, out);
                if out.len() >= 80 {
                    break;
                }
            }
        }
        _ => {
            push(out, title, scalar(value).unwrap_or_default());
        }
    }
}
pub(super) fn sections(title: &str, content: &str) -> Vec<DetailSection> {
    let mut out = Vec::new();
    match serde_json::from_str::<Value>(content) {
        Ok(value) => walk(&value, title, 0, &mut out),
        Err(_) => push(&mut out, title, content.trim().into()),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_results_are_readable_bounded_and_literal() {
        let sections = sections(
            "Matches",
            r#"{"query":"<img src=x>","matches":[{"path":"src/main.rs","line":12,"content":"let x = 1;"}],"truncated":false}"#,
        );
        assert!(sections[0].content.contains("Query: <img src=x>"));
        assert!(sections[1].content.contains("Path: src/main.rs"));
        assert!(sections[1].content.contains("Line: 12"));
        assert!(!sections[0].content.contains("{\""));
        let large = serde_json::to_string(&vec![serde_json::json!({"a": 1}); 1000]).unwrap();
        assert!(super::sections("Result", &large).len() <= 80);
    }
}
