//! Path parsing, breadcrumbs, completion and frecency. See DECISIONS F-009, F-017, F-018.
//!
//! This crate is the engine behind the address bar. It has no UI and no async
//! runtime. The pieces:
//!
//! - [`parse_location`]: turns whatever the user typed or pasted into a
//!   [`Location`] (a folder, a file, a missing path or a remote URL), reporting
//!   which paste cleanups it applied.
//! - [`breadcrumbs`], [`collapse`], [`subfolders`]: the resting state of the bar.
//! - [`copy_path_as`]: every "Copy Path" flavour.
//! - [`Completer`] and [`TabCycle`]: completion as you type, in memory, under 16 ms.
//! - [`Frecency`]: zoxide-style visit ranking, stored in `fstudio-db`.
//!
//! Disk access goes through two small traits ([`FsProbe`] for parsing,
//! [`DirSource`] for completion) so everything can be tested without a disk, and
//! so the core can later plug in `fstudio-fs`. Names are never changed: folded
//! keys (NFC + lower case) are used only for comparing (F-017).

pub mod complete;
pub mod copy;
pub mod crumbs;
pub mod error;
pub mod frecency;
pub mod keys;
pub mod parse;
pub mod probe;
pub mod source;
mod sys;
pub mod testing;
pub mod zoxide;

pub use complete::{
    Completer, CompleterConfig, Completion, CompletionKind, Completions, InputKind, TabCycle,
    TabExtension, TabOutcome,
};
pub use copy::copy_path_as;
pub use crumbs::{Crumb, CrumbKind, Volume, breadcrumbs, collapse, subfolders};
pub use error::PathError;
pub use frecency::{Frecency, FrecencyConfig, FrecencyEntry};
pub use fstudio_model::PathStyle;
pub use parse::{Cleanup, Location, ParseContext, ParsedLocation, parse_location};
pub use probe::{FsProbe, ProbeKind, StdProbe};
pub use source::{CachedDirSource, Child, DirListing, DirSource, StdDirSource};
