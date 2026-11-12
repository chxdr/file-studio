use std::path::PathBuf;

use fstudio_path::testing::MemFs;
use fstudio_path::{Cleanup, Location, ParseContext, parse_location};

fn fs() -> MemFs {
    MemFs::new()
        .dir("/Users/me/Pictures/2026/2026-11-11_gig")
        .dir("/Users/me/My Folder (1)")
        .dir("/Users/me/Caf\u{e9}")
        .file("/Users/me/Documents/Notes.txt")
        .dir("/Users/me/Code")
        .dir("/Users/other")
        .dir("/Volumes/Card/DCIM")
        .dir("/it's")
        .dir("/tmp/real")
        .symlink("/Users/me/link", "/tmp/real")
}

fn ctx() -> ParseContext {
    ParseContext::new("/Users/me/Code", "/Users/me", Box::new(fs()))
        .with_env(|k| match k {
            "HOME" => Some("/Users/me".into()),
            "PICS" => Some("/Users/me/Pictures".into()),
            _ => None,
        })
        .with_user_home(|u| (u == "other").then(|| PathBuf::from("/Users/other")))
}

fn loc(raw: &str) -> Location {
    parse_location(raw, &ctx()).location
}

fn dir(p: &str) -> Location {
    Location::Dir(PathBuf::from(p))
}

#[test]
fn plain_absolute_dir() {
    assert_eq!(loc("/Users/me/Pictures"), dir("/Users/me/Pictures"));
}

#[test]
fn trims_and_takes_first_line() {
    let p = parse_location("  \n\n /Users/me/Pictures  \n/second\n", &ctx());
    assert_eq!(p.location, dir("/Users/me/Pictures"));
    assert!(p.cleanups.contains(&Cleanup::FirstLineOnly));
    assert!(p.chip_text().unwrap().starts_with("Cleaned up paste"));
}

#[test]
fn strips_one_pair_of_quotes() {
    for raw in [
        "\"/Users/me/Pictures\"",
        "'/Users/me/Pictures'",
        "\u{201C}/Users/me/Pictures\u{201D}",
    ] {
        let p = parse_location(raw, &ctx());
        assert_eq!(p.location, dir("/Users/me/Pictures"), "{raw}");
        assert!(p.cleanups.contains(&Cleanup::RemovedQuotes));
        assert!(p.chip_text().unwrap().contains("removed quotes"));
    }
}

#[test]
fn only_one_pair_and_only_matching() {
    // Mismatched or unbalanced quotes are left alone and just do not exist.
    for raw in [
        "\"/Users/me/Pictures",
        "'/Users/me/Pictures\"",
        "\"\"/Users/me/Pictures\"\"",
    ] {
        assert!(matches!(loc(raw), Location::Missing { .. }), "{raw}");
    }
}

#[test]
fn quote_inside_a_real_name_is_kept() {
    assert_eq!(loc("/it's"), dir("/it's"));
}

#[test]
fn file_url_forms() {
    for raw in [
        "file:///Users/me/My%20Folder%20(1)",
        "file://localhost/Users/me/My%20Folder%20(1)/",
        "FILE:///Users/me/My%20Folder%20%281%29",
        "file:/Users/me/My%20Folder%20(1)",
    ] {
        let p = parse_location(raw, &ctx());
        assert_eq!(p.location, dir("/Users/me/My Folder (1)"), "{raw}");
        assert!(p.cleanups.contains(&Cleanup::FileUrl));
    }
}

#[test]
fn file_url_odd_escapes_do_not_panic() {
    for raw in [
        "file://",
        "file:///",
        "file:///%",
        "file:///%zz",
        "file:///%4",
        "file:///%FF%FE",
        "file:///%00",
        "file://localhost",
        "file://otherhost/x",
        "file:",
        "file:foo",
        "file:///Users/me/%E2%82",
    ] {
        let _ = parse_location(raw, &ctx());
    }
    assert_eq!(loc("file:///"), dir("/"));
    assert_eq!(loc("file://localhost"), dir("/"));
    assert!(matches!(loc("file://otherhost/x"), Location::Remote(_)));
}

