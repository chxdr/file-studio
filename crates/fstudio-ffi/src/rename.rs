//! Batch rename (M3) exports: the session behind the "Batch Rename N
//! Items…" sheet, its rows and summary, the steps, and presets.
//!
//! A step crosses as one flat record ([`FfiRenameStep`]) whose `kind` says
//! which fields count, so the sheet can bind its controls to plain fields
//! and switch a step's kind without losing what was typed.

use std::sync::Arc;

use fstudio_core as core;
use fstudio_core::{RenameSession, TabId};

use crate::{FfiEngine, FfiOutcome, R};

/// Which step a [`FfiRenameStep`] is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRenameStepKind {
    /// The name becomes `template`.
    Template,
    /// Find `find`, put `replace`.
    Replace,
    /// Change case (`case_kind`).
    Case,
    /// Add a counter (`start`, `step`, `padding`, `position`, `separator`).
    Sequence,
    /// Remove characters (`trim_mode`, `count`, `chars`).
    Trim,
    /// Insert `text` at `position`.
    Insert,
    /// Change the extension (`ext_change`, `ext`).
    Extension,
}

/// Which part of the name a step changes.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRenameTarget {
    Name,
    Extension,
    Whole,
}

/// A case change.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCaseKind {
    Lower,
    Upper,
    Title,
    Sentence,
}

/// Where inserted text or a counter goes.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRenamePosition {
    Start,
    End,
    /// After `index` characters.
    Index,
}

/// What Trim removes.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiTrimMode {
    Whitespace,
    /// `count` characters from the start.
    Start,
    /// `count` characters from the end.
    End,
    /// Any of `chars` at either end.
    Characters,
}

/// What happens to the extension.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiExtChange {
    Lower,
    Upper,
    /// Becomes `ext`.
    Set,
    Remove,
}

/// One step, flat. Only the fields its `kind` names are used.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRenameStep {
    pub kind: FfiRenameStepKind,
    pub enabled: bool,
    pub template: String,
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub ignore_case: bool,
    pub first_only: bool,
    pub target: FfiRenameTarget,
    pub case_kind: FfiCaseKind,
    pub start: i64,
    pub step: i64,
    pub padding: u32,
    pub position: FfiRenamePosition,
    pub index: u32,
    pub separator: String,
    pub text: String,
    pub trim_mode: FfiTrimMode,
    pub count: u32,
    pub chars: String,
    pub ext_change: FfiExtChange,
    pub ext: String,
}

/// A new step of `kind` with its usual settings.
#[uniffi::export]
pub fn rename_step_new(kind: FfiRenameStepKind) -> FfiRenameStep {
    FfiRenameStep {
        kind,
        enabled: true,
        template: "{name}".into(),
        find: String::new(),
        replace: String::new(),
        regex: false,
        ignore_case: false,
        first_only: false,
        target: FfiRenameTarget::Name,
        case_kind: FfiCaseKind::Title,
        start: 1,
        step: 1,
        padding: 3,
        position: if kind == FfiRenameStepKind::Sequence {
            FfiRenamePosition::End
        } else {
            FfiRenamePosition::Start
        },
        index: 0,
        separator: " ".into(),
        text: String::new(),
        trim_mode: FfiTrimMode::Whitespace,
        count: 1,
        chars: String::new(),
        ext_change: FfiExtChange::Lower,
        ext: String::new(),
    }
}

/// The template tokens for an "Insert Token" menu: `(token, description)`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRenameToken {
    pub token: String,
    pub title: String,
}

/// Every template token, in menu order.
#[uniffi::export]
pub fn rename_tokens() -> Vec<FfiRenameToken> {
    core::RENAME_TOKENS
        .iter()
        .map(|(t, d)| FfiRenameToken {
            token: (*t).into(),
            title: (*d).into(),
        })
        .collect()
}

fn target_in(t: FfiRenameTarget) -> core::Target {
    match t {
        FfiRenameTarget::Name => core::Target::Name,
        FfiRenameTarget::Extension => core::Target::Extension,
        FfiRenameTarget::Whole => core::Target::Whole,
    }
}

fn target_out(t: core::Target) -> FfiRenameTarget {
    match t {
        core::Target::Name => FfiRenameTarget::Name,
        core::Target::Extension => FfiRenameTarget::Extension,
        core::Target::Whole => FfiRenameTarget::Whole,
    }
}

