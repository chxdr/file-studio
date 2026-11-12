use std::path::{Path, PathBuf};
use std::sync::Arc;

use fstudio_path::testing::{MemDirSource, MemFs};
use fstudio_path::{
    Completer, CompleterConfig, CompletionKind, Frecency, FrecencyConfig, ParseContext, TabCycle,
    TabOutcome,
};

const NOW: u64 = 1_800_000_000;

fn ctx() -> ParseContext {
    ParseContext::new("/Users/me/Code", "/Users/me", Box::new(MemFs::new()))
        .with_env(|k| (k == "HOME").then(|| "/Users/me".to_owned()))
}

fn frec() -> Frecency {
    let f = Frecency::in_memory(FrecencyConfig::default());
    f.set_clock(|| NOW);
    f
}

fn source() -> MemDirSource {
    MemDirSource::new()
        .with_dir(
            "/Users/me",
            &[
                "Pictures",
                "Projects",
                "Documents",
                "Downloads",
                ".config",
                "Code",
                "Photoshop.app",
                "Caf\u{e9}",
                "Cafe\u{301}x",
            ],
        )
        .with_dir(
            "/Users/me/Pictures",
            &["2025", "2026", "2026-raw", "Screenshots"],
        )
        .with_dir("/Users/me/Code", &["alpha", "alpine", "beta"])
        .with_dir("/Users/me/Code/alpha", &["src"])
        .with_dir("/", &["Users", "Volumes", "tmp"])
        .with_slow("/Volumes/nas")
}

fn completer(f: Frecency) -> Completer {
    Completer::new(Arc::new(source()), f, CompleterConfig::default())
}

fn names(c: &fstudio_path::Completions) -> Vec<&str> {
    c.items.iter().map(|i| i.display_name.as_str()).collect()
}

#[test]
fn gig_finds_the_gig_folder_first() {
    let f = frec();
    let gig = "/Users/me/Pictures/2026/2026-11-11_gig";
    for _ in 0..5 {
        f.visit_at(Path::new(gig), NOW - 100);
    }
    f.visit_at(Path::new("/Users/me/Documents/gigabyte-manuals"), NOW - 100);
    f.visit_at(Path::new("/Users/me/Music/Gigs"), NOW - 5 * 86_400);
    f.visit_at(Path::new("/Users/me/Code/g/i/g/nested"), NOW - 100);
    f.visit_at(Path::new("/Users/me/Downloads"), NOW - 100);
    let c = completer(f).complete("gig", 3, &ctx());
    assert_eq!(c.items[0].full_path, PathBuf::from(gig), "{:?}", names(&c));
    assert_eq!(c.items[0].kind, CompletionKind::Frecent);
    assert_eq!(c.items[0].breadcrumb_text, "~/Pictures/2026/2026-11-11_gig");
    assert!(c.items[0].last_visit.is_some());
    assert!(!c.items[0].match_ranges.is_empty());
    assert!(
        c.items
            .iter()
            .all(|i| i.full_path != Path::new("/Users/me/Downloads"))
    );
}

#[test]
fn bare_word_is_subsequence_and_case_insensitive() {
    let f = frec();
    f.visit_at(Path::new("/Users/me/Pictures/2026"), NOW);
    let c = completer(f).complete("PCT26", 5, &ctx());
    // "pct26" is a subsequence of ".../pictures/2026" in the path.
    assert_eq!(names(&c), vec!["2026"]);
}

#[test]
fn bare_word_matches_nfd_against_nfc() {
    let f = frec();
    f.visit_at(Path::new("/Users/me/Caf\u{e9}"), NOW);
    let c = completer(f).complete("cafe\u{301}", 6, &ctx());
    assert_eq!(c.items.len(), 1);
}

#[test]
fn path_like_lists_children_prefix_first() {
    let c = completer(frec()).complete("~/Pictures/2026", "~/Pictures/2026".len(), &ctx());
    assert_eq!(names(&c)[..2], ["2026", "2026-raw"], "{:?}", names(&c));
    assert!(c.items.iter().all(|i| i.kind == CompletionKind::Child));
    assert_eq!(c.items[0].insert_text, "~/Pictures/2026/");
    assert_eq!(c.parent.as_deref(), Some(Path::new("/Users/me/Pictures")));
}

#[test]
fn subsequence_children_follow_prefix_children() {
    let c = completer(frec()).complete("/Users/me/P", 11, &ctx());
    let n = names(&c);
    // Prefix: Pictures, Projects. Subsequence only: Photoshop.app is a package (skipped).
    assert_eq!(n[..2], ["Pictures", "Projects"]);
    assert!(!n.contains(&"Photoshop.app"));
    let c = completer(frec()).complete("/Users/me/dcm", 13, &ctx());
    assert_eq!(names(&c), vec!["Documents"]); // d..c..m is a subsequence
    let c = completer(frec()).complete("/Users/me/dwn", 13, &ctx());
    assert_eq!(names(&c), vec!["Downloads"]);
}

