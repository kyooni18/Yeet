//! Read-only declaration outlines and identifier lookups backed by tree-sitter.
//!
//! This is navigation, not editing: unsupported languages and unparsable files
//! degrade to "no symbols" instead of failing, and every signature stops before
//! the body so outlines stay small and only change when an API changes.

use std::path::Path;

use tree_sitter::{Language, Node, Parser};

const MAX_SIGNATURE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub kind: &'static str,
    pub name: String,
    pub signature: String,
    /// 1-based declaration line.
    pub line: usize,
    /// 0 for top-level items, 1 for members of an impl/trait/class/interface.
    pub depth: u8,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub line: usize,
    pub column: usize,
}

pub fn supported(path: &Path) -> bool {
    language_for_path(path).is_some()
}

fn language_for_path(path: &Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "js" | "mjs" | "cjs" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        _ => return None,
    })
}

fn parse(path: &Path, source: &str) -> Option<tree_sitter::Tree> {
    let language = language_for_path(path)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(source, None)
}

/// Top-level declarations plus one level of members, in source order.
pub fn outline(path: &Path, source: &str) -> Vec<Symbol> {
    let Some(tree) = parse(path, source) else {
        return Vec::new();
    };
    let mut symbols = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        collect_item(child, source, 0, false, &mut symbols);
    }
    symbols
}

fn collect_item(node: Node<'_>, source: &str, depth: u8, exported: bool, out: &mut Vec<Symbol>) {
    let kind = node.kind();
    if kind == "export_statement" {
        if let Some(declaration) = node.child_by_field_name("declaration") {
            collect_item(declaration, source, depth, true, out);
        }
        return;
    }
    if kind == "lexical_declaration" || kind == "variable_declaration" {
        // `const handler = (...) => {...}` is the common exported-function form.
        let mut cursor = node.walk();
        for declarator in node.named_children(&mut cursor) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let Some(name) = field_text(declarator, "name", source) else {
                continue;
            };
            let callable = declarator
                .child_by_field_name("value")
                .is_some_and(|value| {
                    matches!(
                        value.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                });
            out.push(Symbol {
                kind: if callable { "function" } else { "const" },
                name,
                signature: signature(node, declarator.child_by_field_name("value"), source),
                line: node.start_position().row + 1,
                depth,
                public: exported,
            });
        }
        return;
    }
    let Some(symbol_kind) = declaration_kind(kind) else {
        return;
    };
    let name = match kind {
        "impl_item" => impl_name(node, source),
        _ => field_text(node, "name", source),
    };
    let Some(name) = name else {
        return;
    };
    let body = node.child_by_field_name("body");
    let stop = match kind {
        // Data declarations keep their whole (truncated) text: fields are API.
        "type_item" | "type_alias_declaration" => None,
        "const_item" | "static_item" => node.child_by_field_name("value"),
        _ => body,
    };
    // Rust spells visibility explicitly; TypeScript members are public unless
    // marked private (trait members inherit the trait's reach, close enough here).
    let rust_node = node.kind().ends_with("_item");
    let public = exported
        || is_rust_public(node)
        || (depth > 0 && !rust_node && !is_ts_private(node, source))
        || node.kind() == "function_signature_item";
    out.push(Symbol {
        kind: symbol_kind,
        name,
        signature: signature(node, stop, source),
        line: node.start_position().row + 1,
        depth,
        public,
    });
    if depth == 0
        && matches!(
            kind,
            "impl_item"
                | "trait_item"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
        )
        && let Some(body) = body
    {
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            collect_item(member, source, 1, false, out);
        }
    }
}

fn declaration_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_item"
        | "function_signature_item"
        | "function_declaration"
        | "generator_function_declaration"
        | "function_signature" => "function",
        "method_definition" | "method_signature" | "abstract_method_signature" => "method",
        "struct_item" => "struct",
        "enum_item" | "enum_declaration" => "enum",
        "trait_item" => "trait",
        "impl_item" => "impl",
        "type_item" | "type_alias_declaration" => "type",
        "const_item" | "static_item" => "const",
        "macro_definition" => "macro",
        "mod_item" | "internal_module" => "module",
        "class_declaration" | "abstract_class_declaration" => "class",
        "interface_declaration" => "interface",
        _ => return None,
    })
}

fn impl_name(node: Node<'_>, source: &str) -> Option<String> {
    let ty = field_text(node, "type", source)?;
    Some(match field_text(node, "trait", source) {
        Some(trait_name) => format!("{trait_name} for {ty}"),
        None => ty,
    })
}

fn is_rust_public(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "visibility_modifier")
}

fn is_ts_private(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "accessibility_modifier"
            && source.get(child.byte_range()) == Some("private")
    })
}

fn field_text(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    source
        .get(child.byte_range())
        .map(normalize_ws)
        .filter(|value| !value.is_empty())
}

/// Declaration text up to `stop` (normally the body), whitespace-normalized and
/// capped so one long generic bound cannot dominate an outline.
fn signature(node: Node<'_>, stop: Option<Node<'_>>, source: &str) -> String {
    let end = stop.map_or(node.end_byte(), |stop| stop.start_byte());
    let text = normalize_ws(source.get(node.start_byte()..end).unwrap_or_default());
    let text = text.trim_end_matches(['=', '{', ';', ' ']).to_owned();
    if text.chars().count() <= MAX_SIGNATURE_CHARS {
        return text;
    }
    let mut truncated: String = text.chars().take(MAX_SIGNATURE_CHARS).collect();
    truncated.push('…');
    truncated
}

fn normalize_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every identifier-like token spelled exactly `name`, excluding comments and
/// strings because those are not identifier nodes in either grammar.
pub fn occurrences(path: &Path, source: &str, name: &str) -> Vec<Occurrence> {
    if name.is_empty() || !source.contains(name) {
        return Vec::new();
    }
    let Some(tree) = parse(path, source) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            if node.kind().ends_with("identifier") && source.get(node.byte_range()) == Some(name) {
                let position = node.start_position();
                found.push(Occurrence {
                    line: position.row + 1,
                    column: position.column + 1,
                });
            }
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    found.sort_by_key(|occurrence| (occurrence.line, occurrence.column));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_rust_and_typescript_signatures_without_bodies() {
        let rust = "use x::y;\npub struct Engine { speed: u32 }\nimpl Engine {\n    pub fn start(&self, gear: u8) -> bool { true }\n    fn stop(&self) {}\n}\nfn helper() {}\n";
        let symbols = outline(Path::new("a.rs"), rust);
        let rendered: Vec<_> = symbols
            .iter()
            .map(|symbol| (symbol.depth, symbol.public, symbol.signature.as_str()))
            .collect();
        assert_eq!(
            rendered,
            [
                (0, true, "pub struct Engine"),
                (0, false, "impl Engine"),
                (1, true, "pub fn start(&self, gear: u8) -> bool"),
                (1, false, "fn stop(&self)"),
                (0, false, "fn helper()"),
            ]
        );

        let ts = "export interface Item { sku: string }\nexport class Inventory {\n  add(item: Item): void { }\n}\nexport const total = (xs: Item[]) => 0;\nfunction local() {}\n";
        let symbols = outline(Path::new("a.ts"), ts);
        let names: Vec<_> = symbols
            .iter()
            .map(|symbol| (symbol.kind, symbol.name.as_str(), symbol.public))
            .collect();
        assert_eq!(
            names,
            [
                ("interface", "Item", true),
                ("class", "Inventory", true),
                ("method", "add", true),
                ("function", "total", true),
                ("function", "local", false),
            ]
        );
        assert!(outline(Path::new("notes.md"), "# hi").is_empty());
    }
}