fn position_in(s: &FfiRenameStep) -> core::Position {
    match s.position {
        FfiRenamePosition::Start => core::Position::Start,
        FfiRenamePosition::End => core::Position::End,
        FfiRenamePosition::Index => core::Position::Index(s.index),
    }
}

impl From<&FfiRenameStep> for core::PipelineStep {
    fn from(s: &FfiRenameStep) -> Self {
        use core::Step as S;
        let step = match s.kind {
            FfiRenameStepKind::Template => S::Template {
                template: s.template.clone(),
            },
            FfiRenameStepKind::Replace => S::Replace {
                find: s.find.clone(),
                replace: s.replace.clone(),
                regex: s.regex,
                ignore_case: s.ignore_case,
                first_only: s.first_only,
                target: target_in(s.target),
            },
            FfiRenameStepKind::Case => S::Case {
                case: match s.case_kind {
                    FfiCaseKind::Lower => core::CaseKind::Lower,
                    FfiCaseKind::Upper => core::CaseKind::Upper,
                    FfiCaseKind::Title => core::CaseKind::Title,
                    FfiCaseKind::Sentence => core::CaseKind::Sentence,
                },
                target: target_in(s.target),
            },
            FfiRenameStepKind::Sequence => S::Sequence {
                start: s.start,
                step: s.step,
                padding: s.padding,
                position: position_in(s),
                separator: s.separator.clone(),
            },
            FfiRenameStepKind::Trim => S::Trim {
                mode: match s.trim_mode {
                    FfiTrimMode::Whitespace => core::TrimMode::Whitespace,
                    FfiTrimMode::Start => core::TrimMode::Start { count: s.count },
                    FfiTrimMode::End => core::TrimMode::End { count: s.count },
                    FfiTrimMode::Characters => core::TrimMode::Characters {
                        chars: s.chars.clone(),
                    },
                },
                target: target_in(s.target),
            },
            FfiRenameStepKind::Insert => S::Insert {
                text: s.text.clone(),
                position: position_in(s),
                target: target_in(s.target),
            },
            FfiRenameStepKind::Extension => S::Extension {
                change: match s.ext_change {
                    FfiExtChange::Lower => core::ExtChange::Lower,
                    FfiExtChange::Upper => core::ExtChange::Upper,
                    FfiExtChange::Set => core::ExtChange::Set { ext: s.ext.clone() },
                    FfiExtChange::Remove => core::ExtChange::Remove,
                },
            },
        };
        core::PipelineStep {
            enabled: s.enabled,
            step,
        }
    }
}

impl From<&core::PipelineStep> for FfiRenameStep {
    fn from(p: &core::PipelineStep) -> Self {
        use core::Step as S;
        let kind = match &p.step {
            S::Template { .. } => FfiRenameStepKind::Template,
            S::Replace { .. } => FfiRenameStepKind::Replace,
            S::Case { .. } => FfiRenameStepKind::Case,
            S::Sequence { .. } => FfiRenameStepKind::Sequence,
            S::Trim { .. } => FfiRenameStepKind::Trim,
            S::Insert { .. } => FfiRenameStepKind::Insert,
            S::Extension { .. } => FfiRenameStepKind::Extension,
        };
        let mut f = rename_step_new(kind);
        f.enabled = p.enabled;
        let set_pos = |f: &mut FfiRenameStep, pos: &core::Position| match pos {
            core::Position::Start => f.position = FfiRenamePosition::Start,
            core::Position::End => f.position = FfiRenamePosition::End,
            core::Position::Index(i) => {
                f.position = FfiRenamePosition::Index;
                f.index = *i;
            }
        };
        match &p.step {
            S::Template { template } => f.template.clone_from(template),
            S::Replace {
                find,
                replace,
                regex,
                ignore_case,
                first_only,
                target,
            } => {
                f.find.clone_from(find);
                f.replace.clone_from(replace);
                f.regex = *regex;
                f.ignore_case = *ignore_case;
                f.first_only = *first_only;
                f.target = target_out(*target);
            }
            S::Case { case, target } => {
                f.case_kind = match case {
                    core::CaseKind::Lower => FfiCaseKind::Lower,
                    core::CaseKind::Upper => FfiCaseKind::Upper,
                    core::CaseKind::Title => FfiCaseKind::Title,
                    core::CaseKind::Sentence => FfiCaseKind::Sentence,
                };
                f.target = target_out(*target);
            }
            S::Sequence {
                start,
                step,
                padding,
                position,
                separator,
            } => {
                f.start = *start;
                f.step = *step;
                f.padding = *padding;
                set_pos(&mut f, position);
                f.separator.clone_from(separator);
            }
            S::Trim { mode, target } => {
                f.target = target_out(*target);
                match mode {
                    core::TrimMode::Whitespace => f.trim_mode = FfiTrimMode::Whitespace,
                    core::TrimMode::Start { count } => {
                        f.trim_mode = FfiTrimMode::Start;
                        f.count = *count;
                    }
                    core::TrimMode::End { count } => {
                        f.trim_mode = FfiTrimMode::End;
                        f.count = *count;
                    }
                    core::TrimMode::Characters { chars } => {
                        f.trim_mode = FfiTrimMode::Characters;
                        f.chars.clone_from(chars);
                    }
                }
            }
            S::Insert {
                text,
                position,
                target,
            } => {
                f.text.clone_from(text);
                set_pos(&mut f, position);
                f.target = target_out(*target);
            }
            S::Extension { change } => match change {
                core::ExtChange::Lower => f.ext_change = FfiExtChange::Lower,
                core::ExtChange::Upper => f.ext_change = FfiExtChange::Upper,
                core::ExtChange::Set { ext } => {
                    f.ext_change = FfiExtChange::Set;
                    f.ext.clone_from(ext);
                }
                core::ExtChange::Remove => f.ext_change = FfiExtChange::Remove,
            },
        }
        f
    }
}

