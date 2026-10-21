//! Text-anchored edits: exact string replacement and Codex-style patches.
//!
//! Both forms are resolved against the read snapshot here and lowered to the
//! line-range `replace`/`delete`/`insert` edits the edit daemon validates, so
//! snapshot safety and transactional commits stay in one place.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

/// Returns whether any edit must be resolved against snapshot text here.
pub(super) fn has_text_edits(edits: &[Value]) -> bool {
    edits.iter().any(|edit| {
        matches!(
            edit.get("kind").and_then(Value::as_str),
            Some("replaceText" | "patchHunk")
        )
    })
}

struct Span {
    start: usize,
    end: usize,
    text: String,
    /// Removing whole lines, so an empty result deletes them.
    drops_lines: bool,
}

/// Replaces `replaceText` and `patchHunk` edits with concrete line edits.
pub(super) fn concretize_text_edits(
    path: &str,
    source: &str,
    edits: &mut Vec<Value>,
) -> Result<()> {
    if !has_text_edits(edits) {
        return Ok(());
    }
    let source = source.replace("\r\n", "\n");
    let lines = line_starts(&source);
    let mut spans = Vec::new();
    let mut kept = Vec::new();
    // Patch hunks apply in order, each searching after the previous one.
    let mut cursor = 0usize;
    for edit in edits.drain(..) {
        match edit.get("kind").and_then(Value::as_str) {
            Some("replaceText") => spans.extend(replace_text_spans(path, &source, &edit)?),
            Some("patchHunk") => match hunk_span(path, &source, &lines, &edit, &mut cursor)? {
                HunkTarget::Span(span) => spans.push(span),
                HunkTarget::Append(text) => kept.push(json!({
                    "kind": "insert", "at": {"kind": "end"}, "text": text,
                })),
            },
            _ => kept.push(edit),
        }
    }
    spans.sort_by_key(|span| (span.start, span.end));
    for pair in spans.windows(2) {
        if pair[1].start < pair[0].end {
            bail!("Text edits in {path} overlap; combine them into one replacement.");
        }
    }
    kept.extend(line_edits(&source, &spans));
    *edits = kept;
    Ok(())
}

fn replace_text_spans(path: &str, source: &str, edit: &Value) -> Result<Vec<Span>> {
    let old = edit
        .get("oldText")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("replaceText requires a non-empty oldText"))?
        .replace("\r\n", "\n");
    let new = edit
        .get("newText")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("replaceText requires newText"))?
        .replace("\r\n", "\n");
    let all = edit.get("replaceAll").and_then(Value::as_bool) == Some(true);
    let starts = source
        .match_indices(&old)
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    if starts.is_empty() {
        bail!(
            "replaceText oldText was not found in {path}. Copy it exactly from read_file output (without line numbers or hashes), including whitespace."
        );
    }
    if starts.len() > 1 && !all {
        let lines = starts
            .iter()
            .map(|at| (line_of(source, *at) + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "replaceText oldText matches {} places in {path} (lines {lines}). Include more surrounding text to make it unique, or set replaceAll:true.",
            starts.len()
        );
    }
    let drops_lines = new.is_empty() && old.ends_with('\n');
    Ok(starts
        .into_iter()
        .map(|start| Span {
            start,
            end: start + old.len(),
            text: new.clone(),
            drops_lines,
        })
        .collect())
}

enum HunkTarget {
    Span(Span),
    Append(String),
}

