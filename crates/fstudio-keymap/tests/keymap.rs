use std::time::{Duration, Instant};

use fstudio_db::Db;
use fstudio_keymap::*;

fn id(s: &str) -> CommandId {
    CommandId::new(s)
}

fn chord(s: &str) -> KeyChord {
    display::parse_chord(s).unwrap_or_else(|| panic!("chord {s}"))
}

fn std_map() -> Keymap {
    Keymap::new(KeymapSettings::default())
}

fn vim_map() -> Keymap {
    Keymap::new(KeymapSettings {
        profile: Profile::Vim,
        ..Default::default()
    })
}

fn dual() -> Context {
    Context {
        dual_pane: true,
        ..Context::list()
    }
}

fn vim_ctx() -> Context {
    Context {
        vim: VimMode::Normal,
        ..Context::list()
    }
}

fn run(k: &mut Keymap, key: &str, ctx: &Context, t: Instant) -> Resolution {
    k.resolve(chord(key), ctx, t)
}

fn cmd(r: Resolution) -> String {
    match r {
        Resolution::Command(c, _) => c.to_string(),
        other => format!("{other:?}"),
    }
}

/// `docs/HANDS-ON.md` §9, row by row: (command, keys that must all trigger it, context).
#[test]
fn standard_table_section_9() {
    let rows: &[(&str, &[&str])] = &[
        ("nav.open", &["⌘↓", "⌘O"]),
        ("nav.up", &["⌘↑"]),
        ("nav.back", &["⌘["]),
        ("nav.forward", &["⌘]"]),
        ("file.rename", &["↩"]),
        ("file.new_folder", &["⇧⌘N", "F7"]),
        ("file.new_file", &["⌥⇧⌘N"]),
        ("file.trash", &["⌘⌫", "F8"]),
        ("file.delete_immediately", &["⌥⌘⌫"]),
        ("file.duplicate", &["⌘D"]),
        ("preview.get_info", &["⌘I"]),
        ("preview.quick_look", &["Space", "⌘Y"]),
        ("edit.copy", &["⌘C"]),
        ("edit.cut", &["⌘X"]),
        ("edit.paste", &["⌘V"]),
        ("file.move_here", &["⌥⌘V"]),
        ("file.copy_to_other_pane", &["F5"]),
        ("file.move_to_other_pane", &["F6"]),
        ("file.rename_dialog", &["⇧F6"]),
        ("pane.switch", &["⇥", "⌥⌘←", "⌥⌘→"]),
        ("pane.toggle_dual", &["⌘\\"]),
        ("path.copy_path", &["⌥⌘C"]),
        ("path.copy_name", &["⇧⌘C"]),
        ("path.copy_folder_path", &["⌥⇧⌘C"]),
        ("tab.new", &["⌘T"]),
        ("tab.close", &["⌘W"]),
        ("window.new", &["⌘N"]),
        ("tab.next", &["⌃⇥"]),
        ("tab.previous", &["⌃⇧⇥"]),
        ("view.show_hidden", &["⇧⌘."]),
        ("nav.address_bar", &["⌘L", "⇧⌘G", "/"]),
        ("nav.other_address_bar", &["⇧⌘L"]),
        ("palette.open", &["⇧⌘P"]),
        ("nav.quick_open", &["⌘P"]),
        ("filter.focus", &["⌘F"]),
        ("search.global", &["⌥⌘F"]),
        ("select.all", &["⌘A"]),
        ("select.invert", &["⌃⌘A"]),
        ("select.by_pattern", &["⌘="]),
        ("select.deselect_pattern", &["⌘-"]),
        ("view.icons", &["⌘1"]),
        ("view.list", &["⌘2"]),
        ("view.columns", &["⌘3"]),
        ("view.gallery", &["⌘4"]),
        ("terminal.open", &["⇧⌘R"]),
        ("file.open_in_editor", &["⌘E"]),
        ("nav.connect_server", &["⌘K"]),
    ];
    let mut k = std_map();
    let t = Instant::now();
    for (command, keys) in rows {
        let defaults = k.defaults_for(&id(command));
        assert!(!defaults.is_empty(), "{command} has no defaults");
        for key in *keys {
            let have = defaults.iter().any(|b| b.text() == *key);
            assert!(
                have,
                "{command} should have default {key}, has {:?}",
                defaults.iter().map(|b| b.text()).collect::<Vec<_>>()
            );
            let r = run(&mut k, key, &dual(), t);
            match r {
                Resolution::Command(c, _) => assert_eq!(c.as_str(), *command, "{key}"),
                other => panic!("{key} → {other:?}, wanted {command}"),
            }
        }
        // Primary is the first key listed in the table.
        assert_eq!(
            k.shortcut_text(&id(command)).as_deref(),
            Some(keys[0]),
            "{command} primary"
        );
    }
    // Finder first: Return renames, it does not open.
    assert_eq!(cmd(run(&mut k, "↩", &Context::list(), t)), "file.rename");
    // "/" starts the address bar with the typed character.
    assert_eq!(
        run(&mut k, "/", &Context::list(), t),
        Resolution::Command(id("nav.address_bar"), Some(CommandArg::Char('/')))
    );
}

