//! Pure name rules: stem and extension, comparison keys (F-017) and name
//! validity. No I/O.

use unicode_normalization::UnicodeNormalization;

/// Longest file name, in bytes, on APFS and HFS+.
pub const NAME_MAX: usize = 255;

/// A name split into its stem and extension (without the dot).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parts {
    /// Everything before the extension.
    pub stem: String,
    /// The extension without its dot; empty when there is none.
    pub ext: String,
}

impl Parts {
    /// Joins stem and extension back into a name (`stem.ext`, or `stem`).
    pub fn join(&self) -> String {
        if self.ext.is_empty() {
            self.stem.clone()
        } else {
            format!("{}.{}", self.stem, self.ext)
        }
    }
}

/// Splits a name into stem and extension. Folders and dot-files
/// (`.zshrc`) have no extension, and neither does a name ending in a dot;
/// otherwise the text after the last dot is the extension.
pub fn split(name: &str, is_dir: bool) -> Parts {
    if !is_dir
        && let Some(i) = name.rfind('.')
        && i > 0
        && i + 1 < name.len()
    {
        return Parts {
            stem: name[..i].to_string(),
            ext: name[i + 1..].to_string(),
        };
    }
    Parts {
        stem: name.to_string(),
        ext: String::new(),
    }
}

/// The NFC form of a name. Steps work on NFC text so a typed "é" matches a
/// decomposed one stored on disk.
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// The comparison key of a name in a folder: NFC, and case-folded when the
/// volume ignores case (F-017). APFS ignores normalisation even on
/// case-sensitive volumes, so the key is always NFC.
pub fn key(name: &str, case_sensitive: bool) -> String {
    let n: String = name.nfc().collect();
    if case_sensitive {
        n
    } else {
        // `to_lowercase` folds every cased letter; good enough for collision
        // prediction, and the OS's exclusive rename has the final say.
        n.to_lowercase().nfc().collect()
    }
}

/// Why a new name can't be used, or `None` when it can.
pub fn invalid_reason(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("the name is empty");
    }
    if name == "." || name == ".." {
        return Some("“.” and “..” aren't names");
    }
    if name.contains('/') {
        return Some("names can't contain “/”");
    }
    if name.contains(':') {
        return Some("names can't contain “:”");
    }
    if name.contains('\0') {
        return Some("names can't contain a NUL character");
    }
    if name.len() > NAME_MAX {
        return Some("the name is longer than 255 bytes");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_finder() {
        assert_eq!(split("a.txt", false).ext, "txt");
        assert_eq!(split("a.tar.gz", false).stem, "a.tar");
        assert_eq!(split(".zshrc", false).ext, "");
        assert_eq!(split("trailing.", false).ext, "");
        assert_eq!(split("Project.v2", true).ext, "");
        assert_eq!(split("noext", false).join(), "noext");
        assert_eq!(split("a.b", false).join(), "a.b");
    }

    #[test]
    fn keys_fold_case_and_normalisation() {
        let nfd = "Cafe\u{301}.txt";
        let nfc_ = "Café.txt";
        assert_ne!(nfd, nfc_);
        assert_eq!(key(nfd, true), key(nfc_, true));
        assert_ne!(key("File", true), key("file", true));
        assert_eq!(key("File", false), key("file", false));
        assert_eq!(key("ÉTÉ", false), key("été", false));
    }

    #[test]
    fn invalid_names() {
        assert!(invalid_reason("").is_some());
        assert!(invalid_reason("a/b").is_some());
        assert!(invalid_reason("a:b").is_some());
        assert!(invalid_reason("..").is_some());
        assert!(invalid_reason(&"x".repeat(256)).is_some());
        assert!(invalid_reason(&"x".repeat(255)).is_none());
        assert!(invalid_reason(".hidden").is_none());
    }
}
