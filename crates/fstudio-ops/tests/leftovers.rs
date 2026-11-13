//! Edge fix 3: a copy that fails leaves no temporary file behind. A card
//! pulled out at 74 % of a 2 GB copy left `.bigfile.1.fstudio-partial`
//! (1.6 GB, hidden) on it: the removal after the failure couldn't reach the
//! card, and the journal forgot the file anyway. Now the record is kept and
//! the file is removed once its disk is back, and nothing else ever is.

mod common;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use fstudio_ops::*;

/// Makes the destination folder unreachable right after the temporary file
/// is written (as if its volume went away) and fails the item.
struct FolderGoesAway {
    folder: PathBuf,
    parked: PathBuf,
}

impl FaultHook for FolderGoesAway {
    fn check(&self, step: FaultStep, _path: &Path) -> io::Result<()> {
        if step == FaultStep::AfterCopy {
            std::fs::rename(&self.folder, &self.parked)?;
            return Err(io::Error::from_raw_os_error(libc::ENXIO));
        }
        Ok(())
    }
}

fn reopen(e: &Env) -> OpQueue {
    let (tx, _rx) = crossbeam_channel::unbounded();
    open_queue(&e.db_path, OpsConfig::default(), &e.trash, Arc::new(tx))
}

/// A failure with the folder reachable: the temporary file goes at once.
#[test]
fn a_failed_copy_leaves_no_temporary_file() {
    let e = env_with(OpsConfig {
        faults: Some(Arc::new(FailAt {
            step: FaultStep::AfterCopy,
            needle: "big".into(),
        })),
        ..OpsConfig::default()
    });
    write(&e.p("src/bigfile"), &vec![7u8; 1 << 20]);
    std::fs::create_dir_all(e.p("card")).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src/bigfile")],
        dest_dir: e.p("card"),
    });
    assert_eq!(st.errors.len(), 1, "{st:?}");
    assert!(partials(&e.p("card")).is_empty());
    assert!(!exists(&e.p("card/bigfile")));
    assert!(e.q.sweep_leftover_partials().unwrap().is_empty());
}

/// The folder vanished with the file in it: the record is kept, and the
/// file is removed once the folder is back, also after a relaunch. A
/// same-pattern name the journal doesn't record is never touched.
#[test]
fn a_partial_on_a_disk_that_went_away_is_removed_when_it_returns() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let parked = dir.path().join("parked");
    std::fs::create_dir_all(&card).unwrap();
    let e = env_with(OpsConfig {
        faults: Some(Arc::new(FolderGoesAway {
            folder: card.clone(),
            parked: parked.clone(),
        })),
        ..OpsConfig::default()
    });
    write(&e.p("src/bigfile"), &vec![7u8; 1 << 20]);
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src/bigfile")],
        dest_dir: card.clone(),
    });
    if on_safe_transfer() {
        // Stricter: Safe Transfer doesn't fail the file. The copy stops,
        // resumable, keeping its journal-matched partial on the card; when
        // the card is back, resuming finishes it and leaves nothing behind.
        assert_eq!(st.state, OpState::Interrupted, "{st:?}");
        assert!(st.errors.is_empty(), "{st:?}");
        let left = partials(&parked);
        assert!(left.len() <= 1, "{left:?}");
        write(&parked.join("mine.txt"), b"keep");
        std::fs::rename(&parked, &card).unwrap();
        let q = reopen(&e);
        let int = q.interrupted().unwrap();
        assert_eq!(int.len(), 1, "{int:?}");
        q.resume(int[0].id).unwrap();
        let st = q.wait(int[0].id, WAIT).unwrap();
        assert_eq!(st.state, OpState::Completed, "{st:?}");
        assert_eq!(read(&card.join("bigfile")), vec![7u8; 1 << 20]);
        assert_eq!(read(&card.join("mine.txt")), b"keep");
        assert!(partials(&card).is_empty());
        return;
    }
    assert_eq!(st.errors.len(), 1, "{st:?}");
    // Left on the "card", which is away.
    let left = partials(&parked);
    assert_eq!(left.len(), 1, "{left:?}");
    let name = left[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(".bigfile.") && name.ends_with(".fstudio-partial"));
    // A decoy with our pattern that the journal doesn't record, and a file
    // of the user's.
    write(&parked.join(".decoy.99.fstudio-partial"), b"not ours");
    write(&parked.join("mine.txt"), b"keep");
    // Still away: nothing is removed and the record stays.
    assert!(e.q.sweep_leftover_partials().unwrap().is_empty());
    // (An empty mount-point folder left on the startup disk is another
    // device, so it isn't taken for the card: see the hdiutil test below.)
    // Relaunch while it is away: the record survives.
    let q = reopen(&e);
    assert!(q.sweep_leftover_partials().unwrap().is_empty());
    // The card comes back.
    std::fs::rename(&parked, &card).unwrap();
    let swept = q.sweep_leftover_partials().unwrap();
    assert_eq!(swept.len(), 1, "{swept:?}");
    assert_eq!(swept[0].bytes, 1 << 20);
    assert_eq!(swept[0].path.file_name().unwrap().to_string_lossy(), name);
    assert!(!exists(&card.join(&name)));
    // Nothing else was touched, and a second sweep has nothing to do.
    assert!(exists(&card.join(".decoy.99.fstudio-partial")));
    assert_eq!(read(&card.join("mine.txt")), b"keep");
    assert!(q.sweep_leftover_partials().unwrap().is_empty());
}

