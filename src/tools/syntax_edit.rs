use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use tree_sitter::{Language, Node, Parser, Tree};

#[derive(Debug, Clone)]
struct SyntaxTarget {
    kind: Option<String>,
    name: String,
    container: Vec<String>,
    signature: Option<String>,
}

#[derive(Debug, Clone)]
struct Candidate<'tree> {
    node: Node<'tree>,
    kind: &'static str,
    name: String,
    containers: Vec<String>,
}

pub(super) fn has_semantic_edits(edits: &[Value]) -> bool {
    edits.iter().any(|edit| {
        edit.get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "replaceNode" | "replaceBody" | "deleteNode" | "insertBefore" | "insertAfter"
                )
            })
    })
}

pub(super) fn concretize_semantic_edits(
    path: &Path,
    snapshot_text: &str,
    edits: &mut [Value],
) -> Result<()> {
    if !has_semantic_edits(edits) {
        return Ok(());
    }

    let language = language_for_path(path)?;
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .with_context(|| format!("load syntax grammar for {}", path.display()))?;
    let tree = parser.parse(snapshot_text, None).ok_or_else(|| {
        anyhow!(
            "Tree-sitter did not return a syntax tree for {}",
            path.display()
        )
    })?;

    let before_errors = syntax_error_count(&tree);
    let mut semantic_concrete = Vec::new();
    for edit in edits.iter_mut() {
        let Some(kind) = edit.get("kind").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(
            kind,
            "replaceNode" | "replaceBody" | "deleteNode" | "insertBefore" | "insertAfter"
        ) {
            continue;
        }

        let target_value = edit
            .get("target")
            .ok_or_else(|| anyhow!("{kind} requires target"))?;
        let target = parse_target(target_value)?;
        let candidate = resolve_target(&tree, snapshot_text, &target)?;
        let text = edit.get("text").and_then(Value::as_str).unwrap_or_default();
        let decorated = decorated_node(candidate.node);

        let concrete = match kind {
            "replaceNode" => replacement_for_span(snapshot_text, candidate.node, text),
            "replaceBody" => {
                let body = body_node(candidate.node).ok_or_else(|| {
                    anyhow!(
                        "Target {} at line {} has no syntactic body that can be replaced",
                        display_target(&candidate),
                        candidate.node.start_position().row + 1
                    )
                })?;
                replacement_for_span(snapshot_text, body, text)
            }
            "deleteNode" => replacement_for_span(snapshot_text, decorated, ""),
            "insertBefore" => json!({
                "kind": "insert",
                "at": {"kind": "before", "line": decorated.start_position().row + 1},
                "text": text,
            }),
            "insertAfter" => json!({
                "kind": "insert",
                "at": {"kind": "after", "line": effective_end_line(decorated)},
                "text": text,
            }),
            _ => unreachable!(),
        };

        semantic_concrete.push(concrete.clone());
        *edit = concrete;
    }

    validate_semantic_syntax(
        &mut parser,
        &language,
        snapshot_text,
        &semantic_concrete,
        before_errors,
        path,
    )?;

    Ok(())
}

fn parse_target(value: &Value) -> Result<SyntaxTarget> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("semantic edit target must be an object"))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("semantic edit target requires a non-empty name"))?
        .to_owned();
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let signature = object
        .get("signature")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let container = match object.get("container") {
        None => Vec::new(),
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("target.container entries must be strings"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => bail!("target.container must be a string or string array"),
    };
    Ok(SyntaxTarget {
        kind,
        name,
        container,
        signature,
    })
}

fn language_for_path(path: &Path) -> Result<Language> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let language = match extension.as_str() {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "js" | "mjs" | "cjs" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        "py" | "pyi" => tree_sitter_python::LANGUAGE.into(),
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => tree_sitter_cpp::LANGUAGE.into(),
        "h" => tree_sitter_c::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        _ => bail!(
            "Semantic edits are not supported for {} files yet; use range edits instead",
            if extension.is_empty() {
                "extensionless"
            } else {
                &extension
            }
        ),
    };
    Ok(language)
}

