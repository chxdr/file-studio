//! The test disk images never outlive a test process that dies without
//! unwinding (an abort or a kill skips `Drop`): the next one removes them.

mod common;
use common::*;
use std::path::PathBuf;
use std::process::Command;

/// The child side: makes an image, prints its path, then aborts.
#[test]
fn child_that_aborts_holding_an_image() {
    if std::env::var("FSTUDIO_ABORT_WITH_IMAGE").is_err() {
        return;
    }
    let img = Image::new("APFS", 16);
    eprintln!("IMAGE {}", img.img.display());
    std::process::abort();
}

#[test]
fn an_image_left_by_an_aborted_test_is_removed_by_the_next() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_that_aborts_holding_an_image",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("FSTUDIO_ABORT_WITH_IMAGE", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the child should have aborted");
    let text = String::from_utf8_lossy(&out.stderr);
    let img = PathBuf::from(
        text.lines()
            .find_map(|l| l.strip_prefix("IMAGE "))
            .expect("the child printed its image"),
    );
    assert!(
        is_attached(&img),
        "the aborted child left {} attached",
        img.display()
    );
    // The next image made in a live process sweeps it.
    let fresh = Image::new("APFS", 16);
    assert!(!is_attached(&img), "{} is still attached", img.display());
    assert!(!img.exists(), "{} wasn't deleted", img.display());
    drop(fresh);
}
