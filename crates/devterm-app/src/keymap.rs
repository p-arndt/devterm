//! Keyboard translation: winit `KeyEvent` -> bytes for the PTY.
//!
//! The mapping targets an xterm-compatible child (ConPTY/PowerShell). It handles named
//! keys (Enter, Tab, arrows, navigation, function keys), `Ctrl`+letter control bytes, and
//! plain text produced by the keypress. Kept independent of the event loop so it stays
//! unit-testable.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Translate a pressed key into the byte sequence to forward to the child.
///
/// Returns `None` when the key produces no input (pure modifiers, dead keys, unmapped
/// named keys).
pub fn keymap(event: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    // Named keys take precedence so Enter/Tab/Esc emit canonical sequences even when the
    // platform also fills in `text`.
    if let Key::Named(named) = event.logical_key
        && let Some(bytes) = named_bytes(named)
    {
        return Some(bytes.to_vec());
    }

    // Ctrl+letter (and a handful of Ctrl+symbol combinations) map to C0 control bytes.
    if mods.control_key()
        && let Key::Character(s) = &event.logical_key
        && let Some(c) = s.chars().next()
        && let Some(b) = ctrl_byte(c)
    {
        return Some(vec![b]);
    }

    // Otherwise forward the text the platform produced for this keypress.
    if let Some(text) = &event.text
        && !text.is_empty()
    {
        return Some(text.as_bytes().to_vec());
    }

    // Fallback: a character logical key with no `text` (and no active Ctrl mapping).
    if let Key::Character(s) = &event.logical_key
        && !mods.control_key()
        && !s.is_empty()
    {
        return Some(s.as_bytes().to_vec());
    }

    None
}

/// Byte sequence for a named key, or `None` if it is not an input-producing key.
fn named_bytes(named: NamedKey) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match named {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => b"\x7f",
        NamedKey::Tab => b"\t",
        NamedKey::Escape => b"\x1b",
        NamedKey::Space => b" ",

        NamedKey::ArrowUp => b"\x1b[A",
        NamedKey::ArrowDown => b"\x1b[B",
        NamedKey::ArrowRight => b"\x1b[C",
        NamedKey::ArrowLeft => b"\x1b[D",

        NamedKey::Home => b"\x1b[H",
        NamedKey::End => b"\x1b[F",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        NamedKey::Insert => b"\x1b[2~",
        NamedKey::Delete => b"\x1b[3~",

        NamedKey::F1 => b"\x1bOP",
        NamedKey::F2 => b"\x1bOQ",
        NamedKey::F3 => b"\x1bOR",
        NamedKey::F4 => b"\x1bOS",
        NamedKey::F5 => b"\x1b[15~",
        NamedKey::F6 => b"\x1b[17~",
        NamedKey::F7 => b"\x1b[18~",
        NamedKey::F8 => b"\x1b[19~",
        NamedKey::F9 => b"\x1b[20~",
        NamedKey::F10 => b"\x1b[21~",
        NamedKey::F11 => b"\x1b[23~",
        NamedKey::F12 => b"\x1b[24~",

        _ => return None,
    };
    Some(bytes)
}

/// Control byte for a `Ctrl`+`c` combination (`c & 0x1f` for letters, plus common symbols).
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' | 'A'..='Z' => Some((c as u8) & 0x1f),
        ' ' | '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '/' => Some(0x1f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_letter_maps_to_control_byte() {
        assert_eq!(ctrl_byte('c'), Some(0x03));
        assert_eq!(ctrl_byte('C'), Some(0x03));
        assert_eq!(ctrl_byte('a'), Some(0x01));
        assert_eq!(ctrl_byte('['), Some(0x1b));
    }

    #[test]
    fn named_sequences() {
        assert_eq!(named_bytes(NamedKey::Enter), Some(&b"\r"[..]));
        assert_eq!(named_bytes(NamedKey::ArrowUp), Some(&b"\x1b[A"[..]));
        assert_eq!(named_bytes(NamedKey::F5), Some(&b"\x1b[15~"[..]));
        assert_eq!(named_bytes(NamedKey::Shift), None);
    }
}
