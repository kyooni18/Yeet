//! Single source for TUI icons. Nerd Font glyphs by default; set
//! `YEET_NERD_FONT=0` to fall back to plain single-cell Unicode.
use std::sync::OnceLock;

fn nerd() -> bool {
    static NERD: OnceLock<bool> = OnceLock::new();
    *NERD.get_or_init(|| {
        std::env::var("YEET_NERD_FONT").map_or(true, |v| {
            !matches!(v.as_str(), "0" | "false" | "no" | "off")
        })
    })
}

pub(in crate::tui::ui) fn home() -> &'static str {
    if nerd() { "\u{f015}" } else { "⌂" }
}

pub(in crate::tui::ui) fn session_tab() -> &'static str {
    if nerd() { "\u{f27a}" } else { "▤" }
}

pub(in crate::tui::ui) fn agent_tab() -> &'static str {
    if nerd() { "\u{f06a9}" } else { "♟" }
}

pub(in crate::tui::ui) fn diff_tab() -> &'static str {
    if nerd() { "\u{f407}" } else { "⑂" }
}

pub(in crate::tui::ui) fn chevron(expanded: bool) -> &'static str {
    match (expanded, nerd()) {
        (true, true) => "\u{f078}",
        (false, true) => "\u{f054}",
        (true, false) => "▾",
        (false, false) => "▸",
    }
}

pub(in crate::tui::ui) fn workspace() -> &'static str {
    if nerd() { "\u{f07b}" } else { "▣" }
}

pub(in crate::tui::ui) fn session(current: bool) -> &'static str {
    match (current, nerd()) {
        (true, true) => "\u{f111}",
        (false, true) => "\u{f10c}",
        (true, false) => "●",
        (false, false) => "○",
    }
}

pub(in crate::tui::ui) fn folder(open: bool) -> &'static str {
    match (open, nerd()) {
        (true, true) => "\u{f07c}",
        (false, true) => "\u{f07b}",
        (true, false) => "▾",
        (false, false) => "▸",
    }
}

pub(in crate::tui::ui) fn file(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let kind = match ext.as_deref() {
        Some(
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "swift" | "java"
            | "rb" | "sh",
        ) => Kind::Code,
        Some("md" | "txt" | "rst") => Kind::Doc,
        Some("json" | "toml" | "yaml" | "yml" | "ini" | "cfg") => Kind::Config,
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => Kind::Image,
        Some("lock") => Kind::Lock,
        _ => Kind::Other,
    };
    match (kind, nerd()) {
        (Kind::Code, true) => match ext.as_deref() {
            Some("rs") => "\u{e7a8}",
            Some("py") => "\u{e73c}",
            Some("js" | "jsx") => "\u{e74e}",
            Some("ts" | "tsx") => "\u{e628}",
            Some("c" | "h" | "cpp") => "\u{e61e}",
            _ => "\u{f121}",
        },
        (Kind::Doc, true) => "\u{e73e}",
        (Kind::Config, true) => "\u{e615}",
        (Kind::Image, true) => "\u{f1c5}",
        (Kind::Lock, true) => "\u{f023}",
        (Kind::Other, true) => "\u{f15b}",
        (Kind::Code, false) => "◇",
        (Kind::Doc, false) => "≡",
        (Kind::Config, false) => "◈",
        (Kind::Image, false) => "▨",
        (Kind::Lock, false) => "▪",
        (Kind::Other, false) => "·",
    }
}

enum Kind {
    Code,
    Doc,
    Config,
    Image,
    Lock,
    Other,
}

/// Translate shared visual meaning into terminal glyphs only.
pub(in crate::tui::ui) fn semantic(
    icon: crate::shared_ui::workbench::Icon,
    label: &str,
) -> &'static str {
    use crate::shared_ui::workbench::Icon;
    match icon {
        Icon::Home => home(),
        Icon::Conversation => session_tab(),
        Icon::Folder => folder(false),
        Icon::File => file(label),
        Icon::Changes => diff_tab(),
        Icon::Agents => agent_tab(),
        Icon::Add => "+",
    }
}