fn resolve_target<'tree>(
    tree: &'tree Tree,
    source: &str,
    target: &SyntaxTarget,
) -> Result<Candidate<'tree>> {
    let mut candidates = Vec::new();
    collect_candidates(tree.root_node(), source, &[], &mut candidates);

    let wanted_signature = target.signature.as_deref().map(normalize_ws);
    let mut matches = candidates
        .into_iter()
        .filter(|candidate| candidate.name == target.name)
        .filter(|candidate| {
            target
                .kind
                .as_deref()
                .is_none_or(|kind| candidate.kind == kind)
        })
        .filter(|candidate| {
            target.container.is_empty()
                || candidate.containers.ends_with(target.container.as_slice())
        })
        .filter(|candidate| {
            wanted_signature.as_ref().is_none_or(|wanted| {
                normalize_ws(&candidate_signature(candidate.node, source)).contains(wanted)
            })
        })
        .collect::<Vec<_>>();

    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }
    if matches.is_empty() {
        bail!(
            "SYMBOL_NOT_FOUND: no {} named {}{} in this snapshot",
            target.kind.as_deref().unwrap_or("declaration"),
            target.name,
            if target.container.is_empty() {
                String::new()
            } else {
                format!(" in {}", target.container.join("::"))
            }
        );
    }

    bail!(
        "SYMBOL_AMBIGUOUS: {} candidates match {}. Add target.container, target.kind, or target.signature using source already returned by read_file.",
        matches.len(),
        target.name
    )
}

fn collect_candidates<'tree>(
    node: Node<'tree>,
    source: &str,
    containers: &[String],
    output: &mut Vec<Candidate<'tree>>,
) {
    if let Some(kind) = declaration_kind(node) {
        if let Some(name) = node_name(node, source) {
            let semantic_kind = if kind == "function" && is_method_context(node) {
                "method"
            } else {
                kind
            };
            output.push(Candidate {
                node,
                kind: semantic_kind,
                name,
                containers: containers.to_vec(),
            });
        }
    }

    let mut child_containers = containers.to_vec();
    if let Some(container) = container_name(node, source) {
        child_containers.push(container);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_candidates(child, source, &child_containers, output);
    }
}

fn declaration_kind(node: Node<'_>) -> Option<&'static str> {
    match node.kind() {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "generator_function_declaration"
        | "local_function_statement"
        | "init_declaration"
        | "deinit_declaration"
        | "subscript_declaration" => Some("function"),
        "method_definition" | "method_declaration" => Some("method"),
        "struct_item"
        | "enum_item"
        | "trait_item"
        | "type_item"
        | "class_declaration"
        | "class_definition"
        | "interface_declaration"
        | "type_alias_declaration"
        | "struct_specifier"
        | "class_specifier"
        | "enum_specifier"
        | "protocol_declaration"
        | "struct_declaration"
        | "enum_declaration"
        | "typealias_declaration" => Some("type"),
        _ => None,
    }
}

fn is_method_context(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(
            parent.kind(),
            "impl_item"
                | "trait_item"
                | "class_declaration"
                | "class_definition"
                | "class_specifier"
                | "struct_declaration"
                | "struct_item"
                | "struct_specifier"
                | "protocol_declaration"
                | "extension_declaration"
        ) {
            return true;
        }
        node = parent;
    }
    false
}

fn container_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "impl_item" | "extension_declaration" => node
            .child_by_field_name("type")
            .or_else(|| node.child_by_field_name("name"))
            .and_then(|child| node_text(child, source)),
        "mod_item"
        | "namespace_definition"
        | "class_declaration"
        | "class_definition"
        | "class_specifier"
        | "struct_declaration"
        | "struct_item"
        | "struct_specifier"
        | "enum_declaration"
        | "enum_item"
        | "enum_specifier"
        | "trait_item"
        | "protocol_declaration"
        | "interface_declaration" => node_name(node, source),
        _ => None,
    }
}

