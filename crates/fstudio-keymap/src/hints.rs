//! "Did you know?" hints and the learned-shortcut counts.
//!
//! A port of Import Studio's `hints.rs` (D-029/D-030): hints appear when a
//! command was done with the mouse and has a keyboard shortcut. How often is
//! the Every time / Sometimes / Rarely setting. Once a shortcut has been used
//! from the keyboard [`LEARNED_AFTER`] times it stops hinting. Learned counts
//! live in the database (`keymap_learned`); everything else is per launch.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use fstudio_db::{Db, DbError, Migrations};
use fstudio_model::Via;

use crate::command::CommandId;
use crate::keymap::Keymap;
use crate::settings::{HintFrequency, ShortcutHints};

/// Keyboard uses after which a shortcut counts as learned.
pub const LEARNED_AFTER: u32 = 3;

/// The module's database schema.
pub const MIGRATIONS: Migrations = Migrations {
    module: "keymap",
    steps: &[(
        1,
        "CREATE TABLE keymap_learned (
             command TEXT PRIMARY KEY,
             uses    INTEGER NOT NULL
         );",
    )],
};

/// A "did you know?" hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub command: CommandId,
    /// The keys to press, with modifiers: `⇧⌘N`, `g g`.
    pub keys: String,
    /// What it does.
    pub label: String,
}

/// Only mouse uses hint: menus and the palette already print the shortcut.
fn hints_for_via(via: Via) -> bool {
    via == Via::Mouse
}

#[derive(Default)]
pub(crate) struct Tracker {
    shown: HashMap<CommandId, (u32, Instant)>,
    keyboard_uses: HashMap<CommandId, u32>,
    last_hint: Option<Instant>,
}

impl Tracker {
    pub(crate) fn learned(&self, id: &CommandId) -> bool {
        self.keyboard_uses.get(id).copied().unwrap_or(0) >= LEARNED_AFTER
    }

    /// Decides whether a mouse use may show a hint now, and books it if so.
    fn allow(&mut self, id: &CommandId, cfg: &ShortcutHints, now: Instant) -> bool {
        if !cfg.enabled || (cfg.stop_when_learned && self.learned(id)) {
            return false;
        }
        let (per_id_gap, max_per_id, global_gap) = match cfg.frequency {
            HintFrequency::Always => (Duration::from_secs(5), u32::MAX, Duration::ZERO),
            HintFrequency::Sometimes => (Duration::from_secs(600), 3, Duration::from_secs(20)),
            HintFrequency::Rarely => (Duration::MAX, 1, Duration::from_secs(120)),
        };
        if let Some(t) = self.last_hint
            && now.saturating_duration_since(t) < global_gap
        {
            return false;
        }
        if let Some((n, t)) = self.shown.get(id)
            && (*n >= max_per_id || now.saturating_duration_since(*t) < per_id_gap)
        {
            return false;
        }
        let e = self.shown.entry(id.clone()).or_insert((0, now));
        e.0 += 1;
        e.1 = now;
        self.last_hint = Some(now);
        true
    }
}

impl Keymap {
    /// Creates the table, then loads the learned counts saved last time and
    /// saves new ones from now on. Call once at start-up.
    pub fn attach_db(&mut self, db: &Db) -> Result<(), DbError> {
        db.migrate(&MIGRATIONS)?;
        let rows: Vec<(String, u32)> = db.with(|c| {
            let mut st = c.prepare("SELECT command, uses FROM keymap_learned")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32)))?
                .collect()
        })?;
        for (id, n) in rows {
            self.tracker
                .keyboard_uses
                .insert(CommandId::new(id), n.min(LEARNED_AFTER));
        }
        self.db = Some(db.clone());
        Ok(())
    }

    /// A command ran. Keyboard uses count toward "learned" (and return `None`).
    /// A mouse use of a command with a shortcut may return a hint, depending on
    /// the hint settings. `now` is passed in so tests can use a fake clock.
    pub fn hint_for(&mut self, command: &CommandId, via: Via, now: Instant) -> Option<Hint> {
        let info = command.info()?;
        if via == Via::Keyboard {
            let before = self
                .tracker
                .keyboard_uses
                .get(command)
                .copied()
                .unwrap_or(0);
            if before < LEARNED_AFTER {
                self.tracker
                    .keyboard_uses
                    .insert(command.clone(), before + 1);
                self.save_learned(command, before + 1);
            }
            return None;
        }
        if !hints_for_via(via) || !info.hint_eligible {
            return None;
        }
        let keys = self.shortcut_text(command)?;
        let cfg = self.hints;
        if !self.tracker.allow(command, &cfg, now) {
            return None;
        }
        Some(Hint {
            command: command.clone(),
            keys,
            label: info.title.to_string(),
        })
    }

    /// Has this shortcut been used from the keyboard enough times?
    pub fn is_learned(&self, command: &CommandId) -> bool {
        self.tracker.learned(command)
    }

    /// Forgets which shortcuts were learned, so their hints come back.
    pub fn reset_learned(&mut self) -> Result<(), DbError> {
        self.tracker.keyboard_uses.clear();
        if let Some(db) = &self.db {
            db.with(|c| c.execute("DELETE FROM keymap_learned", []))?;
        }
        Ok(())
    }

    // At most LEARNED_AFTER writes per command. A failed write only loses a count.
    fn save_learned(&self, command: &CommandId, uses: u32) {
        if let Some(db) = &self.db {
            let _ = db.with(|c| {
                c.execute(
                    "INSERT INTO keymap_learned(command, uses) VALUES (?1, ?2)
                     ON CONFLICT(command) DO UPDATE SET uses = excluded.uses",
                    fstudio_db::rusqlite::params![command.as_str(), uses],
                )
            });
        }
    }
}