#[test]
fn contexts_decide_tab() {
    let mut k = std_map();
    let t = Instant::now();
    assert_eq!(cmd(run(&mut k, "⇥", &dual(), t)), "pane.switch");
    let ab = Context {
        focus: Focus::AddressBar,
        ..Context::list()
    };
    assert_eq!(cmd(run(&mut k, "⇥", &ab, t)), "addressbar.complete_next");
    assert_eq!(cmd(run(&mut k, "⇧⇥", &ab, t)), "addressbar.complete_prev");
    // Return renames in the list, commits in the address bar and while renaming.
    assert_eq!(cmd(run(&mut k, "↩", &ab, t)), "addressbar.commit");
    let rn = Context {
        focus: Focus::Rename,
        ..Context::list()
    };
    assert_eq!(cmd(run(&mut k, "↩", &rn, t)), "rename.commit");
    // List-only keys do nothing in a text field, so the field keeps ⌘A / ⌘C / ⌫.
    assert_eq!(run(&mut k, "⌘A", &ab, t), Resolution::None);
    assert_eq!(run(&mut k, "⌘X", &ab, t), Resolution::None);
    assert_eq!(
        run(&mut k, "⌘⌫", &ab, t),
        Resolution::Command(id("addressbar.clear_line"), None)
    );
    // Palette
    let pal = Context {
        focus: Focus::Palette,
        ..Context::list()
    };
    assert_eq!(cmd(run(&mut k, "⎋", &pal, t)), "palette.close");
    // Global commands work from text fields.
    assert_eq!(cmd(run(&mut k, "⇧⌘P", &ab, t)), "palette.open");
}

#[test]
fn type_to_select_and_filter() {
    let mut k = std_map();
    let t = Instant::now();
    assert_eq!(
        run(&mut k, "q", &Context::list(), t),
        Resolution::TypeSelect('q')
    );
    assert_eq!(
        run(&mut k, "Q", &Context::list(), t),
        Resolution::TypeSelect('Q')
    );
    assert_eq!(
        run(&mut k, "5", &Context::list(), t),
        Resolution::TypeSelect('5')
    );
    assert_eq!(run(&mut k, "⌘Q", &Context::list(), t), Resolution::None);
    // Not in text fields.
    let ab = Context {
        focus: Focus::AddressBar,
        ..Context::list()
    };
    assert_eq!(run(&mut k, "q", &ab, t), Resolution::None);
    let mut f = Keymap::new(KeymapSettings {
        typing_filters: true,
        ..Default::default()
    });
    assert_eq!(
        run(&mut f, "q", &Context::list(), t),
        Resolution::Filter('q')
    );
}

#[test]
fn shells_may_send_shifted_glyphs() {
    let mut k = vim_map();
    let t = Instant::now();
    let r = k.resolve(KeyChord::plain(Key::Char('G')), &vim_ctx(), t);
    assert_eq!(cmd(r), "cursor.bottom");
    let r = k.resolve(KeyChord::plain(Key::Char(':')), &vim_ctx(), t);
    assert_eq!(cmd(r), "palette.open");
}