fn node_name(node: Node<'_>, source: &str) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        if let Some(value) = node_text(name, source) {
            return Some(value);
        }
    }
    if matches!(node.kind(), "init_declaration") {
        return Some("init".into());
    }
    if matches!(node.kind(), "deinit_declaration") {
        return Some("deinit".into());
    }

    let search_root = node.child_by_field_name("declarator").unwrap_or(node);
    first_identifier(search_root, source)
}

fn first_identifier(node: Node<'_>, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "identifier" | "field_identifier" | "type_identifier" | "simple_identifier"
    ) {
        return node_text(node, source);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(value) = first_identifier(child, source) {
            return Some(value);
        }
    }
    None
}

fn node_text(node: Node<'_>, source: &str) -> Option<String> {
    source
        .get(node.byte_range())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn body_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("body").or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "block" | "statement_block" | "compound_statement" | "function_body" | "code_block"
            )
        })
    })
}

fn decorated_node(node: Node<'_>) -> Node<'_> {
    node.parent()
        .filter(|parent| parent.kind() == "decorated_definition")
        .unwrap_or(node)
}

fn candidate_signature(node: Node<'_>, source: &str) -> String {
    let end = body_node(node)
        .map(|body| body.start_byte())
        .unwrap_or(node.end_byte());
    source
        .get(node.start_byte()..end)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn normalize_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn display_target(candidate: &Candidate<'_>) -> String {
    if candidate.containers.is_empty() {
        candidate.name.clone()
    } else {
        format!("{}::{}", candidate.containers.join("::"), candidate.name)
    }
}

fn replacement_for_span(source: &str, node: Node<'_>, replacement: &str) -> Value {
    let start = node.start_byte();
    let end = node.end_byte();
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = source[end..]
        .find('\n')
        .map_or(source.len(), |offset| end + offset);
    let start_line = source[..line_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let end_probe = line_end.saturating_sub(1).max(line_start);
    let end_line = source[..end_probe]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;

    let mut text = String::new();
    text.push_str(&source[line_start..start]);
    text.push_str(replacement);
    text.push_str(&source[end..line_end]);

    if replacement.is_empty() && text.trim().is_empty() {
        return json!({
            "kind": "delete",
            "range": {"start": start_line, "end": end_line},
        });
    }

    json!({
        "kind": "replace",
        "range": {"start": start_line, "end": end_line},
        "text": text,
    })
}

fn effective_end_line(node: Node<'_>) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row
    } else {
        end.row + 1
    }
}

#[derive(Debug)]
struct PreviewSplice {
    start: usize,
    end: usize,
    replacement: Vec<String>,
}

fn validate_semantic_syntax(
    parser: &mut Parser,
    language: &Language,
    snapshot_text: &str,
    concrete: &[Value],
    before_errors: usize,
    path: &Path,
) -> Result<()> {
    for edit in concrete {
        let after = apply_concrete_preview(snapshot_text, std::slice::from_ref(edit))?;
        validate_preview_tree(parser, language, &after, before_errors, path)?;
    }

    if concrete.len() > 1 {
        let after = apply_concrete_preview(snapshot_text, concrete)?;
        validate_preview_tree(parser, language, &after, before_errors, path)?;
    }

    Ok(())
}

fn validate_preview_tree(
    parser: &mut Parser,
    language: &Language,
    after: &str,
    before_errors: usize,
    path: &Path,
) -> Result<()> {
    parser
        .set_language(language)
        .with_context(|| format!("reload syntax grammar for {}", path.display()))?;
    let after_tree = parser.parse(after, None).ok_or_else(|| {
        anyhow!(
            "Tree-sitter did not return a post-edit tree for {}",
            path.display()
        )
    })?;
    let after_errors = syntax_error_count(&after_tree);
    if after_errors > before_errors {
        bail!(
            "INVALID_RESULT: semantic edit introduces {} new syntax error/missing node(s) in {}",
            after_errors - before_errors,
            path.display()
        );
    }
    Ok(())
}

fn preview_payload(edit: &Value) -> Vec<String> {
    let normalized = edit
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut values = normalized
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if normalized.ends_with('\n') {
        values.pop();
    }
    values
}

fn preview_splice(edit: &Value, line_count: usize) -> Result<Option<PreviewSplice>> {
    let Some(kind) = edit.get("kind").and_then(Value::as_str) else {
        return Ok(None);
    };
    if !matches!(kind, "replace" | "delete" | "insert") {
        return Ok(None);
    }

    if kind == "insert" {
        let at = edit
            .get("at")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("semantic concrete insertion is missing at"))?;
        let at_kind = at
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("semantic concrete insertion is missing at.kind"))?;
        let position = match at_kind {
            "start" => 0,
            "end" => line_count,
            "before" => {
                let line = usize_field(at, "line")?;
                if line == 0 || line > line_count {
                    bail!("semantic concrete insertion generated invalid line {line}");
                }
                line - 1
            }
            "after" => {
                let line = usize_field(at, "line")?;
                if line == 0 || line > line_count {
                    bail!("semantic concrete insertion generated invalid line {line}");
                }
                line
            }
            other => bail!("unsupported semantic insertion anchor {other}"),
        };
        return Ok(Some(PreviewSplice {
            start: position,
            end: position,
            replacement: preview_payload(edit),
        }));
    }

    let range = edit
        .get("range")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("semantic concrete edit is missing range"))?;
    let start = usize_field(range, "start")?;
    let end = usize_field(range, "end")?;
    if start == 0 || end < start || end > line_count {
        bail!("semantic concrete edit generated invalid range {start}..{end}");
    }
    Ok(Some(PreviewSplice {
        start: start - 1,
        end,
        replacement: if kind == "delete" {
            Vec::new()
        } else {
            preview_payload(edit)
        },
    }))
}

