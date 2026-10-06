//! Read-only `outline` and `find_symbol` tools over tree-sitter declarations.
//!
//! Both are best-effort navigation aids, not compilers: definitions come from
//! declaration nodes and references are exact identifier tokens, so shadowing
//! and same-named items in other modules are not distinguished.

use super::*;

const MAX_DEFINITIONS: usize = 20;
const MAX_REFERENCES: usize = 40;

impl ToolRegistry {
    pub(super) fn outline_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let requested = string_arg(object, "path")?;
        let path = self.context.resolve_session_path(requested)?;
        self.context
            .ensure_file_scope(&path.to_string_lossy(), false)?;
        if !symbols::supported(&path) {
            return Ok(format!(
                "{requested}: no symbols (outline supports Rust, TypeScript, and JavaScript); use read_file."
            ));
        }
        let source =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let symbols = symbols::outline(&path, &source);
        if symbols.is_empty() {
            return Ok(format!("{requested}: no declarations found."));
        }
        let mut text = format!(
            "{requested} ({} declarations; line: signature)\n",
            symbols.len()
        );
        for symbol in symbols {
            let indent = if symbol.depth > 0 { "    " } else { "  " };
            text.push_str(&format!("{indent}{}: {}\n", symbol.line, symbol.signature));
        }
        Ok(text)
    }

    pub(super) fn find_symbol_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let name = string_arg(object, "name")?.trim().to_owned();
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
        {
            bail!("find_symbol name must be a single identifier");
        }
        let requested = object
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .unwrap_or(".");
        let root = self.context.resolve_session_path(requested)?;
        self.context
            .ensure_file_scope(&root.to_string_lossy(), false)?;
        let display = |path: &Path| {
            path.strip_prefix(&self.context.working_directory)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        };

        let mut definitions = Vec::new();
        let mut references = Vec::new();
        let mut reference_files = 0usize;
        let mut truncated = false;
        for relative in repo_map::source_files(&root) {
            let path = root.join(&relative);
            let Some((symbols, identifiers)) = repo_map::cached_surface(&path) else {
                continue;
            };
            // The identifier index skips words shorter than three characters.
            if name.len() >= 3 && !identifiers.contains(&name) {
                continue;
            }
            let shown = display(&path);
            let mut definition_lines = HashSet::new();
            for symbol in symbols.iter().filter(|symbol| symbol.name == name) {
                definition_lines.insert(symbol.line);
                if definitions.len() < MAX_DEFINITIONS {
                    definitions.push(format!("  {shown}:{}  {}", symbol.line, symbol.signature));
                } else {
                    truncated = true;
                }
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = source.lines().collect();
            let mut seen_lines = HashSet::new();
            let mut file_has_reference = false;
            for occurrence in symbols::occurrences(&path, &source, &name) {
                if definition_lines.contains(&occurrence.line)
                    || !seen_lines.insert(occurrence.line)
                {
                    continue;
                }
                file_has_reference = true;
                if references.len() < MAX_REFERENCES {
                    let text = lines
                        .get(occurrence.line - 1)
                        .map_or("", |line| line.trim());
                    let text: String = text.chars().take(140).collect();
                    references.push(format!("  {shown}:{}  {text}", occurrence.line));
                } else {
                    truncated = true;
                }
            }
            reference_files += usize::from(file_has_reference);
        }

        let mut text = format!("Definitions of {name} ({}):\n", definitions.len());
        if definitions.is_empty() {
            text.push_str("  none found in Rust/TypeScript/JavaScript sources\n");
        }
        for line in &definitions {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&format!(
            "References ({} lines in {reference_files} files):\n",
            references.len()
        ));
        for line in &references {
            text.push_str(line);
            text.push('\n');
        }
        if truncated {
            text.push_str("Results truncated; narrow with path.\n");
        }
        Ok(text)
    }
}
