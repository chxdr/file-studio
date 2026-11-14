//! What a file is, decided from its name only (no content read).

use std::path::Path;

/// The broad kind of an item, which decides what the preview reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// A photo the system can show (JPEG, HEIC, PNG, TIFF, WebP…).
    Image,
    /// A camera raw (CR2, CR3, NEF, ARW, DNG, RAF…).
    Raw,
    /// A movie (MP4, MOV, MKV…).
    Video,
    /// A sound file.
    Audio,
    /// Text or source code, by extension.
    Text,
    /// A folder.
    Folder,
    /// A package or bundle (`.app`, `.photoslibrary`), shown as one item.
    Package,
    /// Something with a known type but nothing to preview (archives, disk images…).
    Other,
    /// No known type: the content is sniffed for text.
    Unknown,
}

const RAW: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef",
    "srw", "raw", "rwl", "3fr", "erf", "mef", "mrw", "iiq", "x3f", "gpr",
];
const IMAGE: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "heic", "heif", "hif", "avif", "png", "tif", "tiff", "webp",
    "gif", "bmp", "jxl", "psd", "ico", "icns", "tga", "exr", "svg",
];
const VIDEO: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "avi", "webm", "mts", "m2ts", "mxf", "braw", "crm", "r3d", "3gp",
    "wmv", "flv", "mpg", "mpeg", "ts",
];
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "opus", "caf", "wma",
];
const OTHER: &[&str] = &[
    "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "dmg", "iso", "pkg", "jar", "xcf", "sqlite",
    "db", "a", "o", "dylib", "so", "class", "wasm", "pdf", "doc", "docx", "xls", "xlsx", "ppt",
    "pptx", "pages", "numbers", "key", "ttf", "otf", "woff", "woff2", "bin", "dat",
];
const TEXT: &[&str] = &[
    "txt",
    "md",
    "markdown",
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
    "js",
    "mjs",
    "cjs",
    "jsx",
    "tsx",
    "json",
    "toml",
    "yaml",
    "yml",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "sh",
    "zsh",
    "bash",
    "fish",
    "go",
    "java",
    "kt",
    "kts",
    "lua",
    "php",
    "pl",
    "sql",
    "csv",
    "tsv",
    "log",
    "ini",
    "conf",
    "cfg",
    "plist",
    "gitignore",
    "lock",
    "rtf",
    "tex",
    "diff",
    "patch",
    "xmp",
    "srt",
    "vtt",
    "env",
    "makefile",
    "cmake",
    "dockerfile",
    "r",
    "scala",
    "cs",
    "hs",
    "ex",
    "exs",
    "erl",
    "clj",
    "vue",
    "svelte",
    "graphql",
    "proto",
];

/// The lower-cased extension of `name`, without the dot ("" for none or a dot-file).
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => name[i + 1..].to_lowercase(),
    }
}

/// Classifies by name. `is_dir` and `is_package` come from the filesystem.
pub fn classify(path: &Path, is_dir: bool) -> Category {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = extension(&name);
    if is_dir {
        return if matches!(
            ext.as_str(),
            "app"
                | "bundle"
                | "framework"
                | "photoslibrary"
                | "xcodeproj"
                | "pkg"
                | "plugin"
                | "appex"
                | "prefpane"
                | "lrdata"
                | "lrcat-data"
        ) {
            Category::Package
        } else {
            Category::Folder
        };
    }
    let has = |l: &[&str]| l.contains(&ext.as_str());
    if has(RAW) {
        Category::Raw
    } else if has(IMAGE) && ext != "svg" {
        Category::Image
    } else if has(VIDEO) && ext != "ts" {
        Category::Video
    } else if has(AUDIO) {
        Category::Audio
    } else if has(TEXT) || ext == "svg" || ext == "ts" || is_text_name(&name) {
        Category::Text
    } else if has(OTHER) {
        Category::Other
    } else {
        Category::Unknown
    }
}

/// Well-known extension-less text files.
fn is_text_name(name: &str) -> bool {
    matches!(
        name,
        "Makefile"
            | "Dockerfile"
            | "README"
            | "LICENSE"
            | "CHANGELOG"
            | "Gemfile"
            | "Rakefile"
            | "Podfile"
            | "Package.swift"
            | ".gitignore"
            | ".zshrc"
            | ".bashrc"
            | ".profile"
            | ".gitattributes"
            | ".editorconfig"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_extension_ignoring_case_and_normalisation() {
        let c = |n: &str| classify(Path::new(n), false);
        assert_eq!(c("DSC0001.ARW"), Category::Raw);
        assert_eq!(c("a.cr3"), Category::Raw);
        assert_eq!(c("IMG.HEIC"), Category::Image);
        assert_eq!(c("clip.MOV"), Category::Video);
        assert_eq!(c("main.rs"), Category::Text);
        assert_eq!(c("notes.ts"), Category::Text);
        assert_eq!(c("Makefile"), Category::Text);
        assert_eq!(c("mystery"), Category::Unknown);
        assert_eq!(c("a.zip"), Category::Other);
        // NFD name (e + combining acute) with a raw extension.
        assert_eq!(c("Cafe\u{301}.nef"), Category::Raw);
        assert_eq!(classify(Path::new("x.app"), true), Category::Package);
        assert_eq!(classify(Path::new("x"), true), Category::Folder);
    }
}
