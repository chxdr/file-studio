use std::path::Path;

use fstudio_db::Db;
use fstudio_path::testing::MemFs;
use fstudio_path::zoxide::{parse_db, parse_query_ls};
use fstudio_path::{Frecency, FrecencyConfig};

const NOW: u64 = 1_800_000_000;

fn f(cfg: FrecencyConfig) -> Frecency {
    let f = Frecency::in_memory(cfg);
    f.set_clock(|| NOW);
    f
}

#[test]
fn visit_adds_one_rank_and_stamps_time() {
    let f = f(Default::default());
    f.visit_at(Path::new("/a/b"), NOW - 10);
    f.visit_at(Path::new("/a/b/"), NOW); // trailing slash is the same folder
    assert_eq!(f.len(), 1);
    assert_eq!(f.rank(Path::new("/a/b")), Some(2.0));
    assert_eq!(f.score(Path::new("/a/b")), Some(8.0)); // within an hour: x4
}

#[test]
fn recency_weights_match_zoxide() {
    let f = f(Default::default());
    f.visit_at(Path::new("/hour"), NOW - 3599);
    f.visit_at(Path::new("/day"), NOW - 3600);
    f.visit_at(Path::new("/day2"), NOW - 86_399);
    f.visit_at(Path::new("/week"), NOW - 86_400);
    f.visit_at(Path::new("/week2"), NOW - 604_799);
    f.visit_at(Path::new("/old"), NOW - 604_800);
    assert_eq!(f.score(Path::new("/hour")), Some(4.0));
    assert_eq!(f.score(Path::new("/day")), Some(2.0));
    assert_eq!(f.score(Path::new("/day2")), Some(2.0));
    assert_eq!(f.score(Path::new("/week")), Some(0.5));
    assert_eq!(f.score(Path::new("/week2")), Some(0.5));
    assert_eq!(f.score(Path::new("/old")), Some(0.25));
}

#[test]
fn ageing_scales_and_prunes_over_the_cap() {
    let f = f(FrecencyConfig {
        max_age: 100.0,
        ..Default::default()
    });
    for _ in 0..90 {
        f.visit_at(Path::new("/big"), NOW);
    }
    for i in 0..9 {
        f.visit_at(Path::new(&format!("/small{i}")), NOW);
    }
    assert!((f.total_rank() - 99.0).abs() < 1e-9);
    // One more visit crosses 100 and triggers ageing: factor = 0.9*100/total.
    f.visit_at(Path::new("/small0"), NOW); // total 100 (not above the cap)
    assert_eq!(f.len(), 10);
    f.visit_at(Path::new("/new"), NOW); // total 101 -> aged
    let factor = 90.0 / 101.0;
    assert!((f.rank(Path::new("/big")).unwrap() - 90.0 * factor).abs() < 1e-9);
    // Entries under rank 1.0 after scaling are dropped (rank 1 * 0.89 < 1).
    assert_eq!(f.rank(Path::new("/small1")), None);
    assert_eq!(f.rank(Path::new("/new")), None);
    assert!(f.total_rank() <= 100.0 * 0.9 + 1e-6);
}

#[test]
fn excluded_globs_are_not_recorded() {
    let f = f(FrecencyConfig {
        excluded: vec!["/Volumes/*".into(), "/tmp/?x".into()],
        ..Default::default()
    });
    assert!(!f.visit_at(Path::new("/Volumes/Card/DCIM"), NOW));
    assert!(!f.visit_at(Path::new("/tmp/ax"), NOW));
    assert!(f.visit_at(Path::new("/tmp/abx"), NOW));
    assert!(f.visit_at(Path::new("/Users/me"), NOW));
    assert_eq!(f.len(), 2);
    // Changing the setting forgets entries that now match.
    f.set_excluded(vec!["/tmp/*".into()]);
    assert_eq!(f.len(), 1);
}

#[test]
fn paths_that_disappeared_are_forgotten_lazily() {
    let fs = MemFs::new().dir("/here");
    let f = f(Default::default());
    f.visit_at(Path::new("/here"), NOW);
    f.visit_at(Path::new("/gone"), NOW);
    f.visit_at(Path::new("/gone2"), NOW);
    assert!(!f.forget_if_missing(Path::new("/here"), &fs));
    assert!(f.forget_if_missing(Path::new("/gone"), &fs));
    assert_eq!(f.len(), 2);
    assert_eq!(f.prune_missing(&fs, 10), 1);
    assert_eq!(f.len(), 1);
}

