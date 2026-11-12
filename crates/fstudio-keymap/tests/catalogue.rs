//! The catalogue test: the registry, the bindings and the overlay can't drift apart.

use std::collections::HashSet;

use fstudio_keymap::*;

const SNAPSHOT: &str = "tests/command_ids.txt";

#[test]
fn ids_are_unique_and_well_formed() {
    let mut seen = HashSet::new();
    for c in registry::all() {
        assert!(seen.insert(c.id.as_str()), "duplicate id {}", c.id);
        let s = c.id.as_str();
        assert!(
            s.split('.').count() == 2
                && s.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'),
            "bad id {s}"
        );
        assert!(!c.title.is_empty() && !c.description.is_empty(), "{s}");
        assert!(!c.contexts.is_empty(), "{s}");
    }
}

/// Ids are persisted in settings: removing or renaming one is a breaking change.
/// Adding is fine: run with `UPDATE_SNAPSHOT=1` and commit the new file.
#[test]
fn id_list_snapshot() {
    let now: Vec<&str> = registry::all().iter().map(|c| c.id.as_str()).collect();
    let text = now.join("\n") + "\n";
    if std::env::var("UPDATE_SNAPSHOT").is_ok_and(|v| v == "1") {
        std::fs::write(SNAPSHOT, &text).unwrap();
        return;
    }
    let old = std::fs::read_to_string(SNAPSHOT).expect("run with UPDATE_SNAPSHOT=1 to create it");
    let old: Vec<&str> = old.lines().collect();
    let missing: Vec<_> = old.iter().filter(|i| !now.contains(i)).collect();
    assert!(
        missing.is_empty(),
        "command ids removed or renamed: {missing:?}"
    );
    let added: Vec<_> = now.iter().filter(|i| !old.contains(i)).collect();
    assert!(
        added.is_empty(),
        "new command ids {added:?}: run UPDATE_SNAPSHOT=1 cargo test -p fstudio-keymap and commit {SNAPSHOT}"
    );
}

#[test]
fn every_binding_maps_to_a_command_and_fits_its_contexts() {
    for info in registry::all() {
        for b in info.standard.iter().chain(&info.vim) {
            assert!(
                info.contexts.iter().any(|c| b.when.implies(c)),
                "{}: binding {} is live outside the command's contexts",
                info.id,
                b.text()
            );
            assert!(validate_binding(b).is_ok(), "{}: {}", info.id, b.text());
            // Key-cap text round-trips.
            assert_eq!(
                Binding::parse(&b.text(), b.when.clone()).map(|x| x.keys),
                Some(b.keys.clone()),
                "{}",
                b.text()
            );
        }
        for b in &info.vim {
            assert!(
                b.when.implies(&When::VimNormal) || b.when.implies(&When::VimVisual),
                "{}",
                info.id
            );
        }
        for b in &info.standard {
            assert!(
                !b.when.overlaps(&When::VimNormal) || !matches!(b.when, When::VimNormal),
                "{}",
                info.id
            );
        }
        if info.shows_in_menu || info.shows_in_palette {
            assert_ne!(info.category, Category::TextFields, "{}", info.id);
        }
    }
    for profile in [Profile::Standard, Profile::Vim] {
        let k = Keymap::new(KeymapSettings {
            profile,
            ..Default::default()
        });
        for e in k.entries() {
            assert!(registry::get(e.command.as_str()).is_some());
        }
    }
}

#[test]
fn registry_covers_what_the_brief_lists() {
    for id in [
        "nav.open",
        "nav.up",
        "nav.back",
        "nav.forward",
        "nav.address_bar",
        "nav.quick_open",
        "nav.go_home",
        "nav.go_desktop",
        "nav.go_downloads",
        "select.all",
        "select.invert",
        "edit.copy",
        "edit.paste",
        "file.move_here",
        "file.copy_to_other_pane",
        "file.move_to_other_pane",
        "file.trash",
        "file.delete_immediately",
        "file.duplicate",
        "file.rename",
        "file.new_folder",
        "file.new_file",
        "view.icons",
        "view.list",
        "view.columns",
        "view.gallery",
        "sort.name",
        "sort.size",
        "view.show_hidden",
        "tab.new",
        "tab.close",
        "tab.next",
        "tab.previous",
        "tab.reopen_closed",
        "pane.toggle_dual",
        "pane.switch",
        "pane.swap",
        "pane.same_folder",
        "workspace.save",
        "workspace.switch",
        "preview.quick_look",
        "preview.get_info",
        "path.copy_path",
        "terminal.open",
        "file.reveal_in_finder",
        "palette.open",
        "filter.focus",
        "edit.undo",
        "edit.redo",
        "app.settings",
    ] {
        assert!(CommandId::new(id).info().is_some(), "{id} missing");
    }
    assert!(registry::all().len() > 100);
}

#[test]
fn delete_is_never_a_vim_key() {
    // `dd` marks for cut; nothing in Vim deletes except the gated Trash command with a confirmation.
    for info in registry::all() {
        for b in &info.vim {
            if info.id.as_str() == "file.delete_immediately" {
                panic!("delete_immediately must not have a Vim key");
            }
            if info.id.as_str() == "file.trash" {
                assert_eq!(b.arg, Some(CommandArg::Confirm));
            }
        }
    }
    let cut = CommandId::new("edit.cut");
    assert_eq!(cut.info().unwrap().vim[0].text(), "d d");
}

#[test]
fn ids_serialise_as_plain_strings() {
    let id = CommandId::new("file.new_folder");
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"file.new_folder\"");
    let a = CommandArg::Index(3);
    assert_eq!(
        serde_json::from_str::<CommandArg>(&serde_json::to_string(&a).unwrap()).unwrap(),
        a
    );
}

#[test]
fn settings_have_defaults() {
    let s: KeymapSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(s, KeymapSettings::default());
    assert_eq!(s.sequence_timeout_ms, 1000);
    assert!(!s.typing_filters);
    let o: ShortcutOverlay = serde_json::from_str("{}").unwrap();
    assert_eq!(
        (o.enabled, o.delay_ms, o.corner, o.list_plain_keys),
        (true, 400, Corner::BottomRight, true)
    );
    let h: ShortcutHints = serde_json::from_str("{}").unwrap();
    assert_eq!(
        (
            h.enabled,
            h.frequency,
            h.seconds,
            h.corner,
            h.stop_when_learned
        ),
        (
            true,
            HintFrequency::Sometimes,
            4.0,
            Corner::BottomRight,
            true
        )
    );
}

/// S6 (safety audit): only movement repeats while a key is held.
#[test]
fn only_movement_commands_repeat() {
    let repeat: Vec<&str> = fstudio_keymap::registry::all()
        .iter()
        .filter(|c| c.repeatable)
        .map(|c| c.id.as_str())
        .collect();
    for id in &repeat {
        assert!(
            id.starts_with("cursor.")
                || id.starts_with("select.extend_")
                || id.ends_with(".next")
                || id.ends_with(".prev")
                || id.starts_with("addressbar.complete_"),
            "{id} must not repeat"
        );
    }
    for id in [
        "file.trash",
        "file.delete_immediately",
        "file.move_to_other_pane",
        "file.copy_to_other_pane",
        "file.duplicate",
        "file.new_folder",
        "edit.paste",
        "file.move_here",
        "edit.undo",
        "edit.redo",
        "addressbar.commit",
        "nav.open",
    ] {
        assert!(!repeat.contains(&id), "{id}");
    }
    assert!(repeat.contains(&"cursor.down") && repeat.contains(&"cursor.page_down"));
}
