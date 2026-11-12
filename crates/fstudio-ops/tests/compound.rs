//! Compound ops (safety audit S12–S14): several steps journaled as one op
//! with one undo entry.

mod common;

use common::*;
use fstudio_ops::*;

fn new_folder_with(e: &Env, items: &[&str]) -> OpRequest {
    OpRequest::Compound {
        label: "New Folder with Items".into(),
        steps: vec![
            OpRequest::NewFolder {
                parent: e.root().to_path_buf(),
                name: "Box".into(),
            },
            OpRequest::Move {
                sources: items.iter().map(|n| e.p(n)).collect(),
                dest_dir: e.p("Box"),
            },
        ],
    }
}

#[test]
fn steps_run_in_order_and_undo_is_one_step() {
    let e = env();
    std::fs::write(e.p("a"), b"a").unwrap();
    std::fs::write(e.p("b"), b"b").unwrap();
    let st = e.run(new_folder_with(&e, &["a", "b"]));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert!(e.p("Box/a").exists() && e.p("Box/b").exists());
    assert_eq!(
        e.q.undo_state().undo.as_deref(),
        Some("Undo New Folder with Items")
    );
    // One undo puts both items back and removes the (now empty) folder.
    e.undo().unwrap();
    assert!(e.p("a").exists() && e.p("b").exists());
    assert!(!e.p("Box").exists());
    assert_eq!(e.q.undo_state().undo, None, "one entry, used up");
    // Redo runs it again.
    let id = e.q.redo().unwrap().unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed);
    assert!(e.p("Box/a").exists());
}

#[test]
fn a_failing_step_stops_the_rest_and_undo_covers_what_ran() {
    let e = env();
    std::fs::write(e.p("a"), b"a").unwrap();
    // The second step moves an item that doesn't exist.
    let st = e.run(new_folder_with(&e, &["missing"]));
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert!(e.p("Box").is_dir());
    e.undo().unwrap();
    assert!(!e.p("Box").exists());
}

#[test]
fn undo_refuses_when_the_folder_gained_other_items() {
    let e = env();
    std::fs::write(e.p("a"), b"a").unwrap();
    e.run(new_folder_with(&e, &["a"]));
    std::fs::write(e.p("Box/new-work"), b"x").unwrap();
    // The move back runs; the folder isn't empty, so it stays (and says why).
    let ids = e.q.undo().unwrap();
    let st = e.q.wait(ids[0], WAIT).unwrap();
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert!(st.errors.iter().any(|x| x.message.contains("isn't empty")));
    assert!(e.p("a").exists());
    assert!(e.p("Box/new-work").exists());
}

#[test]
fn a_chain_of_new_folders_is_one_op() {
    let e = env();
    let st = e.run(OpRequest::Compound {
        label: "New Folders".into(),
        steps: vec![
            OpRequest::NewFolder {
                parent: e.root().to_path_buf(),
                name: "x".into(),
            },
            OpRequest::NewFolder {
                parent: e.p("x"),
                name: "y".into(),
            },
        ],
    });
    assert_eq!(st.state, OpState::Completed);
    assert!(e.p("x/y").is_dir());
    e.undo().unwrap();
    assert!(!e.p("x").exists());
    assert!(
        OpQueue::plan(
            &e.q,
            &OpRequest::Compound {
                label: "x".into(),
                steps: vec![]
            }
        )
        .refused
        .is_some()
    );
}
