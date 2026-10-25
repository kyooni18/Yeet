//! Streaming artifact paging/search: large transcripts and single lines never
//! need to be loaded into memory just to return a small model-visible excerpt.

use std::{collections::VecDeque, fs::File, io::Read, path::Path};

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// Read a fixed file-length snapshot; an actively growing log cannot keep this
/// read alive indefinitely. Invalid bytes are decoded lossily, including at EOF.
fn visit_chars(path: &Path, mut visit: impl FnMut(char) -> bool) -> Result<()> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut reader = file.take(length);
    let mut buffer = [0_u8; 16 * 1024 + 4];
    let mut carry = 0;
    let mut pending_cr = false;
    let mut emit = |ch| {
        if pending_cr && ch != '\n' && !visit('\r') {
            return false;
        }
        pending_cr = ch == '\r';
        ch == '\r' || visit(ch)
    };
    loop {
        let count = reader.read(&mut buffer[carry..])?;
        let length = carry + count;
        if count == 0 {
            if carry > 0 && !emit('\u{fffd}') {
                return Ok(());
            }
            break;
        }
        let mut at = 0;
        carry = 0;
        while at < length {
            match std::str::from_utf8(&buffer[at..length]) {
                Ok(text) => {
                    for ch in text.chars() {
                        if !emit(ch) {
                            return Ok(());
                        }
                    }
                    break;
                }
                Err(error) => {
                    let valid_end = at + error.valid_up_to();
                    let text = std::str::from_utf8(&buffer[at..valid_end])?;
                    for ch in text.chars() {
                        if !emit(ch) {
                            return Ok(());
                        }
                    }
                    if let Some(invalid) = error.error_len() {
                        if !emit('\u{fffd}') {
                            return Ok(());
                        }
                        at = valid_end + invalid;
                    } else {
                        carry = length - valid_end;
                        buffer.copy_within(valid_end..length, 0);
                        break;
                    }
                }
            }
        }
    }
    if pending_cr {
        visit('\r');
    }
    Ok(())
}

pub(super) fn page(
    path: &Path,
    start: Option<usize>,
    end: Option<usize>,
    offset: usize,
    limit: usize,
) -> Result<String> {
    let start = start.unwrap_or(1).max(1);
    let end = end.unwrap_or_else(|| start.saturating_add(159));
    let end = if end < start {
        start.saturating_add(end.max(1) - 1)
    } else {
        end
    };
    let limit = limit.clamp(1, 8192);
    let mut line = 1;
    let mut in_line = false;
    let mut selected_line = false;
    let mut content = String::new();
    let mut total = 0;
    let mut append = |ch| {
        if total >= offset && total - offset < limit {
            content.push(ch);
        }
        total += 1;
    };
    visit_chars(path, |ch| {
        let selected = line >= start && line <= end;
        if !in_line && selected {
            if selected_line {
                append('\n');
            }
            selected_line = true;
        }
        in_line = true;
        if ch == '\n' {
            line += 1;
            in_line = false;
            return line <= end;
        }
        if selected {
            append(ch);
        }
        true
    })?;
    let lines = line - 1 + usize::from(in_line);
    if start > lines {
        return Ok(format!(
            "[artifact has {lines} lines; startLine {start} is past the end]"
        ));
    }
    if offset == 0 && total <= limit {
        return Ok(content);
    }
    let next = offset.saturating_add(content.chars().count());
    Ok(json!({
        "content": content, "offset": offset, "totalChars": total,
        "truncated": next < total,
        "nextOffset": (next < total).then_some(next),
        "hint": "Offsets are characters within the selected line range. Keep startLine/endLine unchanged when paging."
    }).to_string())
}

pub(super) fn search(path: &Path, id: &str, query: &str, max_results: usize) -> Result<Value> {
    if query.len() > 32 * 1024 {
        bail!("Artifact query exceeds 32768 bytes");
    }
    let query: Vec<char> = query.chars().collect();
    // KMP keeps searches linear even on very long lines or repeated prefixes.
    let mut failure = vec![0; query.len()];
    let mut prefix = 0;
    for at in 1..query.len() {
        while prefix > 0 && query[at] != query[prefix] {
            prefix = failure[prefix - 1];
        }
        if query[at] == query[prefix] {
            prefix += 1;
        }
        failure[at] = prefix;
    }
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut line = 1;
    let mut chars: usize = 0;
    let mut matched = query.is_empty();
    let mut matched_prefix = 0;
    let mut context = VecDeque::new();
    let mut excerpt = Vec::new();
    let mut offset = 0;
    visit_chars(path, |ch| {
        if ch == '\n' {
            if matched {
                if matches.len() >= max_results {
                    truncated = true;
                    return false;
                }
                matches.push(
                    json!({"line":line, "text":excerpt.iter().collect::<String>(),
                    "offset":offset, "truncated":offset > 0 || chars > 512}),
                );
            }
            line += 1;
            chars = 0;
            matched = query.is_empty();
            matched_prefix = 0;
            offset = 0;
            context.clear();
            excerpt.clear();
            return true;
        }
        chars += 1;
        if matched {
            if excerpt.len() < 512 {
                excerpt.push(ch);
            }
        } else {
            context.push_back(ch);
            if context.len() > query.len() + 80 {
                context.pop_front();
            }
            while matched_prefix > 0 && ch != query[matched_prefix] {
                matched_prefix = failure[matched_prefix - 1];
            }
            if ch == query[matched_prefix] {
                matched_prefix += 1;
            }
            if matched_prefix == query.len() {
                if matches.len() >= max_results {
                    truncated = true;
                    return false;
                }
                matched = true;
                offset = chars.saturating_sub(query.len() + 80);
                excerpt.extend(context.iter().take(512).copied());
            }
        }
        true
    })?;
    if !truncated && chars > 0 && matched {
        if matches.len() >= max_results {
            truncated = true;
        } else {
            matches.push(
                json!({"line":line, "text":excerpt.iter().collect::<String>(),
            "offset":offset, "truncated":offset > 0 || chars > 512}),
            );
        }
    }
    Ok(json!({"artifactId":id, "matches":matches, "truncated":truncated}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_lines_unicode_boundaries_and_search_are_paged_without_data_loss() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let text = format!(
            "first\r\n{}€needle{}\nlast",
            "x".repeat(16376),
            "y".repeat(100_000)
        );
        std::fs::write(file.path(), &text).unwrap();
        let result = page(file.path(), Some(2), Some(2), 16374, 12).unwrap();
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["content"], "xx€needleyyy");
        assert_eq!(result["nextOffset"], 16386);
        assert_eq!(result["truncated"], true);
        assert_eq!(page(file.path(), Some(3), None, 0, 8192).unwrap(), "last");
        let found = search(file.path(), "id", "€needle", 1).unwrap();
        assert_eq!(found["matches"][0]["line"], 2);
        assert_eq!(found["matches"][0]["offset"], 16296);
        assert!(
            found["matches"][0]["text"]
                .as_str()
                .unwrap()
                .contains("€needle")
        );
        assert_eq!(found["truncated"], false);
        std::fs::write(file.path(), b"a\xffb\r\nc\xe2\x82").unwrap();
        assert_eq!(page(file.path(), None, None, 0, 8192).unwrap(), "a�b\nc�");
    }
}
