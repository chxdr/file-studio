//! App-path proofs for Safe Transfer (F-026, proposal §17): a copy started
//! through the core's engine onto a disk image, the "card" pulled out
//! mid-copy, put back, offered for resuming, and resumed to the end, with
//! every real name holding a complete file at every step.
//!
//! `#[ignore]`d: they create, yank and delete disk images (expect a "Disk
//! Not Ejected Properly" notification each). Run them in one batch:
//! `cargo test -p fstudio-core --test transfer_proof -- --ignored --test-threads=1`.
#![cfg(target_os = "macos")]

mod common;
#[path = "../../safe-transfer/tests/common/mod.rs"]
mod st_harness;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use fstudio_core::*;
use st_harness::Image;

/// Deterministic, incompressible bytes.
fn bytes(n: usize, seed: u64) -> Vec<u8> {
    st_harness::bytes(n, seed)
}

/// A shoot: six 24 MB raws and 60 sidecars.
fn shoot(dir: &Path) {
    let d = dir.join("shoot/100MSDCF");
    std::fs::create_dir_all(&d).unwrap();
    for i in 0..6 {
        std::fs::write(d.join(format!("DSC0{i:04}.ARW")), bytes(24 << 20, i)).unwrap();
    }
    for i in 0..60 {
        std::fs::write(d.join(format!("DSC0{i:04}.XMP")), format!("xmp {i}")).unwrap();
    }
}

/// Every file under `copy` (a real name) is complete and equal to its
/// original under `orig`.
fn real_names_are_complete(orig: &Path, copy: &Path) {
    let Ok(rd) = std::fs::read_dir(copy) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if n.ends_with(".st-partial") || n.starts_with("._") {
            continue;
        }
        let (o, c) = (orig.join(&name), e.path());
        if c.is_dir() {
            real_names_are_complete(&o, &c);
        } else {
            assert_eq!(
                std::fs::read(&c).unwrap(),
                std::fs::read(&o).unwrap(),
                "{} is under its real name but isn't complete",
                c.display()
            );
        }
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(120), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "creates and yanks a disk image; run with --ignored"]
fn a_card_pulled_mid_copy_is_offered_back_and_resumes_through_the_app() {
    let h = harness();
    h.engine
        .set_setting("operations.copy_backend", "safe_transfer")
        .unwrap();
    shoot(&h.root);
    let card = Image::new("ExFAT", 512);
    let q = h.engine.ops();
    let id = q
        .enqueue(OpRequest::Copy {
            sources: vec![h.root.join("shoot")],
            dest_dir: card.mount.clone(),
        })
        .unwrap();
    // Pull the card once a good part of the copy has landed.
    wait_until("40 MB copied", || {
        q.status(id)
            .is_some_and(|s| s.bytes_done > 40 << 20 || s.state.is_finished())
    });
    card.pull_out();
    let st = q.wait(id, Duration::from_secs(120)).unwrap();
    assert_eq!(st.state, OpState::Interrupted, "{st:?}");
    let int = q.interrupted().unwrap();
    assert_eq!(int.len(), 1, "{int:?}");
    let (ready, note) = int[0].resume_hint();
    assert!(!ready, "the card is away: {int:?}");
    assert!(note.unwrap().starts_with("Insert “"), "{int:?}");

    // The card comes back: the app is told, and offers to resume.
    card.put_back();
    while h.rx.try_recv().is_ok() {}
    h.engine.volumes_changed();
    let t = Instant::now();
    let prompt = loop {
        match h.rx.recv_timeout(Duration::from_secs(30)) {
            Ok(CoreEvent::Shell {
                request: ShellRequest::ResumePrompt { op, question, .. },
                ..
            }) if op == id => break question,
            Ok(_) => {}
            Err(e) => panic!("no resume prompt: {e}"),
        }
        assert!(t.elapsed() < Duration::from_secs(30), "no resume prompt");
    };
    assert!(prompt.starts_with("Resume “"), "{prompt}");
    // Before resuming: whatever is under a real name is complete.
    real_names_are_complete(&h.root.join("shoot"), &card.mount.join("shoot"));
    assert!(q.interrupted().unwrap()[0].resume_hint().0);

    q.resume(id).unwrap();
    let st = q.wait(id, Duration::from_secs(300)).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    real_names_are_complete(&h.root.join("shoot"), &card.mount.join("shoot"));
    let n = std::fs::read_dir(card.mount.join("shoot/100MSDCF"))
        .unwrap()
        .filter(|e| {
            let n = e.as_ref().unwrap().file_name();
            let n = n.to_string_lossy();
            !n.starts_with("._") && !n.ends_with(".st-partial")
        })
        .count();
    assert_eq!(n, 66, "every file arrived");
    // Undo works on what Safe Transfer placed: it goes to the Trash.
    assert!(h.engine.undo_state().undo.is_some());
}

#[test]
#[ignore = "creates and yanks a disk image; run with --ignored"]
fn the_sidebar_offers_resume_transfer_for_the_card_and_resume_all_runs_it() {
    let h = harness();
    h.engine
        .set_setting("operations.copy_backend", "safe_transfer")
        .unwrap();
    shoot(&h.root);
    let card = Image::new("ExFAT", 512);
    let q = h.engine.ops();
    let id = q
        .enqueue(OpRequest::Copy {
            sources: vec![h.root.join("shoot")],
            dest_dir: card.mount.clone(),
        })
        .unwrap();
    wait_until("20 MB copied", || {
        q.status(id)
            .is_some_and(|s| s.bytes_done > 20 << 20 || s.state.is_finished())
    });
    card.pull_out();
    assert_eq!(
        q.wait(id, Duration::from_secs(120)).unwrap().state,
        OpState::Interrupted
    );
    // Nothing to resume while it is away: the command says what it needs.
    let out = h
        .engine
        .perform(CommandInvocation::new("transfer.resume_all"), Via::Mouse)
        .unwrap();
    assert!(
        matches!(&out.shell, Some(ShellRequest::Toast { text, .. }) if text.starts_with("Insert “")),
        "{out:?}"
    );
    card.put_back();
    assert_eq!(q.interrupted_on_volume(&card.mount).unwrap(), vec![id]);
    let out = h
        .engine
        .perform(CommandInvocation::new("transfer.resume_all"), Via::Mouse)
        .unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("Resumed the transfer"),
        "{out:?}"
    );
    assert_eq!(
        q.wait(id, Duration::from_secs(300)).unwrap().state,
        OpState::Completed
    );
    real_names_are_complete(&h.root.join("shoot"), &card.mount.join("shoot"));
}
