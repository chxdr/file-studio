//! The effective keymap: profile defaults plus user overrides, the resolver,
//! conflict detection and the held-modifier overlay.

use std::time::{Duration, Instant};

use fstudio_model::{Key, KeyChord, Mods};

use crate::binding::{Binding, KeySpec};
use crate::command::{Category, CommandArg, CommandId};
use crate::display::{key_cap, mods_title, normalize_chord};
use crate::hints::Tracker;
use crate::registry;
use crate::settings::{
    KeymapSettings, Profile, ShortcutHints, ShortcutOverlay, Tier, default_bindings,
};
use crate::system::system_shortcut;
use crate::when::{Context, Focus, VimMode};

/// What a key press means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Run this command.
    Command(CommandId, Option<CommandArg>),
    /// The start of a longer sequence; show this prefix (`g`) and wait.
    Pending(String),
    /// A printable key nobody binds: select by name (type-to-select).
    TypeSelect(char),
    /// A printable key nobody binds, with "typing filters" on: filter the list.
    Filter(char),
    /// Not ours; let the focused control handle it.
    None,
}

/// One binding in force.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: CommandId,
    pub binding: Binding,
    pub tier: Tier,
}

/// Why two bindings are reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two commands answer the same keys in overlapping contexts.
    Duplicate,
    /// The keys are a macOS shortcut (named here); such a binding is ignored.
    SystemShortcut(&'static str),
}

/// A problem in the effective keymap, for the Settings Conflicts tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// Key-cap text of the keys.
    pub keys: String,
    /// The commands involved (two for a duplicate, one for a system shortcut).
    pub commands: Vec<CommandId>,
}

/// One row of the held-modifier overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortcutRow {
    pub id: CommandId,
    /// Key cap text without modifiers (`P`, `↓`, `F5`).
    pub keys: String,
    pub label: String,
    pub category: Category,
}

/// What to show while modifiers are held.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldShortcuts {
    /// Badge text: `⇧ Shift`, `⌘ Command`, `⇧⌘`; `Keys` for none.
    pub title: String,
    /// Rows grouped by category (in category order), then sorted by label.
    pub rows: Vec<ShortcutRow>,
}

struct Pending {
    chords: Vec<KeyChord>,
    since: Instant,
    /// What to run if the timeout passes with nothing more typed.
    fallback: Option<(CommandId, Option<CommandArg>)>,
}

/// The live keymap. Build it from [`KeymapSettings`]; call [`Keymap::resolve`]
/// for every key press.
pub struct Keymap {
    pub(crate) settings: KeymapSettings,
    entries: Vec<Entry>,
    pending: Option<Pending>,
    pub(crate) overlay: ShortcutOverlay,
    pub(crate) hints: ShortcutHints,
    pub(crate) tracker: Tracker,
    pub(crate) db: Option<fstudio_db::Db>,
}

impl Keymap {
    /// Builds the effective keymap.
    pub fn new(settings: KeymapSettings) -> Self {
        let mut k = Self {
            settings,
            entries: vec![],
            pending: None,
            overlay: ShortcutOverlay::default(),
            hints: ShortcutHints::default(),
            tracker: Tracker::default(),
            db: None,
        };
        k.rebuild();
        k
    }

    fn rebuild(&mut self) {
        self.pending = None;
        self.entries = registry::all()
            .iter()
            .flat_map(|info| {
                self.settings
                    .overrides
                    .effective(self.settings.profile, &info.id)
                    .into_iter()
                    .map(|(binding, tier)| Entry {
                        command: info.id.clone(),
                        binding,
                        tier,
                    })
            })
            .collect();
    }

    /// Replaces the settings (profile, overrides, timeout…) and rebuilds.
    pub fn set_settings(&mut self, settings: KeymapSettings) {
        self.settings = settings;
        self.rebuild();
    }

    pub fn settings(&self) -> &KeymapSettings {
        &self.settings
    }

    /// Updates the overlay settings (delay is clamped to 0–2000 ms).
    pub fn set_overlay(&mut self, mut o: ShortcutOverlay) {
        o.delay_ms = o.delay_ms.min(2000);
        self.overlay = o;
    }

    /// Updates the hint settings (duration is clamped to 1.5–15 s).
    pub fn set_hints(&mut self, mut h: ShortcutHints) {
        h.seconds = h.seconds.clamp(1.5, 15.0);
        self.hints = h;
    }

    pub fn overlay_settings(&self) -> ShortcutOverlay {
        self.overlay
    }

    pub fn hint_settings(&self) -> ShortcutHints {
        self.hints
    }