fn preview_splices_overlap(left: &PreviewSplice, right: &PreviewSplice) -> bool {
    let left_insert = left.start == left.end;
    let right_insert = right.start == right.end;
    if left_insert && right_insert {
        return left.start == right.start;
    }
    if left_insert {
        return left.start >= right.start && left.start < right.end;
    }
    if right_insert {
        return right.start >= left.start && right.start < left.end;
    }
    left.start < right.end && right.start < left.end
}

fn apply_concrete_preview(source: &str, edits: &[Value]) -> Result<String> {
    let keep_final_newline = source.ends_with('\n');
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    if keep_final_newline {
        lines.pop();
    }

    let mut splices = edits
        .iter()
        .map(|edit| preview_splice(edit, lines.len()))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    for left in 0..splices.len() {
        for right in left + 1..splices.len() {
            if preview_splices_overlap(&splices[left], &splices[right]) {
                bail!(
                    "OVERLAPPING_SEMANTIC_EDITS: semantic edits resolve to overlapping or ambiguous spans"
                );
            }
        }
    }

    splices.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
    });
    for splice in splices {
        lines.splice(splice.start..splice.end, splice.replacement);
    }

    let mut out = lines.join("\n");
    if keep_final_newline && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn usize_field(object: &Map<String, Value>, name: &str) -> Result<usize> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| anyhow!("semantic concrete edit requires integer {name}"))
}