#[test]
fn hidden_folders_only_when_asked() {
    let c = completer(frec()).complete("/Users/me/", 10, &ctx());
    assert!(!names(&c).contains(&".config"));
    let c = completer(frec()).complete("/Users/me/.", 11, &ctx());
    assert_eq!(names(&c), vec![".config"]);
}

#[test]
fn case_and_normalisation_insensitive_children() {
    let c = completer(frec()).complete("/Users/me/pic", 13, &ctx());
    assert_eq!(names(&c)[0], "Pictures");
    // NFC typed matches NFD-stored name and vice versa.
    let c = completer(frec()).complete("/Users/me/Caf\u{e9}", "/Users/me/Caf\u{e9}".len(), &ctx());
    let n = names(&c);
    assert!(
        n.contains(&"Caf\u{e9}") && n.contains(&"Cafe\u{301}x"),
        "{n:?}"
    );
}

#[test]
fn frecent_children_are_boosted() {
    let f = frec();
    for _ in 0..10 {
        f.visit_at(Path::new("/Users/me/Projects"), NOW);
    }
    let c = completer(f).complete("/Users/me/P", 11, &ctx());
    assert_eq!(names(&c)[0], "Projects");
}

#[test]
fn deeper_frecent_folders_are_offered_after_children() {
    let f = frec();
    f.visit_at(Path::new("/Users/me/Pictures/2026/2026-11-11_gig"), NOW);
    let c = completer(f).complete("~/Pictures/gig", "~/Pictures/gig".len(), &ctx());
    assert_eq!(c.items.len(), 1);
    assert_eq!(c.items[0].kind, CompletionKind::Frecent);
    assert_eq!(
        c.items[0].insert_text,
        "/Users/me/Pictures/2026/2026-11-11_gig/"
    );
}

#[test]
fn relative_dot_and_dollar_paths() {
    let c = completer(frec()).complete("../Pic", 6, &ctx());
    assert_eq!(names(&c)[0], "Pictures");
    let c = completer(frec()).complete("$HOME/Pic", 9, &ctx());
    assert_eq!(names(&c)[0], "Pictures");
    assert_eq!(c.items[0].insert_text, "$HOME/Pictures/");
    let c = completer(frec()).complete("./al", 4, &ctx());
    assert_eq!(names(&c), vec!["alpha", "alpine"]);
}

#[test]
fn slow_folders_are_never_listed() {
    // MemDirSource panics if a slow folder is listed.
    let c = completer(frec()).complete("/Volumes/nas/ph", 15, &ctx());
    assert!(c.skipped_slow);
    assert!(c.items.is_empty());
}

#[test]
fn missing_parent_gives_nothing() {
    let c = completer(frec()).complete("/Users/me/Nope/x", 16, &ctx());
    assert!(c.items.is_empty());
}

#[test]
fn ghost_text_for_best_prefix_match() {
    let c = completer(frec()).complete("/Users/me/Pict", 14, &ctx());
    assert_eq!(c.ghost_text, "ures/");
    // Not at the end of the text: no ghost.
    let c = completer(frec()).complete("/Users/me/Pict", 12, &ctx());
    assert_eq!(c.ghost_text, "");
    // Case differs from disk: still completes the rest.
    let c = completer(frec()).complete("/Users/me/pict", 14, &ctx());
    assert_eq!(c.ghost_text, "ures/");
}

#[test]
fn tab_extends_common_prefix_then_cycles() {
    let comp = completer(frec());
    let mut tab = TabCycle::new();

    // "al" -> alpha, alpine share "alp".
    let input = "./al";
    let c = comp.complete(input, input.len(), &ctx());
    assert_eq!(
        tab.press(&c, false),
        TabOutcome::Extend {
            new_input: "./alp".into()
        }
    );
    assert!(!tab.is_cycling());

    // Nothing more to extend: select row 1, then cycle, wrapping.
    let input = "./alp";
    let c = comp.complete(input, input.len(), &ctx());
    assert!(c.common_prefix_extension.is_none());
    assert_eq!(
        tab.press(&c, false),
        TabOutcome::Select {
            index: 0,
            text: "./alpha/".into()
        }
    );
    assert!(tab.is_cycling());
    assert_eq!(
        tab.press(&c, false),
        TabOutcome::Select {
            index: 1,
            text: "./alpine/".into()
        }
    );
    assert_eq!(
        tab.press(&c, false),
        TabOutcome::Select {
            index: 0,
            text: "./alpha/".into()
        }
    );
    assert_eq!(
        tab.press(&c, true),
        TabOutcome::Select {
            index: 1,
            text: "./alpine/".into()
        }
    );
    tab.reset();
    assert!(!tab.is_cycling());
}

