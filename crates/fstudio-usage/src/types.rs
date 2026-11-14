//! What kinds of files use the space: totals per extension, grouped into a
//! few broad kinds, and the colour slots the treemap gives the biggest types.

use std::collections::HashMap;

use crate::tree::{Node, NodeId, NodeKind, SizeMode, Tree, flags};

/// A broad kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// RAW camera files.
    RawPhoto,
    /// Other images.
    Image,
    /// Video.
    Video,
    /// Audio.
    Audio,
    /// Documents, text and spreadsheets.
    Document,
    /// Archives and disk images.
    Archive,
    /// Source code and data files.
    Code,
    /// Apps, libraries and other packages.
    Package,
    /// Everything else.
    Other,
}

impl Category {
    /// The name shown in the breakdown.
    pub fn label(self) -> &'static str {
        match self {
            Category::RawPhoto => "RAW Photo",
            Category::Image => "Image",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Document => "Document",
            Category::Archive => "Archive",
            Category::Code => "Code",
            Category::Package => "Package",
            Category::Other => "Other",
        }
    }
}

const RAW: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "dng",
    "3fr", "fff", "iiq", "erf", "kdc", "mrw", "x3f", "srw", "rwl",
];
const IMAGE: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "heic", "heif", "tif", "tiff", "bmp", "webp", "psd", "psb", "svg",
    "avif", "jxl", "exr", "hdr", "ico", "icns", "xmp",
];
const VIDEO: &[&str] = &[
    "mov", "mp4", "m4v", "avi", "mkv", "mts", "m2ts", "mxf", "braw", "r3d", "webm", "wmv", "mpg",
    "mpeg", "3gp", "insv", "lrv",
];
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "alac", "ogg", "opus", "caf", "mid",
];
const DOCUMENT: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key", "txt", "rtf",
    "rtfd", "md", "csv", "epub", "odt", "ods",
];
const ARCHIVE: &[&str] = &[
    "zip",
    "rar",
    "7z",
    "tar",
    "gz",
    "tgz",
    "bz2",
    "xz",
    "zst",
    "dmg",
    "iso",
    "sparseimage",
    "sparsebundle",
    "pkg",
    "xip",
    "img",
];
const CODE: &[&str] = &[
    "rs", "swift", "c", "h", "cpp", "hpp", "m", "mm", "js", "ts", "tsx", "jsx", "py", "rb", "go",
    "java", "kt", "json", "yml", "yaml", "toml", "xml", "html", "css", "sh", "sql", "o", "a",
    "rlib", "rmeta", "d", "dylib", "so", "class", "jar", "wasm", "pyc", "db", "sqlite", "log",
];

/// The kind of file an extension (lower case, no dot) names.
pub fn category(ext: &str, kind: NodeKind) -> Category {
    if kind == NodeKind::Package {
        return Category::Package;
    }
    let is = |list: &[&str]| list.contains(&ext);
    if is(RAW) {
        Category::RawPhoto
    } else if is(IMAGE) {
        Category::Image
    } else if is(VIDEO) {
        Category::Video
    } else if is(AUDIO) {
        Category::Audio
    } else if is(DOCUMENT) {
        Category::Document
    } else if is(ARCHIVE) {
        Category::Archive
    } else if is(CODE) {
        Category::Code
    } else {
        Category::Other
    }
}

/// The totals for one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeStat {
    /// The extension index in the tree (0 = no extension).
    pub ext: u16,
    /// The extension, lower case ("" for none).
    pub ext_name: String,
    /// Its broad kind.
    pub category: Category,
    /// Allocated bytes.
    pub allocated: u64,
    /// Logical bytes.
    pub logical: u64,
    /// How many items.
    pub count: u64,
}

impl TypeStat {
    /// The size in `mode`.
    pub fn size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Allocated => self.allocated,
            SizeMode::Logical => self.logical,
        }
    }
}

/// Totals per type for everything below `root`, largest first in `mode`.
/// Packages count as their own type ("app", "photoslibrary"). Extra hard
/// links add a count but no size.
pub fn breakdown(tree: &Tree, root: NodeId, mode: SizeMode) -> Vec<TypeStat> {
    let mut acc: HashMap<(u16, bool), TypeStat> = HashMap::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let n = tree.node(id);
        if n.has(flags::REMOVED) {
            continue;
        }
        match n.kind {
            NodeKind::Dir => stack.extend(n.children.iter().copied()),
            NodeKind::File | NodeKind::Package | NodeKind::Symlink | NodeKind::Other => {
                add(&mut acc, tree, n);
            }
        }
    }
    let mut v: Vec<TypeStat> = acc.into_values().collect();
    v.sort_by(|a, b| {
        b.size(mode)
            .cmp(&a.size(mode))
            .then(b.count.cmp(&a.count))
            .then_with(|| a.ext_name.cmp(&b.ext_name))
    });
    v
}

fn add(acc: &mut HashMap<(u16, bool), TypeStat>, tree: &Tree, n: &Node) {
    let pkg = n.kind == NodeKind::Package;
    let e = acc.entry((n.ext, pkg)).or_insert_with(|| {
        let name = tree.ext_name(n.ext).to_string();
        TypeStat {
            ext: n.ext,
            category: category(&name, n.kind),
            ext_name: name,
            allocated: 0,
            logical: 0,
            count: 0,
        }
    });
    e.allocated += n.allocated;
    e.logical += n.logical;
    e.count += 1;
}

/// Which file types get their own colour: the largest few, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Palette {
    slots: HashMap<u16, u8>,
    /// The types with a colour, slot 1 first.
    pub top: Vec<u16>,
}

impl Palette {
    /// How many types get a colour.
    pub const SIZE: usize = 12;

    /// Gives slots `1..=SIZE` to the largest types in `stats` (which is
    /// sorted largest first). Files without an extension stay in "other".
    pub fn from_stats(stats: &[TypeStat]) -> Palette {
        let mut p = Palette::default();
        for s in stats.iter().filter(|s| s.ext != 0) {
            if p.top.len() >= Self::SIZE {
                break;
            }
            if p.slots.contains_key(&s.ext) {
                continue;
            }
            p.top.push(s.ext);
            p.slots.insert(s.ext, p.top.len() as u8);
        }
        p
    }

    /// The slot for a type: `1..=SIZE`, or 0 (other).
    pub fn slot(&self, ext: u16) -> u8 {
        self.slots.get(&ext).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(category("cr3", NodeKind::File), Category::RawPhoto);
        assert_eq!(category("jpg", NodeKind::File), Category::Image);
        assert_eq!(category("mov", NodeKind::File), Category::Video);
        assert_eq!(category("app", NodeKind::Package), Category::Package);
        assert_eq!(category("", NodeKind::File), Category::Other);
        assert_eq!(category("zip", NodeKind::File), Category::Archive);
    }

    #[test]
    fn palette_takes_the_largest_types() {
        let stat = |ext: u16, name: &str, a: u64| TypeStat {
            ext,
            ext_name: name.into(),
            category: Category::Other,
            allocated: a,
            logical: a,
            count: 1,
        };
        let stats = vec![stat(3, "mov", 100), stat(0, "", 90), stat(1, "jpg", 50)];
        let p = Palette::from_stats(&stats);
        assert_eq!(p.top, vec![3, 1]);
        assert_eq!(p.slot(3), 1);
        assert_eq!(p.slot(1), 2);
        assert_eq!(p.slot(0), 0);
        assert_eq!(p.slot(9), 0);
    }
}
