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
) -> Result<Vec<Value>> {
    if !has_text_edits(edits) {
        return Ok(Vec::new());
    }
    let source = source.replace("\r\n", "\n");
    let lines = line_starts(&source);
    let mut spans = Vec::new();
    let mut kept = Vec::new();
    let mut match_modes = Vec::new();
    // Patch hunks apply in order, each searching after the previous one.
    let mut cursor = 0usize;
    for (edit_index, edit) in edits.drain(..).enumerate() {
        match edit.get("kind").and_then(Value::as_str) {
            Some("replaceText") => {
                let (resolved, mode) = replace_text_spans(path, &source, &edit)?;
                spans.extend(resolved);
                match_modes.push(json!({"editIndex": edit_index, "mode": mode}));
            }
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
    Ok(match_modes)
}

fn replace_text_spans(path: &str, source: &str, edit: &Value) -> Result<(Vec<Span>, &'static str)> {
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
    let relaxed = match edit.get("matchPolicy") {
        None => false,
        Some(Value::String(policy)) if policy == "exact" => false,
        Some(Value::String(policy)) if policy == "relaxed" => true,
        _ => bail!("replaceText matchPolicy must be exact or relaxed"),
    };
    let starts = source
        .match_indices(&old)
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    if starts.is_empty() {
        if !relaxed || all {
            bail!(
                "replaceText oldText was not found exactly in {path}. Copy it from read_file output without line numbers or hashes. Relaxed matching requires matchPolicy:\"relaxed\" and cannot be used with replaceAll:true."
            );
        }
        let (span, tier) = replace_text_fallback(path, source, &old, &new)?;
        return Ok((
            vec![span],
            match tier {
                Tier::TrailingWhitespace => "trailing_whitespace",
                Tier::Indentation => "indentation",
                Tier::Exact => "exact",
            },
        ));
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
    Ok((
        starts
            .into_iter()
            .map(|start| Span {
                start,
                end: start + old.len(),
                text: new.clone(),
                drops_lines,
            })
            .collect(),
        "exact",
    ))
}

/// Matches whole lines of `oldText` with whitespace tolerance when the exact
/// text is absent; only a unique match is accepted.
fn replace_text_fallback(path: &str, source: &str, old: &str, new: &str) -> Result<(Span, Tier)> {
    let whole_lines = old.ends_with('\n');
    let old_body = old.strip_suffix('\n').unwrap_or(old);
    let new_body = if whole_lines {
        new.strip_suffix('\n').unwrap_or(new)
    } else {
        new
    };
    let old_lines = old_body.split('\n').collect::<Vec<_>>();
    let file_lines = source.lines().collect::<Vec<_>>();
    let not_found = || {
        anyhow!(
            "replaceText oldText was not found in {path}, even ignoring whitespace. Copy it exactly from read_file output (without line numbers or hashes).{}",
            nearest_hint(&file_lines, &old_lines)
        )
    };
    for tier in LOOSE_TIERS {
        let found = (0..=file_lines.len().saturating_sub(old_lines.len()))
            .filter(|index| block_matches(&file_lines, &old_lines, *index, tier))
            .collect::<Vec<_>>();
        match found.as_slice() {
            [] => continue,
            [index] => {
                let starts = line_starts(source);
                let last = index + old_lines.len() - 1;
                let new_lines = new_body.split('\n').map(str::to_owned).collect::<Vec<_>>();
                let text = reindent(tier, &old_lines, &file_lines[*index..=last], new_lines);
                return Ok((
                    Span {
                        start: starts[*index],
                        end: starts[last] + file_lines[last].len(),
                        drops_lines: whole_lines && text.is_empty(),
                        text,
                    },
                    tier,
                ));
            }
            many => bail!(
                "replaceText oldText was not found exactly in {path}; ignoring whitespace it matches {} places (lines {}). Include more surrounding text to make it unique.",
                many.len(),
                many.iter()
                    .map(|at| (at + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    Err(not_found())
}

#[derive(Clone, Copy, PartialEq)]
enum Tier {
    Exact,
    TrailingWhitespace,
    Indentation,
}

const LOOSE_TIERS: [Tier; 2] = [Tier::TrailingWhitespace, Tier::Indentation];

fn lines_equal(tier: Tier, file: &str, wanted: &str) -> bool {
    match tier {
        Tier::Exact => file == wanted,
        Tier::TrailingWhitespace => file.trim_end() == wanted.trim_end(),
        Tier::Indentation => file.trim() == wanted.trim(),
    }
}

fn block_matches(
    file_lines: &[&str],
    wanted: &[impl AsRef<str>],
    index: usize,
    tier: Tier,
) -> bool {
    if tier == Tier::Indentation && index + wanted.len() <= file_lines.len() {
        // Only a uniform indentation shift is recoverable. Trimming every
        // line independently can confuse distinct Python/YAML nesting.
        let anchor = wanted
            .iter()
            .enumerate()
            .find(|(_, line)| !line.as_ref().trim().is_empty());
        if let Some((offset, line)) = anchor {
            let expected_indent = indent_of(line.as_ref());
            let actual_indent = indent_of(file_lines[index + offset]);
            return wanted.iter().enumerate().all(|(offset, line)| {
                let expected = line.as_ref();
                let actual = file_lines[index + offset];
                if expected.trim().is_empty() {
                    actual.trim().is_empty()
                } else {
                    match (
                        expected.strip_prefix(expected_indent),
                        actual.strip_prefix(actual_indent),
                    ) {
                        (Some(expected), Some(actual)) => expected.trim_end() == actual.trim_end(),
                        _ => false,
                    }
                }
            });
        }
    }
    index + wanted.len() <= file_lines.len()
        && wanted
            .iter()
            .enumerate()
            .all(|(offset, line)| lines_equal(tier, file_lines[index + offset], line.as_ref()))
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Shifts replacement lines by the indentation the model got wrong, so an
/// indentation-insensitive match keeps the file's real indentation.
fn reindent(tier: Tier, old: &[impl AsRef<str>], matched: &[&str], new: Vec<String>) -> String {
    let anchor = old
        .iter()
        .zip(matched)
        .find(|(line, _)| !line.as_ref().trim().is_empty());
    let (Tier::Indentation, Some((wanted, actual))) = (tier, anchor) else {
        return new.join("\n");
    };
    let (wanted, actual) = (indent_of(wanted.as_ref()), indent_of(actual));
    if wanted == actual {
        return new.join("\n");
    }
    new.into_iter()
        .map(|line| match line.strip_prefix(wanted) {
            Some(rest) if !line.trim().is_empty() => format!("{actual}{rest}"),
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Points at the file lines most similar to the first expected line.
fn nearest_hint(file_lines: &[&str], wanted: &[impl AsRef<str>]) -> String {
    let Some(target) = wanted
        .iter()
        .map(|line| line.as_ref().trim())
        .find(|line| !line.is_empty())
    else {
        return String::new();
    };
    let mut scored = file_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| (similarity(line.trim(), target), index))
        .filter(|(score, _)| *score >= 0.5)
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let closest = scored
        .iter()
        .take(2)
        .map(|(_, index)| format!("\n  line {}: {}", index + 1, file_lines[*index].trim_end()))
        .collect::<String>();
    if closest.is_empty() {
        closest
    } else {
        format!(" Closest lines:{closest}")
    }
}

/// Normalized Levenshtein similarity over at most 200 characters per side.
fn similarity(a: &str, b: &str) -> f64 {
    let a = a.chars().take(200).collect::<Vec<_>>();
    let b = b.chars().take(200).collect::<Vec<_>>();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut row = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    1.0 - row[b.len()] as f64 / longest as f64
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
        let exact = (*cursor..file_lines.len()).find(|index| file_lines[*index].trim() == anchor);
        let found = match exact {
            Some(found) => found,
            None => {
                // A partial anchor is only trusted when it names one line.
                let partial = (*cursor..file_lines.len())
                    .filter(|index| file_lines[*index].contains(anchor))
                    .collect::<Vec<_>>();
                match partial.as_slice() {
                    [found] => *found,
                    [] => bail!(
                        "Patch context `@@ {anchor}` was not found in {path}.{}",
                        nearest_hint(&file_lines, &[anchor])
                    ),
                    many => bail!(
                        "Patch context `@@ {anchor}` is not an exact line in {path} and appears in {} lines ({}). Use the full line text.",
                        many.len(),
                        many.iter()
                            .map(|at| (at + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }
            }
        };
        *cursor = found + 1;
    }
    if old.is_empty() {
        return Ok(HunkTarget::Append(new.join("\n")));
    }
    let at_end = edit.get("endOfFile").and_then(Value::as_bool) == Some(true);
    let candidates = || match file_lines.len().checked_sub(old.len()) {
        None => 0..0,
        Some(last) if at_end => last.max(*cursor)..last + 1,
        Some(last) => *cursor..last + 1,
    };
    // Exact first, then tolerate trailing and surrounding whitespace drift.
    let (found, tier) = std::iter::once(Tier::Exact)
        .chain(LOOSE_TIERS)
        .find_map(|tier| {
            candidates()
                .find(|index| block_matches(&file_lines, &old, *index, tier))
                .map(|index| (index, tier))
        })
        .ok_or_else(|| {
            anyhow!(
                "Patch hunk for {path} did not match the file (first expected line: `{}`). Re-read the file and regenerate the hunk with exact context lines.{}",
                old[0],
                nearest_hint(&file_lines, &old)
            )
        })?;
    *cursor = found + old.len();
    let start = starts[found];
    let last_line = found + old.len() - 1;
    let end = starts[last_line] + file_lines[last_line].len();
    let drops_lines = new.is_empty();
    Ok(HunkTarget::Span(Span {
        start,
        end,
        text: reindent(tier, &old, &file_lines[found..=last_line], new),
        drops_lines,
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

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn exact_replacement_never_silently_repairs_whitespace() {
        for (path, source, old) in [
            ("Makefile", "\tbuild\n", "    build\n"),
            ("example.py", "    run()\n", "        run()\n"),
            ("config.yaml", "key: value  \n", "key: value\n"),
        ] {
            let mut edits = vec![json!({"kind":"replaceText","oldText":old,"newText":"updated\n"})];
            let error = concretize_text_edits(path, source, &mut edits).unwrap_err();
            assert!(error.to_string().contains("not found exactly"));
        }
    }

    #[test]
    fn relaxed_replacement_reports_mode_and_preserves_relative_indentation() {
        let source = "    if ready:\n        run()\n";
        let mut edits = vec![json!({"kind":"replaceText", "matchPolicy":"relaxed",
            "oldText":"if ready:\n    run()", "newText":"if ready:\n    stop()"})];
        let modes = concretize_text_edits("example.py", source, &mut edits).unwrap();
        assert_eq!(modes, vec![json!({"editIndex":0,"mode":"indentation"})]);
        assert_eq!(edits[0]["text"], "    if ready:\n        stop()");

        let mut edits = vec![json!({"kind":"replaceText", "matchPolicy":"relaxed",
            "oldText":"key: value\n", "newText":"key: changed\n"})];
        let modes = concretize_text_edits("config.yaml", "key: value  \n", &mut edits).unwrap();
        assert_eq!(modes[0]["mode"], "trailing_whitespace");
    }

    #[test]
    fn relaxed_replacement_rejects_different_nesting_and_nonexact_replace_all() {
        let source = "    if ready:\n    run()\n";
        let old = "  if ready:\n      run()";
        // Force the fallback path: a missing first-line prefix alone can
        // still leave a valid exact substring, which is allowed by contract.
        assert!(!source.contains(old));
        let mut edits = vec![json!({"kind":"replaceText", "matchPolicy":"relaxed",
            "oldText":old, "newText":"  if ready:\n      stop()"})];
        assert!(concretize_text_edits("example.py", source, &mut edits).is_err());

        let mut exact = vec![json!({"kind":"replaceText", "matchPolicy":"relaxed",
            "oldText":"if ready:\n    run()", "newText":"if ready:\n    stop()"})];
        let modes = concretize_text_edits("example.py", source, &mut exact).unwrap();
        assert_eq!(modes[0]["mode"], "exact");
        assert_eq!(exact[0]["text"], "    if ready:\n    stop()");

        let mut edits = vec![
            json!({"kind":"replaceText", "matchPolicy":"relaxed", "replaceAll":true,
            "oldText":"run()\n", "newText":"stop()\n"}),
        ];
        assert!(concretize_text_edits("example.py", "    run()  \n", &mut edits).is_err());
    }
}
