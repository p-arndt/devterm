//! Inline settings overlay: an arrow-key-navigable editor for `config.toml`.
//!
//! Opened with the "open settings" action, this draws a centered panel with two pages:
//! **General** (font, theme, shell, cursor, …) and **Keybindings** (rebind any action by
//! pressing a new chord). `Tab` switches pages. It edits a working copy of [`Config`]; on
//! close, if anything changed, the copy is serialized back to disk and the existing file
//! watcher hot-reloads it. `Ctrl+E` closes the overlay and opens the raw file in an editor.
//!
//! The panel is rendered like any other pane: [`SettingsMenu::snapshot`] synthesizes a
//! [`Snapshot`] (a grid of cells) that the renderer paints via the overlay layer, so no
//! new draw path is needed.

use devterm_config::{Action, Config, CursorShapePref, KeyChord, KeymapPreset, ShellChoice, Theme};
use devterm_term::{Cursor, CursorShape, Palette, RenderCell, Rgb, Snapshot};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::input::chord_from_event;

/// Left margin (cols) before labels.
const LEFT: u16 = 2;
/// Column where values start on the General page.
const VALUE_COL: u16 = LEFT + 16;
/// Column where chords start on the Keybindings page (labels are longer there).
const KEY_VALUE_COL: u16 = LEFT + 22;
/// First body row (below the tab header).
const BODY_TOP: u16 = 3;

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

/// Which page of the overlay is showing.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    General,
    Keybindings,
}

impl Page {
    fn toggle(self) -> Page {
        match self {
            Page::General => Page::Keybindings,
            Page::Keybindings => Page::General,
        }
    }
}

/// One editable row on the General page.
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
                let idx = SHAPES
                    .iter()
                    .position(|s| *s == cfg.cursor.shape)
                    .unwrap_or(0);
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
    /// Which page is showing.
    page: Page,
    // --- General page ---
    /// Currently highlighted row on the General page.
    selected: usize,
    /// `Some(buffer)` while inline-editing the selected text field.
    editing: Option<String>,
    // --- Keybindings page ---
    /// One row per action with its current chord (`None` = unbound).
    keys: Vec<(Action, Option<KeyChord>)>,
    /// Currently highlighted row on the Keybindings page.
    key_selected: usize,
    /// `true` while waiting for the user to press a chord to rebind the selected action.
    capturing: bool,
    /// Transient status line (conflict warnings, prompts).
    message: Option<String>,
}

impl SettingsMenu {
    /// Open the overlay editing a copy of `config`.
    pub(super) fn new(config: Config) -> Self {
        let keys = key_rows(&config);
        SettingsMenu {
            config,
            dirty: false,
            page: Page::General,
            selected: 0,
            editing: None,
            keys,
            key_selected: 0,
            capturing: false,
            message: None,
        }
    }

    /// Handle one pressed key event. Returns what the caller should do next.
    pub(super) fn handle_key(
        &mut self,
        event: &KeyEvent,
        mods: ModifiersState,
    ) -> SettingsResponse {
        // Chord-capture (Keybindings page) swallows everything until it resolves.
        if self.capturing {
            return self.handle_capture(event, mods);
        }
        // Inline text editing (General page) swallows everything until Enter/Escape.
        if self.editing.is_some() {
            return self.handle_text_edit(&event.logical_key, event.text.as_deref());
        }

        let key = &event.logical_key;
        // Ctrl+E: bail out to the raw-file editor from either page.
        if mods.control_key()
            && let Key::Character(s) = key
            && s.eq_ignore_ascii_case("e")
        {
            return SettingsResponse::OpenEditor;
        }
        match key {
            Key::Named(NamedKey::Tab) => {
                self.page = self.page.toggle();
                self.message = None;
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::Escape) => SettingsResponse::Close,
            _ => match self.page {
                Page::General => self.handle_general(key),
                Page::Keybindings => self.handle_keybindings(key),
            },
        }
    }

    // --- General page ---------------------------------------------------------