#[test]
fn persists_through_the_database_and_reloads() {
    let db = Db::open_in_memory().unwrap();
    {
        let f = Frecency::open(db.clone(), Default::default()).unwrap();
        f.visit_at(Path::new("/a"), NOW);
        f.visit_at(Path::new("/a"), NOW);
        f.visit_at(Path::new("/b"), NOW);
        f.remove(Path::new("/b"));
        f.flush();
    }
    let f = Frecency::open(db.clone(), Default::default()).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f.rank(Path::new("/a")), Some(2.0));
    let n: i64 = db
        .with(|c| c.query_row("SELECT count(*) FROM path_frecency", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn dropping_the_index_flushes_pending_writes() {
    let db = Db::open_in_memory().unwrap();
    {
        let f = Frecency::open(db.clone(), Default::default()).unwrap();
        for i in 0..200 {
            f.visit_at(Path::new(&format!("/p{i}")), NOW);
        }
    }
    let n: i64 = db
        .with(|c| c.query_row("SELECT count(*) FROM path_frecency", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(n, 200);
}

#[test]
fn ageing_is_persisted() {
    let db = Db::open_in_memory().unwrap();
    let f = Frecency::open(
        db.clone(),
        FrecencyConfig {
            max_age: 20.0,
            ..Default::default()
        },
    )
    .unwrap();
    f.set_clock(|| NOW);
    for i in 0..30 {
        f.visit_at(Path::new(&format!("/p{}", i % 3)), NOW);
    }
    f.flush();
    let sum: f64 = db
        .with(|c| c.query_row("SELECT sum(rank) FROM path_frecency", [], |r| r.get(0)))
        .unwrap();
    assert!((sum - f.total_rank()).abs() < 1e-6);
    assert!(sum <= 20.0 + 1e-6);
}

fn zo_bytes(entries: &[(&str, f64, u64)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (p, r, t) in entries {
        b.extend_from_slice(&(p.len() as u64).to_le_bytes());
        b.extend_from_slice(p.as_bytes());
        b.extend_from_slice(&r.to_le_bytes());
        b.extend_from_slice(&t.to_le_bytes());
    }
    b
}

#[test]
fn imports_zoxide_binary_db() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("db.zo");
    std::fs::write(
        &file,
        zo_bytes(&[
            ("/Users/me/code", 12.5, NOW - 10),
            ("/Users/me/docs", 3.0, NOW - 99_999),
            ("relative", 1.0, 1),
        ]),
    )
    .unwrap();
    let f = f(Default::default());
    f.visit_at(Path::new("/Users/me/code"), NOW - 5);
    let rep = f.import_zoxide(&file).unwrap();
    assert_eq!((rep.imported, rep.skipped), (2, 1));
    assert_eq!(f.rank(Path::new("/Users/me/code")), Some(13.5)); // merged
    assert_eq!(f.rank(Path::new("/Users/me/docs")), Some(3.0));
}

#[test]
fn zoxide_db_errors_are_clear() {
    assert!(parse_db(&[]).is_err());
    let mut v4 = zo_bytes(&[]);
    v4[0] = 4;
    assert!(parse_db(&v4).unwrap_err().to_string().contains("version 4"));
    let mut b = zo_bytes(&[("/a", 1.0, 1)]);
    b.truncate(b.len() - 3);
    assert!(parse_db(&b).is_err());
    // A huge entry count cannot allocate.
    let mut c = 3u32.to_le_bytes().to_vec();
    c.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(parse_db(&c).is_err());
}

#[test]
fn imports_query_ls_text() {
    let text = " 12.5 /Users/me/code\n  3.0 /Users/me/My Docs\n garbage\n 1.0 relative\n";
    let e = parse_query_ls(text, NOW);
    assert_eq!(e.len(), 2);
    assert_eq!(e[1].path, "/Users/me/My Docs");
    let f = f(Default::default());
    assert_eq!(f.import_entries(e).imported, 2);
}
