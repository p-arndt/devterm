//! Winit-to-config input mapping: modifiers, named keys, chords, keymap and palette.
//!
//! These helpers translate winit's keyboard/theme types into the `devterm-config` and
//! `devterm-term` vocabulary. They hold no state, so they stay independently unit-testable.

use std::collections::HashMap;

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use devterm_config::{
    Action, Color, Config, CursorShapePref, KeyChord, KeyCode, Mods, Named, Theme,
};
use devterm_term::{CursorShape, Palette, Rgb};

/// Translate a pressed key into a [`KeyChord`], or `None` for pure modifiers / dead keys.
pub(super) fn chord_from_event(event: &KeyEvent, mods: ModifiersState) -> Option<KeyChord> {
    let code = match &event.logical_key {
        Key::Character(s) => KeyCode::Char(s.chars().next()?.to_ascii_lowercase()),
        Key::Named(named) => KeyCode::Named(map_named(*named)?),
        _ => return None,
    };
    Some(KeyChord::new(mods_from_winit(mods), code))
}

/// Resolve config keybindings into a chord -> action lookup (later bindings win).
pub(super) fn build_keymap(config: &Config) -> HashMap<KeyChord, Action> {
    config.resolve_keybindings().into_iter().collect()
}

/// Map a config [`Theme`] onto a `devterm-term` [`Palette`].
pub(super) fn palette_from_theme(theme: &Theme) -> Palette {
    let convert = |c: Color| Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    };
    let mut ansi = [Rgb::default(); 16];
    for (dst, src) in ansi.iter_mut().zip(theme.ansi.iter()) {
        *dst = convert(*src);
    }
    Palette {
        ansi,
        foreground: convert(theme.foreground),
        background: convert(theme.background),
        cursor: convert(theme.cursor),
    }
}

/// Map a config [`CursorShapePref`] onto the term's fallback [`CursorShape`].
///
/// The term only overrides a *reported* `Block` (alacritty's default) with this fallback,
/// so `Block` is the neutral value: [`CursorShapePref::Default`] maps to it and thereby
/// lets the running program's own DECSCUSR choice win. Applying the mapped shape on every
/// pane also makes reloads correct — switching back to `Default` resets the override.
pub(super) fn term_cursor_shape(pref: CursorShapePref) -> CursorShape {
    match pref {
        CursorShapePref::Default | CursorShapePref::Block => CursorShape::Block,
        CursorShapePref::Underline => CursorShape::Underline,
        CursorShapePref::Beam => CursorShape::Beam,
    }
}

/// Map winit modifier state onto config [`Mods`].
fn mods_from_winit(mods: ModifiersState) -> Mods {
    Mods {
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        shift: mods.shift_key(),
        logo: mods.super_key(),
    }
}

/// Map a winit [`NamedKey`] onto a config [`Named`], or `None` for keys we do not bind.
fn map_named(named: NamedKey) -> Option<Named> {
    Some(match named {
        NamedKey::Enter => Named::Enter,
        NamedKey::Tab => Named::Tab,
        NamedKey::Escape => Named::Escape,
        NamedKey::Space => Named::Space,
        NamedKey::Backspace => Named::Backspace,
        NamedKey::Delete => Named::Delete,
        NamedKey::Insert => Named::Insert,
        NamedKey::Home => Named::Home,
        NamedKey::End => Named::End,
        NamedKey::PageUp => Named::PageUp,
        NamedKey::PageDown => Named::PageDown,
        NamedKey::ArrowUp => Named::ArrowUp,
        NamedKey::ArrowDown => Named::ArrowDown,
        NamedKey::ArrowLeft => Named::ArrowLeft,
        NamedKey::ArrowRight => Named::ArrowRight,
        NamedKey::F1 => Named::F1,
        NamedKey::F2 => Named::F2,
        NamedKey::F3 => Named::F3,
        NamedKey::F4 => Named::F4,
        NamedKey::F5 => Named::F5,
        NamedKey::F6 => Named::F6,
        NamedKey::F7 => Named::F7,
        NamedKey::F8 => Named::F8,
        NamedKey::F9 => Named::F9,
        NamedKey::F10 => Named::F10,
        NamedKey::F11 => Named::F11,
        NamedKey::F12 => Named::F12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_keys() {
        assert_eq!(map_named(NamedKey::Enter), Some(Named::Enter));
        assert_eq!(map_named(NamedKey::ArrowLeft), Some(Named::ArrowLeft));
        assert_eq!(map_named(NamedKey::PageUp), Some(Named::PageUp));
        assert_eq!(map_named(NamedKey::F5), Some(Named::F5));
        // A key we deliberately do not bind.
        assert_eq!(map_named(NamedKey::CapsLock), None);
    }

    #[test]
    fn winit_mods_map_to_config_mods() {
        let mods = mods_from_winit(ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert!(mods.ctrl && mods.shift);
        assert!(!mods.alt && !mods.logo);
    }

    #[test]
    fn theme_maps_to_palette() {
        let palette = palette_from_theme(&Theme::default());
        assert_eq!(
            palette.foreground,
            Rgb {
                r: 0xd0,
                g: 0xd0,
                b: 0xd0
            }
        );
        assert_eq!(palette.background, Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            palette.ansi[1],
            Rgb {
                r: 0x80,
                g: 0,
                b: 0
            }
        );

        // A different theme yields a different background.
        let gruvbox = palette_from_theme(&Theme::builtin("gruvbox-dark").unwrap());
        assert_ne!(gruvbox.background, palette.background);
    }

    #[test]
    fn cursor_pref_maps_to_term_shape() {
        // Default is neutral (Block), letting the program's own shape win.
        assert_eq!(
            term_cursor_shape(CursorShapePref::Default),
            CursorShape::Block
        );
        assert_eq!(
            term_cursor_shape(CursorShapePref::Block),
            CursorShape::Block
        );
        assert_eq!(
            term_cursor_shape(CursorShapePref::Underline),
            CursorShape::Underline
        );
        assert_eq!(term_cursor_shape(CursorShapePref::Beam), CursorShape::Beam);
    }

    #[test]
    fn default_keymap_binds_split_chord() {
        // The resolved lookup contains the documented default Copy chord.
        let map = build_keymap(&Config::default());
        let chord: KeyChord = "ctrl+shift+c".parse().unwrap();
        assert_eq!(map.get(&chord), Some(&Action::Copy));
    }
}
