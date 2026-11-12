use std::path::{Path, PathBuf};

use fstudio_path::testing::MemDirSource;
use fstudio_path::{CrumbKind, PathStyle, Volume, breadcrumbs, collapse, copy_path_as, subfolders};

fn vols() -> Vec<Volume> {
    vec![
        Volume {
            name: "Macintosh HD".into(),
            mount: "/".into(),
        },
        Volume {
            name: "Card".into(),
            mount: "/Volumes/Card".into(),
        },
    ]
}

#[test]
fn breadcrumbs_start_at_the_volume_name() {
    let c = breadcrumbs(
        Path::new("/Users/me/Apps/Foo.app/Contents"),
        Path::new("/Users/me"),
        &vols(),
    );
    let got: Vec<(&str, CrumbKind)> = c.iter().map(|c| (c.name.as_str(), c.kind)).collect();
    assert_eq!(
        got,
        vec![
            ("Macintosh HD", CrumbKind::VolumeRoot),
            ("Users", CrumbKind::Folder),
            ("me", CrumbKind::Home),
            ("Apps", CrumbKind::Folder),
            ("Foo.app", CrumbKind::Package),
            ("Contents", CrumbKind::Folder),
        ]
    );
    assert_eq!(c[0].path, PathBuf::from("/"));
    assert_eq!(c[2].path, PathBuf::from("/Users/me"));
    assert!(c.last().unwrap().is_current && !c[0].is_current);
}

#[test]
fn breadcrumbs_on_another_volume_use_its_name() {
    let c = breadcrumbs(
        Path::new("/Volumes/Card/DCIM/100"),
        Path::new("/Users/me"),
        &vols(),
    );
    assert_eq!(c[0].name, "Card");
    assert_eq!(c[0].path, PathBuf::from("/Volumes/Card"));
    assert_eq!(c.len(), 3);
    // `/Volumes/CardX` must not match the `Card` mount.
    let c = breadcrumbs(
        Path::new("/Volumes/CardX/a"),
        Path::new("/Users/me"),
        &vols(),
    );
    assert_eq!(c[0].name, "Macintosh HD");
    assert_eq!(c[1].name, "Volumes");
}

#[test]
fn breadcrumbs_of_root_and_odd_input() {
    let c = breadcrumbs(Path::new("/"), Path::new("/Users/me"), &vols());
    assert_eq!(c.len(), 1);
    assert!(c[0].is_current);
    assert_eq!(
        breadcrumbs(Path::new("/a//b/./c/"), Path::new(""), &[]).len(),
        4
    );
}

#[test]
fn collapse_keeps_first_and_last_two() {
    let c = breadcrumbs(Path::new("/a/b/c/d/e/f"), Path::new("/zz"), &vols());
    assert_eq!(c.len(), 7);
    let (head, hidden, tail) = collapse(&c, 3);
    assert_eq!(head.len(), 1);
    assert_eq!(
        tail.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["e", "f"]
    );
    assert_eq!(
        hidden.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c", "d"]
    );
    let (head, hidden, tail) = collapse(&c, 7);
    assert_eq!((head.len(), hidden.len(), tail.len()), (7, 0, 0));
    let (head, hidden, tail) = collapse(&c, 0); // clamped to 2
    assert_eq!((head.len(), hidden.len(), tail.len()), (1, 5, 1));
}

#[test]
fn subfolders_for_the_chevron_menu() {
    let src = MemDirSource::new()
        .with_dir("/p", &["b", "A", ".hidden", "Thing.app"])
        .with_slow("/net");
    let s = subfolders(Path::new("/p"), &src);
    assert_eq!(
        s.iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect::<Vec<_>>(),
        vec![
            ("A", CrumbKind::Folder),
            ("b", CrumbKind::Folder),
            ("Thing.app", CrumbKind::Package),
        ]
    );
    assert_eq!(s[0].path, PathBuf::from("/p/A"));
    assert!(subfolders(Path::new("/net"), &src).is_empty());
    assert!(subfolders(Path::new("/missing"), &src).is_empty());
}

#[test]
fn copy_path_styles() {
    let home = Path::new("/Users/me");
    let p = Path::new("/Users/me/My Folder (1)/it's");
    assert_eq!(
        copy_path_as(p, PathStyle::Posix, home),
        "/Users/me/My Folder (1)/it's"
    );
    assert_eq!(
        copy_path_as(p, PathStyle::Escaped, home),
        r"/Users/me/My\ Folder\ \(1\)/it\'s"
    );
    assert_eq!(
        copy_path_as(p, PathStyle::HomeRelative, home),
        "~/My Folder (1)/it's"
    );
    assert_eq!(
        copy_path_as(p, PathStyle::Quoted, home),
        r"'/Users/me/My Folder (1)/it'\''s'"
    );
    assert_eq!(
        copy_path_as(Path::new("/Users/me/My Folder/"), PathStyle::FileUrl, home),
        "file:///Users/me/My%20Folder/"
    );
    assert_eq!(
        copy_path_as(Path::new("/Users/me/Café"), PathStyle::FileUrl, home),
        "file:///Users/me/Caf%C3%A9"
    );
    assert_eq!(
        copy_path_as(Path::new("/Users/me"), PathStyle::HomeRelative, home),
        "~"
    );
    // Whole components only: /Users/meadow is not inside /Users/me.
    assert_eq!(
        copy_path_as(Path::new("/Users/meadow/x"), PathStyle::HomeRelative, home),
        "/Users/meadow/x"
    );
    assert_eq!(
        copy_path_as(Path::new("/x"), PathStyle::HomeRelative, Path::new("")),
        "/x"
    );
    // Escaped output parses back to the original.
}

#[test]
fn escaped_copy_round_trips_through_the_parser() {
    use fstudio_path::testing::MemFs;
    use fstudio_path::{Location, ParseContext, parse_location};
    let name = "/Users/me/A $b (c) & 'd' ~e";
    let fs = MemFs::new().dir(name);
    let ctx = ParseContext::new("/", "/Users/me", Box::new(fs)).with_env(|_| Some("BAD".into()));
    // Quoted output is for shells; it round-trips only for names without `$` or `'`
    // (the parser strips one pair of quotes, then expands as usual).
    let plain = "/Users/me/A (c) & d ~e";
    let fs2 = MemFs::new().dir(plain);
    let ctx2 = ParseContext::new("/", "/Users/me", Box::new(fs2));
    for style in [PathStyle::Quoted, PathStyle::HomeRelative] {
        let q = copy_path_as(Path::new(plain), style, Path::new("/Users/me"));
        assert_eq!(
            parse_location(&q, &ctx2).location,
            Location::Dir(plain.into()),
            "{style:?}"
        );
    }
    for style in [PathStyle::Escaped, PathStyle::FileUrl] {
        let text = copy_path_as(Path::new(name), style, Path::new("/Users/me"));
        assert_eq!(
            parse_location(&text, &ctx).location,
            Location::Dir(name.into()),
            "{style:?}: {text}"
        );
    }
}
