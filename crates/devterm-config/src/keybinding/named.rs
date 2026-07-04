//! Named (non-character) keys and their canonical string names.
//!
//! [`Named`] covers the keys that have no single-character representation
//! (arrows, function keys, `enter`, and so on). Its parse/display helpers keep
//! the spelling in chord strings consistent with what chords render back to.

/// A named (non-character) key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Named {
    Enter,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl Named {
    /// The canonical lowercase name used in chord strings.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Named::Enter => "enter",
            Named::Tab => "tab",
            Named::Escape => "escape",
            Named::Space => "space",
            Named::Backspace => "backspace",
            Named::Delete => "delete",
            Named::Insert => "insert",
            Named::Home => "home",
            Named::End => "end",
            Named::PageUp => "pageup",
            Named::PageDown => "pagedown",
            Named::ArrowUp => "up",
            Named::ArrowDown => "down",
            Named::ArrowLeft => "left",
            Named::ArrowRight => "right",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
            Named::F6 => "f6",
            Named::F7 => "f7",
            Named::F8 => "f8",
            Named::F9 => "f9",
            Named::F10 => "f10",
            Named::F11 => "f11",
            Named::F12 => "f12",
        }
    }

    /// Parse a name token (already lowercased). Accepts a few aliases.
    pub(crate) fn parse(token: &str) -> Option<Named> {
        let named = match token {
            "enter" | "return" => Named::Enter,
            "tab" => Named::Tab,
            "escape" | "esc" => Named::Escape,
            "space" => Named::Space,
            "backspace" => Named::Backspace,
            "delete" | "del" => Named::Delete,
            "insert" | "ins" => Named::Insert,
            "home" => Named::Home,
            "end" => Named::End,
            "pageup" | "pgup" => Named::PageUp,
            "pagedown" | "pgdn" | "pgdown" => Named::PageDown,
            "up" | "arrowup" => Named::ArrowUp,
            "down" | "arrowdown" => Named::ArrowDown,
            "left" | "arrowleft" => Named::ArrowLeft,
            "right" | "arrowright" => Named::ArrowRight,
            "f1" => Named::F1,
            "f2" => Named::F2,
            "f3" => Named::F3,
            "f4" => Named::F4,
            "f5" => Named::F5,
            "f6" => Named::F6,
            "f7" => Named::F7,
            "f8" => Named::F8,
            "f9" => Named::F9,
            "f10" => Named::F10,
            "f11" => Named::F11,
            "f12" => Named::F12,
            _ => return None,
        };
        Some(named)
    }
}
