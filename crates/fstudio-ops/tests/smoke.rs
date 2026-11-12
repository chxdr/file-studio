mod common;
use common::*;
use fstudio_ops::*;

#[test]
fn copy_tree_and_undo() {
    let e = env();
    write(&e.p("src/a.txt"), b"hello");
    write(&e.p("src/sub/b.bin"), &vec![7u8; 3 << 20]);
    std::os::unix::fs::symlink("a.txt", e.p("src/link")).unwrap();
    std::fs::create_dir_all(e.p("dst")).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(tree(&e.p("src")), tree(&e.p("dst/src")));
    assert!(partials(e.root()).is_empty());
    assert_eq!(e.q.undo_state().undo.as_deref(), Some("Undo Copy of “src”"));
    e.undo().unwrap();
    assert!(!exists(&e.p("dst/src")));
    assert!(exists(&e.trash.join("src")));
}
