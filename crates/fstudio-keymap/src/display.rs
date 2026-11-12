//! Key-cap text for chords (`⇧⌘N`, `F5`, `g`) and the reverse parser.
//!
//! The text is consistent with [`Mods::symbols`] (Apple order ⌃⌥⇧⌘). Rules:
//! - a chord with only Shift and a printable key shows the shifted glyph
//!   (`G`, `:`, `?`) as a US keyboard would type it;
//! - a chord with no modifiers shows the character as typed (`g`);
//! - any other chord shows the modifier symbols plus an upper-case key (`⌘N`).
//!
//! [`parse_chord`] accepts exactly what [`chord_text`] writes, plus a few
//! spelled-out names (`Esc`, `Return`, `PgUp`), so the Standard table can be
//! written the way it appears in `docs/HANDS-ON.md`.

use fstudio_model::{Key, KeyChord, Mods};

/// US-layout shifted glyph for a base character, if there is one.
fn shifted_glyph(base: char) -> Option<char> {
    Some(match base {
        ';' => ':',
        '`' => '~',
        '/' => '?',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        '-' => '_',
        '=' => '+',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        c if c.is_alphabetic() => {
            let mut up = c.to_uppercase();
            let u = up.next()?;
            if up.next().is_some() || u == c {
                return None;
            }
            u
        }
        _ => return None,
    })
}

/// The base character (typed without Shift) for a shifted glyph.
fn unshifted_base(glyph: char) -> Option<char> {
    if glyph.is_alphabetic() {
        let mut lo = glyph.to_lowercase();
        let l = lo.next()?;
        if lo.next().is_some() || l == glyph {
            return None;
        }
        return Some(l);
    }
    ";`/',.[]\\-=1234567890"
        .chars()
        .find(|b| shifted_glyph(*b) == Some(glyph))
}

/// Turns a shifted glyph the shell may have sent (`G`, `:`) into the model's
/// canonical form (`Char('g')` + Shift), so shells that report the typed
/// character still resolve correctly. Chords with ⌘, ⌃ or ⌥ are left alone.
pub fn normalize_chord(c: KeyChord) -> KeyChord {
    if let Key::Char(ch) = c.key
        && !c.mods.cmd
        && !c.mods.ctrl
        && !c.mods.alt
        && let Some(base) = unshifted_base(ch)
    {
        return KeyChord::new(
            Key::Char(base),
            Mods {
                shift: true,
                ..c.mods
            },
        );
    }
    c
}

/// Name or symbol of a non-printable key.
fn key_name(key: Key) -> String {
    match key {
        Key::Char(c) => c.to_uppercase().collect(),
        Key::Left => "←".into(),
        Key::Right => "→".into(),
        Key::Up => "↑".into(),
        Key::Down => "↓".into(),
        Key::Space => "Space".into(),
        Key::Return => "↩".into(),
        Key::Enter => "⌤".into(),
        Key::Escape => "⎋".into(),
        Key::Tab => "⇥".into(),
        Key::Delete => "⌫".into(),
        Key::ForwardDelete => "⌦".into(),
        Key::Home => "↖".into(),
        Key::End => "↘".into(),
        Key::PageUp => "⇞".into(),
        Key::PageDown => "⇟".into(),
        Key::F(n) => format!("F{n}"),
    }
}

/// Key-cap text of just the key, upper-cased, with no modifiers (`P`, `F5`, `↩`).
pub fn key_cap(key: Key) -> String {
    key_name(key)
}

/// Key-cap text for one chord: `⇧⌘N`, `F5`, `⌘↓`, `g`, `G`.
pub fn chord_text(c: &KeyChord) -> String {
    let m = c.mods;
    if let Key::Char(ch) = c.key {
        let shift_only = m.shift && !m.cmd && !m.alt && !m.ctrl;
        if shift_only && let Some(g) = shifted_glyph(ch) {
            return g.to_string();
        }
        if m.is_empty() {
            return ch.to_string();
        }
    }
    format!("{}{}", m.symbols(), key_name(c.key))
}

