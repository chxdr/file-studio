//! Parsing against the real disk (temporary folders only).

use std::path::PathBuf;

use fstudio_path::{Location, ParseContext, parse_location};

#[test]
fn real_disk_spelling_symlinks_and_loops() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("Caf\u{e9}/Sub")).unwrap();
    std::fs::write(root.join("Caf\u{e9}/Notes.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(root.join("Caf\u{e9}"), root.join("alias")).unwrap();
    std::os::unix::fs::symlink(root.join("loop"), root.join("loop")).unwrap();
    let ctx = ParseContext::new(&root, &root, Box::new(fstudio_path::StdProbe));

    // The exact spelling always resolves; a symlink to a folder is a folder.
    let p = parse_location(&format!("{}/Caf\u{e9}/Sub", root.display()), &ctx);
    assert_eq!(p.location, Location::Dir(root.join("Caf\u{e9}/Sub")));
    assert_eq!(
        parse_location("alias/Sub", &ctx).location,
        Location::Dir(root.join("alias/Sub"))
    );
    assert_eq!(
        parse_location("Caf\u{e9}/Notes.txt", &ctx).location,
        Location::File {
            dir: root.join("Caf\u{e9}"),
            name: "Notes.txt".into()
        }
    );
    // A self-referencing symlink is just missing (no hang, no panic).
    assert!(matches!(
        parse_location("loop/x", &ctx).location,
        Location::Missing { .. }
    ));

    // APFS is normalisation-insensitive on every volume, and case-insensitive by
    // default. Only assert what this volume actually does.
    let nfd = parse_location("Cafe\u{301}/Sub", &ctx).location;
    if root.join("Cafe\u{301}").exists() {
        assert_eq!(nfd, Location::Dir(root.join("Caf\u{e9}/Sub")));
    }
    if root.join("CAF\u{c9}").exists() {
        assert_eq!(
            parse_location("CAF\u{c9}", &ctx).location,
            Location::Dir(root.join("Caf\u{e9}"))
        );
    }
}

#[test]
fn real_disk_missing_and_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let locked = root.join("locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let ctx = ParseContext::new(&root, &root, Box::new(fstudio_path::StdProbe));
    let p = parse_location("locked/inner", &ctx);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Unreadable folders look missing from the inside; the nearest existing is the
    // folder we can see. (Running as root would see through it.)
    if let Location::Missing {
        nearest_existing, ..
    } = p.location
    {
        assert_eq!(nearest_existing, PathBuf::from(&root).join("locked"));
    }
    assert!(matches!(
        parse_location("nope/deeper", &ctx).location,
        Location::Missing { nearest_existing, .. } if nearest_existing == root
    ));
}
