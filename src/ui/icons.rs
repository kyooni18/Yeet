//! Single source for TUI icons. Plain single-cell Unicode by default; set
//! `YEET_NERD_FONT=1` to use Nerd Font glyphs for file types and folders.
use std::sync::OnceLock;

fn nerd() -> bool {
    static NERD: OnceLock<bool> = OnceLock::new();
    *NERD.get_or_init(|| {
        std::env::var("YEET_NERD_FONT").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
    })
}

pub(super) fn home() -> &'static str {
    if nerd() { "\u{f015}" } else { "⌂" }
}

pub(super) fn session_tab() -> &'static str {
    if nerd() { "\u{f27a}" } else { "▤" }
}

pub(super) fn workspace() -> &'static str {
    if nerd() { "\u{f07b}" } else { "▣" }
}

pub(super) fn session(current: bool) -> &'static str {
    match (current, nerd()) {
        (true, true) => "\u{f111}",
        (false, true) => "\u{f10c}",
        (true, false) => "●",
        (false, false) => "○",
    }
}

pub(super) fn folder(open: bool) -> &'static str {
    match (open, nerd()) {
        (true, true) => "\u{f07c}",
        (false, true) => "\u{f07b}",
        (true, false) => "▾",
        (false, false) => "▸",
    }
}

pub(super) fn file(name: &str) -> &'static str {
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
