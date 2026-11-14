//! The records a preview is made of. Plain data, no behaviour that needs I/O
//! beyond the two small readers at the bottom (tags, folder count).

use std::path::Path;

use crate::cancel::CancelToken;
use crate::kind::{Category, classify};
use crate::stat::FileStat;

/// Limits and switches for one preview. Part of the cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewOptions {
    /// Most lines of text to return (the line count is separate).
    pub max_text_lines: u32,
    /// Colour the text with syntax spans.
    pub highlight: bool,
    /// Count lines of the whole file up to this many bytes; beyond that the
    /// count is an estimate flagged `lines_exact = false`.
    pub line_count_cap_bytes: u64,
    /// Photos larger than this are not opened for EXIF (0 = no limit).
    pub max_photo_bytes: u64,
    /// A folder's item count stops here (and says "N+").
    pub folder_count_limit: u32,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        PreviewOptions {
            max_text_lines: 200,
            highlight: true,
            line_count_cap_bytes: 64 * 1024 * 1024,
            max_photo_bytes: 4 * 1024 * 1024 * 1024,
            folder_count_limit: 100_000,
        }
    }
}

/// A Finder tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTag {
    /// The tag's name.
    pub name: String,
    /// Finder's colour index 0–7 (0 none, 1 gray, 2 green, 3 purple, 4 blue,
    /// 5 yellow, 6 red, 7 orange).
    pub color: u8,
}

/// EXIF facts, already formatted for display. A field is `None` when the
/// file doesn't say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhotoInfo {
    /// When the picture was taken, local time, `2026-11-11 14:03:22`.
    pub date_taken: Option<String>,
    /// Make and model, `SONY ILCE-7M4`.
    pub camera: Option<String>,
    /// The lens, `FE 35mm F1.4 GM`.
    pub lens: Option<String>,
    /// `35 mm`.
    pub focal_length: Option<String>,
    /// `ƒ/2.8`.
    pub aperture: Option<String>,
    /// `1/250 s`.
    pub shutter: Option<String>,
    /// `ISO 400`.
    pub iso: Option<String>,
    /// `+0.7 EV`.
    pub exposure_comp: Option<String>,
    /// `7008 × 4672`; the shell may fill it from ImageIO when the file
    /// carries no dimensions.
    pub dimensions: Option<String>,
    /// Pixel width, when known.
    pub width: Option<u32>,
    /// Pixel height, when known.
    pub height: Option<u32>,
    /// EXIF orientation 1–8.
    pub orientation: Option<u16>,
}

impl PhotoInfo {
    /// True when nothing was found.
    pub fn is_empty(&self) -> bool {
        *self == PhotoInfo::default()
    }
}

/// Facts from a movie's `moov` box, formatted for display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoInfo {
    /// `H.264`, `HEVC`, `Apple ProRes 422 HQ`.
    pub codec: Option<String>,
    /// Length in seconds.
    pub duration_secs: Option<f64>,
    /// `1:23:45` or `0:42`.
    pub duration: Option<String>,
    /// `3840 × 2160`.
    pub resolution: Option<String>,
    /// Pixel width.
    pub width: Option<u32>,
    /// Pixel height.
    pub height: Option<u32>,
    /// `29.97 fps`.
    pub frame_rate: Option<String>,
    /// The sound track, `AAC`, `PCM`.
    pub audio: Option<String>,
}

/// One coloured run inside a line, in UTF-16 code units of the line text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First code unit.
    pub start: u32,
    /// Length in code units.
    pub len: u32,
    /// What kind of token it is; the shell chooses the colour.
    pub class: crate::highlight::TokenClass,
}

/// One line of a text preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// The text without its line ending (tabs kept).
    pub text: String,
    /// Coloured runs, in order, not overlapping. Empty for plain text.
    pub spans: Vec<Span>,
}

/// The start of a text file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPreview {
    /// `UTF-8`, `UTF-8 with BOM`, `UTF-16 LE`, `Latin-1 (guessed)`.
    pub encoding: String,
    /// The language the highlighter chose (`Rust`), if any.
    pub language: Option<String>,
    /// Number of lines (a last line without a newline counts).
    pub line_count: u64,
    /// False when the file was too big to count whole and `line_count` is an estimate.
    pub lines_exact: bool,
    /// The first lines.
    pub lines: Vec<TextLine>,
    /// There are more lines than `lines` holds.
    pub truncated: bool,
}

/// What a folder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderInfo {
    /// Visible items directly inside (dot-files not counted).
    pub items: u64,
    /// Counting stopped at the limit, so there are at least this many.
    pub at_least: bool,
    /// The folder couldn't be read (permissions).
    pub unreadable: bool,
}

/// Everything the preview pane shows about one item.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// The full path, as given.
    pub path: String,
    /// The name exactly as stored (never normalised, F-017).
    pub name: String,
    /// What the item is.
    pub category: Category,
    /// Logical size in bytes.
    pub size: u64,
    /// Modification time, ns since the Unix epoch.
    pub modified_ns: i64,
    /// Creation time, ns since the Unix epoch.
    pub created_ns: Option<i64>,
    /// Last access time, ns since the Unix epoch.
    pub accessed_ns: Option<i64>,
    /// It is a symbolic link (the facts describe its target).
    pub is_symlink: bool,
    /// An iCloud placeholder whose content is not here: show a Download button.
    pub dataless: bool,
    /// Finder tags.
    pub tags: Vec<PreviewTag>,
    /// Photo or raw metadata.
    pub photo: Option<PhotoInfo>,
    /// Movie metadata.
    pub video: Option<VideoInfo>,
    /// Text start.
    pub text: Option<TextPreview>,
    /// Folder contents.
    pub folder: Option<FolderInfo>,
    /// A plain-language remark: why there is little to show.
    pub note: Option<String>,
}

impl Preview {
    /// A summary with only the facts that come from `stat` and the name.
    pub fn basic(path: &Path, stat: &FileStat) -> Preview {
        Preview {
            path: path.to_string_lossy().into_owned(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            category: classify(path, stat.is_dir),
            size: stat.size,
            modified_ns: stat.mtime_ns,
            created_ns: stat.btime_ns,
            accessed_ns: stat.atime_ns,
            is_symlink: stat.is_symlink,
            dataless: stat.dataless,
            tags: Vec::new(),
            photo: None,
            video: None,
            text: None,
            folder: None,
            note: None,
        }
    }
}

/// The Finder tags on `path`; unreadable tags are simply none.
pub(crate) fn tags_of(path: &Path) -> Vec<PreviewTag> {
    fstudio_fs::finder_tags(path)
        .unwrap_or_default()
        .into_iter()
        .map(|t| PreviewTag {
            name: t.name,
            color: t.color.index(),
        })
        .collect()
}

/// Counts the visible items in a folder. `None` when cancelled.
pub(crate) fn folder_info(path: &Path, limit: u32, cancel: &CancelToken) -> Option<FolderInfo> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Some(FolderInfo {
            items: 0,
            at_least: false,
            unreadable: true,
        });
    };
    let mut n = 0u64;
    for (i, e) in rd.enumerate() {
        if i % 512 == 0 && cancel.is_cancelled() {
            return None;
        }
        let Ok(e) = e else { continue };
        if !e.file_name().to_string_lossy().starts_with('.') {
            n += 1;
            if n >= u64::from(limit) {
                return Some(FolderInfo {
                    items: n,
                    at_least: true,
                    unreadable: false,
                });
            }
        }
    }
    Some(FolderInfo {
        items: n,
        at_least: false,
        unreadable: false,
    })
}
