//! The rename pipeline: steps applied in order to each name. Pure.
//!
//! Steps work on NFC text (so a typed "é" matches a decomposed one on disk);
//! a name the steps leave unchanged keeps its exact bytes (F-017).

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::item::ItemInfo;
use crate::name::{Parts, nfc, split};
use crate::template::{RenderCtx, RenderNotes, Template, counter};

/// Which part of a name a step changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The name without its extension.
    #[default]
    Name,
    /// The extension only.
    Extension,
    /// The whole name, extension included.
    Whole,
}

/// A change of letter case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseKind {
    /// "all lower case"
    #[default]
    Lower,
    /// "ALL UPPER CASE"
    Upper,
    /// "Each Word Capitalised"
    Title,
    /// "Only the first letter"
    Sentence,
}

/// Where inserted text or a number goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "at", content = "index")]
pub enum Position {
    /// Before the text.
    Start,
    /// After the text.
    #[default]
    End,
    /// After this many characters (clamped to the length).
    Index(u32),
}

/// What Trim removes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum TrimMode {
    /// Spaces at both ends, and runs of spaces inside become one.
    #[default]
    Whitespace,
    /// This many characters from the start.
    Start {
        /// How many.
        count: u32,
    },
    /// This many characters from the end.
    End {
        /// How many.
        count: u32,
    },
    /// Any of these characters at either end.
    Characters {
        /// The characters.
        chars: String,
    },
}

/// What happens to the extension.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum ExtChange {
    /// lower case.
    #[default]
    Lower,
    /// UPPER CASE.
    Upper,
    /// Replaced by this one (a leading dot is ignored; empty removes it).
    Set {
        /// The new extension.
        ext: String,
    },
    /// Removed.
    Remove,
}

/// One step of the pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Step {
    /// The name (without extension) becomes the template's text.
    Template {
        /// A token template, e.g. `{exif.date:%Y%m%d}_{n:3}`.
        template: String,
    },
    /// Find and replace, as plain text or a regular expression.
    Replace {
        /// What to find.
        find: String,
        /// The replacement; with `regex`, `$1` and `${name}` insert capture groups.
        replace: String,
        /// `find` is a regular expression.
        regex: bool,
        /// Match regardless of case.
        ignore_case: bool,
        /// Only the first match (otherwise every match).
        first_only: bool,
        /// Where to look.
        target: Target,
    },
    /// Change letter case.
    Case {
        /// Which case.
        case: CaseKind,
        /// Where.
        target: Target,
    },
    /// Add a sequence number.
    Sequence {
        /// The first item's number.
        start: i64,
        /// Added for each next item.
        step: i64,
        /// Minimum digits (zero padded).
        padding: u32,
        /// Where it goes in the name.
        position: Position,
        /// Between the number and the name (at the start or end).
        separator: String,
    },
    /// Remove characters.
    Trim {
        /// What to remove.
        mode: TrimMode,
        /// Where.
        target: Target,
    },
    /// Insert text (it may contain tokens).
    Insert {
        /// The text, a template.
        text: String,
        /// Where it goes.
        position: Position,
        /// In which part.
        target: Target,
    },
    /// Change the extension.
    Extension {
        /// How.
        change: ExtChange,
    },
}

/// A step and whether it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStep {
    /// Off steps are skipped.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The step.
    #[serde(flatten)]
    pub step: Step,
}

fn yes() -> bool {
    true
}

/// The steps, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Pipeline {
    /// Applied first to last.
    pub steps: Vec<PipelineStep>,
}

impl Pipeline {
    /// A pipeline of these steps, all on.
    pub fn of(steps: Vec<Step>) -> Pipeline {
        Pipeline {
            steps: steps
                .into_iter()
                .map(|step| PipelineStep {
                    enabled: true,
                    step,
                })
                .collect(),
        }
    }
}

/// Why a pipeline can't run: which step, and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("step {}: {message}", .step + 1)]
pub struct PipelineError {
    /// The step's index, from 0.
    pub step: usize,
    /// What is wrong, in words.
    pub message: String,
}

