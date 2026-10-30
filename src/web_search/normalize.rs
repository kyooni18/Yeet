use super::*;

pub(super) fn normalize_agent_reach_output(
    query: &str,
    output: &str,
    max_results: usize,
) -> Result<Value> {
    let output = output.trim();
    if output.is_empty() {
        bail!("Agent-Reach Exa search returned no output");
    }

    let mut results = Vec::new();
    let mut text_blocks = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        collect_agent_reach_results(&value, &mut results, max_results);
        collect_agent_reach_text(&value, &mut text_blocks);
    } else {
        text_blocks.push(output.to_owned());
    }

    for text in &text_blocks {
        if results.len() >= max_results {
            break;
        }
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            collect_agent_reach_results(&value, &mut results, max_results);
        }
        collect_agent_reach_text_results(text, &mut results, max_results);
    }

    results.retain(|entry| {
        entry
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
    });
    let mut seen = std::collections::HashSet::new();
    results.retain(|entry| {
        entry
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| seen.insert(url.to_owned()))
    });
    results.truncate(max_results);

    let answers = if results.is_empty() {
        let context = text_blocks
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n\n");
        if context.trim().is_empty() {
            Vec::new()
        } else {
            vec![json!(truncate_chars(&context, 12_000))]
        }
    } else {
        Vec::new()
    };

    if results.is_empty() && answers.is_empty() {
        bail!("Agent-Reach Exa search returned no usable search evidence");
    }
    Ok(sparsify_search_evidence(json!({
        "query": query,
        "source": "agent-reach/exa",
        "numberOfResults": results.len(),
        "results": results,
        "answers": answers,
        "suggestions": [],
        "corrections": [],
    })))
}

pub(super) fn collect_agent_reach_results(
    value: &Value,
    results: &mut Vec<Value>,
    max_results: usize,
) {
    if results.len() >= max_results {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_agent_reach_results(value, results, max_results);
                if results.len() >= max_results {
                    break;
                }
            }
        }
        Value::Object(object) => {
            if let Some(url) = object.get("url").and_then(Value::as_str)
                && (url.starts_with("http://") || url.starts_with("https://"))
            {
                let snippet = object
                    .get("snippet")
                    .or_else(|| object.get("summary"))
                    .or_else(|| object.get("text"))
                    .and_then(Value::as_str)
                    .map(|value| truncate_chars(&strip_markup(value), 420))
                    .or_else(|| {
                        object
                            .get("highlights")
                            .and_then(Value::as_array)
                            .and_then(|values| values.iter().find_map(Value::as_str))
                            .map(|value| truncate_chars(&strip_markup(value), 420))
                    })
                    .unwrap_or_default();
                results.push(json!({
                    "title": object.get("title").and_then(Value::as_str).unwrap_or(url),
                    "url": url,
                    "snippet": snippet,
                    "publishedAt": object.get("publishedDate").or_else(|| object.get("publishedAt")).cloned().unwrap_or(Value::Null),
                    "category": Value::Null,
                    "engines": ["agent-reach", "exa"],
                    "score": object.get("score").cloned().unwrap_or(Value::Null),
                }));
                if results.len() >= max_results {
                    return;
                }
            }
            for value in object.values() {
                collect_agent_reach_results(value, results, max_results);
                if results.len() >= max_results {
                    break;
                }
            }
        }
        _ => {}
    }
}

