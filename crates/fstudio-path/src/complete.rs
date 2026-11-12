//! Completion for the address bar (HANDS-ON §8).
//!
//! Two kinds of input:
//!
//! - **Path-like** (starts with `/`, `~`, `.`, `$`, or contains `/`): list the
//!   sub-folders of the typed parent and rank them exact-prefix, then subsequence,
//!   then frecent folders deeper under that parent.
//! - **Bare word** (`gig`): fuzzy/subsequence over the frecency index, ranked by
//!   match quality × frecency, with the last path segment weighted higher.
//!
//! Everything comes from memory: the [`DirSource`] (cached per folder) and the
//! [`Frecency`] index. Slow folders (network, cloud placeholders) are never listed.
//! Text after the cursor is ignored, and dropped when a candidate is inserted.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::frecency::Frecency;
use crate::keys::{fold, is_subsequence};
use crate::parse::{ParseContext, expand, normalise};
use crate::source::{Child, DirSource};

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// A sub-folder of the typed parent.
    Child,
    /// A frecent folder (bare word, or deeper under the typed parent).
    Frecent,
}

/// What sort of input was completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// Looks like a path.
    PathLike,
    /// A bare word, matched against frecent folders.
    BareWord,
}

/// One suggestion row.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The folder's name (last segment), as spelled on disk.
    pub display_name: String,
    /// The folder's absolute path.
    pub full_path: PathBuf,
    /// The full breadcrumb to show on the row, with the home folder written as `~`.
    pub breadcrumb_text: String,
    /// Child or frecent.
    pub kind: CompletionKind,
    /// Byte ranges of `display_name` to highlight. Empty when the match cannot be
    /// mapped back (names whose case/Unicode folding changes their length).
    pub match_ranges: Vec<Range<usize>>,
    /// Higher is better. Prefix matches score above subsequence matches.
    pub score: f64,
    /// The whole new input text if this row is chosen (ends with `/`).
    pub insert_text: String,
    /// When a frecent folder was last visited (seconds since the epoch), for the
    /// "recent use" hint.
    pub last_visit: Option<u64>,
}

/// What Tab should do first: replace `input[replace_from..cursor]` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabExtension {
    /// Byte offset in the input where the replaced text starts.
    pub replace_from: usize,
    /// The replacement (the on-disk spelling; ends with `/` for a unique folder).
    pub text: String,
    /// The whole input after applying the replacement.
    pub new_input: String,
}

/// The result of [`Completer::complete`].
#[derive(Debug, Clone, PartialEq)]
pub struct Completions {
    /// The rows, best first.
    pub items: Vec<Completion>,
    /// Tab's first action: extend to the common prefix (or add `/` to a unique
    /// folder). `None` if there is nothing to extend.
    pub common_prefix_extension: Option<TabExtension>,
    /// Greyed text to show after the cursor for the best prefix match; accept with →.
    pub ghost_text: String,
    /// Path-like or bare word.
    pub kind: InputKind,
    /// The folder whose children were listed (path-like input).
    pub parent: Option<PathBuf>,
    /// Byte offset where the last path segment starts in the input.
    pub segment_start: usize,
    /// The parent was not listed because it is a network or cloud folder.
    pub skipped_slow: bool,
}

impl Completions {
    fn empty(kind: InputKind) -> Self {
        Self {
            items: Vec::new(),
            common_prefix_extension: None,
            ghost_text: String::new(),
            kind,
            parent: None,
            segment_start: 0,
            skipped_slow: false,
        }
    }
}

/// Tuning knobs.
#[derive(Debug, Clone)]
pub struct CompleterConfig {
    /// Most rows returned.
    pub limit: usize,
    /// Most frecent (deeper) rows added under a path-like parent.
    pub frecent_extra: usize,
    /// List dot-folders even if the typed segment does not start with a dot.
    pub show_hidden: bool,
    /// Most subsequence candidates scored per keystroke. In a folder with more
    /// survivors than this, ranking among them is approximate (the first ones in
    /// name order are scored), which keeps the worst case bounded.
    pub max_scored: usize,
}

impl Default for CompleterConfig {
    fn default() -> Self {
        Self {
            limit: 50,
            frecent_extra: 8,
            show_hidden: false,
            max_scored: 10_000,
        }
    }
}