#[test]
fn vim_movement_and_sequences() {
    let mut k = vim_map();
    let c = vim_ctx();
    let t = Instant::now();
    for (key, want) in [
        ("j", "cursor.down"),
        ("k", "cursor.up"),
        ("l", "nav.open"),
        ("↩", "nav.open"),
        ("h", "nav.up"),
        ("G", "cursor.bottom"),
        ("⌃D", "cursor.page_down"),
        ("⌃U", "cursor.page_up"),
        ("/", "filter.focus"),
        (":", "palette.open"),
        ("p", "edit.paste"),
        ("v", "select.visual"),
        ("V", "select.visual_line"),
        ("u", "edit.undo"),
    ] {
        assert_eq!(cmd(run(&mut k, key, &c, t)), want, "{key}");
    }
    // gg
    assert_eq!(run(&mut k, "g", &c, t), Resolution::Pending("g".into()));
    assert_eq!(k.pending_text().as_deref(), Some("g"));
    assert_eq!(
        cmd(run(&mut k, "g", &c, t + Duration::from_millis(300))),
        "cursor.top"
    );
    assert!(k.pending_text().is_none());
    // yy copies, dd marks for cut (never delete), x trashes with confirmation
    assert_eq!(run(&mut k, "y", &c, t), Resolution::Pending("y".into()));
    assert_eq!(cmd(run(&mut k, "y", &c, t)), "edit.copy");
    assert_eq!(run(&mut k, "d", &c, t), Resolution::Pending("d".into()));
    assert_eq!(cmd(run(&mut k, "d", &c, t)), "edit.cut");
    assert_eq!(
        run(&mut k, "x", &c, t),
        Resolution::Command(id("file.trash"), Some(CommandArg::Confirm))
    );
    // A stray key aborts the sequence and is swallowed (no type-to-select in Vim).
    assert_eq!(run(&mut k, "d", &c, t), Resolution::Pending("d".into()));
    assert_eq!(run(&mut k, "j", &c, t), Resolution::None);
    assert!(k.pending_text().is_none());
    // Unbound printable keys are ignored in Vim normal mode.
    assert_eq!(run(&mut k, "q", &c, t), Resolution::None);
    // Navigation sequences
    assert_eq!(run(&mut k, "g", &c, t), Resolution::Pending("g".into()));
    assert_eq!(cmd(run(&mut k, "h", &c, t)), "nav.go_home");
    assert_eq!(run(&mut k, "z", &c, t), Resolution::Pending("z".into()));
    assert_eq!(cmd(run(&mut k, "h", &c, t)), "view.show_hidden");
}

#[test]
fn vim_marks_carry_the_letter() {
    let mut k = vim_map();
    let c = vim_ctx();
    let t = Instant::now();
    assert_eq!(run(&mut k, "m", &c, t), Resolution::Pending("m".into()));
    assert_eq!(
        run(&mut k, "q", &c, t),
        Resolution::Command(id("nav.mark_set"), Some(CommandArg::Char('q')))
    );
    assert_eq!(run(&mut k, "'", &c, t), Resolution::Pending("'".into()));
    assert_eq!(
        run(&mut k, "b", &c, t),
        Resolution::Command(id("nav.mark_jump"), Some(CommandArg::Char('b')))
    );
    // Digits are not marks.
    assert_eq!(run(&mut k, "m", &c, t), Resolution::Pending("m".into()));
    assert_eq!(run(&mut k, "5", &c, t), Resolution::None);
}

#[test]
fn vim_sequence_timeout() {
    let mut k = vim_map();
    let c = vim_ctx();
    let t = Instant::now();
    assert_eq!(run(&mut k, "g", &c, t), Resolution::Pending("g".into()));
    // Just inside the default 1 s: still pending.
    assert_eq!(
        k.pending_time_left(t + Duration::from_millis(900)),
        Some(Duration::from_millis(100))
    );
    assert_eq!(k.poll(t + Duration::from_millis(900)), None);
    // Past it: the prefix is forgotten, so the second g starts over.
    let late = t + Duration::from_millis(1500);
    assert_eq!(run(&mut k, "g", &c, late), Resolution::Pending("g".into()));
    assert_eq!(
        k.poll(late + Duration::from_secs(2)),
        Some(Resolution::None)
    );
    assert!(k.pending_text().is_none());
    // Timeout is configurable.
    let mut k = Keymap::new(KeymapSettings {
        profile: Profile::Vim,
        sequence_timeout_ms: 300,
        ..Default::default()
    });
    assert_eq!(run(&mut k, "g", &c, t), Resolution::Pending("g".into()));
    assert_eq!(
        cmd(run(&mut k, "g", &c, t + Duration::from_millis(400))),
        "Pending(\"g\")"
    );
}