enum Ready {
    Template(Template),
    Replace {
        re: Regex,
        replace: String,
        literal: bool,
        first_only: bool,
        target: Target,
    },
    Case(CaseKind, Target),
    Sequence {
        start: i64,
        step: i64,
        padding: u32,
        position: Position,
        separator: String,
    },
    Trim(TrimMode, Target),
    Insert(Template, Position, Target),
    Ext(ExtChange),
}

/// A pipeline with its regexes and templates compiled, ready for many items.
pub struct Compiled {
    steps: Vec<Ready>,
    uses_exif: bool,
}

/// The result of renaming one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The new name. Exactly the old name when nothing changed.
    pub name: String,
    /// The steps left the name as it was.
    pub unchanged: bool,
    /// Warnings raised by tokens.
    pub notes: RenderNotes,
}

impl Compiled {
    /// Compiles a pipeline. Off steps are ignored.
    pub fn new(p: &Pipeline) -> Result<Compiled, PipelineError> {
        let mut steps = Vec::new();
        let mut uses_exif = false;
        for (i, ps) in p.steps.iter().enumerate() {
            if !ps.enabled {
                continue;
            }
            let err = |message: String| PipelineError { step: i, message };
            let tpl = |s: &str| Template::parse(s).map_err(err);
            let r = match &ps.step {
                Step::Template { template } => {
                    let t = tpl(template)?;
                    uses_exif |= t.uses_exif();
                    Ready::Template(t)
                }
                Step::Replace {
                    find,
                    replace,
                    regex,
                    ignore_case,
                    first_only,
                    target,
                } => {
                    if find.is_empty() {
                        continue;
                    }
                    let pat = if *regex {
                        find.clone()
                    } else {
                        regex::escape(find)
                    };
                    let re = RegexBuilder::new(&pat)
                        .case_insensitive(*ignore_case)
                        .size_limit(1 << 20)
                        .build()
                        .map_err(|e| err(regex_message(&e)))?;
                    Ready::Replace {
                        re,
                        replace: nfc(replace),
                        literal: !*regex,
                        first_only: *first_only,
                        target: *target,
                    }
                }
                Step::Case { case, target } => Ready::Case(*case, *target),
                Step::Sequence {
                    start,
                    step,
                    padding,
                    position,
                    separator,
                } => {
                    if *padding > 12 {
                        return Err(err("padding must be 0 to 12".into()));
                    }
                    Ready::Sequence {
                        start: *start,
                        step: *step,
                        padding: *padding,
                        position: *position,
                        separator: nfc(separator),
                    }
                }
                Step::Trim { mode, target } => Ready::Trim(mode.clone(), *target),
                Step::Insert {
                    text,
                    position,
                    target,
                } => {
                    let t = tpl(&nfc(text))?;
                    uses_exif |= t.uses_exif();
                    Ready::Insert(t, *position, *target)
                }
                Step::Extension { change } => Ready::Ext(change.clone()),
            };
            steps.push(r);
        }
        Ok(Compiled { steps, uses_exif })
    }

    /// Whether any step needs EXIF (so the caller should read it).
    pub fn uses_exif(&self) -> bool {
        self.uses_exif
    }