fn pipeline_in(steps: &[FfiRenameStep]) -> core::Pipeline {
    core::Pipeline {
        steps: steps.iter().map(core::PipelineStep::from).collect(),
    }
}

fn steps_out(p: &core::Pipeline) -> Vec<FfiRenameStep> {
    p.steps.iter().map(FfiRenameStep::from).collect()
}

/// What happens to one row.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRenameRowStatus {
    Ready,
    Unchanged,
    Problem,
}

/// One preview row.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRenameRow {
    pub index: u32,
    pub old_name: String,
    pub new_name: String,
    pub status: FfiRenameRowStatus,
    /// The problem, in words, or empty.
    pub message: String,
    /// A warning that doesn't stop the rename, or empty.
    pub warning: String,
    /// The item's folder, when the batch spans folders; else empty.
    pub folder: String,
}

impl From<core::RenameRow> for FfiRenameRow {
    fn from(r: core::RenameRow) -> Self {
        FfiRenameRow {
            index: r.index,
            old_name: r.old_name,
            new_name: r.new_name,
            status: match r.status {
                core::RenameRowStatus::Ready => FfiRenameRowStatus::Ready,
                core::RenameRowStatus::Unchanged => FfiRenameRowStatus::Unchanged,
                core::RenameRowStatus::Problem => FfiRenameRowStatus::Problem,
            },
            message: r.message,
            warning: r.warning,
            folder: r.folder,
        }
    }
}

/// The preview's counts and state.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRenameSummary {
    /// Reload the visible rows when this changes.
    pub version: u64,
    pub total: u32,
    pub ready: u32,
    pub unchanged: u32,
    pub problems: u32,
    pub warnings: u32,
    /// EXIF is being read: poll `summary()` until it isn't.
    pub exif_loading: bool,
    pub exif_done: u32,
    pub exif_total: u32,
    /// Why the steps can't run, if they can't.
    pub error: Option<String>,
    pub error_step: Option<u32>,
    pub many_folders: bool,
}

impl From<core::RenameSummary> for FfiRenameSummary {
    fn from(s: core::RenameSummary) -> Self {
        FfiRenameSummary {
            version: s.version,
            total: s.total,
            ready: s.ready,
            unchanged: s.unchanged,
            problems: s.problems,
            warnings: s.warnings,
            exif_loading: s.exif_loading,
            exif_done: s.exif_done,
            exif_total: s.exif_total,
            error: s.error,
            error_step: s.error_step,
            many_folders: s.many_folders,
        }
    }
}

/// A saved preset.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRenamePreset {
    pub name: String,
    pub steps: Vec<FfiRenameStep>,
}

/// One open "Batch Rename" sheet. Every call is quick and never touches
/// the disk (EXIF is read on a background thread).
#[derive(uniffi::Object)]
pub struct FfiRenameSession {
    inner: Arc<RenameSession>,
}

