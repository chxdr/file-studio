//! The kind filter: what sort of item an entry is, from its type and
//! extension (no file is opened).

use fstudio_model::{Entry, EntryKind};
use serde::{Deserialize, Serialize};

/// A kind of item, as the search filter offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// A folder (not a package).
    Folder,
    /// An application bundle (`.app`).
    Application,
    /// A picture, including camera raw files.
    Image,
    /// A video.
    Movie,
    /// Music or other sound.
    Audio,
    /// A PDF.
    Pdf,
    /// Plain text, markup, data and source code.
    Text,
    /// Word-processing documents, spreadsheets and presentations.
    Document,
    /// Archives and disk images.
    Archive,
    /// Anything else (including symlinks and special files).
    Other,
}

const IMAGE: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "heic", "heif", "tif", "tiff", "bmp", "webp", "svg", "ico",
    "icns", "psd", "avif", "jxl", "arw", "cr2", "cr3", "nef", "orf", "raf", "rw2", "dng", "raw",
    "srw", "pef", "exr", "hdr",
];
const MOVIE: &[&str] = &[
    "mov", "mp4", "m4v", "avi", "mkv", "webm", "mpg", "mpeg", "mts", "m2ts", "3gp", "wmv", "flv",
    "braw", "r3d", "mxf",
];
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "alac", "ogg", "opus", "wma", "caf", "mid",
    "midi",
];
const TEXT: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "rst",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "rs",
    "swift",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "m",
    "mm",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "kts",
    "sh",
    "zsh",
    "bash",
    "fish",
    "pl",
    "php",
    "lua",
    "sql",
    "ini",
    "cfg",
    "conf",
    "log",
    "tex",
    "srt",
    "vtt",
    "plist",
    "strings",
    "gradle",
    "cmake",
    "make",
    "mk",
    "dockerfile",
    "gitignore",
    "env",
    "properties",
    "rtf",
];
const DOCUMENT: &[&str] = &[
    "doc", "docx", "odt", "pages", "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key",
    "epub", "rtfd",
];
const ARCHIVE: &[&str] = &[
    "zip",
    "tar",
    "gz",
    "tgz",
    "bz2",
    "tbz",
    "xz",
    "txz",
    "zst",
    "7z",
    "rar",
    "dmg",
    "iso",
    "sparseimage",
    "sparsebundle",
    "pkg",
    "cpio",
    "lz4",
    "lzma",
];

fn has(list: &[&str], ext: &str) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

/// The kind of `e`. Packages count by extension (`.app` is an application,
/// `.pages` a document, `.sparsebundle` an archive); other folders are
/// folders.
pub fn kind_of(e: &Entry) -> Kind {
    let ext = e.extension();
    match e.kind {
        EntryKind::Dir if e.is_package || fstudio_fs::is_package_name(&e.name) => {
            if ext.eq_ignore_ascii_case("app") {
                Kind::Application
            } else {
                by_ext(ext).unwrap_or(Kind::Other)
            }
        }
        EntryKind::Dir => Kind::Folder,
        EntryKind::File => by_ext(ext).unwrap_or(Kind::Other),
        EntryKind::Symlink | EntryKind::Other => Kind::Other,
    }
}

fn by_ext(ext: &str) -> Option<Kind> {
    if ext.is_empty() {
        return None;
    }
    if ext.eq_ignore_ascii_case("pdf") {
        Some(Kind::Pdf)
    } else if has(IMAGE, ext) {
        Some(Kind::Image)
    } else if has(MOVIE, ext) {
        Some(Kind::Movie)
    } else if has(AUDIO, ext) {
        Some(Kind::Audio)
    } else if has(DOCUMENT, ext) {
        Some(Kind::Document)
    } else if has(ARCHIVE, ext) {
        Some(Kind::Archive)
    } else if has(TEXT, ext) {
        Some(Kind::Text)
    } else {
        None
    }
}

/// True when the extension is one we know to be text (used to skip the
/// binary sniff for obvious cases; the sniff still runs for the rest).
pub(crate) fn is_text_ext(ext: &str) -> bool {
    has(TEXT, ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind, pkg: bool) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: 0,
            alloc_size: None,
            mtime_ns: 0,
            btime_ns: None,
            mode: 0,
            flags: 0,
            inode: 1,
            dev: 1,
            is_package: pkg,
        }
    }

    #[test]
    fn kinds() {
        use EntryKind::*;
        assert_eq!(kind_of(&e("a", Dir, false)), Kind::Folder);
        assert_eq!(kind_of(&e("Safari.app", Dir, true)), Kind::Application);
        assert_eq!(kind_of(&e("Doc.pages", Dir, true)), Kind::Document);
        assert_eq!(kind_of(&e("DSC0001.ARW", File, false)), Kind::Image);
        assert_eq!(kind_of(&e("clip.MOV", File, false)), Kind::Movie);
        assert_eq!(kind_of(&e("x.pdf", File, false)), Kind::Pdf);
        assert_eq!(kind_of(&e("main.rs", File, false)), Kind::Text);
        assert_eq!(kind_of(&e("a.tar.gz", File, false)), Kind::Archive);
        assert_eq!(kind_of(&e("song.flac", File, false)), Kind::Audio);
        assert_eq!(kind_of(&e("README", File, false)), Kind::Other);
        assert_eq!(kind_of(&e("l.txt", Symlink, false)), Kind::Other);
    }
}