pub(super) fn collect_agent_reach_text(value: &Value, text_blocks: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_agent_reach_text(value, text_blocks);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = object.get("text").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                text_blocks.push(text.to_owned());
            }
            for value in object.values() {
                collect_agent_reach_text(value, text_blocks);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_agent_reach_text_results(
    text: &str,
    results: &mut Vec<Value>,
    max_results: usize,
) {
    for block in text.split("\n---\n") {
        if results.len() >= max_results {
            return;
        }
        let mut title: Option<String> = None;
        let mut url: Option<String> = None;
        let mut published_at: Option<String> = None;
        let mut snippet_lines = Vec::new();
        let mut in_content = false;

        for raw_line in block.lines() {
            let line = raw_line.trim().trim_start_matches(['-', '*', '#', ' ']);
            if title.is_none()
                && let Some(value) = strip_field(line, "Title:")
            {
                title = Some(value.to_owned());
                continue;
            }
            if url.is_none()
                && let Some(value) = strip_field(line, "URL:").or_else(|| strip_field(line, "Url:"))
            {
                url = Some(value.trim_matches(['<', '>']).to_owned());
                continue;
            }
            if published_at.is_none()
                && let Some(value) =
                    strip_field(line, "Published Date:").or_else(|| strip_field(line, "Published:"))
            {
                if value != "N/A" {
                    published_at = Some(value.to_owned());
                }
                continue;
            }
            if line == "Highlights:" || line == "Text:" || line == "Summary:" {
                in_content = true;
                continue;
            }
            if let Some(value) =
                strip_field(line, "Text:").or_else(|| strip_field(line, "Summary:"))
            {
                in_content = true;
                snippet_lines.push(value.to_owned());
                continue;
            }
            if in_content && !line.is_empty() && line != "..." {
                snippet_lines.push(line.to_owned());
            }
        }

        let Some(url) = url else {
            continue;
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            continue;
        }
        let title = title.unwrap_or_else(|| url.clone());
        results.push(json!({
            "title": title,
            "url": url,
            "snippet": truncate_chars(&strip_markup(&snippet_lines.join(" ")), 420),
            "publishedAt": published_at.map(Value::String).unwrap_or(Value::Null),
            "category": Value::Null,
            "engines": ["agent-reach", "exa"],
            "score": Value::Null,
        }));
    }
}

pub(super) fn strip_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(field)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn normalize_results(query: &str, value: Value, max_results: usize) -> Value {
    let results = value.get("results").and_then(Value::as_array).map(|values| {
        values.iter().take(max_results).map(|entry| {
            let engines = entry.get("engines").cloned().or_else(|| entry.get("engine").and_then(Value::as_str).map(|engine| json!([engine]))).unwrap_or_else(|| json!([]));
            json!({
                "title": entry.get("title").and_then(Value::as_str).unwrap_or_default(),
                "url": entry.get("url").and_then(Value::as_str).unwrap_or_default(),
                "snippet": truncate_chars(&strip_markup(entry.get("content").and_then(Value::as_str).unwrap_or_default()), 420),
                "publishedAt": entry.get("publishedDate").or_else(|| entry.get("published_at")).cloned().unwrap_or(Value::Null),
                "category": entry.get("category").cloned().unwrap_or(Value::Null),
                "engines": engines,
                "score": entry.get("score").cloned().unwrap_or(Value::Null),
            })
        }).collect::<Vec<_>>()
    }).unwrap_or_default();
    sparsify_search_evidence(json!({
        "query": query,
        "source": "searxng",
        "numberOfResults": value.get("number_of_results").cloned().unwrap_or_else(|| json!(results.len())),
        "results": results,
        "answers": value.get("answers").cloned().unwrap_or_else(|| json!([])),
        "suggestions": value.get("suggestions").cloned().unwrap_or_else(|| json!([])),
        "corrections": value.get("corrections").cloned().unwrap_or_else(|| json!([])),
    }))
}

pub(super) fn sparsify_search_evidence(mut value: Value) -> Value {
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    for key in ["answers", "suggestions", "corrections"] {
        if object
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            object.remove(key);
        }
    }
    if let Some(results) = object.get_mut("results").and_then(Value::as_array_mut) {
        for result in results {
            let Some(result) = result.as_object_mut() else {
                continue;
            };
            for key in ["publishedAt", "category", "score"] {
                if result.get(key).is_some_and(Value::is_null) {
                    result.remove(key);
                }
            }
            if result
                .get("engines")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
            {
                result.remove("engines");
            }
        }
    }
    value
}

pub(super) fn truncate_chars(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_owned();
    }
    input
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

pub(super) fn strip_markup(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