struct Scratch {
    name: Matcher,
    path: Matcher,
    hay: Vec<char>,
    needle: Vec<char>,
    idx: Vec<u32>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            name: Matcher::new(Config::DEFAULT),
            path: Matcher::new(Config::DEFAULT.match_paths()),
            hay: Vec::new(),
            needle: Vec::new(),
            idx: Vec::new(),
        }
    }

    fn name_score(&mut self, hay: &str, needle: &str) -> Option<u16> {
        let h = Utf32Str::new(hay, &mut self.hay);
        let n = Utf32Str::new(needle, &mut self.needle);
        self.name.fuzzy_match(h, n)
    }

    fn path_score(&mut self, hay: &str, needle: &str) -> Option<u16> {
        let h = Utf32Str::new(hay, &mut self.hay);
        let n = Utf32Str::new(needle, &mut self.needle);
        self.path.fuzzy_match(h, n)
    }

    /// Char indices of the match of `needle` in the folded `key`.
    fn indices(&mut self, key: &str, needle: &str) -> Vec<u32> {
        self.idx.clear();
        let h = Utf32Str::new(key, &mut self.hay);
        let n = Utf32Str::new(needle, &mut self.needle);
        if self.name.fuzzy_indices(h, n, &mut self.idx).is_none() {
            return Vec::new();
        }
        self.idx.clone()
    }
}

/// The completion engine. One per process is enough; it is `Sync`.
pub struct Completer {
    source: Arc<dyn DirSource>,
    frecency: Frecency,
    cfg: CompleterConfig,
    scratch: Mutex<Scratch>,
}

impl Completer {
    /// Creates a completer over a directory source and a frecency index.
    pub fn new(source: Arc<dyn DirSource>, frecency: Frecency, cfg: CompleterConfig) -> Self {
        Self {
            source,
            frecency,
            cfg,
            scratch: Mutex::new(Scratch::new()),
        }
    }

    /// The frecency index this completer reads.
    pub fn frecency(&self) -> &Frecency {
        &self.frecency
    }

    /// The directory source this completer reads.
    pub fn source(&self) -> &Arc<dyn DirSource> {
        &self.source
    }

    /// Completes `input` with the cursor at byte offset `cursor`. Never touches the
    /// disk except to list one (cached, non-slow) folder through the source.
    pub fn complete(&self, input: &str, cursor: usize, ctx: &ParseContext) -> Completions {
        let mut cursor = cursor.min(input.len());
        while !input.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let typed = &input[..cursor];
        let at_end = cursor == input.len();
        if typed.trim().is_empty() {
            return Completions::empty(InputKind::BareWord);
        }
        let path_like = typed.starts_with(['/', '~', '.', '$']) || typed.contains('/');
        let mut guard = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        if path_like {
            self.complete_path(typed, at_end, ctx, &mut guard)
        } else {
            self.complete_word(typed.trim(), ctx, &mut guard)
        }
    }

    fn complete_word(&self, word: &str, ctx: &ParseContext, sc: &mut Scratch) -> Completions {
        let mut out = Completions::empty(InputKind::BareWord);
        let needle = fold(word);
        let now = self.frecency.now();
        let mut scored: Vec<(f64, usize)> = Vec::new();
        let snapshot = self.frecency.with_entries(|entries| {
            for (i, e) in entries.iter().enumerate() {
                if !is_subsequence(&e.key_path, &needle) {
                    continue;
                }
                let last = sc
                    .name_score(&e.key_name, &needle)
                    .map(|s| f64::from(s) * 1.5);
                let full = sc.path_score(&e.key_path, &needle).map(f64::from);
                let m = match (last, full) {
                    (Some(a), Some(b)) => a.max(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => continue,
                };
                scored.push((m * (1.0 + (1.0 + e.score(now)).ln()), i));
            }
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            scored.truncate(self.cfg.limit);
            scored
                .iter()
                .map(|&(s, i)| (s, entries[i].clone()))
                .collect::<Vec<_>>()
        });
        for (score, e) in snapshot {
            let path = PathBuf::from(&e.path);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.path.clone());
            let ranges = highlight(sc, &name, &needle);
            out.items.push(Completion {
                breadcrumb_text: home_collapse(&e.path, &ctx.home),
                insert_text: format!("{}/", e.path.trim_end_matches('/')),
                display_name: name,
                full_path: path,
                kind: CompletionKind::Frecent,
                match_ranges: ranges,
                score,
                last_visit: Some(e.last_accessed),
            });
        }
        out
    }

