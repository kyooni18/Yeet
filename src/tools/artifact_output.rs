//! Bounds artifact text sent to the model, including single-line data files.

use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};

const PAGE_CHARS: usize = 8192;
const MATCH_CHARS: usize = 512;

pub(super) fn page(text: &str, arguments: &Map<String, Value>) -> Result<String> {
    let offset = super::usize_arg(arguments, "offset").unwrap_or(0);
    let limit = super::usize_arg(arguments, "maxChars")
        .unwrap_or(PAGE_CHARS)
        .clamp(1, PAGE_CHARS);
    let total = text.chars().count();
    ensure!(
        offset <= total,
        "artifact offset {offset} exceeds range length {total}"
    );
    let content: String = text.chars().skip(offset).take(limit).collect();
    let end = offset + content.chars().count();
    if offset == 0 && end == total {
        return Ok(content);
    }
    Ok(json!({
        "content": content,
        "offset": offset,
        "totalChars": total,
        "truncated": end < total,
        "nextOffset": (end < total).then_some(end),
        "hint": "Offsets are characters within the selected line range. Keep startLine/endLine unchanged when paging."
    }).to_string())
}

pub(super) fn search_match(line_number: usize, line: &str, byte_position: usize) -> Value {
    let position = line[..byte_position].chars().count();
    let offset = position.saturating_sub(80);
    let text: String = line.chars().skip(offset).take(MATCH_CHARS).collect();
    let truncated = offset > 0 || line.chars().count() > MATCH_CHARS;
    json!({"line": line_number, "text": text, "offset": offset, "truncated": truncated})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_unicode_line_can_be_reconstructed_without_unbounded_responses() {
        let original = format!("{}END", "한🙂".repeat(10000));
        let mut offset = 0;
        let mut restored = String::new();
        loop {
            let response = page(&original, json!({"offset":offset}).as_object().unwrap()).unwrap();
            let value: Value = serde_json::from_str(&response).unwrap();
            let content = value["content"].as_str().unwrap();
            assert!(content.chars().count() <= PAGE_CHARS);
            restored.push_str(content);
            let Some(next) = value["nextOffset"].as_u64() else {
                break;
            };
            assert!(next as usize > offset);
            offset = next as usize;
        }
        assert_eq!(restored, original);
    }

    #[test]
    fn small_outputs_are_unchanged_and_offsets_are_validated() {
        assert_eq!(page(" exact\ntext", &Map::new()).unwrap(), " exact\ntext");
        assert!(page("abc", json!({"offset":4}).as_object().unwrap()).is_err());
    }

    #[test]
    fn search_excerpt_locates_match_deep_in_unicode_line() {
        let line = format!("{}needle{}", "한".repeat(10000), "x".repeat(10000));
        let result = search_match(7, &line, line.find("needle").unwrap());
        assert_eq!(result["line"], 7);
        assert_eq!(result["offset"], 9920);
        assert_eq!(result["truncated"], true);
        let excerpt = result["text"].as_str().unwrap();
        assert!(excerpt.contains("needle"));
        assert_eq!(excerpt.chars().count(), MATCH_CHARS);
        let read = page(
            &line,
            json!({"offset":9920,"maxChars":512}).as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&read).unwrap()["content"],
            excerpt
        );
    }

    #[test]
    fn stored_artifact_search_is_bounded_and_reports_actual_more_matches() {
        let store = super::super::ArtifactStore::new().unwrap();
        let line = format!("{}needle{}", "x".repeat(20000), "y".repeat(20000));
        let id = store.store(&format!("{line}\nneedle")).unwrap();
        let result = store.search(&id, "needle", 1).unwrap();
        assert_eq!(result["truncated"], true);
        assert!(result.to_string().len() < 1000);
        let result = store.search(&id, "needle", 2).unwrap();
        assert_eq!(result["truncated"], false);
        let full = store.read(&id, Some(1), Some(1)).unwrap();
        assert_eq!(full, line);
        let first_page: Value = serde_json::from_str(&page(&full, &Map::new()).unwrap()).unwrap();
        assert_eq!(first_page["nextOffset"], PAGE_CHARS);
    }
}
