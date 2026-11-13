//! Keybindings and terminal key encoding.

use std::collections::HashMap;

use alacritty_terminal::term::TermMode;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Application-level actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    ClosePane,
    NextTab,
    PrevTab,
    SelectTab(usize),
    SplitSideBySide,
    SplitStacked,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    Copy,
    Paste,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ClearScrollback,
}

impl Action {
    /// Parse an action name from the config, e.g. `"new_tab"`.
    pub fn from_name(name: &str) -> Option<Self> {
        use Action::*;
        Some(
            match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "new_tab" => NewTab,
                "close_pane" | "close_tab" => ClosePane,
                "next_tab" => NextTab,
                "prev_tab" | "previous_tab" => PrevTab,
                "select_tab" => SelectTab(0),
                "split_side_by_side" | "split_vertical" => SplitSideBySide,
                "split_stacked" | "split_horizontal" => SplitStacked,
                "focus_left" => FocusLeft,
                "focus_right" => FocusRight,
                "focus_up" => FocusUp,
                "focus_down" => FocusDown,
                "copy" => Copy,
                "paste" => Paste,
                "zoom_in" => ZoomIn,
                "zoom_out" => ZoomOut,
                "zoom_reset" => ZoomReset,
                "scroll_page_up" => ScrollPageUp,
                "scroll_page_down" => ScrollPageDown,
                "scroll_top" => ScrollTop,
                "scroll_bottom" => ScrollBottom,
                "clear_scrollback" => ClearScrollback,
                // `select_tab_1` .. `select_tab_9`
                other => {
                    let index = other.strip_prefix("select_tab_")?.parse::<usize>().ok()?;
                    SelectTab(index.saturating_sub(1))
                }
            },
        )
    }
}

/// Default keybindings. Users can override these in `[keybindings]`.
pub fn default_bindings() -> HashMap<String, Action> {
    use Action::*;
    let mut map = HashMap::new();
    let mut add = |chord: &str, action| {
        map.insert(chord.to_string(), action);
    };

    add("ctrl+shift+t", NewTab);
    add("ctrl+shift+w", ClosePane);
    add("ctrl+tab", NextTab);
    add("ctrl+shift+tab", PrevTab);
    for i in 1..=9 {
        add(&format!("ctrl+shift+{i}"), SelectTab(i - 1));
    }
    add("ctrl+shift+d", SplitSideBySide);
    add("ctrl+shift+e", SplitStacked);
    add("ctrl+shift+left", FocusLeft);
    add("ctrl+shift+right", FocusRight);
    add("ctrl+shift+up", FocusUp);
    add("ctrl+shift+down", FocusDown);
    add("ctrl+shift+c", Copy);
    add("ctrl+shift+v", Paste);
    add("ctrl+equal", ZoomIn);
    add("ctrl+plus", ZoomIn);
    add("ctrl+minus", ZoomOut);
    add("ctrl+0", ZoomReset);
    add("shift+pageup", ScrollPageUp);
    add("shift+pagedown", ScrollPageDown);
    add("ctrl+shift+home", ScrollTop);
    add("ctrl+shift+end", ScrollBottom);
    add("ctrl+shift+k", ClearScrollback);
    map
}

fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Character(s) => Some(s.to_lowercase()),
        Key::Named(named) => Some(format!("{named:?}").to_lowercase()),
        _ => None,
    }
}

/// Build a canonical chord string such as `ctrl+shift+t`.
pub fn chord(mods: ModifiersState, key: &Key) -> Option<String> {
    let name = key_name(key)?;
    let mut parts = Vec::with_capacity(5);
    if mods.control_key() {
        parts.push("ctrl");
    }
    if mods.alt_key() {
        parts.push("alt");
    }
    if mods.shift_key() {
        parts.push("shift");
    }
    if mods.super_key() {
        parts.push("super");
    }
    parts.push(&name);
    Some(parts.join("+"))
}

/// Look up an action for a key event.
pub fn action_for(
    bindings: &HashMap<String, Action>,
    mods: ModifiersState,
    key: &Key,
) -> Option<Action> {
    let chord = chord(mods, key)?;
    bindings.get(&chord).copied()
}

