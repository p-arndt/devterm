//! Inline settings overlay: an arrow-key-navigable editor for `config.toml`.
//!
//! Opened with the "open settings" action, this draws a centered panel (reusing the
//! floating terminal's rectangle) listing the common config fields. It edits a working
//! copy of [`Config`]; on close, if anything changed, the copy is serialized back to
//! disk and the existing file watcher hot-reloads it. `Ctrl+E` closes the overlay and
//! falls back to opening the raw file in an editor.
//!
//! The panel is rendered like any other pane: [`SettingsMenu::snapshot`] synthesizes a
//! [`Snapshot`] (a grid of cells) that the renderer paints via the overlay layer, so no
//! new draw path is needed.

use devterm_config::{Config, CursorShapePref, KeymapPreset, ShellChoice, Theme};
use devterm_term::{Cursor, CursorShape, Palette, RenderCell, Rgb, Snapshot};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Left margin (cols) before labels, and the column where values start.
const LEFT: u16 = 2;
const VALUE_COL: u16 = LEFT + 18;

/// What the caller should do after a key was handled by the overlay.
pub(super) enum SettingsResponse {
    /// Key consumed; nothing visible changed.
    Ignore,
    /// Key consumed; the panel changed and should be repainted.
    Redraw,
    /// Close the overlay (persisting edits if any).
    Close,
    /// Close the overlay and open the raw config file in an editor.
    OpenEditor,
}

/// One editable row in the settings panel.
#[derive(Clone, Copy)]
enum Field {
    FontFamily,
    FontSize,
    LineHeight,
    Scrollback,
    Theme,
    KeymapPreset,
    Shell,
    CursorShape,
    CursorBlink,
}

impl Field {
    /// Every row, top to bottom.
    const ALL: [Field; 9] = [
        Field::FontFamily,
        Field::FontSize,
        Field::LineHeight,
        Field::Scrollback,
        Field::Theme,
        Field::KeymapPreset,
        Field::Shell,
        Field::CursorShape,
        Field::CursorBlink,
    ];

    fn label(self) -> &'static str {
        match self {
            Field::FontFamily => "Font family",
            Field::FontSize => "Font size",
            Field::LineHeight => "Line height",
            Field::Scrollback => "Scrollback",
            Field::Theme => "Theme",
            Field::KeymapPreset => "Keymap preset",
            Field::Shell => "Shell",
            Field::CursorShape => "Cursor shape",
            Field::CursorBlink => "Cursor blink",
        }
    }

    /// Text fields are edited by typing (Enter opens an inline text buffer); every other
    /// field is changed with Left/Right.
    fn is_text(self) -> bool {
        matches!(self, Field::FontFamily)
    }

    /// The current value rendered as display text.
    fn value(self, cfg: &Config) -> String {
        match self {
            Field::FontFamily => {
                if cfg.font_family.is_empty() {
                    "(default)".to_owned()
                } else {
                    cfg.font_family.clone()
                }
            }
            Field::FontSize => format!("{}", cfg.font_size),
            Field::LineHeight => format!("{}", cfg.line_height),
            Field::Scrollback => format!("{}", cfg.scrollback_lines),
            Field::Theme => cfg.theme_name.as_deref().unwrap_or("default").to_owned(),
            Field::KeymapPreset => match cfg.keymap_preset {
                KeymapPreset::Default => "default",
                KeymapPreset::Tmux => "tmux",
            }
            .to_owned(),
            Field::Shell => shell_name(cfg.shell).to_owned(),
            Field::CursorShape => cursor_shape_name(cfg.cursor.shape).to_owned(),
            Field::CursorBlink => if cfg.cursor.blink { "on" } else { "off" }.to_owned(),
        }
    }

    /// Change a non-text field by `dir` (+1 = Right/next, -1 = Left/prev). Toggles ignore
    /// the sign. No-op for text fields.
    fn adjust(self, cfg: &mut Config, dir: i32) {
        match self {
            Field::FontFamily => {}
            Field::FontSize => {
                cfg.font_size = (cfg.font_size + dir as f32).clamp(4.0, 72.0);
            }
            Field::LineHeight => {
                let next = (cfg.line_height + dir as f32 * 0.05).clamp(0.5, 3.0);
                // Round to 2 decimals so repeated stepping does not accrue float noise.
                cfg.line_height = (next * 100.0).round() / 100.0;
            }
            Field::Scrollback => {
                let next = cfg.scrollback_lines as i64 + dir as i64 * 1000;
                cfg.scrollback_lines = next.clamp(0, 1_000_000) as usize;
            }
            Field::Theme => {
                let names = Theme::BUILTIN_NAMES;
                let cur = cfg.theme_name.as_deref().unwrap_or("default");
                let idx = names.iter().position(|n| *n == cur).unwrap_or(0);
                let name = names[wrap(names.len(), idx, dir)];
                cfg.theme_name = if name == "default" {
                    None
                } else {
                    Some(name.to_owned())
                };
            }
            Field::KeymapPreset => {
                cfg.keymap_preset = match cfg.keymap_preset {
                    KeymapPreset::Default => KeymapPreset::Tmux,
                    KeymapPreset::Tmux => KeymapPreset::Default,
                };
            }
            Field::Shell => {
                const SHELLS: [ShellChoice; 6] = [
                    ShellChoice::Auto,
                    ShellChoice::Pwsh,
                    ShellChoice::WindowsPowerShell,
                    ShellChoice::Cmd,
                    ShellChoice::GitBash,
                    ShellChoice::Wsl,
                ];
                let idx = SHELLS.iter().position(|s| *s == cfg.shell).unwrap_or(0);
                cfg.shell = SHELLS[wrap(SHELLS.len(), idx, dir)];
            }
            Field::CursorShape => {
                const SHAPES: [CursorShapePref; 4] = [
                    CursorShapePref::Default,
                    CursorShapePref::Block,
                    CursorShapePref::Underline,
                    CursorShapePref::Beam,
                ];
                let idx = SHAPES.iter().position(|s| *s == cfg.cursor.shape).unwrap_or(0);
                cfg.cursor.shape = SHAPES[wrap(SHAPES.len(), idx, dir)];
            }
            Field::CursorBlink => cfg.cursor.blink = !cfg.cursor.blink,
        }
    }

    /// The raw string to seed the inline editor with (text fields only).
    fn edit_seed(self, cfg: &Config) -> String {
        match self {
            Field::FontFamily => cfg.font_family.clone(),
            _ => String::new(),
        }
    }

    /// Commit an edited text buffer back into the config (text fields only).
    fn set_text(self, cfg: &mut Config, text: String) {
        if let Field::FontFamily = self {
            cfg.font_family = text.trim().to_owned();
        }
    }
}