#[test]
fn vim_falls_back_to_standard() {
    let mut k = vim_map();
    let c = vim_ctx();
    let t = Instant::now();
    for (key, want) in [
        ("⌘C", "edit.copy"),
        ("⌘X", "edit.cut"),
        ("⌘↓", "nav.open"),
        ("F5", "Pending"), // F5 needs DualPane: single pane, no command
        ("⇧⌘N", "file.new_folder"),
        ("Space", "preview.quick_look"),
        ("⇧⌘P", "palette.open"),
        ("⌘F", "filter.focus"),
    ] {
        let r = cmd(run(&mut k, key, &c, t));
        if want == "Pending" {
            assert_eq!(r, "None");
        } else {
            assert_eq!(r, want, "{key}");
        }
    }
    // Standard's own Return (rename) still applies where Vim says nothing: visual mode.
    let v = Context {
        vim: VimMode::Visual,
        ..Context::list()
    };
    assert_eq!(cmd(run(&mut k, "↩", &v, t)), "file.rename");
    assert_eq!(cmd(run(&mut k, "j", &v, t)), "select.extend_down");
    assert_eq!(cmd(run(&mut k, "⎋", &v, t)), "select.exit_visual");
    assert_eq!(
        run(&mut k, "x", &v, t),
        Resolution::Command(id("file.trash"), Some(CommandArg::Confirm))
    );
    // Vim over Standard on the same key: "/" is the address bar in Standard, the filter in Vim.
    assert_eq!(
        cmd(run(&mut std_map(), "/", &Context::list(), t)),
        "nav.address_bar"
    );
}

#[test]
fn vim_does_not_depend_on_standard_being_free() {
    // Remove every Standard default; Vim's own layer still resolves.
    let mut s = KeymapSettings {
        profile: Profile::Vim,
        ..Default::default()
    };
    for info in registry::all() {
        for b in &info.standard {
            s.remove_binding(&info.id, b);
        }
    }
    let mut k = Keymap::new(s);
    let t = Instant::now();
    assert_eq!(cmd(run(&mut k, "j", &vim_ctx(), t)), "cursor.down");
    assert_eq!(cmd(run(&mut k, "l", &vim_ctx(), t)), "nav.open");
    assert_eq!(cmd(run(&mut k, "⌘C", &vim_ctx(), t)), "None");
}

#[test]
fn ambiguous_prefix_fires_on_poll() {
    // User binds "g" alone on top of Vim's "g g": the shorter one runs after the timeout.
    let mut s = KeymapSettings {
        profile: Profile::Vim,
        ..Default::default()
    };
    s.add_binding(
        &id("nav.go_home"),
        Binding::parse("g", When::vim_normal()).unwrap(),
    )
    .unwrap();
    s.add_binding(
        &id("nav.go_desktop"),
        Binding::parse("g x", When::vim_normal()).unwrap(),
    )
    .unwrap();
    let mut k = Keymap::new(s);
    let t = Instant::now();
    assert_eq!(
        run(&mut k, "g", &vim_ctx(), t),
        Resolution::Pending("g".into())
    );
    assert_eq!(
        k.poll(t + Duration::from_secs(2)),
        Some(Resolution::Command(id("nav.go_home"), None))
    );
}

#[test]
fn override_diff_roundtrip_and_reset() {
    let mut s = KeymapSettings::default();
    let rename = id("file.rename");
    // Replace the primary key.
    s.set_slot(
        &rename,
        Slot::Primary,
        Binding::parse("F2", When::ListFocused),
    )
    .unwrap();
    let mut k = Keymap::new(s.clone());
    let t = Instant::now();
    assert_eq!(cmd(run(&mut k, "F2", &Context::list(), t)), "file.rename");
    assert_ne!(cmd(run(&mut k, "↩", &Context::list(), t)), "file.rename");
    assert!(k.is_customized(&rename));

    // Secondary key on a command with one default.
    s.set_slot(
        &id("file.duplicate"),
        Slot::Secondary,
        Binding::parse("⌃D", When::ListFocused),
    )
    .unwrap();
    // Remove one of two defaults.
    let trash = id("file.trash");
    let f8 = Binding::parse("F8", When::ListFocused).unwrap();
    s.remove_binding(&trash, &f8);
    // Add.
    s.add_binding(
        &id("nav.up"),
        Binding::parse("⌫", When::ListFocused).unwrap(),
    )
    .unwrap();

    // The diff is small: only what changed.
    assert_eq!(s.overrides.commands.len(), 4);
    let o = &s.overrides.commands[&trash];
    assert_eq!(o.removed, vec![f8.clone()]);
    assert!(o.added.is_empty());

    // JSON round trip of settings, and of an export.
    let j = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<KeymapSettings>(&j).unwrap(), s);
    let imp = KeymapSettings::import_json(&s.export_json()).unwrap();
    assert!(imp.warnings.is_empty());
    assert_eq!(imp.overrides, s.overrides);
    assert_eq!(imp.profile, Profile::Standard);

    let k = Keymap::new(s.clone());
    assert_eq!(k.bindings_for(&trash).len(), 1);
    assert_eq!(
        k.shortcut_text(&id("file.duplicate")).as_deref(),
        Some("⌘D")
    );
    assert_eq!(k.bindings_for(&id("file.duplicate"))[1].text(), "⌃D");

    // Reset one, then all.
    s.reset_command(&rename);
    assert!(!s.overrides.is_customized(&rename));
    assert_eq!(s.overrides.commands.len(), 3);
    s.reset_all();
    assert_eq!(s, KeymapSettings::default());
    // Setting a default back removes the override entirely.
    let mut s = KeymapSettings::default();
    s.set_slot(
        &rename,
        Slot::Primary,
        Binding::parse("F2", When::ListFocused),
    )
    .unwrap();
    s.set_slot(
        &rename,
        Slot::Primary,
        Binding::parse("↩", When::ListFocused),
    )
    .unwrap();
    assert!(s.overrides.is_empty());
    // Clearing a slot.
    s.set_slot(&id("file.new_folder"), Slot::Secondary, None)
        .unwrap();
    assert_eq!(Keymap::new(s).bindings_for(&id("file.new_folder")).len(), 1);
}

