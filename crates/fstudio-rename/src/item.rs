//! What the pipeline knows about one item: its name, folder, dates and EXIF.

use crate::date::DateParts;

/// EXIF values the `{exif.*}` tokens use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExifInfo {
    /// When the photo was taken (`DateTimeOriginal`, else `DateTimeDigitized`, else `DateTime`).
    pub date: Option<DateParts>,
    /// The camera ("Canon EOS R5"; the make is dropped when the model repeats it).
    pub camera: Option<String>,
    /// The lens model.
    pub lens: Option<String>,
}

/// Whether an item's EXIF is known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Exif {
    /// Not read yet (it is read only when a step uses an `{exif.*}` token).
    #[default]
    NotLoaded,
    /// Read; fields the file doesn't have are `None`.
    Loaded(ExifInfo),
    /// The file has no EXIF that could be read (not an image, or none in it).
    Missing,
    /// An iCloud placeholder (`SF_DATALESS`): never read, so it isn't
    /// downloaded (F-016). Its EXIF tokens are empty, with a warning.
    Placeholder,
}

impl Exif {
    /// The loaded values, if any.
    pub fn info(&self) -> Option<&ExifInfo> {
        match self {
            Exif::Loaded(i) => Some(i),
            _ => None,
        }
    }
}

/// One item to rename, as the pipeline sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemInfo {
    /// The name exactly as the filesystem returned it.
    pub name: String,
    /// Folders never have an extension.
    pub is_dir: bool,
    /// The name of the folder it is in (`{parent}`).
    pub parent_name: String,
    /// Local modification date (`{date}`).
    pub modified: Option<DateParts>,
    /// Local creation date (`{created}`).
    pub created: Option<DateParts>,
    /// EXIF values (`{exif.*}`).
    pub exif: Exif,
}

impl ItemInfo {
    /// An item with just a name (tests, mocks).
    pub fn named(name: &str) -> ItemInfo {
        ItemInfo {
            name: name.to_string(),
            ..ItemInfo::default()
        }
    }
}
