//! Which folders are shown as a single item (a "package" or bundle).

/// Extensions (lower case, no dot) of folders that Finder treats as one item.
pub const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "xpc",
    "prefpane",
    "saver",
    "qlgenerator",
    "mdimporter",
    "component",
    "colorpicker",
    "wdgt",
    "xcodeproj",
    "xcworkspace",
    "xcassets",
    "playground",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "imovielibrary",
    "fcpbundle",
    "logicx",
    "band",
    "rtfd",
    "pages",
    "numbers",
    "key",
    "sparsebundle",
    "sparseimage",
];

/// True when the folder name's extension is one of [`PACKAGE_EXTENSIONS`] (case-insensitive).
///
/// Dot-files such as `.app` have no extension and never count.
pub fn is_package_name(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    if dot == 0 {
        return false;
    }
    let ext = &name[dot + 1..];
    PACKAGE_EXTENSIONS
        .iter()
        .any(|p| ext.eq_ignore_ascii_case(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extensions() {
        assert!(is_package_name("Safari.app"));
        assert!(is_package_name("Photos Library.PHOTOSLIBRARY"));
        assert!(is_package_name("a.b.framework"));
        assert!(!is_package_name("notes.txt"));
        assert!(!is_package_name(".app"));
        assert!(!is_package_name("app"));
    }
}