#[test]
fn swapping_order_survives_a_round_trip() {
    let mut s = KeymapSettings::default();
    let nf = id("file.new_folder");
    // Make F7 primary, ⇧⌘N secondary.
    s.set_slot(&nf, Slot::Primary, Binding::parse("F7", When::ListFocused))
        .unwrap();
    let k = Keymap::new(s.clone());
    let texts: Vec<_> = k.bindings_for(&nf).iter().map(Binding::text).collect();
    assert_eq!(texts, ["F7", "⇧⌘N"]);
    let again = KeymapSettings::import_json(&s.export_json()).unwrap();
    s.overrides = again.overrides;
    let texts: Vec<_> = Keymap::new(s)
        .bindings_for(&nf)
        .iter()
        .map(Binding::text)
        .collect();
    assert_eq!(texts, ["F7", "⇧⌘N"]);
}

#[test]
fn system_shortcuts_are_refused() {
    let mut s = KeymapSettings::default();
    for bad in ["⌘Q", "⌘H", "⌘M", "⌘`", "⌘⇥", "⌘Space"] {
        let r = s.set_slot(
            &id("file.duplicate"),
            Slot::Primary,
            Binding::parse(bad, When::ListFocused),
        );
        assert!(
            matches!(r, Err(BindError::SystemShortcut(_))),
            "{bad}: {r:?}"
        );
    }
    assert!(s.overrides.is_empty());
    assert_eq!(
        s.add_binding(
            &id("nope.nothing"),
            Binding::parse("F9", When::Always).unwrap()
        ),
        Err(BindError::UnknownCommand("nope.nothing".into()))
    );
    // A hand-edited file that sneaks one in: import drops it with a warning…
    let json = r#"{"version":1,"profile":"Standard","overrides":{
        "file.duplicate":{"added":[{"keys":["⌘Q"],"when":"Always"},{"keys":["F9"],"when":"ListFocused"}]},
        "gone.command":{"added":[{"keys":["F9"],"when":"Always"}]}}}"#;
    let imp = KeymapSettings::import_json(json).unwrap();
    assert_eq!(imp.warnings.len(), 2, "{:?}", imp.warnings);
    assert_eq!(imp.overrides.commands[&id("file.duplicate")].added.len(), 1);
    // …and a settings file read straight from disk is reported by conflicts() and never resolves.
    let raw: KeymapSettings = serde_json::from_str(
        r#"{"overrides":{"file.duplicate":{"added":[{"keys":["⌘Q"],"when":"Always"}]}}}"#,
    )
    .unwrap();
    let mut k = Keymap::new(raw);
    let cs = k.conflicts();
    assert!(
        cs.iter()
            .any(|c| matches!(c.kind, ConflictKind::SystemShortcut("Quit"))),
        "{cs:?}"
    );
    assert_eq!(
        run(&mut k, "⌘Q", &Context::list(), Instant::now()),
        Resolution::None
    );
    assert!(
        KeymapSettings::import_json(r#"{"version":9,"profile":"Vim","overrides":{}}"#).is_err()
    );
}