    /// Renames one item. `index` is its position in the batch, from 0.
    pub fn apply(&self, item: &ItemInfo, index: usize) -> Applied {
        let original = nfc(&item.name);
        let orig = split(&original, item.is_dir);
        let mut w = orig.clone();
        let mut notes = RenderNotes::default();
        for s in &self.steps {
            match s {
                Ready::Template(t) => {
                    let cx = RenderCtx {
                        item,
                        orig: &orig,
                        stem: &w.stem,
                        index,
                    };
                    w.stem = t.render(&cx, &mut notes);
                }
                Ready::Replace {
                    re,
                    replace,
                    literal,
                    first_only,
                    target,
                } => edit(&mut w, *target, item.is_dir, |s| {
                    let limit = if *first_only { 1 } else { 0 };
                    if *literal {
                        re.replacen(s, limit, regex::NoExpand(replace)).into_owned()
                    } else {
                        re.replacen(s, limit, replace.as_str()).into_owned()
                    }
                }),
                Ready::Case(c, target) => {
                    edit(&mut w, *target, item.is_dir, |s| change_case(s, *c))
                }
                Ready::Sequence {
                    start,
                    step,
                    padding,
                    position,
                    separator,
                } => {
                    let n = start.saturating_add(step.saturating_mul(index as i64));
                    let num = counter(n, *padding);
                    w.stem = match position {
                        Position::Start => format!("{num}{separator}{}", w.stem),
                        Position::End => format!("{}{separator}{num}", w.stem),
                        Position::Index(i) => insert_at(&w.stem, *i as usize, &num),
                    };
                }
                Ready::Trim(mode, target) => edit(&mut w, *target, item.is_dir, |s| trim(s, mode)),
                Ready::Insert(t, position, target) => {
                    let cx = RenderCtx {
                        item,
                        orig: &orig,
                        stem: &w.stem,
                        index,
                    };
                    let text = t.render(&cx, &mut notes);
                    edit(&mut w, *target, item.is_dir, |s| match position {
                        Position::Start => format!("{text}{s}"),
                        Position::End => format!("{s}{text}"),
                        Position::Index(i) => insert_at(s, *i as usize, &text),
                    });
                }
                Ready::Ext(change) => {
                    if !item.is_dir {
                        w.ext = match change {
                            ExtChange::Lower => w.ext.to_lowercase(),
                            ExtChange::Upper => w.ext.to_uppercase(),
                            ExtChange::Set { ext } => nfc(ext.trim().trim_start_matches('.')),
                            ExtChange::Remove => String::new(),
                        };
                    }
                }
            }
        }
        let new = nfc(&w.join());
        if new == original {
            Applied {
                name: item.name.clone(),
                unchanged: true,
                notes,
            }
        } else {
            Applied {
                name: new,
                unchanged: false,
                notes,
            }
        }
    }
}

fn regex_message(e: &regex::Error) -> String {
    match e {
        regex::Error::Syntax(s) => {
            // The last line of the syntax error says what is wrong.
            let last = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(s);
            format!(
                "the regular expression isn't valid: {}",
                last.trim_start_matches("error: ")
            )
        }
        regex::Error::CompiledTooBig(_) => "the regular expression is too big".into(),
        _ => "the regular expression isn't valid".into(),
    }
}

/// Applies `f` to the part of the name `target` names.
fn edit(w: &mut Parts, target: Target, is_dir: bool, f: impl FnOnce(&str) -> String) {
    match target {
        Target::Name => w.stem = f(&w.stem),
        Target::Extension => {
            if !w.ext.is_empty() {
                w.ext = f(&w.ext);
            }
        }
        Target::Whole => {
            let whole = f(&w.join());
            *w = split(&whole, is_dir);
        }
    }
}

fn insert_at(s: &str, chars: usize, text: &str) -> String {
    let at = s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i);
    format!("{}{text}{}", &s[..at], &s[at..])
}

fn capitalise(word: &str) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) => f
            .to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// Changes letter case. Title case capitalises after spaces, `_`, `-`,
/// `.` and brackets.
pub fn change_case(s: &str, c: CaseKind) -> String {
    match c {
        CaseKind::Lower => s.to_lowercase(),
        CaseKind::Upper => s.to_uppercase(),
        CaseKind::Sentence => capitalise(s),
        CaseKind::Title => {
            let mut out = String::with_capacity(s.len());
            let mut word = String::new();
            for ch in s.chars() {
                if ch.is_whitespace() || "_-.([{".contains(ch) {
                    out.push_str(&capitalise(&word));
                    word.clear();
                    out.push(ch);
                } else {
                    word.push(ch);
                }
            }
            out.push_str(&capitalise(&word));
            out
        }
    }
}