#[test]
fn file_url_does_not_expand_dollar_or_tilde() {
    // %24 is a literal `$`; it must not become an environment variable.
    assert!(matches!(loc("file:///%24HOME"), Location::Missing { .. }));
}

#[test]
fn backslash_unescape_when_literal_is_not_a_path() {
    let p = parse_location(r"/Users/me/My\ Folder\ \(1\)", &ctx());
    assert_eq!(p.location, dir("/Users/me/My Folder (1)"));
    assert!(p.cleanups.contains(&Cleanup::Unescaped));
}

#[test]
fn backslash_kept_when_literal_exists() {
    let fs = fs().dir(r"/Users/me/back\slash");
    let c = ParseContext::new("/Users/me", "/Users/me", Box::new(fs));
    let p = parse_location(r"/Users/me/back\slash", &c);
    assert_eq!(p.location, dir(r"/Users/me/back\slash"));
    assert!(!p.cleanups.contains(&Cleanup::Unescaped));
}

#[test]
fn escaped_dollar_stays_literal() {
    let fs = fs().dir("/Users/me/$HOME");
    let c = ParseContext::new("/Users/me", "/Users/me", Box::new(fs))
        .with_env(|_| Some("/nope".into()));
    assert_eq!(
        parse_location(r"/Users/me/\$HOME", &c).location,
        dir("/Users/me/$HOME")
    );
}