#[test]
fn defaults_have_no_conflicts() {
    for profile in [Profile::Standard, Profile::Vim] {
        let k = Keymap::new(KeymapSettings {
            profile,
            ..Default::default()
        });
        assert_eq!(k.conflicts(), vec![], "{profile:?}");
    }
}

#[test]
fn conflicts_are_detected_by_context() {
    let mut s = KeymapSettings::default();
    // ⌘D (Duplicate) given to Get Info too: same keys, same context.
    s.add_binding(
        &id("preview.get_info"),
        Binding::parse("⌘D", When::ListFocused).unwrap(),
    )
    .unwrap();
    let k = Keymap::new(s.clone());
    let cs = k.conflicts();
    assert_eq!(cs.len(), 1, "{cs:?}");
    assert_eq!(cs[0].kind, ConflictKind::Duplicate);
    assert_eq!(cs[0].keys, "⌘D");
    assert!(
        cs[0].commands.contains(&id("file.duplicate"))
            && cs[0].commands.contains(&id("preview.get_info"))
    );
    // The user's binding wins while it lasts.
    let mut k = k;
    assert_eq!(
        cmd(run(&mut k, "⌘D", &Context::list(), Instant::now())),
        "preview.get_info"
    );
    // Same keys in disjoint contexts are fine (Tab: list vs address bar is the default).
    let mut s = KeymapSettings::default();
    s.add_binding(
        &id("preview.get_info"),
        Binding::parse("F9", When::ListFocused).unwrap(),
    )
    .unwrap();
    s.add_binding(
        &id("file.duplicate"),
        Binding::parse("F9", When::RenameEditing).unwrap(),
    )
    .unwrap();
    assert!(Keymap::new(s).conflicts().is_empty());
}

fn mouse_cfg(f: HintFrequency) -> ShortcutHints {
    ShortcutHints {
        frequency: f,
        ..Default::default()
    }
}

#[test]
fn hint_frequency_sometimes_with_fake_clock() {
    let mut k = std_map();
    k.set_hints(mouse_cfg(HintFrequency::Sometimes));
    let t0 = Instant::now();
    let nf = id("file.new_folder");
    let h = k.hint_for(&nf, Via::Mouse, t0).unwrap();
    assert_eq!(h.keys, "⇧⌘N");
    assert_eq!(h.label, "New Folder");
    assert_eq!(h.command, nf);
    assert!(
        k.hint_for(&nf, Via::Mouse, t0 + Duration::from_secs(30))
            .is_none()
    );
    let dup = id("file.duplicate");
    assert!(
        k.hint_for(&dup, Via::Mouse, t0 + Duration::from_secs(5))
            .is_none()
    );
    assert!(
        k.hint_for(&dup, Via::Mouse, t0 + Duration::from_secs(25))
            .is_some()
    );
    let mut at = t0;
    for _ in 0..2 {
        at += Duration::from_secs(601);
        assert!(k.hint_for(&nf, Via::Mouse, at).is_some());
    }
    at += Duration::from_secs(601);
    assert!(
        k.hint_for(&nf, Via::Mouse, at).is_none(),
        "three per launch"
    );
}

#[test]
fn hint_frequency_always_and_rarely() {
    let t0 = Instant::now();
    let nf = id("file.new_folder");
    let mut k = std_map();
    k.set_hints(mouse_cfg(HintFrequency::Always));
    assert!(k.hint_for(&nf, Via::Mouse, t0).is_some());
    assert!(
        k.hint_for(&nf, Via::Mouse, t0 + Duration::from_secs(2))
            .is_none(),
        "a few seconds apart"
    );
    for i in 1..20 {
        assert!(
            k.hint_for(&nf, Via::Mouse, t0 + Duration::from_secs(6 * i))
                .is_some()
        );
    }
    let mut k = std_map();
    k.set_hints(mouse_cfg(HintFrequency::Rarely));
    assert!(k.hint_for(&nf, Via::Mouse, t0).is_some());
    assert!(
        k.hint_for(&nf, Via::Mouse, t0 + Duration::from_secs(3600))
            .is_none()
    );
    // A different command within two minutes: no; after: yes.
    let dup = id("file.duplicate");
    assert!(
        k.hint_for(&dup, Via::Mouse, t0 + Duration::from_secs(3600))
            .is_some()
    );
    assert!(
        k.hint_for(&id("edit.copy"), Via::Mouse, t0 + Duration::from_secs(3660))
            .is_none()
    );
    assert!(
        k.hint_for(&id("edit.copy"), Via::Mouse, t0 + Duration::from_secs(3730))
            .is_some()
    );
}