    /// Every binding in force.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The bindings in force for a command; first is primary, second secondary.
    pub fn bindings_for(&self, id: &CommandId) -> Vec<Binding> {
        self.entries
            .iter()
            .filter(|e| &e.command == id)
            .map(|e| e.binding.clone())
            .collect()
    }

    /// Key-cap text of a command's primary binding, for menus and the palette.
    pub fn shortcut_text(&self, id: &CommandId) -> Option<String> {
        self.entries
            .iter()
            .find(|e| &e.command == id)
            .map(|e| e.binding.text())
    }

    /// Has the user changed this command's keys?
    pub fn is_customized(&self, id: &CommandId) -> bool {
        self.settings.overrides.is_customized(id)
    }

    /// The default bindings of a command in the active profile.
    pub fn defaults_for(&self, id: &CommandId) -> Vec<Binding> {
        default_bindings(self.settings.profile, id.as_str())
            .into_iter()
            .map(|(b, _)| b)
            .collect()
    }

    /// The prefix typed so far in an unfinished sequence, if any.
    pub fn pending_text(&self) -> Option<String> {
        self.pending.as_ref().map(|p| chords_text(&p.chords))
    }

    /// Drops an unfinished sequence (Esc, focus change).
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    fn live(&self, ctx: &Context) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| {
            e.binding.when.matches(ctx)
                && !e
                    .binding
                    .first_chord()
                    .is_some_and(|c| system_shortcut(&c).is_some())
        })
    }

    /// The winning entry for exactly this sequence in this context.
    fn winner(&self, seq: &[KeyChord], ctx: &Context) -> Option<&Entry> {
        let mut best: Option<&Entry> = None;
        for e in self.live(ctx) {
            if seq_matches(&e.binding.keys, seq, true) && best.is_none_or(|b| e.tier > b.tier) {
                best = Some(e);
            }
        }
        best
    }

    /// Decides what a key press means. Call for every key press that is not
    /// plain text inside a text field. `now` drives sequence timeouts.
    ///
    /// Unfinished sequences wait for the next key; a longer pause than
    /// `sequence_timeout_ms` forgets them. Call [`Keymap::poll`] from a timer
    /// to run a command whose keys are also the start of a longer one.
    pub fn resolve(&mut self, chord: KeyChord, ctx: &Context, now: Instant) -> Resolution {
        let chord = normalize_chord(chord);
        let timeout = self.settings.sequence_timeout();
        if let Some(p) = &self.pending
            && now.saturating_duration_since(p.since) >= timeout
        {
            self.pending = None;
        }
        let had_pending = self.pending.is_some();
        let mut seq = self.pending.take().map(|p| p.chords).unwrap_or_default();
        seq.push(chord);

        let exact = self.winner(&seq, ctx).cloned();
        let longer_tier = self
            .live(ctx)
            .filter(|e| {
                e.binding.keys.len() > seq.len() && seq_matches(&e.binding.keys, &seq, false)
            })
            .map(|e| e.tier)
            .max();

        match (exact, longer_tier) {
            (Some(e), lt) if lt.is_none_or(|t| t < e.tier) => {
                let arg = e
                    .binding
                    .arg
                    .clone()
                    .or_else(|| letter_arg(&e.binding, &seq));
                Resolution::Command(e.command, arg)
            }
            (e, Some(_)) => {
                let text = chords_text(&seq);
                let fallback = e.map(|e| {
                    let arg = e
                        .binding
                        .arg
                        .clone()
                        .or_else(|| letter_arg(&e.binding, &seq));
                    (e.command, arg)
                });
                self.pending = Some(Pending {
                    chords: seq,
                    since: now,
                    fallback,
                });
                Resolution::Pending(text)
            }
            (_, None) if had_pending => Resolution::None,
            (_, None) => self.passthrough(chord, ctx),
        }
    }

    fn passthrough(&self, chord: KeyChord, ctx: &Context) -> Resolution {
        if ctx.focus != Focus::List || ctx.vim != VimMode::Off {
            return Resolution::None;
        }
        let m = chord.mods;
        if m.cmd || m.ctrl || m.alt {
            return Resolution::None;
        }
        let Key::Char(base) = chord.key else {
            return Resolution::None;
        };
        let ch = if m.shift {
            crate::display::chord_text(&chord)
                .chars()
                .next()
                .unwrap_or(base)
        } else {
            base
        };
        if self.settings.typing_filters {
            Resolution::Filter(ch)
        } else {
            Resolution::TypeSelect(ch)
        }
    }

    /// Call from a timer. Once the sequence timeout has passed with keys still
    /// pending, returns what to do: the command for the keys typed so far (if
    /// they were one), or `Resolution::None` to hide the prefix indicator.
    /// Returns `None` when nothing is pending or time is not up.
    pub fn poll(&mut self, now: Instant) -> Option<Resolution> {
        let timeout = self.settings.sequence_timeout();
        let p = self.pending.as_ref()?;
        if now.saturating_duration_since(p.since) < timeout {
            return None;
        }
        let p = self.pending.take()?;
        Some(match p.fallback {
            Some((id, arg)) => Resolution::Command(id, arg),
            None => Resolution::None,
        })
    }

    /// Time left before the pending sequence is forgotten.
    pub fn pending_time_left(&self, now: Instant) -> Option<Duration> {
        let p = self.pending.as_ref()?;
        Some(
            self.settings
                .sequence_timeout()
                .saturating_sub(now.saturating_duration_since(p.since)),
        )
    }

    /// Everything wrong with the effective keymap: commands sharing keys in
    /// overlapping contexts, and bindings on macOS shortcuts.
    ///
    /// The Vim layer deliberately shadows Standard bindings (`/`, Return, Esc),
    /// so a Vim binding over a Standard one is not a conflict.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut out = Vec::new();
        for e in &self.entries {
            if let Some(c) = e.binding.first_chord()
                && let Some(name) = system_shortcut(&c)
            {
                out.push(Conflict {
                    kind: ConflictKind::SystemShortcut(name),
                    keys: e.binding.text(),
                    commands: vec![e.command.clone()],
                });
            }
        }
        for (i, a) in self.entries.iter().enumerate() {
            for b in &self.entries[i + 1..] {
                if a.command == b.command || !a.binding.collides_with(&b.binding) {
                    continue;
                }
                let layered = matches!(
                    (a.tier, b.tier),
                    (Tier::Vim, Tier::Standard) | (Tier::Standard, Tier::Vim)
                );
                if layered {
                    continue;
                }
                out.push(Conflict {
                    kind: ConflictKind::Duplicate,
                    keys: a.binding.text(),
                    commands: vec![a.command.clone(), b.command.clone()],
                });
            }
        }
        out
    }

    /// The overlay for exactly these held modifiers in this context; `None`
    /// when the overlay is off or nothing applies. Empty `mods` lists plain keys
    /// if the overlay is set to. Only what would really run is listed.
    pub fn held_shortcuts(&self, mods: Mods, ctx: &Context) -> Option<HeldShortcuts> {
        let cfg = self.overlay;
        if !cfg.enabled || (mods.is_empty() && !cfg.list_plain_keys) {
            return None;
        }
        let mut rows: Vec<ShortcutRow> = Vec::new();
        for e in self.live(ctx) {
            let [KeySpec::Chord(c)] = e.binding.keys.as_slice() else {
                continue;
            };
            if c.mods != mods {
                continue;
            }
            let Some(info) = e.command.info() else {
                continue;
            };
            if !info.shows_in_overlay() {
                continue;
            }
            // Skip rows another binding shadows in this context.
            if self.winner(&[*c], ctx).map(|w| &w.command) != Some(&e.command) {
                continue;
            }
            let keys = key_cap(c.key);
            if rows.iter().any(|r| r.id == e.command && r.keys == keys) {
                continue;
            }
            rows.push(ShortcutRow {
                id: e.command.clone(),
                keys,
                label: info.title.to_string(),
                category: info.category,
            });
        }
        if rows.is_empty() {
            return None;
        }
        rows.sort_by(|a, b| (a.category, &a.label, &a.keys).cmp(&(b.category, &b.label, &b.keys)));
        Some(HeldShortcuts {
            title: mods_title(mods),
            rows,
        })
    }

    /// Which profile is active.
    pub fn profile(&self) -> Profile {
        self.settings.profile
    }
}

fn chords_text(chords: &[KeyChord]) -> String {
    chords
        .iter()
        .map(crate::display::chord_text)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Do `keys` match `seq`? With `exact`, the lengths must be equal; otherwise
/// `seq` must be a proper prefix.
fn seq_matches(keys: &[KeySpec], seq: &[KeyChord], exact: bool) -> bool {
    if exact {
        if keys.len() != seq.len() {
            return false;
        }
    } else if keys.len() < seq.len() {
        return false;
    }
    seq.iter().zip(keys).all(|(c, k)| k.matches(c))
}

/// The letter typed for a wildcard step, as an argument.
fn letter_arg(b: &Binding, seq: &[KeyChord]) -> Option<CommandArg> {
    let i = b.keys.iter().position(|k| matches!(k, KeySpec::Letter))?;
    match seq.get(i)?.key {
        Key::Char(c) => Some(CommandArg::Char(c)),
        _ => None,
    }
}
