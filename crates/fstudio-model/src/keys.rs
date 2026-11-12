//! Neutral key input. Shells translate native events (`NSEvent`, GDK, Win32)
//! into [`KeyChord`]s; the keymap decides what they mean. This is the same
//! shape as Import Studio's `istudio-cull::keymap::KeyChord`, extended with
//! the keys a file manager needs.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Key {
    /// A printable character as typed *without* Shift, lowercased ('3', 'p', '/', '.').
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Space,
    Return,
    /// The numeric keypad's Enter.
    Enter,
    Escape,
    Tab,
    /// ⌫ (backspace).
    Delete,
    /// ⌦.
    ForwardDelete,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1–F20.
    F(u8),
}

/// Modifier keys.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub struct Mods {
    pub shift: bool,
    pub cmd: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        cmd: false,
        alt: false,
        ctrl: false,
    };
    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }
    /// macOS-style symbols in Apple's order: ⌃⌥⇧⌘.
    pub fn symbols(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push('⌃');
        }
        if self.alt {
            s.push('⌥');
        }
        if self.shift {
            s.push('⇧');
        }
        if self.cmd {
            s.push('⌘');
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KeyChord {
    pub key: Key,
    pub mods: Mods,
}

impl KeyChord {
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Mods::NONE,
        }
    }
    pub const fn new(key: Key, mods: Mods) -> Self {
        Self { key, mods }
    }
    pub const fn cmd(key: Key) -> Self {
        Self {
            key,
            mods: Mods {
                cmd: true,
                ..Mods::NONE
            },
        }
    }
    pub const fn shift_cmd(key: Key) -> Self {
        Self {
            key,
            mods: Mods {
                cmd: true,
                shift: true,
                ..Mods::NONE
            },
        }
    }
}

/// How a command was issued. Used for "Did you know?" hints (a mouse use of
/// something with a shortcut can hint) and for learned-shortcut counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Via {
    Keyboard,
    Mouse,
    Menu,
    Palette,
    /// Scripted (debug automation, CLI).
    Script,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_in_apple_order() {
        let m = Mods {
            shift: true,
            cmd: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(m.symbols(), "⌃⌥⇧⌘");
    }

    #[test]
    fn serde_roundtrip() {
        let c = KeyChord::shift_cmd(Key::Char('n'));
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<KeyChord>(&s).unwrap(), c);
    }
}