#[test]
fn hints_stop_when_learned_and_respect_settings() {
    let t0 = Instant::now();
    let nf = id("file.new_folder");
    let mut k = std_map();
    k.set_hints(mouse_cfg(HintFrequency::Always));
    for _ in 0..LEARNED_AFTER {
        assert!(!k.is_learned(&nf));
        assert!(k.hint_for(&nf, Via::Keyboard, t0).is_none());
    }
    assert!(k.is_learned(&nf));
    assert!(k.hint_for(&nf, Via::Mouse, t0).is_none());
    k.set_hints(ShortcutHints {
        stop_when_learned: false,
        ..mouse_cfg(HintFrequency::Always)
    });
    assert!(k.hint_for(&nf, Via::Mouse, t0).is_some());
    // Disabled.
    k.set_hints(ShortcutHints {
        enabled: false,
        ..mouse_cfg(HintFrequency::Always)
    });
    assert!(k.hint_for(&id("edit.copy"), Via::Mouse, t0).is_none());
    // Menus and the palette already show the key; scripts never hint.
    k.set_hints(mouse_cfg(HintFrequency::Always));
    assert!(k.hint_for(&id("edit.copy"), Via::Menu, t0).is_none());
    assert!(k.hint_for(&id("edit.copy"), Via::Palette, t0).is_none());
    assert!(k.hint_for(&id("edit.copy"), Via::Script, t0).is_none());
    // No shortcut, no hint; unknown ids and ineligible commands too.
    assert!(k.hint_for(&id("sort.name"), Via::Mouse, t0).is_none());
    assert!(k.hint_for(&id("no.such"), Via::Mouse, t0).is_none());
    assert!(k.hint_for(&id("cursor.down"), Via::Mouse, t0).is_none());
    // Settings are clamped.
    k.set_hints(ShortcutHints {
        seconds: 100.0,
        ..Default::default()
    });
    assert_eq!(k.hint_settings().seconds, 15.0);
    k.set_overlay(ShortcutOverlay {
        delay_ms: 99_999,
        ..Default::default()
    });
    assert_eq!(k.overlay_settings().delay_ms, 2000);
}

#[test]
fn hint_shows_the_users_key_and_vim_key() {
    let mut s = KeymapSettings::default();
    s.set_slot(
        &id("file.duplicate"),
        Slot::Primary,
        Binding::parse("⌃⌘D", When::ListFocused),
    )
    .unwrap();
    let mut k = Keymap::new(s);
    assert_eq!(
        k.hint_for(&id("file.duplicate"), Via::Mouse, Instant::now())
            .unwrap()
            .keys,
        "⌃⌘D"
    );
    let mut k = vim_map();
    assert_eq!(
        k.hint_for(&id("edit.copy"), Via::Mouse, Instant::now())
            .unwrap()
            .keys,
        "y y"
    );
}