/// The real thing: an exFAT image force-detached in the middle of a copy,
/// then attached again. Needs `hdiutil`; run with `--ignored`.
#[test]
#[ignore = "attaches and force-detaches a disk image"]
fn a_card_pulled_out_mid_copy_is_cleaned_when_it_comes_back() {
    let img = Image::new("ExFAT", 96);
    let e = env_with(OpsConfig {
        progress_interval: Duration::from_millis(5),
        ..OpsConfig::default()
    });
    write(&e.p("src/bigfile"), &vec![0x5au8; 64 << 20]);
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("src/bigfile")],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    // Pause once some bytes are on the card (the copy blocks inside its
    // progress callback), pull the card out, and let it go on.
    let t = Instant::now();
    loop {
        let st = e.q.status(id).unwrap();
        if st.bytes_done > 0 {
            break;
        }
        assert!(!st.state.is_finished(), "finished too soon: {st:?}");
        assert!(t.elapsed() < WAIT);
        std::thread::sleep(Duration::from_millis(1));
    }
    e.q.pause(id).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    img.pull_out();
    e.q.resume(id).unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.errors.len(), 1, "the item failed: {st:?}");
    // The mount point is an empty folder on the startup disk now: nothing
    // to remove there, and the record is kept.
    assert!(e.q.sweep_leftover_partials().unwrap().is_empty());
    img.put_back();
    let on_card = partials(&img.mount);
    assert!(
        !on_card.is_empty(),
        "the pulled copy left its temporary file"
    );
    // A file of the user's that happens to share our pattern stays.
    write(&img.mount.join(".theirs.7.fstudio-partial"), b"not ours");
    let swept = e.q.sweep_leftover_partials().unwrap();
    assert_eq!(swept.len(), 1, "{swept:?}");
    // The temporary file and its AppleDouble companion (`._…`, exFAT keeps
    // extended attributes there) are gone; nothing else was touched.
    let left: Vec<String> = partials(&img.mount)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        !left.iter().any(|n| n.contains("bigfile")),
        "ours are gone: {left:?}"
    );
    assert!(left.contains(&".theirs.7.fstudio-partial".to_string()));
    assert_eq!(
        read(&img.mount.join(".theirs.7.fstudio-partial")),
        b"not ours"
    );
    assert!(!exists(&img.mount.join("bigfile")));
}
