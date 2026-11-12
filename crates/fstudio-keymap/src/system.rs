//! macOS shortcuts the app must never shadow.

use std::sync::OnceLock;

use fstudio_model::KeyChord;

use crate::display::parse_chord;

const SYSTEM: &[(&str, &str)] = &[
    ("⌘Q", "Quit"),
    ("⌘H", "Hide"),
    ("⌥⌘H", "Hide Others"),
    ("⌘M", "Minimize"),
    ("⌥⌘M", "Minimize All"),
    ("⌘`", "Switch Windows"),
    ("⇧⌘`", "Switch Windows (reverse)"),
    ("⌘⇥", "App Switcher"),
    ("⇧⌘⇥", "App Switcher (reverse)"),
    ("⌘Space", "Spotlight"),
    ("⌥⌘Space", "Finder Search"),
    ("⌃Space", "Input Source"),
    ("⌃⌘Space", "Emoji & Symbols"),
    ("⌥⌘⎋", "Force Quit"),
    ("⌃⌘Q", "Lock Screen"),
    ("⇧⌘3", "Screenshot"),
    ("⇧⌘4", "Screenshot Selection"),
    ("⇧⌘5", "Screenshot Options"),
    ("⌥⌘D", "Show or Hide Dock"),
    ("⌃←", "Move Left a Space"),
    ("⌃→", "Move Right a Space"),
    ("⌃↑", "Mission Control"),
    ("⌃↓", "App Exposé"),
];

fn table() -> &'static [(KeyChord, &'static str)] {
    static CELL: OnceLock<Vec<(KeyChord, &'static str)>> = OnceLock::new();
    CELL.get_or_init(|| {
        SYSTEM
            .iter()
            .map(|(k, n)| {
                (
                    parse_chord(k).unwrap_or_else(|| panic!("bad system key {k}")),
                    *n,
                )
            })
            .collect()
    })
}

/// The name of the macOS shortcut this chord would shadow, if any.
pub fn system_shortcut(chord: &KeyChord) -> Option<&'static str> {
    table().iter().find(|(c, _)| c == chord).map(|(_, n)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_system_chords() {
        for k in ["⌘Q", "⌘H", "⌘M", "⌘`", "⌘⇥", "⌘Space"] {
            assert!(system_shortcut(&parse_chord(k).unwrap()).is_some(), "{k}");
        }
        assert!(system_shortcut(&parse_chord("⌘T").unwrap()).is_none());
    }
}