#[test]
fn trailing_backslash_is_harmless() {
    assert!(matches!(
        loc(r"/Users/me/Pictures\"),
        Location::Missing { .. }
    ));
    let _ = loc("\\");
    let _ = loc("\\\\\\");
}

#[test]
fn tilde_forms() {
    assert_eq!(loc("~"), dir("/Users/me"));
    assert_eq!(loc("~/Pictures"), dir("/Users/me/Pictures"));
    assert_eq!(loc("~other"), dir("/Users/other"));
    assert_eq!(loc("~other/"), dir("/Users/other"));
    // Unknown user and `~~` stay literal and are just missing relative names.
    assert!(matches!(loc("~nobody/x"), Location::Missing { .. }));
    match loc("~~") {
        Location::Missing {
            path,
            nearest_existing,
        } => {
            assert_eq!(path, PathBuf::from("/Users/me/Code/~~"));
            assert_eq!(nearest_existing, PathBuf::from("/Users/me/Code"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn env_vars() {
    assert_eq!(loc("$HOME/Pictures"), dir("/Users/me/Pictures"));
    assert_eq!(loc("${PICS}/2026"), dir("/Users/me/Pictures/2026"));
    let p = parse_location("$PICS", &ctx());
    assert!(p.cleanups.contains(&Cleanup::ExpandedVars));
    // Undefined and malformed names are left alone.
    assert!(matches!(loc("$NOPE/x"), Location::Missing { .. }));
    assert!(matches!(loc("${PICS"), Location::Missing { .. }));
    assert!(matches!(loc("$"), Location::Missing { .. }));
    assert!(matches!(loc("${}"), Location::Missing { .. }));
}

#[test]
fn relative_and_lexical() {
    assert_eq!(loc(""), dir("/Users/me/Code"));
    assert_eq!(loc("."), dir("/Users/me/Code"));
    assert_eq!(loc(".."), dir("/Users/me"));
    assert_eq!(
        loc("../Pictures/./2026/../2026"),
        dir("/Users/me/Pictures/2026")
    );
    assert_eq!(loc("/../../.."), dir("/"));
    assert_eq!(loc("//Users///me//"), dir("/Users/me"));
    let p = parse_location("../Pictures", &ctx());
    assert!(p.cleanups.contains(&Cleanup::ResolvedRelative));
    assert!(p.cleanups.contains(&Cleanup::Normalised));
    assert!(
        p.chip_text().is_none(),
        "ordinary typing is not a paste cleanup"
    );
}

#[test]
fn file_path_selects_in_its_folder() {
    assert_eq!(
        loc("/Users/me/Documents/Notes.txt"),
        Location::File {
            dir: "/Users/me/Documents".into(),
            name: "Notes.txt".into()
        }
    );
    assert_eq!(
        loc("~/documents/notes.TXT"),
        Location::File {
            dir: "/Users/me/Documents".into(),
            name: "Notes.txt".into()
        }
    );
}

#[test]
fn on_disk_spelling_case_and_normalisation() {
    let p = parse_location("/users/ME/pictures", &ctx());
    assert_eq!(p.location, dir("/Users/me/Pictures"));
    assert!(p.cleanups.contains(&Cleanup::OnDiskSpelling));
    // Typed NFD, stored NFC.
    assert_eq!(loc("/Users/me/Cafe\u{301}"), dir("/Users/me/Caf\u{e9}"));
    // Typed NFC, stored NFD.
    let fs = MemFs::new().dir("/Users/me/Cafe\u{301}");
    let c = ParseContext::new("/", "/Users/me", Box::new(fs));
    assert_eq!(
        parse_location("/Users/me/Caf\u{e9}", &c).location,
        dir("/Users/me/Cafe\u{301}")
    );
}

#[test]
fn missing_reports_nearest_existing_ancestor() {
    match loc("~/Pictures/2027/new/deep") {
        Location::Missing {
            path,
            nearest_existing,
        } => {
            assert_eq!(path, PathBuf::from("/Users/me/Pictures/2027/new/deep"));
            assert_eq!(nearest_existing, PathBuf::from("/Users/me/Pictures"));
        }
        other => panic!("{other:?}"),
    }
    // The existing part uses the on-disk spelling.
    match loc("/users/me/pictures/x") {
        Location::Missing { path, .. } => assert_eq!(path, PathBuf::from("/Users/me/Pictures/x")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn below_a_file_is_missing_with_the_folder_as_nearest() {
    match loc("/Users/me/Documents/Notes.txt/inside") {
        Location::Missing {
            nearest_existing, ..
        } => {
            assert_eq!(nearest_existing, PathBuf::from("/Users/me/Documents"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn symlink_to_folder_is_a_dir_by_the_given_name() {
    assert_eq!(loc("~/link"), dir("/Users/me/link"));
}

#[test]
fn remote_urls_are_parse_only() {
    for raw in [
        "sftp://host/x",
        "smb://nas/share",
        "s3://bucket/key",
        "\"SMB://nas/share\"",
    ] {
        assert!(matches!(loc(raw), Location::Remote(_)), "{raw}");
    }
}

#[test]
fn leading_quote_never_becomes_a_remote_or_touches_disk_early() {
    // A probe that records every call: unclean text must not reach it.
    use fstudio_path::{FsProbe, ProbeKind};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    struct Spy(Arc<Mutex<Vec<String>>>);
    impl FsProbe for Spy {
        fn kind(&self, p: &Path) -> Option<ProbeKind> {
            self.0.lock().unwrap().push(p.display().to_string());
            None
        }
        fn lookup(&self, d: &Path, n: &str) -> Option<(String, ProbeKind)> {
            self.0.lock().unwrap().push(format!("{}|{n}", d.display()));
            None
        }
    }
    let log = Arc::new(Mutex::new(Vec::new()));
    let c = ParseContext::new("/cwd", "/h", Box::new(Spy(log.clone())));
    let p = parse_location("\"/Users/me/x\"", &c);
    assert!(matches!(p.location, Location::Missing { .. }));
    // The probe only ever saw the cleaned path, never the quote.
    assert!(
        log.lock().unwrap().iter().all(|s| !s.contains('"')),
        "{:?}",
        log.lock().unwrap()
    );
    // Remote URLs and file URLs for other hosts never reach the probe at all.
    log.lock().unwrap().clear();
    let _ = parse_location("smb://x/y", &c);
    let _ = parse_location("file://otherhost/y", &c);
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn control_characters_are_removed() {
    let p = parse_location("/Users/me/Pic\0tures", &ctx());
    assert_eq!(p.location, dir("/Users/me/Pictures"));
    assert!(p.cleanups.contains(&Cleanup::RemovedControlChars));
}

#[test]
fn long_lines_do_not_panic() {
    let long = format!("/{}", "a".repeat(4096));
    assert!(matches!(loc(&long), Location::Missing { .. }));
    let deep = "a/".repeat(2000);
    let _ = loc(&deep);
}