#[test]
fn learned_counts_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fs.sqlite3");
    let nf = id("file.new_folder");
    let t0 = Instant::now();
    {
        let db = Db::open(&path).unwrap();
        let mut k = std_map();
        k.attach_db(&db).unwrap();
        k.set_hints(mouse_cfg(HintFrequency::Always));
        for _ in 0..LEARNED_AFTER + 2 {
            k.hint_for(&nf, Via::Keyboard, t0);
        }
        k.hint_for(&id("edit.copy"), Via::Keyboard, t0); // one use only
        assert!(k.is_learned(&nf));
        assert_eq!(db.version("keymap").unwrap(), 1);
        let n: i64 = db
            .with(|c| {
                c.query_row(
                    "SELECT uses FROM keymap_learned WHERE command='file.new_folder'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(n, LEARNED_AFTER as i64, "capped");
    }
    // New launch: learned, but per-launch hint counters start fresh.
    let db = Db::open(&path).unwrap();
    let mut k = std_map();
    k.attach_db(&db).unwrap();
    k.set_hints(mouse_cfg(HintFrequency::Always));
    assert!(k.is_learned(&nf));
    assert!(!k.is_learned(&id("edit.copy")));
    assert!(k.hint_for(&nf, Via::Mouse, t0).is_none());
    assert!(k.hint_for(&id("edit.copy"), Via::Mouse, t0).is_some());
    k.reset_learned().unwrap();
    assert!(
        k.hint_for(&nf, Via::Mouse, t0 + Duration::from_secs(60))
            .is_some()
    );
    let k2 = {
        let mut k2 = std_map();
        k2.attach_db(&db).unwrap();
        k2
    };
    assert!(!k2.is_learned(&nf));
}

#[test]
fn overlay_lists_what_would_run() {
    let k = std_map();
    let ctx = dual();
    let held = k.held_shortcuts(chord("⌘A").mods, &ctx).unwrap();
    assert_eq!(held.title, "⌘ Command");
    assert!(
        held.rows
            .iter()
            .any(|r| r.id == id("edit.copy") && r.keys == "C" && r.category == Category::Edit)
    );
    // Grouped by category.
    let cats: Vec<_> = held.rows.iter().map(|r| r.category).collect();
    let mut sorted = cats.clone();
    sorted.sort();
    assert_eq!(cats, sorted);
    // ⇧⌘ has its own badge.
    let sc = k.held_shortcuts(chord("⇧⌘A").mods, &ctx).unwrap();
    assert_eq!(sc.title, "⇧⌘");
    assert!(
        sc.rows
            .iter()
            .any(|r| r.id == id("file.new_folder") && r.keys == "N")
    );
    // Plain keys list F-keys; text-field keys never show.
    let plain = k.held_shortcuts(Mods::NONE, &ctx).unwrap();
    assert_eq!(plain.title, "Keys");
    assert!(plain.rows.iter().any(|r| r.keys == "F5"));
    assert!(
        !plain
            .rows
            .iter()
            .any(|r| r.category == Category::TextFields)
    );
    // Single pane: F5 (dual pane only) is not listed.
    let single = k.held_shortcuts(Mods::NONE, &Context::list()).unwrap();
    assert!(!single.rows.iter().any(|r| r.keys == "F5"));
    // Off / plain keys off.
    let mut k2 = std_map();
    k2.set_overlay(ShortcutOverlay {
        enabled: false,
        ..Default::default()
    });
    assert!(k2.held_shortcuts(chord("⌘A").mods, &ctx).is_none());
    k2.set_overlay(ShortcutOverlay {
        list_plain_keys: false,
        ..Default::default()
    });
    assert!(k2.held_shortcuts(Mods::NONE, &ctx).is_none());
    assert!(k2.held_shortcuts(chord("⌘A").mods, &ctx).is_some());
    // Vim: shadowed Standard rows disappear, Vim's own show.
    let v = vim_map();
    let plain = v.held_shortcuts(Mods::NONE, &vim_ctx()).unwrap();
    assert!(
        plain
            .rows
            .iter()
            .any(|r| r.id == id("cursor.down") && r.keys == "J")
    );
    assert!(
        plain
            .rows
            .iter()
            .all(|r| !(r.id == id("file.rename") && r.keys == "↩"))
    );
}

#[test]
fn overlay_rows_match_the_resolver() {
    // The drift test: every overlay row, in every profile and context, resolves to its command.
    let t = Instant::now();
    for profile in [Profile::Standard, Profile::Vim] {
        let mut k = Keymap::new(KeymapSettings {
            profile,
            ..Default::default()
        });
        for ctx in Context::all() {
            for bits in 0..16u8 {
                let mods = Mods {
                    ctrl: bits & 1 != 0,
                    alt: bits & 2 != 0,
                    shift: bits & 4 != 0,
                    cmd: bits & 8 != 0,
                };
                let Some(held) = k.held_shortcuts(mods, &ctx) else {
                    continue;
                };
                for row in &held.rows {
                    assert!(registry::get(row.id.as_str()).is_some());
                    // Find the chord for this row among the command's bindings.
                    let b = k
                        .bindings_for(&row.id)
                        .into_iter()
                        .find(|b| {
                            b.when.matches(&ctx)
                                && matches!(b.keys.as_slice(), [KeySpec::Chord(c)] if c.mods == mods && display::key_cap(c.key) == row.keys)
                        })
                        .unwrap_or_else(|| panic!("no binding behind row {row:?}"));
                    let KeySpec::Chord(c) = b.keys[0] else {
                        unreachable!()
                    };
                    k.cancel_pending();
                    match k.resolve(c, &ctx, t) {
                        Resolution::Command(got, _) => assert_eq!(got, row.id, "{ctx:?} {mods:?}"),
                        other => panic!("{ctx:?}: row {row:?} resolved to {other:?}"),
                    }
                }
            }
        }
    }
}