    fn complete_path(
        &self,
        typed: &str,
        at_end: bool,
        ctx: &ParseContext,
        sc: &mut Scratch,
    ) -> Completions {
        let mut out = Completions::empty(InputKind::PathLike);
        let (dir_text, seg) = match typed.rfind('/') {
            Some(i) => (&typed[..=i], &typed[i + 1..]),
            None => ("", typed),
        };
        out.segment_start = typed.len() - seg.len();

        // A bare `~` or `..` has no folder to list; Tab just adds the slash.
        if dir_text.is_empty() && (seg == "~" || seg == "..") {
            out.common_prefix_extension = Some(TabExtension {
                replace_from: out.segment_start,
                text: format!("{seg}/"),
                new_input: format!("{typed}/"),
            });
            return out;
        }
        if dir_text.is_empty() && seg.starts_with(['~', '$']) {
            return out;
        }

        let dir = resolve_dir(dir_text, ctx);
        out.parent = Some(dir.clone());
        let needle = fold(seg);
        let show_hidden = self.cfg.show_hidden || seg.starts_with('.');
        let limit = self.cfg.limit.max(1);
        let dir_key = {
            let mut k = fold(&dir.to_string_lossy());
            if !k.ends_with('/') {
                k.push('/');
            }
            k
        };

        // Frecent folders under this parent: direct children (boost) and deeper ones.
        let now = self.frecency.now();
        let mut child_boost: std::collections::HashMap<String, f64> = Default::default();
        let mut deeper: Vec<(f64, crate::frecency::FrecencyEntry)> = Vec::new();
        self.frecency.with_entries(|entries| {
            for e in entries {
                let Some(rest) = e.key_path.strip_prefix(&dir_key) else {
                    continue;
                };
                if rest.is_empty() {
                    continue;
                }
                if !rest.contains('/') {
                    child_boost.insert(rest.to_owned(), e.score(now));
                } else if needle.is_empty() || is_subsequence(rest, &needle) {
                    let ms = if needle.is_empty() {
                        Some(1.0)
                    } else {
                        sc.name_score(&e.key_name, &needle)
                            .map(|s| f64::from(s) * 1.5)
                            .or_else(|| sc.path_score(rest, &needle).map(f64::from))
                    };
                    if let Some(m) = ms {
                        deeper.push((m * (1.0 + (1.0 + e.score(now)).ln()), e.clone()));
                    }
                }
            }
        });
        deeper.sort_by(|a, b| b.0.total_cmp(&a.0));
        deeper.truncate(self.cfg.frecent_extra);

        // Children of the parent, from the cache. Never list slow folders.
        let mut rows: Vec<(f64, Child, bool)> = Vec::new(); // (score, child, is_prefix)
        let mut tab: Option<(Child, Child)> = None; // first / last visible prefix match
        let mut prefix_count = 0usize;
        if self.source.may_be_slow(&dir) {
            out.skipped_slow = true;
        } else if let Ok(listing) = self.source.list_dirs(&dir) {
            let ch = &listing.children;
            let visible = |c: &Child| (show_hidden || !c.hidden) && !c.is_package;
            let lo = ch.partition_point(|c| c.key.as_str() < needle.as_str());
            let hi = lo + ch[lo..].partition_point(|c| c.key.starts_with(&needle));
            let range = &ch[lo..hi];

            // Tab data: first and last visible name in the sorted prefix range.
            let first = range.iter().find(|c| visible(c));
            let last = range.iter().rev().find(|c| visible(c));
            if let (Some(f), Some(l)) = (first, last) {
                tab = Some((f.clone(), l.clone()));
            }

            // Tier 1: prefix matches. Frecent ones first, then name order.
            let mut chosen: Vec<&Child> = Vec::new();
            let mut frecent_hits: Vec<(&Child, f64)> = range
                .iter()
                .filter(|c| visible(c))
                .filter_map(|c| child_boost.get(&c.key).map(|s| (c, *s)))
                .collect();
            if !child_boost.is_empty() && range.len() > 4096 {
                // Huge range: look the frecent names up directly instead of scanning.
                frecent_hits = child_boost
                    .iter()
                    .filter(|(k, _)| k.starts_with(&needle))
                    .filter_map(|(k, s)| {
                        let p = ch.partition_point(|c| c.key.as_str() < k.as_str());
                        ch.get(p)
                            .filter(|c| &c.key == k && visible(c))
                            .map(|c| (c, *s))
                    })
                    .collect();
            }
            frecent_hits.sort_by(|a, b| b.1.total_cmp(&a.1));
            let extra_slots = deeper.len();
            let want = limit.saturating_sub(extra_slots).max(1);
            for (c, _) in frecent_hits.iter().take(want) {
                chosen.push(c);
            }
            for c in range.iter().filter(|c| visible(c)) {
                if chosen.len() >= want {
                    break;
                }
                if !frecent_hits.iter().any(|(f, _)| std::ptr::eq(*f, c)) {
                    chosen.push(c);
                }
            }
            prefix_count = chosen.len();
            for c in chosen {
                let boost = child_boost.get(&c.key).copied().unwrap_or(0.0);
                let exact = if c.key == needle { 500.0 } else { 0.0 };
                let score = 10_000.0 + exact + (1.0 + boost).ln() * 100.0
                    - (c.key.chars().count() as f64) * 0.01;
                rows.push((score, c.clone(), true));
            }

            // Tier 2: subsequence matches (only if there is room).
            if !needle.is_empty() && rows.len() < want {
                let mut sub: Vec<(f64, &Child)> = Vec::new();
                for (i, c) in ch.iter().enumerate() {
                    if i >= lo && i < hi {
                        continue;
                    }
                    if !visible(c) || !is_subsequence(&c.key, &needle) {
                        continue;
                    }
                    if sub.len() >= self.cfg.max_scored {
                        break;
                    }
                    if let Some(s) = sc.name_score(&c.key, &needle) {
                        let boost = child_boost.get(&c.key).copied().unwrap_or(0.0);
                        sub.push((f64::from(s) * (1.0 + (1.0 + boost).ln()), c));
                    }
                }
                sub.sort_by(|a, b| b.0.total_cmp(&a.0));
                for (s, c) in sub.into_iter().take(want - rows.len()) {
                    rows.push((s.min(9_999.0), c.clone(), false));
                }
            }
        }

        let prefix_len = out.segment_start;
        let prefix_text = &typed[..prefix_len];
        for (score, c, _is_prefix) in rows {
            let full = dir.join(&c.name);
            let ranges = highlight(sc, &c.name, &needle);
            out.items.push(Completion {
                breadcrumb_text: home_collapse(&full.to_string_lossy(), &ctx.home),
                insert_text: format!("{prefix_text}{}/", c.name),
                display_name: c.name,
                full_path: full,
                kind: CompletionKind::Child,
                match_ranges: ranges,
                score,
                last_visit: None,
            });
        }
        for (score, e) in deeper {
            let path = PathBuf::from(&e.path);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.path.clone());
            let ranges = highlight(sc, &name, &needle);
            out.items.push(Completion {
                breadcrumb_text: home_collapse(&e.path, &ctx.home),
                insert_text: format!("{}/", e.path.trim_end_matches('/')),
                display_name: name,
                full_path: path,
                kind: CompletionKind::Frecent,
                match_ranges: ranges,
                score: score.min(9_999.0),
                last_visit: Some(e.last_accessed),
            });
        }