/// Encode a keypress for the PTY, honoring the terminal's current modes.
pub fn encode_key(key: &Key, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let mod_param = 1
        + u8::from(mods.shift_key())
        + 2 * u8::from(mods.alt_key())
        + 4 * u8::from(mods.control_key());

    match key {
        Key::Character(text) => {
            if text.is_empty() {
                return None;
            }
            if mods.control_key() {
                let c = text.chars().next()?.to_ascii_lowercase();
                let byte = match c {
                    'a'..='z' => Some(c as u8 - b'a' + 1),
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '^' => Some(0x1e),
                    '_' => Some(0x1f),
                    ' ' | '@' => Some(0x00),
                    '?' => Some(0x7f),
                    _ => None,
                };
                return byte.map(|b| vec![b]);
            }

            let mut bytes = text.as_bytes().to_vec();
            if mods.alt_key() {
                bytes.insert(0, 0x1b);
            }
            Some(bytes)
        }
        Key::Named(named) => {
            let mut out = match named {
                NamedKey::Enter => b"\r".to_vec(),
                NamedKey::Tab => {
                    if mods.shift_key() {
                        b"\x1b[Z".to_vec()
                    } else {
                        b"\t".to_vec()
                    }
                }
                NamedKey::Backspace => vec![0x7f],
                NamedKey::Escape => vec![0x1b],
                NamedKey::Space => b" ".to_vec(),
                NamedKey::ArrowUp => cursor_key(b'A', app_cursor, mod_param),
                NamedKey::ArrowDown => cursor_key(b'B', app_cursor, mod_param),
                NamedKey::ArrowRight => cursor_key(b'C', app_cursor, mod_param),
                NamedKey::ArrowLeft => cursor_key(b'D', app_cursor, mod_param),
                NamedKey::Home => cursor_key(b'H', app_cursor, mod_param),
                NamedKey::End => cursor_key(b'F', app_cursor, mod_param),
                NamedKey::PageUp => tilde_key(5, mod_param),
                NamedKey::PageDown => tilde_key(6, mod_param),
                NamedKey::Insert => tilde_key(2, mod_param),
                NamedKey::Delete => tilde_key(3, mod_param),
                NamedKey::F1 => function_key(1, mod_param),
                NamedKey::F2 => function_key(2, mod_param),
                NamedKey::F3 => function_key(3, mod_param),
                NamedKey::F4 => function_key(4, mod_param),
                NamedKey::F5 => function_key(5, mod_param),
                NamedKey::F6 => function_key(6, mod_param),
                NamedKey::F7 => function_key(7, mod_param),
                NamedKey::F8 => function_key(8, mod_param),
                NamedKey::F9 => function_key(9, mod_param),
                NamedKey::F10 => function_key(10, mod_param),
                NamedKey::F11 => function_key(11, mod_param),
                NamedKey::F12 => function_key(12, mod_param),
                _ => return None,
            };

            // Alt+Enter and friends: also honor the modifier for keys that do
            // not carry a CSI modifier parameter.
            if mods.alt_key()
                && matches!(named, NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace)
            {
                out.insert(0, 0x1b);
            }
            Some(out)
        }
        _ => None,
    }
}

fn cursor_key(final_byte: u8, app_cursor: bool, mod_param: u8) -> Vec<u8> {
    if mod_param > 1 {
        format!("\x1b[1;{mod_param}{}", final_byte as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn tilde_key(number: u8, mod_param: u8) -> Vec<u8> {
    if mod_param > 1 {
        format!("\x1b[{number};{mod_param}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn function_key(number: u8, mod_param: u8) -> Vec<u8> {
    match number {
        1..=4 => {
            let final_byte = b'P' + (number - 1);
            if mod_param > 1 {
                format!("\x1b[1;{mod_param}{}", final_byte as char).into_bytes()
            } else {
                vec![0x1b, b'O', final_byte]
            }
        }
        _ => {
            let code = match number {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return Vec::new(),
            };
            tilde_key(code, mod_param)
        }
    }
}

/// Wrap pasted text in bracketed-paste markers when requested by the terminal.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Terminals expect CR for line endings in pasted content.
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_are_canonical() {
        assert_eq!(
            chord(
                ModifiersState::CONTROL | ModifiersState::SHIFT,
                &Key::Character("T".into())
            ),
            Some("ctrl+shift+t".into())
        );
    }

    #[test]
    fn arrows_respect_app_cursor() {
        assert_eq!(
            encode_key(
                &Key::Named(NamedKey::ArrowUp),
                ModifiersState::empty(),
                TermMode::APP_CURSOR
            ),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(
                &Key::Named(NamedKey::ArrowUp),
                ModifiersState::empty(),
                TermMode::empty()
            ),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_encode() {
        assert_eq!(
            encode_key(
                &Key::Character("c".into()),
                ModifiersState::CONTROL,
                TermMode::empty()
            ),
            Some(vec![3])
        );
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;

    #[test]
    fn parses_action_names() {
        assert_eq!(Action::from_name("new_tab"), Some(Action::NewTab));
        assert_eq!(
            Action::from_name("select-tab-3"),
            Some(Action::SelectTab(2))
        );
        assert_eq!(Action::from_name("focus_left"), Some(Action::FocusLeft));
        assert_eq!(Action::from_name("nonsense"), None);
    }
}