#[uniffi::export]
impl FfiRenameSession {
    /// The steps now.
    pub fn steps(&self) -> Vec<FfiRenameStep> {
        steps_out(&self.inner.pipeline())
    }
    /// Replaces the steps and recomputes the preview (every keystroke).
    pub fn set_steps(&self, steps: Vec<FfiRenameStep>) -> FfiRenameSummary {
        self.inner.set_pipeline(pipeline_in(&steps)).into()
    }
    /// The counts and state (poll while `exif_loading`).
    pub fn summary(&self) -> FfiRenameSummary {
        self.inner.summary().into()
    }
    /// Rows `start..start+count`.
    pub fn rows(&self, start: u32, count: u32) -> Vec<FfiRenameRow> {
        self.inner
            .rows(start, count)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    /// Renames every ready row as one undoable operation.
    pub fn apply(&self) -> R<FfiOutcome> {
        Ok(self.inner.apply()?.into())
    }
}

#[uniffi::export]
impl FfiEngine {
    /// Opens a batch-rename session for `paths` (in list order). Reads each
    /// item's metadata and lists each folder once: blocks on disk like a
    /// folder listing.
    pub fn batch_rename(&self, tab: Option<u64>, paths: Vec<String>) -> R<Arc<FfiRenameSession>> {
        let inner = self.inner.batch_rename(tab.map(TabId), paths)?;
        Ok(Arc::new(FfiRenameSession { inner }))
    }
    /// The saved presets.
    pub fn rename_presets(&self) -> Vec<FfiRenamePreset> {
        self.inner
            .rename_presets()
            .into_iter()
            .map(|p| FfiRenamePreset {
                name: p.name,
                steps: steps_out(&p.pipeline),
            })
            .collect()
    }
    /// Saves `steps` as the preset `name` (replacing one with that name).
    pub fn save_rename_preset(&self, name: String, steps: Vec<FfiRenameStep>) -> R<()> {
        Ok(self.inner.save_rename_preset(&name, pipeline_in(&steps))?)
    }
    /// Deletes a preset. Returns whether there was one.
    pub fn delete_rename_preset(&self, name: String) -> R<bool> {
        Ok(self.inner.delete_rename_preset(&name)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind survives the trip core → FFI → core.
    #[test]
    fn steps_round_trip() {
        use fstudio_core::Step as S;
        let p = core::Pipeline::of(vec![
            S::Template {
                template: "{n:2}".into(),
            },
            S::Replace {
                find: "a".into(),
                replace: "b".into(),
                regex: true,
                ignore_case: true,
                first_only: true,
                target: core::Target::Whole,
            },
            S::Case {
                case: core::CaseKind::Sentence,
                target: core::Target::Extension,
            },
            S::Sequence {
                start: -3,
                step: 2,
                padding: 4,
                position: core::Position::Index(2),
                separator: "_".into(),
            },
            S::Trim {
                mode: core::TrimMode::Characters { chars: "-".into() },
                target: core::Target::Name,
            },
            S::Insert {
                text: "x".into(),
                position: core::Position::End,
                target: core::Target::Name,
            },
            S::Extension {
                change: core::ExtChange::Set { ext: "md".into() },
            },
        ]);
        let mut p2 = p.clone();
        p2.steps[2].enabled = false;
        for p in [p, p2] {
            assert_eq!(pipeline_in(&steps_out(&p)), p);
        }
    }

    #[test]
    fn new_steps_compile_and_tokens_are_listed() {
        for k in [
            FfiRenameStepKind::Template,
            FfiRenameStepKind::Replace,
            FfiRenameStepKind::Case,
            FfiRenameStepKind::Sequence,
            FfiRenameStepKind::Trim,
            FfiRenameStepKind::Insert,
            FfiRenameStepKind::Extension,
        ] {
            let s = rename_step_new(k);
            fstudio_rename_compiles(&pipeline_in(&[s]));
        }
        assert!(rename_tokens().iter().any(|t| t.token == "{exif.camera}"));
    }

    fn fstudio_rename_compiles(p: &core::Pipeline) {
        // A pipeline the session accepts has no error in its summary.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"").unwrap();
        let e = core::Engine::new(
            core::EngineConfig {
                data_dir: d.path().join("data").to_string_lossy().into_owned(),
                ..Default::default()
            },
            Arc::new(core::NullEventSink),
        )
        .unwrap();
        let s = e
            .batch_rename(
                None,
                vec![d.path().join("a.txt").to_string_lossy().into_owned()],
            )
            .unwrap();
        let sum = s.set_pipeline(p.clone());
        assert!(sum.error.is_none(), "{sum:?}");
    }
}
