#![warn(missing_docs)]
//! Batch rename (M3): a pure pipeline of steps, a live preview that flags
//! problems per item, EXIF tokens, and saved presets.
//!
//! - [`pipeline`]: the steps (template, find and replace, case, sequence,
//!   trim, insert, extension) and [`Compiled`], which applies them.
//! - [`template`]: `{name}`, `{n:3}`, `{date:%Y-%m-%d}`, `{exif.camera}`…
//! - [`preview`]: new names plus collisions, invalid names and unchanged
//!   names, fast enough for every keystroke on 10,000 items.
//! - [`exif`]: reads EXIF without ever opening an iCloud placeholder (F-016).
//! - [`preset`]: named pipelines.
//!
//! Applying a rename is not done here: the core turns the preview's ready
//! rows into one `fstudio-ops` request, which renames with `RENAME_EXCL`,
//! handles swaps and cycles, and undoes as one step (F-007).
//!
//! ```
//! use fstudio_rename::*;
//! let p = Pipeline::of(vec![Step::Template { template: "Trip {n:3}".into() }]);
//! let c = Compiled::new(&p).unwrap();
//! assert_eq!(c.apply(&ItemInfo::named("IMG_1.JPG"), 4).name, "Trip 005.JPG");
//! ```

pub mod date;
pub mod exif;
pub mod item;
pub mod name;
pub mod pipeline;
pub mod preset;
pub mod preview;
pub mod template;

pub use date::DateParts;
pub use item::{Exif, ExifInfo, ItemInfo};
pub use pipeline::{
    CaseKind, Compiled, ExtChange, Pipeline, PipelineError, PipelineStep, Position, Step, Target,
    TrimMode,
};
pub use preset::{Preset, Presets};
pub use preview::{Entry, Folder, Preview, Row, Status, Summary, preview};
pub use template::{RenderNotes, Template};