#[test]
fn tab_on_unique_directory_adds_slash() {
    let comp = completer(frec());
    let c = comp.complete("/Users/me/Pictures/Sc", 21, &ctx());
    let ext = c.common_prefix_extension.expect("extension");
    assert_eq!(ext.new_input, "/Users/me/Pictures/Screenshots/");
    // Exact name without slash: Tab adds the slash.
    let c = comp.complete("/Users/me/Pictures/Screenshots", 30, &ctx());
    assert_eq!(
        c.common_prefix_extension.unwrap().new_input,
        "/Users/me/Pictures/Screenshots/"
    );
    // After the slash, the children are listed (the shell recomputes).
    let c = comp.complete("/Users/me/Code/alpha/", 21, &ctx());
    assert_eq!(names(&c), vec!["src"]);
}

#[test]
fn tab_extension_uses_on_disk_spelling() {
    let comp = completer(frec());
    let c = comp.complete("/Users/me/Pictures/screen", 25, &ctx());
    assert_eq!(
        c.common_prefix_extension.unwrap().new_input,
        "/Users/me/Pictures/Screenshots/"
    );
    let c = comp.complete("/Users/me/Pictures/20", 21, &ctx());
    assert_eq!(
        c.common_prefix_extension.unwrap().new_input,
        "/Users/me/Pictures/202"
    ); // 2025, 2026, 2026-raw share "202"
}

#[test]
fn tab_shift_starts_from_the_last_row() {
    let comp = completer(frec());
    let c = comp.complete("./alp", 5, &ctx());
    let mut tab = TabCycle::new();
    assert_eq!(
        tab.press(&c, true),
        TabOutcome::Select {
            index: 1,
            text: "./alpine/".into()
        }
    );
}

#[test]
fn tilde_and_dotdot_tab_add_a_slash() {
    let comp = completer(frec());
    let c = comp.complete("~", 1, &ctx());
    assert_eq!(c.common_prefix_extension.unwrap().new_input, "~/");
    let c = comp.complete("..", 2, &ctx());
    assert_eq!(c.common_prefix_extension.unwrap().new_input, "../");
}

#[test]
fn empty_input_and_bad_cursor() {
    let comp = completer(frec());
    assert!(comp.complete("", 0, &ctx()).items.is_empty());
    let _ = comp.complete("/Users/me/Caf\u{e9}", 14, &ctx()); // inside a multibyte char
    let _ = comp.complete("/Users", 9999, &ctx());
}

#[test]
fn match_ranges_cover_the_typed_prefix() {
    let c = completer(frec()).complete("/Users/me/Pic", 13, &ctx());
    assert_eq!(c.items[0].display_name, "Pictures");
    assert_eq!(c.items[0].match_ranges, vec![0..3]);
}

#[test]
fn cache_is_used_after_first_keystroke() {
    use fstudio_path::{CachedDirSource, DirSource, StdDirSource};
    let dir = tempfile::tempdir().unwrap();
    for n in ["alpha", "alpine", "beta"] {
        std::fs::create_dir(dir.path().join(n)).unwrap();
    }
    let src = CachedDirSource::new(StdDirSource::new(), 2);
    let a = src.list_dirs(dir.path()).unwrap();
    let b = src.list_dirs(dir.path()).unwrap();
    assert!(
        Arc::ptr_eq(&a, &b),
        "second listing must come from the cache"
    );
    // A change invalidates (mtime differs).
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::create_dir(dir.path().join("gamma")).unwrap();
    let c = src.list_dirs(dir.path()).unwrap();
    assert_eq!(c.children.len(), 4);
    // LRU: capacity 2.
    let d2 = tempfile::tempdir().unwrap();
    let d3 = tempfile::tempdir().unwrap();
    src.list_dirs(d2.path()).unwrap();
    src.list_dirs(d3.path()).unwrap();
    assert_eq!(src.len(), 2);
}

#[test]
fn std_source_skips_packages_and_files_and_follows_symlinks() {
    use fstudio_path::{DirSource, StdDirSource};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::create_dir(p.join("Real")).unwrap();
    std::fs::create_dir(p.join("Thing.app")).unwrap();
    std::fs::write(p.join("file.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(p.join("Real"), p.join("ToReal")).unwrap();
    std::os::unix::fs::symlink(p.join("file.txt"), p.join("ToFile")).unwrap();
    std::os::unix::fs::symlink(p.join("loop"), p.join("loop")).unwrap(); // self loop
    let l = StdDirSource::new().list_dirs(p).unwrap();
    let n: Vec<&str> = l.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(n, ["Real", "ToReal"]);
    let l = StdDirSource::with_packages().list_dirs(p).unwrap();
    assert!(
        l.children
            .iter()
            .any(|c| c.name == "Thing.app" && c.is_package)
    );
}

#[test]
fn std_source_unreadable_folder_is_an_error_not_a_panic() {
    use fstudio_path::{DirSource, StdDirSource};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = StdDirSource::new().list_dirs(&locked);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Running as root can still read it; otherwise it must be an error.
    if unsafe_euid_nonzero() {
        assert!(r.is_err());
    }
}

fn unsafe_euid_nonzero() -> bool {
    std::env::var("USER").map(|u| u != "root").unwrap_or(true)
}