fn trim(s: &str, mode: &TrimMode) -> String {
    match mode {
        TrimMode::Whitespace => s.split_whitespace().collect::<Vec<_>>().join(" "),
        TrimMode::Start { count } => s.chars().skip(*count as usize).collect(),
        TrimMode::End { count } => {
            let n = s.chars().count().saturating_sub(*count as usize);
            s.chars().take(n).collect()
        }
        TrimMode::Characters { chars } => {
            let set: Vec<char> = nfc(chars).chars().collect();
            s.trim_matches(|c| set.contains(&c)).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(steps: Vec<Step>, name: &str) -> String {
        Compiled::new(&Pipeline::of(steps))
            .unwrap()
            .apply(&ItemInfo::named(name), 0)
            .name
    }

    fn replace(find: &str, rep: &str, regex: bool, target: Target) -> Step {
        Step::Replace {
            find: find.into(),
            replace: rep.into(),
            regex,
            ignore_case: false,
            first_only: false,
            target,
        }
    }

    #[test]
    fn template_step_keeps_the_extension() {
        let s = Step::Template {
            template: "Holiday {n:2}".into(),
        };
        assert_eq!(run(vec![s], "IMG_1.JPG"), "Holiday 01.JPG");
    }

    #[test]
    fn replace_plain_and_regex() {
        assert_eq!(
            run(vec![replace("_", " ", false, Target::Name)], "a_b_c.txt"),
            "a b c.txt"
        );
        // Plain text: `$1` and `.` are literal.
        assert_eq!(
            run(vec![replace(".", "$1", false, Target::Whole)], "a.b"),
            "a$1b"
        );
        assert_eq!(
            run(
                vec![replace(
                    r"(\d{4})-(\d{2})-(\d{2})",
                    "$3.$2.$1",
                    true,
                    Target::Name
                )],
                "2024-05-06 notes.md"
            ),
            "06.05.2024 notes.md"
        );
        assert_eq!(
            run(
                vec![replace(r"(?P<w>\w+)", "[${w}]", true, Target::Extension)],
                "x.txt"
            ),
            "x.[txt]"
        );
        let ci = Step::Replace {
            find: "img".into(),
            replace: "photo".into(),
            regex: false,
            ignore_case: true,
            first_only: true,
            target: Target::Name,
        };
        assert_eq!(run(vec![ci], "IMG img.jpg"), "photo img.jpg");
    }

    #[test]
    fn bad_regex_names_the_step() {
        let e = Compiled::new(&Pipeline::of(vec![
            Step::Case {
                case: CaseKind::Lower,
                target: Target::Name,
            },
            replace("(", "", true, Target::Name),
        ]))
        .err()
        .unwrap();
        assert_eq!(e.step, 1);
        assert!(e.message.contains("isn't valid"), "{}", e.message);
    }

    #[test]
    fn cases() {
        assert_eq!(
            change_case("hello wORLD-x", CaseKind::Title),
            "Hello World-X"
        );
        assert_eq!(
            change_case("hello WORLD", CaseKind::Sentence),
            "Hello world"
        );
        assert_eq!(change_case("Straße", CaseKind::Upper), "STRASSE");
        assert_eq!(change_case("ÉTÉ", CaseKind::Lower), "été");
        let s = Step::Case {
            case: CaseKind::Upper,
            target: Target::Extension,
        };
        assert_eq!(run(vec![s], "photo.jpg"), "photo.JPG");
    }

    #[test]
    fn sequences() {
        let p = Compiled::new(&Pipeline::of(vec![Step::Sequence {
            start: 10,
            step: 5,
            padding: 3,
            position: Position::Start,
            separator: " - ".into(),
        }]))
        .unwrap();
        assert_eq!(p.apply(&ItemInfo::named("a.txt"), 0).name, "010 - a.txt");
        assert_eq!(p.apply(&ItemInfo::named("b.txt"), 2).name, "020 - b.txt");
        let at = Step::Sequence {
            start: 1,
            step: 1,
            padding: 2,
            position: Position::Index(3),
            separator: String::new(),
        };
        assert_eq!(run(vec![at], "abcdef.txt"), "abc01def.txt");
    }

    #[test]
    fn trims() {
        let t = |mode| Step::Trim {
            mode,
            target: Target::Name,
        };
        assert_eq!(
            run(vec![t(TrimMode::Whitespace)], "  a   b  .txt"),
            "a b.txt"
        );
        assert_eq!(
            run(vec![t(TrimMode::Start { count: 4 })], "IMG_0001.jpg"),
            "0001.jpg"
        );
        assert_eq!(run(vec![t(TrimMode::End { count: 99 })], "abc.jpg"), ".jpg");
        assert_eq!(
            run(
                vec![t(TrimMode::Characters { chars: "_-".into() })],
                "__a-b--.txt"
            ),
            "a-b.txt"
        );
    }

    #[test]
    fn inserts() {
        let ins = |text: &str, position| Step::Insert {
            text: text.into(),
            position,
            target: Target::Name,
        };
        assert_eq!(run(vec![ins("x_", Position::Start)], "a.txt"), "x_a.txt");
        assert_eq!(run(vec![ins("_{n:2}", Position::End)], "a.txt"), "a_01.txt");
        assert_eq!(run(vec![ins("é", Position::Index(1))], "ab.txt"), "aéb.txt");
        assert_eq!(
            run(vec![ins("-", Position::Index(99))], "ab.txt"),
            "ab-.txt"
        );
    }

    #[test]
    fn extensions() {
        let e = |change| Step::Extension { change };
        assert_eq!(run(vec![e(ExtChange::Lower)], "a.JPG"), "a.jpg");
        assert_eq!(
            run(
                vec![e(ExtChange::Set {
                    ext: ".jpeg".into()
                })],
                "a.jpg"
            ),
            "a.jpeg"
        );
        assert_eq!(run(vec![e(ExtChange::Remove)], "a.jpg"), "a");
        assert_eq!(
            run(vec![e(ExtChange::Set { ext: "md".into() })], "noext"),
            "noext.md"
        );
        // Folders never get or lose an extension.
        let p = Compiled::new(&Pipeline::of(vec![e(ExtChange::Upper)])).unwrap();
        let mut dir = ItemInfo::named("folder.v2");
        dir.is_dir = true;
        assert!(p.apply(&dir, 0).unchanged);
    }

    #[test]
    fn whole_name_targets_resplit() {
        assert_eq!(
            run(
                vec![replace("_v1.tar", ".tar", false, Target::Whole)],
                "x_v1.tar.gz"
            ),
            "x.tar.gz"
        );
    }

    #[test]
    fn steps_run_in_order_and_off_steps_are_skipped() {
        let mut p = Pipeline::of(vec![
            replace("a", "b", false, Target::Name),
            replace("b", "c", false, Target::Name),
        ]);
        let c = Compiled::new(&p).unwrap();
        assert_eq!(c.apply(&ItemInfo::named("a.txt"), 0).name, "c.txt");
        p.steps[1].enabled = false;
        let c = Compiled::new(&p).unwrap();
        assert_eq!(c.apply(&ItemInfo::named("a.txt"), 0).name, "b.txt");
    }

    #[test]
    fn nfd_names_match_typed_text_and_stay_unchanged_when_untouched() {
        let nfd = "Cafe\u{301}.txt";
        // A no-op pipeline leaves the exact bytes.
        let c = Compiled::new(&Pipeline::of(vec![replace(
            "zzz",
            "y",
            false,
            Target::Name,
        )]))
        .unwrap();
        let a = c.apply(&ItemInfo::named(nfd), 0);
        assert!(a.unchanged);
        assert_eq!(a.name, nfd);
        // Typed NFC text finds the decomposed letter.
        assert_eq!(
            run(vec![replace("é", "e", false, Target::Name)], nfd),
            "Cafe.txt"
        );
    }

    #[test]
    fn case_only_changes_are_changes() {
        let a = Compiled::new(&Pipeline::of(vec![Step::Case {
            case: CaseKind::Lower,
            target: Target::Whole,
        }]))
        .unwrap()
        .apply(&ItemInfo::named("File.TXT"), 0);
        assert!(!a.unchanged);
        assert_eq!(a.name, "file.txt");
    }

    #[test]
    fn serde_round_trip() {
        let p = Pipeline::of(vec![
            Step::Template {
                template: "{name}".into(),
            },
            Step::Sequence {
                start: 1,
                step: 1,
                padding: 3,
                position: Position::Index(2),
                separator: "_".into(),
            },
            Step::Trim {
                mode: TrimMode::Start { count: 2 },
                target: Target::Whole,
            },
            Step::Extension {
                change: ExtChange::Set { ext: "x".into() },
            },
        ]);
        let j = serde_json::to_string(&p).unwrap();
        let back: Pipeline = serde_json::from_str(&j).unwrap();
        assert_eq!(back, p);
        // `enabled` defaults to on.
        let one: Pipeline =
            serde_json::from_str(r#"{"steps":[{"type":"case","case":"upper","target":"name"}]}"#)
                .unwrap();
        assert!(one.steps[0].enabled);
    }
}