        // Tab extension from the sorted prefix range.
        if let Some((first, last)) = tab {
            let text = if first.name == last.name {
                Some(format!("{}/", first.name))
            } else {
                let l = common_prefix_chars(&first.key, &last.key);
                if l > needle.chars().count() {
                    raw_prefix_for_folded(&first.name, l)
                } else {
                    None
                }
            };
            if let Some(text) = text {
                out.common_prefix_extension = Some(TabExtension {
                    replace_from: out.segment_start,
                    new_input: format!("{prefix_text}{text}"),
                    text,
                });
            }
        }

        // Ghost text: the rest of the best prefix match, only when typing at the end.
        if at_end
            && !seg.is_empty()
            && prefix_count > 0
            && let Some(best) = out.items.first()
            && best.kind == CompletionKind::Child
            && let Some(n) = raw_prefix_for_folded(&best.display_name, needle.chars().count())
            && fold(&n) == needle
        {
            out.ghost_text = format!("{}/", &best.display_name[n.len()..]);
        }
        out
    }
}

/// Resolves the typed folder part (`~/Pic`, `../x/`, `/a/`, `` ) to an absolute path.
fn resolve_dir(dir_text: &str, ctx: &ParseContext) -> PathBuf {
    if dir_text.is_empty() {
        return ctx.cwd.clone();
    }
    let (t, _, _) = expand(dir_text, ctx);
    let abs = if t.starts_with('/') {
        t
    } else {
        format!("{}/{}", ctx.cwd.to_string_lossy().trim_end_matches('/'), t)
    };
    PathBuf::from(normalise(&abs))
}

