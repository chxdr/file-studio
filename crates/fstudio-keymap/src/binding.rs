//! Key sequences and bindings.

use std::fmt;

use fstudio_model::{Key, KeyChord};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::command::CommandArg;
use crate::display::{chord_text, parse_chord};
use crate::when::When;

/// One step of a key sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeySpec {
    /// Exactly this chord.
    Chord(KeyChord),
    /// Any plain letter a–z. The letter typed becomes [`CommandArg::Char`] (marks: `m a`, `' a`).
    Letter,
}

/// Text of the wildcard step.
const LETTER_TEXT: &str = "{a-z}";

impl KeySpec {
    /// Does this step accept `chord`?
    pub fn matches(&self, chord: &KeyChord) -> bool {
        match self {
            KeySpec::Chord(c) => c == chord,
            KeySpec::Letter => {
                chord.mods.is_empty() && matches!(chord.key, Key::Char(c) if c.is_ascii_lowercase())
            }
        }
    }

    /// Could some single chord match both steps?
    pub fn overlaps(&self, other: &KeySpec) -> bool {
        match (self, other) {
            (KeySpec::Letter, KeySpec::Letter) => true,
            (KeySpec::Letter, KeySpec::Chord(c)) | (KeySpec::Chord(c), KeySpec::Letter) => {
                KeySpec::Letter.matches(c)
            }
            (KeySpec::Chord(a), KeySpec::Chord(b)) => a == b,
        }
    }

    /// Key-cap text of this step.
    pub fn text(&self) -> String {
        match self {
            KeySpec::Chord(c) => chord_text(c),
            KeySpec::Letter => LETTER_TEXT.into(),
        }
    }

    /// Parses key-cap text.
    pub fn parse(s: &str) -> Option<KeySpec> {
        if s == LETTER_TEXT {
            Some(KeySpec::Letter)
        } else {
            parse_chord(s).map(KeySpec::Chord)
        }
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

// Settings files and exports are written by hand sometimes, so a step is the
// key-cap text (`"⇧⌘N"`), not a nested object.
impl Serialize for KeySpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text())
    }
}

impl<'de> Deserialize<'de> for KeySpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        KeySpec::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad key '{s}'")))
    }
}

/// One or more chords pressed in turn, live in a context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub keys: Vec<KeySpec>,
    pub when: When,
    /// Argument passed with the command. A [`KeySpec::Letter`] step supplies `Char` if this is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<CommandArg>,
}

impl Binding {
    /// A single-chord binding.
    pub fn chord(chord: KeyChord, when: When) -> Self {
        Self {
            keys: vec![KeySpec::Chord(chord)],
            when,
            arg: None,
        }
    }

    /// Parses space-separated key-cap text: `"⇧⌘N"`, `"g g"`, `"m {a-z}"`.
    pub fn parse(keys: &str, when: When) -> Option<Self> {
        let keys: Option<Vec<KeySpec>> = keys.split_whitespace().map(KeySpec::parse).collect();
        let keys = keys?;
        if keys.is_empty() {
            return None;
        }
        Some(Self {
            keys,
            when,
            arg: None,
        })
    }

    /// The same binding with an argument.
    pub fn with_arg(mut self, arg: CommandArg) -> Self {
        self.arg = Some(arg);
        self
    }

    /// Key-cap text of the whole sequence: `⇧⌘N`, `g g`.
    pub fn text(&self) -> String {
        self.keys
            .iter()
            .map(KeySpec::text)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first chord, if the first step is a fixed chord.
    pub fn first_chord(&self) -> Option<KeyChord> {
        match self.keys.first() {
            Some(KeySpec::Chord(c)) => Some(*c),
            _ => None,
        }
    }

    /// Could both bindings be triggered by the same key presses in the same state?
    pub fn collides_with(&self, other: &Binding) -> bool {
        self.keys.len() == other.keys.len()
            && self
                .keys
                .iter()
                .zip(&other.keys)
                .all(|(a, b)| a.overlaps(b))
            && self.when.overlaps(&other.when)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_key_cap_text() {
        let b = Binding::parse("g g", When::ListFocused).unwrap();
        let j = serde_json::to_string(&b).unwrap();
        assert!(j.contains(r#"["g","g"]"#), "{j}");
        assert_eq!(serde_json::from_str::<Binding>(&j).unwrap(), b);
        let m = Binding::parse("m {a-z}", When::vim_normal()).unwrap();
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Binding>(&j).unwrap(), m);
        assert!(serde_json::from_str::<Binding>(r#"{"keys":["??x"],"when":"Always"}"#).is_err());
    }

    #[test]
    fn letter_wildcard() {
        assert!(KeySpec::Letter.matches(&KeyChord::plain(Key::Char('q'))));
        assert!(!KeySpec::Letter.matches(&KeyChord::cmd(Key::Char('q'))));
        assert!(!KeySpec::Letter.matches(&KeyChord::plain(Key::Char('1'))));
    }
}