fn hunk_span(
    path: &str,
    source: &str,
    starts: &[usize],
    edit: &Value,
    cursor: &mut usize,
) -> Result<HunkTarget> {
    let strings = |key: &str| -> Vec<String> {
        edit.get(key)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let old = strings("oldLines");
    let new = strings("newLines");
    let file_lines = source.lines().collect::<Vec<_>>();
    if let Some(anchor) = edit.get("anchor").and_then(Value::as_str) {
        let anchor = anchor.trim();
        let found = (*cursor..file_lines.len())
            .find(|index| file_lines[*index].trim() == anchor)
            .or_else(|| {
                (*cursor..file_lines.len()).find(|index| file_lines[*index].contains(anchor))
            })
            .ok_or_else(|| anyhow!("Patch context `@@ {anchor}` was not found in {path}."))?;
        *cursor = found + 1;
    }
    if old.is_empty() {
        return Ok(HunkTarget::Append(new.join("\n")));
    }
    let at_end = edit.get("endOfFile").and_then(Value::as_bool) == Some(true);
    let matches = |index: usize, compare: &dyn Fn(&str, &str) -> bool| {
        old.iter()
            .enumerate()
            .all(|(offset, line)| compare(file_lines[index + offset], line))
    };
    let candidates = || match file_lines.len().checked_sub(old.len()) {
        None => 0..0,
        Some(last) if at_end => last.max(*cursor)..last + 1,
        Some(last) => *cursor..last + 1,
    };
    // Exact first, then tolerate trailing and surrounding whitespace drift.
    let comparisons: [&dyn Fn(&str, &str) -> bool; 3] = [
        &|a, b| a == b,
        &|a, b| a.trim_end() == b.trim_end(),
        &|a, b| a.trim() == b.trim(),
    ];
    let found = comparisons
        .iter()
        .find_map(|compare| candidates().find(|index| matches(*index, *compare)))
        .ok_or_else(|| {
            anyhow!(
                "Patch hunk for {path} did not match the file (first expected line: `{}`). Re-read the file and regenerate the hunk with exact context lines.",
                old[0]
            )
        })?;
    *cursor = found + old.len();
    let start = starts[found];
    let last_line = found + old.len() - 1;
    let end = starts[last_line] + file_lines[last_line].len();
    Ok(HunkTarget::Span(Span {
        start,
        end,
        text: new.join("\n"),
        drops_lines: new.is_empty(),
    }))
}

/// Expands byte spans to whole lines, merging spans that share a line.
fn line_edits(source: &str, spans: &[Span]) -> Vec<Value> {
    let line_end_after = |end: usize| {
        source[end..]
            .find('\n')
            .map_or(source.len(), |offset| end + offset)
    };
    let mut edits = Vec::new();
    let mut index = 0;
    while index < spans.len() {
        let line_start = source[..spans[index].start]
            .rfind('\n')
            .map_or(0, |at| at + 1);
        let mut text = source[line_start..spans[index].start].to_owned();
        let mut drops_lines = false;
        let mut end = spans[index].start;
        loop {
            let span = &spans[index];
            text.push_str(&source[end..span.start]);
            text.push_str(&span.text);
            drops_lines |= span.drops_lines;
            end = span.end;
            index += 1;
            if index == spans.len() || spans[index].start > line_end_after(end) {
                break;
            }
        }
        let line_end = line_end_after(end);
        text.push_str(&source[end..line_end]);
        let start_line = line_of(source, line_start) + 1;
        let end_line = line_of(source, line_end.saturating_sub(1).max(line_start)) + 1;
        let range = json!({"start": start_line, "end": end_line});
        if text.is_empty() && drops_lines {
            edits.push(json!({"kind": "delete", "range": range}));
        } else {
            edits.push(json!({"kind": "replace", "range": range, "text": text}));
        }
    }
    edits
}

fn line_of(source: &str, at: usize) -> usize {
    source[..at].bytes().filter(|byte| *byte == b'\n').count()
}

fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

/// Parses a `*** Begin Patch` document into apply_file_edits changes.
pub(super) fn parse_patch(patch: &str) -> Result<Vec<Value>> {
    let patch = patch.replace("\r\n", "\n");
    let mut rows = patch.lines().peekable();
    while rows
        .peek()
        .is_some_and(|row| row.trim() != "*** Begin Patch")
    {
        rows.next();
    }
    if rows.next().is_none() {
        bail!("patch must start with `*** Begin Patch`");
    }
    let mut changes = Vec::new();
    while let Some(row) = rows.next() {
        let row = row.trim_end();
        if row == "*** End Patch" {
            return if changes.is_empty() {
                Err(anyhow!("patch contains no file sections"))
            } else {
                Ok(changes)
            };
        }
        if let Some(path) = row.strip_prefix("*** Add File: ") {
            let mut text = String::new();
            while let Some(line) = rows.next_if(|row| !row.starts_with("*** ")) {
                let line = line.strip_prefix('+').ok_or_else(|| {
                    anyhow!("Add File lines for {path} must start with `+`: `{line}`")
                })?;
                text.push_str(line);
                text.push('\n');
            }
            changes.push(json!({"path": path.trim(), "fileOp": {"kind": "create", "text": text}}));
        } else if let Some(path) = row.strip_prefix("*** Delete File: ") {
            changes.push(json!({"path": path.trim(), "fileOp": {"kind": "delete"}}));
        } else if let Some(path) = row.strip_prefix("*** Update File: ") {
            let mut change = json!({"path": path.trim()});
            if let Some(destination) = rows
                .next_if(|row| row.starts_with("*** Move to: "))
                .and_then(|row| row.strip_prefix("*** Move to: "))
            {
                change["fileOp"] = json!({"kind": "move", "destination": destination.trim()});
            }
            let mut hunks = Vec::new();
            let mut hunk: Option<(Option<String>, Vec<String>, Vec<String>)> = None;
            let flush = |hunk: &mut Option<(Option<String>, Vec<String>, Vec<String>)>,
                         hunks: &mut Vec<Value>,
                         end_of_file: bool| {
                if let Some((anchor, old, new)) = hunk.take()
                    && (anchor.is_some() || !old.is_empty() || !new.is_empty())
                {
                    let mut value = json!({"kind": "patchHunk", "oldLines": old, "newLines": new});
                    if let Some(anchor) = anchor {
                        value["anchor"] = json!(anchor);
                    }
                    if end_of_file {
                        value["endOfFile"] = json!(true);
                    }
                    hunks.push(value);
                }
            };
            while let Some(line) =
                rows.next_if(|row| !row.starts_with("*** ") || row.trim_end() == "*** End of File")
            {
                if line.trim_end() == "*** End of File" {
                    flush(&mut hunk, &mut hunks, true);
                    continue;
                }
                if let Some(anchor) = line.strip_prefix("@@") {
                    flush(&mut hunk, &mut hunks, false);
                    let anchor = anchor.trim();
                    hunk = Some((
                        (!anchor.is_empty()).then(|| anchor.to_owned()),
                        Vec::new(),
                        Vec::new(),
                    ));
                    continue;
                }
                let (_, old, new) = hunk.get_or_insert_with(|| (None, Vec::new(), Vec::new()));
                match line.chars().next() {
                    Some('-') => old.push(line[1..].to_owned()),
                    Some('+') => new.push(line[1..].to_owned()),
                    Some(' ') => {
                        old.push(line[1..].to_owned());
                        new.push(line[1..].to_owned());
                    }
                    // Models often drop the leading space on blank context lines.
                    None => {
                        old.push(String::new());
                        new.push(String::new());
                    }
                    Some(_) => bail!(
                        "Patch line for {} must start with ` `, `-`, `+`, or `@@`: `{line}`",
                        path.trim()
                    ),
                }
            }
            flush(&mut hunk, &mut hunks, false);
            if hunks.is_empty() && change.get("fileOp").is_none() {
                bail!("Update File: {} has no hunks", path.trim());
            }
            if !hunks.is_empty() {
                change["edits"] = json!(hunks);
            }
            changes.push(change);
        } else if !row.trim().is_empty() {
            bail!("Unexpected patch line: `{row}`");
        }
    }
    bail!("patch must end with `*** End Patch`")
}