/// The live state of the inline settings overlay.
pub(super) struct SettingsMenu {
    /// Working copy of the config, mutated as the user edits.
    pub(super) config: Config,
    /// Whether any value changed (drives whether close persists to disk).
    pub(super) dirty: bool,
    /// Currently highlighted row.
    selected: usize,
    /// `Some(buffer)` while inline-editing the selected text field.
    editing: Option<String>,
}

impl SettingsMenu {
    /// Open the overlay editing a copy of `config`.
    pub(super) fn new(config: Config) -> Self {
        SettingsMenu {
            config,
            dirty: false,
            selected: 0,
            editing: None,
        }
    }

    /// Handle one pressed key. `text` is winit's committed text for the event (used for
    /// typing into a text field).
    pub(super) fn handle_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> SettingsResponse {
        // Inline text editing captures every key until Enter/Escape.
        if let Some(buffer) = self.editing.as_mut() {
            match key {
                Key::Named(NamedKey::Enter) => {
                    let value = self.editing.take().unwrap();
                    Field::ALL[self.selected].set_text(&mut self.config, value);
                    self.dirty = true;
                    return SettingsResponse::Redraw;
                }
                Key::Named(NamedKey::Escape) => {
                    self.editing = None;
                    return SettingsResponse::Redraw;
                }
                Key::Named(NamedKey::Backspace) => {
                    buffer.pop();
                    return SettingsResponse::Redraw;
                }
                _ => {
                    if let Some(text) = text {
                        for ch in text.chars().filter(|c| !c.is_control()) {
                            buffer.push(ch);
                        }
                        return SettingsResponse::Redraw;
                    }
                    return SettingsResponse::Ignore;
                }
            }
        }

        // Navigation mode.
        // Ctrl+E: bail out to the raw-file editor.
        if mods.control_key()
            && let Key::Character(s) = key
            && s.eq_ignore_ascii_case("e")
        {
            return SettingsResponse::OpenEditor;
        }

        match key {
            Key::Named(NamedKey::Escape) => SettingsResponse::Close,
            Key::Named(NamedKey::ArrowUp) => {
                self.selected = wrap(Field::ALL.len(), self.selected, -1);
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.selected = wrap(Field::ALL.len(), self.selected, 1);
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::ArrowLeft) => self.change(-1),
            Key::Named(NamedKey::ArrowRight) => self.change(1),
            Key::Named(NamedKey::Enter) => {
                let field = Field::ALL[self.selected];
                if field.is_text() {
                    self.editing = Some(field.edit_seed(&self.config));
                    SettingsResponse::Redraw
                } else {
                    self.change(1)
                }
            }
            _ => SettingsResponse::Ignore,
        }
    }

