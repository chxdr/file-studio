//! A folder on a card that is pulled out and put back lists every file again,
//! not only the ones created after the card came back.
//!
//! `#[ignore]`d: it creates, yanks and deletes a disk image (expect a "Disk
//! Not Ejected Properly" notification). Run it with
//! `cargo test -p fstudio-core --test remount -- --ignored`.
#![cfg(target_os = "macos")]

mod common;
#[path = "../../safe-transfer/tests/common/mod.rs"]
mod st_harness;

use common::*;
use st_harness::Image;

#[test]
#[ignore = "creates and yanks a disk image; run with --ignored"]
fn a_folder_on_a_card_put_back_lists_every_file() {
    let h = harness();
    let card = Image::new("ExFAT", 64);
    let folder = card.mount.join("DCIM");
    std::fs::create_dir(&folder).unwrap();
    for i in 0..20 {
        std::fs::write(folder.join(format!("IMG_{i:04}.JPG")), b"x").unwrap();
    }
    let t = h.window_at("");
    t.go(&folder.to_string_lossy()).unwrap();
    wait_complete(&t);
    assert_eq!(names(&t).len(), 20);

    card.pull_out();
    h.engine.volumes_changed();
    eventually("the pane notices the card is gone", || {
        t.listing().state() != fstudio_core::ListingState::Complete || names(&t).len() != 20
    });

    card.put_back();
    // Files written after the card is back, as a resumed copy would.
    for i in 20..25 {
        std::fs::write(folder.join(format!("IMG_{i:04}.JPG")), b"y").unwrap();
    }
    h.engine.volumes_changed();
    eventually("all 25 files are listed again", || {
        t.listing().state() == fstudio_core::ListingState::Complete && names(&t).len() == 25
    });
}