fn syntax_error_count(tree: &Tree) -> usize {
    fn count(node: Node<'_>) -> usize {
        let mut total = usize::from(node.is_error() || node.is_missing());
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            total += count(child);
        }
        total
    }
    count(tree.root_node())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_resolves(path: &str, source: &str, kind: &str, name: &str, container: &[&str]) {
        let language = language_for_path(Path::new(path)).unwrap();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        let target = SyntaxTarget {
            kind: Some(kind.into()),
            name: name.into(),
            container: container.iter().map(|value| (*value).to_owned()).collect(),
            signature: None,
        };
        let candidate = resolve_target(&tree, source, &target).unwrap();
        assert_eq!(candidate.kind, kind);
        if matches!(kind, "function" | "method") {
            assert!(body_node(candidate.node).is_some());
        }
    }

    #[test]
    fn semantic_body_replacement_preserves_surrounding_declaration() {
        let source = "impl Engine {\n    fn start(&self) { old(); }\n}\n";
        let mut edits = vec![json!({
            "kind": "replaceBody",
            "target": {"kind": "method", "name": "start", "container": "Engine"},
            "text": "{ fresh(); }"
        })];

        concretize_semantic_edits(Path::new("sample.rs"), source, &mut edits).unwrap();

        assert_eq!(edits[0]["kind"], "replace");
        assert_eq!(edits[0]["range"]["start"], 2);
        assert_eq!(edits[0]["range"]["end"], 2);
        assert_eq!(edits[0]["text"], "    fn start(&self) { fresh(); }");
    }

    #[test]
    fn supported_languages_resolve_named_callables() {
        for (path, source, kind, name, container) in [
            (
                "sample.rs",
                "impl Engine { fn start(&self) {} }\n",
                "method",
                "start",
                &["Engine"][..],
            ),
            (
                "sample.ts",
                "class Engine { start(): void {} }\n",
                "method",
                "start",
                &["Engine"][..],
            ),
            (
                "sample.js",
                "class Engine { start() {} }\n",
                "method",
                "start",
                &["Engine"][..],
            ),
            (
                "sample.py",
                "class Engine:\n    def start(self):\n        pass\n",
                "method",
                "start",
                &["Engine"][..],
            ),
            (
                "sample.c",
                "void start(void) {}\n",
                "function",
                "start",
                &[][..],
            ),
            (
                "sample.cpp",
                "class Engine { public: void start() {} };\n",
                "method",
                "start",
                &["Engine"][..],
            ),
            (
                "sample.swift",
                "final class Engine { func start() {} }\n",
                "method",
                "start",
                &["Engine"][..],
            ),
        ] {
            assert_resolves(path, source, kind, name, container);
        }
    }

    #[test]
    fn semantic_edits_fail_closed() {
        let ambiguous = "fn run() {}\nmod nested { fn run() {} }\n";
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(ambiguous, None).unwrap();
        let target = SyntaxTarget {
            kind: Some("function".into()),
            name: "run".into(),
            container: Vec::new(),
            signature: None,
        };
        assert!(
            resolve_target(&tree, ambiguous, &target)
                .unwrap_err()
                .to_string()
                .contains("SYMBOL_AMBIGUOUS")
        );

        let source = "fn run() { ok(); }\n";
        let mut invalid = vec![json!({
            "kind": "replaceBody",
            "target": {"kind": "function", "name": "run"},
            "text": "{ let = ; }"
        })];
        assert!(
            concretize_semantic_edits(Path::new("sample.rs"), source, &mut invalid)
                .unwrap_err()
                .to_string()
                .contains("INVALID_RESULT")
        );

        let mut overlapping = vec![
            json!({
                "kind": "replaceNode",
                "target": {"kind": "function", "name": "run"},
                "text": "fn run() { first(); }"
            }),
            json!({
                "kind": "replaceBody",
                "target": {"kind": "function", "name": "run"},
                "text": "{ second(); }"
            }),
        ];
        assert!(
            concretize_semantic_edits(Path::new("sample.rs"), source, &mut overlapping)
                .unwrap_err()
                .to_string()
                .contains("OVERLAPPING_SEMANTIC_EDITS")
        );
    }
}
