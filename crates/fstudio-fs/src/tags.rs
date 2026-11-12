//! Finder tags, stored in the `com.apple.metadata:_kMDItemUserTags` extended attribute as
//! a binary property list of strings: `"Name"` or `"Name\n<colour index>"`.
//!
//! Finder also keeps the colour of a tagged item in the label bits of `com.apple.FinderInfo`;
//! [`set_finder_tags`] keeps both in step, so Finder, Spotlight and `tag(1)` all agree.

use crate::error::{Error, Result};
use crate::xattr::{xattr_get, xattr_remove, xattr_set};
use std::path::Path;

/// The xattr that holds the tag list.
pub const TAGS_ATTR: &str = "com.apple.metadata:_kMDItemUserTags";
/// The xattr with Finder's 32-byte info block (label colour in byte 9).
pub const FINDER_INFO_ATTR: &str = "com.apple.FinderInfo";

/// Finder's tag colours, numbered as Finder stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TagColor {
    /// No colour.
    #[default]
    None = 0,
    /// Gray.
    Gray = 1,
    /// Green.
    Green = 2,
    /// Purple.
    Purple = 3,
    /// Blue.
    Blue = 4,
    /// Yellow.
    Yellow = 5,
    /// Red.
    Red = 6,
    /// Orange.
    Orange = 7,
}

impl TagColor {
    /// The colour for Finder's index (0–7); anything else is [`TagColor::None`].
    pub fn from_index(i: u8) -> TagColor {
        match i {
            1 => TagColor::Gray,
            2 => TagColor::Green,
            3 => TagColor::Purple,
            4 => TagColor::Blue,
            5 => TagColor::Yellow,
            6 => TagColor::Red,
            7 => TagColor::Orange,
            _ => TagColor::None,
        }
    }

    /// Finder's index for this colour.
    pub fn index(self) -> u8 {
        self as u8
    }
}

/// A Finder tag: a name and an optional colour.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FinderTag {
    /// The tag's name.
    pub name: String,
    /// Its colour.
    pub color: TagColor,
}

impl FinderTag {
    /// A tag with a name and colour.
    pub fn new(name: impl Into<String>, color: TagColor) -> FinderTag {
        FinderTag {
            name: name.into(),
            color,
        }
    }
}

/// Decodes the attribute's bytes. Plain names (no colour suffix) are accepted too.
pub fn decode_tags(bytes: &[u8]) -> std::result::Result<Vec<FinderTag>, String> {
    let value =
        plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let items = value.into_array().ok_or("not an array")?;
    let mut tags = Vec::with_capacity(items.len());
    for item in items {
        let s = item.into_string().ok_or("tag is not a string")?;
        tags.push(match s.rsplit_once('\n') {
            Some((name, idx)) => match idx.trim().parse::<u8>() {
                Ok(i) => FinderTag::new(name, TagColor::from_index(i)),
                Err(_) => FinderTag::new(s.as_str(), TagColor::None),
            },
            None => FinderTag::new(s, TagColor::None),
        });
    }
    Ok(tags)
}

/// Encodes tags as the binary property list Finder writes.
pub fn encode_tags(tags: &[FinderTag]) -> std::result::Result<Vec<u8>, String> {
    let arr = plist::Value::Array(
        tags.iter()
            .map(|t| {
                plist::Value::String(if t.color == TagColor::None {
                    t.name.clone()
                } else {
                    format!("{}\n{}", t.name, t.color.index())
                })
            })
            .collect(),
    );
    let mut out = Vec::new();
    plist::to_writer_binary(&mut out, &arr).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The Finder tags on `path` (empty when there are none).
pub fn finder_tags(path: impl AsRef<Path>) -> Result<Vec<FinderTag>> {
    let path = path.as_ref();
    match xattr_get(path, TAGS_ATTR)? {
        None => Ok(Vec::new()),
        Some(bytes) => decode_tags(&bytes).map_err(|message| Error::Plist {
            path: path.to_path_buf(),
            message,
        }),
    }
}

/// Replaces the Finder tags on `path`. An empty slice removes them.
///
/// Also sets the label colour in `com.apple.FinderInfo` to the colour of the last coloured
/// tag (as Finder does), or clears it when there is none.
pub fn set_finder_tags(path: impl AsRef<Path>, tags: &[FinderTag]) -> Result<()> {
    let path = path.as_ref();
    if tags.is_empty() {
        xattr_remove(path, TAGS_ATTR)?;
    } else {
        let bytes = encode_tags(tags).map_err(|message| Error::Plist {
            path: path.to_path_buf(),
            message,
        })?;
        xattr_set(path, TAGS_ATTR, &bytes)?;
    }
    let color = tags
        .iter()
        .rev()
        .map(|t| t.color)
        .find(|c| *c != TagColor::None)
        .unwrap_or(TagColor::None);
    update_label(path, color)
}

fn update_label(path: &Path, color: TagColor) -> Result<()> {
    let existing = xattr_get(path, FINDER_INFO_ATTR)?;
    let mut info = [0u8; 32];
    if let Some(e) = &existing {
        let n = e.len().min(32);
        info[..n].copy_from_slice(&e[..n]);
    }
    if existing.is_none() && color == TagColor::None {
        return Ok(());
    }
    info[9] = (info[9] & !0x0E) | (color.index() << 1);
    if info.iter().all(|b| *b == 0) {
        xattr_remove(path, FINDER_INFO_ATTR)?;
    } else {
        xattr_set(path, FINDER_INFO_ATTR, &info)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let tags = vec![
            FinderTag::new("Work", TagColor::None),
            FinderTag::new("Urgent", TagColor::Red),
            FinderTag::new("Ünï\u{301}", TagColor::Orange),
        ];
        let bytes = encode_tags(&tags).unwrap();
        assert!(bytes.starts_with(b"bplist00"));
        assert_eq!(decode_tags(&bytes).unwrap(), tags);
    }

    #[test]
    fn decodes_plain_and_odd_entries() {
        let arr = plist::Value::Array(vec![
            plist::Value::String("Plain".into()),
            plist::Value::String("Green\n2".into()),
            plist::Value::String("Weird\nx".into()),
        ]);
        let mut out = Vec::new();
        plist::to_writer_xml(&mut out, &arr).unwrap();
        let t = decode_tags(&out).unwrap();
        assert_eq!(t[0], FinderTag::new("Plain", TagColor::None));
        assert_eq!(t[1], FinderTag::new("Green", TagColor::Green));
        assert_eq!(t[2].color, TagColor::None);
    }
}