    /// Adjust the selected field, or start editing if it is a text field.
    fn change(&mut self, dir: i32) -> SettingsResponse {
        let field = Field::ALL[self.selected];
        if field.is_text() {
            return SettingsResponse::Ignore;
        }
        field.adjust(&mut self.config, dir);
        self.dirty = true;
        SettingsResponse::Redraw
    }

    /// Build the panel's frame as a synthetic [`Snapshot`] sized to `cols`x`rows`.
    pub(super) fn snapshot(&self, palette: &Palette, cols: u16, rows: u16) -> Snapshot {
        let fg = palette.foreground;
        let bg = palette.background;
        let accent = palette.ansi[6]; // value colour (cyan)
        let dim = palette.ansi[8]; // footer / hints (bright black)
        // Selected row is a full-width inverted bar.
        let sel_bg = palette.foreground;
        let sel_fg = palette.background;

        let mut cells: Vec<RenderCell> = Vec::new();
        let mut put = |line: u16, col: u16, s: &str, fg: Rgb, cell_bg: Rgb, bold: bool| {
            for (i, ch) in s.chars().enumerate() {
                let c = col + i as u16;
                if c >= cols || line >= rows {
                    break;
                }
                cells.push(RenderCell {
                    line,
                    col: c,
                    c: ch,
                    fg,
                    bg: cell_bg,
                    bold,
                    italic: false,
                    underline: false,
                    strikeout: false,
                    wide: false,
                });
            }
        };

        // Title.
        put(1, LEFT, "Settings", accent, bg, true);

        // Field rows start at line 3.
        let first_row = 3u16;
        let mut cursor = Cursor {
            line: 0,
            col: 0,
            shape: CursorShape::Hidden,
            color: palette.cursor,
        };

        for (i, field) in Field::ALL.iter().enumerate() {
            let line = first_row + i as u16;
            let selected = i == self.selected;
            let editing = selected && self.editing.is_some();

            let value = match &self.editing {
                Some(buffer) if selected => buffer.clone(),
                _ => field.value(&self.config),
            };

            let (row_fg, row_bg, val_fg) = if selected {
                (sel_fg, sel_bg, sel_fg)
            } else {
                (fg, bg, accent)
            };

            // Fill the whole row with the (possibly highlighted) background.
            if selected {
                for c in 0..cols {
                    put(line, c, " ", row_fg, row_bg, false);
                }
            }
            put(line, LEFT, field.label(), row_fg, row_bg, false);
            put(line, VALUE_COL, &value, val_fg, row_bg, false);

            if editing {
                // A beam cursor at the end of the edit buffer.
                let end = VALUE_COL + value.chars().count() as u16;
                cursor = Cursor {
                    line,
                    col: end.min(cols.saturating_sub(1)),
                    shape: CursorShape::Beam,
                    color: palette.cursor,
                };
            }
        }

        // Footer hint line.
        let hint = if self.editing.is_some() {
            "Type to edit    Enter: commit    Esc: cancel"
        } else {
            "Up/Dn: move   Left/Right: change   Enter: edit   Ctrl+E: file   Esc: save & close"
        };
        let footer = rows.saturating_sub(2);
        if footer > first_row {
            put(footer, LEFT, hint, dim, bg, false);
        }

        Snapshot {
            cols,
            rows,
            cells,
            cursor,
            default_fg: fg,
            default_bg: bg,
            scrollback_offset: 0,
            title: None,
        }
    }
}

/// Wrap `cur + dir` into `0..len`.
fn wrap(len: usize, cur: usize, dir: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let n = len as i32;
    (((cur as i32 + dir) % n + n) % n) as usize
}

fn shell_name(shell: ShellChoice) -> &'static str {
    match shell {
        ShellChoice::Auto => "auto",
        ShellChoice::Pwsh => "pwsh",
        ShellChoice::WindowsPowerShell => "windows-powershell",
        ShellChoice::Cmd => "cmd",
        ShellChoice::GitBash => "git-bash",
        ShellChoice::Wsl => "wsl",
    }
}

fn cursor_shape_name(shape: CursorShapePref) -> &'static str {
    match shape {
        CursorShapePref::Default => "default",
        CursorShapePref::Block => "block",
        CursorShapePref::Underline => "underline",
        CursorShapePref::Beam => "beam",
    }
}
