//! Key -> action layer. Views ask the keymap what an event means and then
//! perform the action, so rebinding a key never touches view behavior.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Close,
    FocusInput,
    ToggleHints,
    MoveUp,
    MoveDown,
    MoveTop,
    MoveBottom,
    PageUp,
    PageDown,
    ParentFolder,
    OpenEntry,
    OpenAsTab,
    OpenViews,
    NextTab,
    PrevTab,
    ToggleInfo,
    ToggleChangedOnly,
    ToggleDiff,
    Find,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Close => "close",
            Action::FocusInput => "input",
            Action::ToggleHints => "hints",
            Action::MoveUp => "up",
            Action::MoveDown => "down",
            Action::MoveTop => "top",
            Action::MoveBottom => "bottom",
            Action::PageUp => "page up",
            Action::PageDown => "page down",
            Action::ParentFolder => "parent folder",
            Action::OpenEntry => "open",
            Action::OpenAsTab => "open as tab",
            Action::OpenViews => "views",
            Action::NextTab => "next tab",
            Action::PrevTab => "previous tab",
            Action::ToggleInfo => "info",
            Action::ToggleChangedOnly => "changed only",
            Action::ToggleDiff => "diff",
            Action::Find => "find",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySpec {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            modifiers: normalize(code, modifiers),
        }
    }

    pub fn plain(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    pub fn char(c: char) -> Self {
        Self::plain(KeyCode::Char(c))
    }

    /// Accepts `q`, `?`, `esc`, `backspace`, `space`, `ctrl+n`, `alt+f`, ...
    pub fn parse(text: &str) -> Option<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let mut rest = text;
        while let Some((prefix, tail)) = rest.split_once('+') {
            if tail.is_empty() {
                break;
            }
            modifiers |= match prefix.to_ascii_lowercase().as_str() {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "cmd" | "super" => KeyModifiers::SUPER,
                _ => return None,
            };
            rest = tail;
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "esc" => KeyCode::Esc,
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "space" => KeyCode::Char(' '),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            _ => {
                let mut chars = rest.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        };
        Some(Self::new(code, modifiers))
    }

    pub fn display(&self) -> String {
        let base = match self.code {
            KeyCode::Esc => "esc".to_owned(),
            KeyCode::Enter => "enter".to_owned(),
            KeyCode::Tab => "tab".to_owned(),
            KeyCode::Backspace => "⌫".to_owned(),
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::Home => "home".to_owned(),
            KeyCode::End => "end".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            other => format!("{other:?}").to_lowercase(),
        };
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            out.push_str("cmd+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("alt+");
        }
        out + &base
    }
}

/// Shift is already encoded in the character (`?`, `G`), so drop it.
fn normalize(code: KeyCode, modifiers: KeyModifiers) -> KeyModifiers {
    match code {
        KeyCode::Char(_) => modifiers - KeyModifiers::SHIFT,
        _ => modifiers,
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<(Context, KeySpec, Action)>,
}

impl Default for Keymap {
    fn default() -> Self {
        use Action::*;
        let mut map = Self {
            bindings: Vec::new(),
        };
        let files = [
            ("esc", Close),
            ("q", Close),
            ("i", FocusInput),
            ("?", ToggleHints),
            ("j", MoveDown),
            ("down", MoveDown),
            ("k", MoveUp),
            ("up", MoveUp),
            ("g", MoveTop),
            ("home", MoveTop),
            ("G", MoveBottom),
            ("end", MoveBottom),
            ("backspace", ParentFolder),
            ("h", ParentFolder),
            ("left", ParentFolder),
            ("enter", OpenEntry),
            ("l", OpenEntry),
            ("right", OpenEntry),
            ("space", OpenAsTab),
            ("v", OpenViews),
            ("tab", NextTab),
            ("p", ToggleInfo),
            ("c", ToggleChangedOnly),
            ("d", ToggleDiff),
            ("/", Find),
            ("alt+up", PageUp),
            ("alt+down", PageDown),
            ("cmd+up", MoveTop),
            ("cmd+down", MoveBottom),
            ("alt+left", PrevTab),
            ("alt+right", NextTab),
            ("cmd+left", ParentFolder),
            ("cmd+right", OpenAsTab),
        ];
        for (key, action) in files {
            if let Some(spec) = KeySpec::parse(key) {
                map.bind(Context::Files, spec, action);
            }
        }
        map
    }
}

impl Keymap {
    /// Binds `key` to `action`, replacing whatever the key did in `context`.
    pub fn bind(&mut self, context: Context, key: KeySpec, action: Action) {
        self.bindings
            .retain(|(c, k, _)| !(*c == context && *k == key));
        self.bindings.push((context, key, action));
    }

    pub fn lookup(&self, context: Context, event: &KeyEvent) -> Option<Action> {
        let key = KeySpec::new(event.code, event.modifiers);
        self.bindings
            .iter()
            .find(|(c, k, _)| *c == context && *k == key)
            .map(|(_, _, action)| *action)
    }

    pub fn keys_for(&self, context: Context, action: Action) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(c, _, a)| *c == context && *a == action)
            .map(|(_, key, _)| key.display())
            .collect()
    }

    /// First key per action, in the order `actions` lists them.
    pub fn hints(&self, context: Context, actions: &[Action]) -> Vec<(String, &'static str)> {
        actions
            .iter()
            .filter_map(|action| {
                let key = self.keys_for(context, *action).into_iter().next()?;
                Some((key, action.label()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_characters_match_and_rebinding_replaces() {
        let mut map = Keymap::default();
        let question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert_eq!(
            map.lookup(Context::Files, &question),
            Some(Action::ToggleHints)
        );
        map.bind(Context::Files, KeySpec::char('x'), Action::ToggleInfo);
        map.bind(Context::Files, KeySpec::char('p'), Action::ToggleDiff);
        let p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(map.lookup(Context::Files, &p), Some(Action::ToggleDiff));
        assert_eq!(KeySpec::parse("ctrl+n").unwrap().display(), "ctrl+n");
        assert_eq!(map.keys_for(Context::Files, Action::ToggleInfo), ["x"]);
    }
}