/// Parses a chord written the way [`chord_text`] writes it.
pub fn parse_chord(s: &str) -> Option<KeyChord> {
    let mut mods = Mods::NONE;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        match c {
            '⌃' => mods.ctrl = true,
            '⌥' => mods.alt = true,
            '⇧' => mods.shift = true,
            '⌘' => mods.cmd = true,
            _ => break,
        }
        rest = &rest[c.len_utf8()..];
    }
    let key = match rest {
        "" => return None,
        "Space" => Key::Space,
        "↩" | "Return" => Key::Return,
        "⌤" | "Enter" => Key::Enter,
        "⎋" | "Esc" | "Escape" => Key::Escape,
        "⇥" | "Tab" => Key::Tab,
        "⌫" | "Delete" | "Backspace" => Key::Delete,
        "⌦" | "ForwardDelete" => Key::ForwardDelete,
        "←" => Key::Left,
        "→" => Key::Right,
        "↑" => Key::Up,
        "↓" => Key::Down,
        "↖" | "Home" => Key::Home,
        "↘" | "End" => Key::End,
        "⇞" | "PgUp" => Key::PageUp,
        "⇟" | "PgDn" => Key::PageDown,
        f if f.len() > 1 && f.starts_with('F') && f[1..].bytes().all(|b| b.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().ok()?;
            if !(1..=20).contains(&n) {
                return None;
            }
            Key::F(n)
        }
        t => {
            let mut it = t.chars();
            let ch = it.next()?;
            if it.next().is_some() {
                return None;
            }
            if mods.is_empty()
                && let Some(base) = unshifted_base(ch)
            {
                mods.shift = true;
                Key::Char(base)
            } else if ch.is_alphabetic() {
                Key::Char(ch.to_lowercase().next().unwrap_or(ch))
            } else {
                Key::Char(ch)
            }
        }
    };
    Some(KeyChord::new(key, mods))
}

/// Badge text for held modifiers: `⇧ Shift`, `⌘ Command`, `⇧⌘`; `Keys` for none.
pub fn mods_title(m: Mods) -> String {
    let n = [m.ctrl, m.alt, m.shift, m.cmd]
        .iter()
        .filter(|b| **b)
        .count();
    match (n, m) {
        (0, _) => "Keys".into(),
        (1, m) if m.shift => "⇧ Shift".into(),
        (1, m) if m.cmd => "⌘ Command".into(),
        (1, m) if m.alt => "⌥ Option".into(),
        (1, _) => "⌃ Control".into(),
        _ => m.symbols(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_from_the_brief() {
        assert_eq!(chord_text(&KeyChord::shift_cmd(Key::Char('n'))), "⇧⌘N");
        assert_eq!(chord_text(&KeyChord::plain(Key::F(5))), "F5");
        assert_eq!(chord_text(&KeyChord::plain(Key::Char('g'))), "g");
        assert_eq!(chord_text(&KeyChord::cmd(Key::Down)), "⌘↓");
        assert_eq!(chord_text(&parse_chord("G").unwrap()), "G");
        assert_eq!(chord_text(&parse_chord(":").unwrap()), ":");
    }

    #[test]
    fn symbols_agree_with_mods() {
        let c = parse_chord("⌃⌥⇧⌘K").unwrap();
        assert_eq!(chord_text(&c), format!("{}K", c.mods.symbols()));
    }

    #[test]
    fn roundtrip() {
        for s in [
            "⌘↓",
            "⇧⌘.",
            "⌥⇧⌘N",
            "⌃⇥",
            "⌃⇧⇥",
            "F7",
            "⇧F6",
            "⌘⌫",
            "⌥⌘⌫",
            "Space",
            "↩",
            "⎋",
            "/",
            ":",
            "G",
            "j",
            "⌃D",
            "⌘\\",
            "⌘=",
            "⌘-",
            "⌘[",
            "⇧⌘P",
            "⌘,",
            "'",
            "m",
            "⇞",
            "↖",
        ] {
            let c = parse_chord(s).unwrap_or_else(|| panic!("parse {s}"));
            assert_eq!(chord_text(&c), s, "roundtrip {s}");
        }
    }

    #[test]
    fn normalizes_shifted_glyphs() {
        let c = normalize_chord(KeyChord::plain(Key::Char('G')));
        assert_eq!(c, parse_chord("G").unwrap());
        assert_eq!(
            normalize_chord(KeyChord::plain(Key::Char(':'))),
            KeyChord::new(
                Key::Char(';'),
                Mods {
                    shift: true,
                    ..Mods::NONE
                }
            )
        );
        let cmd_n = KeyChord::cmd(Key::Char('n'));
        assert_eq!(normalize_chord(cmd_n), cmd_n);
    }

    #[test]
    fn titles() {
        assert_eq!(mods_title(Mods::NONE), "Keys");
        assert_eq!(mods_title(parse_chord("⇧A").unwrap().mods), "⇧ Shift");
        assert_eq!(mods_title(parse_chord("⌘A").unwrap().mods), "⌘ Command");
        assert_eq!(mods_title(parse_chord("⇧⌘A").unwrap().mods), "⇧⌘");
    }
}