    fn handle_general(&mut self, key: &Key) -> SettingsResponse {
        match key {
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

    /// Adjust the selected General field (no-op / edit-only for text fields).
    fn change(&mut self, dir: i32) -> SettingsResponse {
        let field = Field::ALL[self.selected];
        if field.is_text() {
            return SettingsResponse::Ignore;
        }
        field.adjust(&mut self.config, dir);
        // Switching the keymap preset changes every default chord: re-seed the rows.
        if let Field::KeymapPreset = field {
            self.keys = key_rows(&self.config);
        }
        self.dirty = true;
        SettingsResponse::Redraw
    }

    fn handle_text_edit(&mut self, key: &Key, text: Option<&str>) -> SettingsResponse {
        let Some(buffer) = self.editing.as_mut() else {
            return SettingsResponse::Ignore;
        };
        match key {
            Key::Named(NamedKey::Enter) => {
                let value = self.editing.take().unwrap();
                Field::ALL[self.selected].set_text(&mut self.config, value);
                self.dirty = true;
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::Escape) => {
                self.editing = None;
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::Backspace) => {
                buffer.pop();
                SettingsResponse::Redraw
            }
            _ => {
                if let Some(text) = text {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        buffer.push(ch);
                    }
                    SettingsResponse::Redraw
                } else {
                    SettingsResponse::Ignore
                }
            }
        }
    }

    // --- Keybindings page -----------------------------------------------------

    fn handle_keybindings(&mut self, key: &Key) -> SettingsResponse {
        match key {
            Key::Named(NamedKey::ArrowUp) => {
                self.key_selected = wrap(self.keys.len(), self.key_selected, -1);
                self.message = None;
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.key_selected = wrap(self.keys.len(), self.key_selected, 1);
                self.message = None;
                SettingsResponse::Redraw
            }
            Key::Named(NamedKey::Enter) => {
                self.capturing = true;
                self.message = None;
                SettingsResponse::Redraw
            }
            _ => SettingsResponse::Ignore,
        }
    }

    /// While capturing, the next real chord rebinds the selected action; Escape cancels.
    fn handle_capture(&mut self, event: &KeyEvent, mods: ModifiersState) -> SettingsResponse {
        if let Key::Named(NamedKey::Escape) = event.logical_key {
            self.capturing = false;
            self.message = None;
            return SettingsResponse::Redraw;
        }
        // Ignore key auto-repeat so holding the Enter that opened capture cannot bind Enter.
        if event.repeat {
            return SettingsResponse::Ignore;
        }
        // Ignore pure-modifier / dead-key presses and keep waiting.
        let Some(chord) = chord_from_event(event, mods) else {
            return SettingsResponse::Ignore;
        };

        let action = self.keys[self.key_selected].0;
        // Reject a chord already used by another action rather than silently stealing it.
        if let Some((other, _)) = self
            .keys
            .iter()
            .find(|(a, c)| *a != action && *c == Some(chord))
        {
            self.message = Some(format!(
                "{} is already bound to {}",
                pretty_chord(&chord),
                action_label(*other)
            ));
            return SettingsResponse::Redraw;
        }

        self.rebind(action, chord);
        self.capturing = false;
        self.message = None;
        SettingsResponse::Redraw
    }

    /// Record a rebinding into the working config and the visible rows.
    ///
    /// User keybindings are authoritative per-action (see [`Config::resolve_keybindings`]),
    /// so we drop any existing entries that name this action or claim this chord, then
    /// record an explicit override only when it differs from the preset default (rebinding
    /// back to the default simply removes the override).
    fn rebind(&mut self, action: Action, chord: KeyChord) {
        let preset_chord = self
            .config
            .keymap_preset
            .bindings()
            .into_iter()
            .find(|(_, a)| *a == action)
            .map(|(c, _)| c);

        let action_str = action.as_str();
        self.config.keybindings.retain(|k, v| {
            let same_action = v == action_str;
            let same_chord = k.parse::<KeyChord>().map(|c| c == chord).unwrap_or(false);
            !(same_action || same_chord)
        });
        if preset_chord != Some(chord) {
            self.config
                .keybindings
                .insert(chord.to_string(), action_str.to_owned());
        }

        if let Some(row) = self.keys.iter_mut().find(|(a, _)| *a == action) {
            row.1 = Some(chord);
        }
        self.dirty = true;
    }

    // --- rendering ------------------------------------------------------------

    /// Build the panel's frame as a synthetic [`Snapshot`] sized to `cols`x`rows`.
    pub(super) fn snapshot(&self, palette: &Palette, cols: u16, rows: u16) -> Snapshot {
        let fg = palette.foreground;
        let bg = palette.background;
        let accent = palette.ansi[6]; // value colour (cyan)
        let dim = palette.ansi[8]; // inactive tabs / hints (bright black)

        let mut cells: Vec<RenderCell> = Vec::new();
        let mut cursor = Cursor {
            line: 0,
            col: 0,
            shape: CursorShape::Hidden,
            color: palette.cursor,
        };
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

        // Header: two page tabs, the active one bold/accented.
        let (gen_fg, gen_bold) = if self.page == Page::General {
            (accent, true)
        } else {
            (dim, false)
        };
        let (key_fg, key_bold) = if self.page == Page::Keybindings {
            (accent, true)
        } else {
            (dim, false)
        };
        put(1, LEFT, "General", gen_fg, bg, gen_bold);
        put(1, LEFT + 10, "Keybindings", key_fg, bg, key_bold);

        match self.page {
            Page::General => self.draw_general(&mut put, &mut cursor, cols, fg, bg, accent),
            Page::Keybindings => self.draw_keybindings(&mut put, cols, rows, fg, bg, accent),
        }

        // Footer hint line. A pending message (e.g. a conflict warning) always wins so it is
        // visible even mid-capture.
        let (hint, hint_fg): (String, Rgb) = if let Some(message) = &self.message {
            (message.clone(), palette.ansi[9]) // bright red for warnings
        } else if self.capturing {
            ("Press the new key combo…    Esc: cancel".to_owned(), accent)
        } else if self.editing.is_some() {
            (
                "Type to edit    Enter: commit    Esc: cancel".to_owned(),
                dim,
            )
        } else {
            let text = match self.page {
                Page::General => {
                    "Up/Dn: move   Left/Right: change   Enter: edit   Tab: keybindings   Ctrl+E: file   Esc: save & close"
                }
                Page::Keybindings => {
                    "Up/Dn: move   Enter: rebind   Tab: general   Ctrl+E: file   Esc: save & close"
                }
            };
            (text.to_owned(), dim)
        };
        let footer = rows.saturating_sub(2);
        if footer > BODY_TOP {
            put(footer, LEFT, &hint, hint_fg, bg, false);
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

    #[allow(clippy::too_many_arguments)]
    fn draw_general(
        &self,
        put: &mut impl FnMut(u16, u16, &str, Rgb, Rgb, bool),
        cursor: &mut Cursor,
        cols: u16,
        fg: Rgb,
        bg: Rgb,
        accent: Rgb,
    ) {
        for (i, field) in Field::ALL.iter().enumerate() {
            let line = BODY_TOP + i as u16;
            let selected = i == self.selected;
            let editing = selected && self.editing.is_some();

            let value = match &self.editing {
                Some(buffer) if selected => buffer.clone(),
                _ => field.value(&self.config),
            };

            let (row_fg, row_bg, val_fg) = row_colors(selected, fg, bg, accent);
            if selected {
                fill_row(put, line, cols, row_fg, row_bg);
            }
            put(line, LEFT, field.label(), row_fg, row_bg, false);
            put(line, VALUE_COL, &value, val_fg, row_bg, false);

            if editing {
                let end = VALUE_COL + value.chars().count() as u16;
                *cursor = Cursor {
                    line,
                    col: end.min(cols.saturating_sub(1)),
                    shape: CursorShape::Beam,
                    color: cursor.color,
                };
            }
        }
    }

    fn draw_keybindings(
        &self,
        put: &mut impl FnMut(u16, u16, &str, Rgb, Rgb, bool),
        cols: u16,
        rows: u16,
        fg: Rgb,
        bg: Rgb,
        accent: Rgb,
    ) {
        let footer = rows.saturating_sub(2);
        let visible = footer.saturating_sub(BODY_TOP) as usize;
        if visible == 0 {
            return;
        }
        let start = scroll_start(self.keys.len(), visible, self.key_selected);

        for row in 0..visible {
            let idx = start + row;
            if idx >= self.keys.len() {
                break;
            }
            let (action, chord) = &self.keys[idx];
            let line = BODY_TOP + row as u16;
            let selected = idx == self.key_selected;

            let value = if selected && self.capturing {
                "<press keys>".to_owned()
            } else {
                match chord {
                    Some(c) => pretty_chord(c),
                    None => "(unset)".to_owned(),
                }
            };

            let (row_fg, row_bg, val_fg) = row_colors(selected, fg, bg, accent);
            if selected {
                fill_row(put, line, cols, row_fg, row_bg);
            }
            put(line, LEFT, action_label(*action), row_fg, row_bg, false);
            put(line, KEY_VALUE_COL, &value, val_fg, row_bg, false);
        }

        // A small "more below/above" affordance when the list is scrolled.
        if self.keys.len() > visible {
            let pos = format!("{}/{}", self.key_selected + 1, self.keys.len());
            let col = cols.saturating_sub(pos.len() as u16 + 1);
            put(1, col, &pos, accent, bg, false);
        }
    }
}

/// Seed one row per action with its currently resolved chord.
fn key_rows(config: &Config) -> Vec<(Action, Option<KeyChord>)> {
    let resolved = config.resolve_keybindings();
    Action::ALL
        .iter()
        .map(|action| {
            let chord = resolved.iter().find(|(_, a)| a == action).map(|(c, _)| *c);
            (*action, chord)
        })
        .collect()
}

/// The (label, background, value) colours for a row.
fn row_colors(selected: bool, fg: Rgb, bg: Rgb, accent: Rgb) -> (Rgb, Rgb, Rgb) {
    if selected {
        (bg, fg, bg) // inverted bar: text = bg on an fg background
    } else {
        (fg, bg, accent)
    }
}

/// Paint a full-width background bar for a highlighted row.
fn fill_row(
    put: &mut impl FnMut(u16, u16, &str, Rgb, Rgb, bool),
    line: u16,
    cols: u16,
    fg: Rgb,
    bg: Rgb,
) {
    for c in 0..cols {
        put(line, c, " ", fg, bg, false);
    }
}

/// The first visible index so `selected` stays within a `visible`-row window.
fn scroll_start(total: usize, visible: usize, selected: usize) -> usize {
    if total <= visible {
        0
    } else {
        let half = visible / 2;
        selected.saturating_sub(half).min(total - visible)
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

/// Render a chord for display, capitalizing each token (`ctrl+shift+c` -> `Ctrl+Shift+C`).
fn pretty_chord(chord: &KeyChord) -> String {
    chord
        .to_string()
        .split('+')
        .map(|token| {
            let mut chars = token.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// A friendly, human-readable name for an action.
fn action_label(action: Action) -> &'static str {
    match action {
        Action::SplitHorizontal => "Split horizontal",
        Action::SplitVertical => "Split vertical",
        Action::ClosePane => "Close pane",
        Action::NewTab => "New tab",
        Action::CloseTab => "Close tab",
        Action::NextTab => "Next tab",
        Action::PrevTab => "Previous tab",
        Action::FocusLeft => "Focus left",
        Action::FocusRight => "Focus right",
        Action::FocusUp => "Focus up",
        Action::FocusDown => "Focus down",
        Action::ResizeLeft => "Resize left",
        Action::ResizeRight => "Resize right",
        Action::ResizeUp => "Resize up",
        Action::ResizeDown => "Resize down",
        Action::Copy => "Copy",
        Action::Paste => "Paste",
        Action::ScrollLineUp => "Scroll line up",
        Action::ScrollLineDown => "Scroll line down",
        Action::ScrollPageUp => "Scroll page up",
        Action::ScrollPageDown => "Scroll page down",
        Action::OpenConfig => "Open config (file)",
        Action::OpenSettings => "Open settings",
        Action::ToggleFloatingTerminal => "Floating terminal",
        Action::Quit => "Quit",
    }
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