/// Writes the home folder as `~` at the start of a path.
fn home_collapse(path: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    let h = h.trim_end_matches('/');
    if h.is_empty() {
        return path.to_owned();
    }
    if path == h {
        return "~".to_owned();
    }
    match path.strip_prefix(h) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

fn common_prefix_chars(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// The shortest prefix of `name` (as spelled) whose folded form has `l` chars.
fn raw_prefix_for_folded(name: &str, l: usize) -> Option<String> {
    if name.is_ascii() {
        return name.get(..l).map(str::to_owned);
    }
    for (b, c) in name.char_indices() {
        let end = b + c.len_utf8();
        let n = fold(&name[..end]).chars().count();
        if n == l {
            return Some(name[..end].to_owned());
        }
        if n > l {
            return None;
        }
    }
    None
}

/// Byte ranges of `name` that matched `needle` (folded), merged into runs. Empty if
/// folding changes the name's length, since indices would not line up.
fn highlight(sc: &mut Scratch, name: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let key = fold(name);
    if key.chars().count() != name.chars().count() {
        return Vec::new();
    }
    let idx = sc.indices(&key, needle);
    if idx.is_empty() {
        return Vec::new();
    }
    let offs: Vec<(usize, usize)> = name
        .char_indices()
        .map(|(b, c)| (b, b + c.len_utf8()))
        .collect();
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut sorted = idx;
    sorted.sort_unstable();
    for i in sorted {
        let Some(&(s, e)) = offs.get(i as usize) else {
            continue;
        };
        match out.last_mut() {
            Some(r) if r.end == s => r.end = e,
            _ => out.push(s..e),
        }
    }
    out
}

/// What pressing Tab (or ⇧Tab) should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabOutcome {
    /// Nothing to complete.
    Nothing,
    /// Extend to the common prefix (or add `/` to a unique folder). Set the field to
    /// `new_input`, keep the list open and recompute completions.
    Extend {
        /// The new field text.
        new_input: String,
    },
    /// Cycle onto a row: set the field to `text` and highlight row `index`. Do not
    /// recompute completions (and do not call [`TabCycle::reset`]) for this change.
    Select {
        /// Index into the rows that were current when cycling started.
        index: usize,
        /// The field text for that row.
        text: String,
    },
}

/// The pure state machine behind Tab. The shell owns one per address bar: call
/// [`TabCycle::press`] on Tab/⇧Tab and [`TabCycle::reset`] on any other edit.
#[derive(Debug, Default, Clone)]
pub struct TabCycle {
    rows: Vec<String>,
    index: Option<usize>,
}

impl TabCycle {
    /// A fresh, idle cycle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget any cycle in progress (the user typed, deleted or moved the cursor).
    pub fn reset(&mut self) {
        self.rows.clear();
        self.index = None;
    }

    /// True while Tab is cycling through rows.
    pub fn is_cycling(&self) -> bool {
        self.index.is_some()
    }

    /// Handles Tab (`backwards == false`) or ⇧Tab. The first Tab extends the common
    /// prefix if there is one; otherwise (and on later presses) it selects row 1,
    /// then cycles down. ⇧Tab cycles up and wraps.
    pub fn press(&mut self, completions: &Completions, backwards: bool) -> TabOutcome {
        let rows: Vec<String> = completions
            .items
            .iter()
            .map(|i| i.insert_text.clone())
            .collect();
        let ext = completions
            .common_prefix_extension
            .as_ref()
            .map(|e| e.new_input.as_str());
        self.press_rows(&rows, ext, backwards)
    }

    /// [`TabCycle::press`] for a shell that holds the completions in its own
    /// form: `rows` are the rows' `insert_text`, `extension` the common-prefix
    /// extension's `new_input`.
    pub fn press_rows(
        &mut self,
        rows: &[String],
        extension: Option<&str>,
        backwards: bool,
    ) -> TabOutcome {
        if let Some(cur) = self.index {
            let n = self.rows.len();
            if n == 0 {
                return TabOutcome::Nothing;
            }
            let next = if backwards {
                (cur + n - 1) % n
            } else {
                (cur + 1) % n
            };
            self.index = Some(next);
            return TabOutcome::Select {
                index: next,
                text: self.rows[next].clone(),
            };
        }
        if !backwards && let Some(ext) = extension {
            return TabOutcome::Extend {
                new_input: ext.to_owned(),
            };
        }
        if rows.is_empty() {
            return TabOutcome::Nothing;
        }
        self.rows = rows.to_vec();
        let first = if backwards { self.rows.len() - 1 } else { 0 };
        self.index = Some(first);
        TabOutcome::Select {
            index: first,
            text: self.rows[first].clone(),
        }
    }
}

/// ⌥⌫ in the address bar: deletes back one path component before `cursor`
/// (a UTF-8 byte offset), treating `/` as the boundary. A trailing `/` goes
/// with the component before it, so `/a/b/` and `/a/b` both become `/a/`.
/// Returns the new text and cursor. A leading `/` is never deleted unless it
/// is all that is left.
pub fn delete_path_component(input: &str, cursor: usize) -> (String, usize) {
    let mut cursor = cursor.min(input.len());
    while !input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &input[..cursor];
    let trimmed = before.strip_suffix('/').unwrap_or(before);
    let start = if trimmed.is_empty() {
        0
    } else {
        trimmed.rfind('/').map(|i| i + 1).unwrap_or(0)
    };
    let mut out = String::with_capacity(input.len());
    out.push_str(&input[..start]);
    out.push_str(&input[cursor..]);
    (out, start)
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn delete_component_goes_back_to_the_previous_slash() {
        assert_eq!(
            delete_path_component("/Users/me/Pic", 13),
            ("/Users/me/".into(), 10)
        );
        assert_eq!(
            delete_path_component("/Users/me/", 10),
            ("/Users/".into(), 7)
        );
        assert_eq!(delete_path_component("~/Docs", 6), ("~/".into(), 2));
        assert_eq!(delete_path_component("gig", 3), (String::new(), 0));
        assert_eq!(delete_path_component("/", 1), (String::new(), 0));
        assert_eq!(delete_path_component("/a", 2), ("/".into(), 1));
        // Text after the cursor stays.
        assert_eq!(delete_path_component("/a/bc/d", 5), ("/a//d".into(), 3));
        // Unicode: never splits a character.
        assert_eq!(delete_path_component("/Café/x", 7), ("/x".into(), 1));
        assert_eq!(delete_path_component("/Café/", 7), ("/".into(), 1));
    }

    #[test]
    fn tab_cycle_from_rows() {
        let rows = vec!["/a/x/".to_string(), "/a/y/".to_string()];
        let mut t = TabCycle::new();
        assert_eq!(
            t.press_rows(&rows, Some("/a/"), false),
            TabOutcome::Extend {
                new_input: "/a/".into()
            }
        );
        assert_eq!(
            t.press_rows(&rows, None, false),
            TabOutcome::Select {
                index: 0,
                text: "/a/x/".into()
            }
        );
        assert_eq!(
            t.press_rows(&rows, None, false),
            TabOutcome::Select {
                index: 1,
                text: "/a/y/".into()
            }
        );
        assert_eq!(
            t.press_rows(&rows, None, true),
            TabOutcome::Select {
                index: 0,
                text: "/a/x/".into()
            }
        );
        t.reset();
        assert!(!t.is_cycling());
        assert_eq!(t.press_rows(&[], None, false), TabOutcome::Nothing);
    }
}
