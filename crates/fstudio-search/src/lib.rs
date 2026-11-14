#![warn(missing_docs)]
//! Search for File Studio (milestone 3).
//!
//! A search has two sources that run at the same time and are merged:
//!
//! - **The Spotlight index** ([`IndexSource`], [`Mdfind`] on macOS) answers
//!   fast but can be stale or incomplete. Every path it returns is checked
//!   again on disk with the same filters before it is shown, so it can only
//!   make results arrive sooner, never make them wrong.
//! - **A parallel filesystem walk** ([`walk`]) is the accurate one. It runs on
//!   plain threads (F-010), never follows symlinks (so it can't loop), skips
//!   package contents and hidden items by default, never descends into or
//!   reads iCloud placeholders (F-016), and reports folders it can't read
//!   without stopping.
//!
//! [`start`] runs both and hands out de-duplicated [`Hit`]s in batches as
//! they are found, then a [`Summary`]. Cancel with [`SearchHandle::cancel`].
//!
//! Names are matched on NFC, case-folded keys (F-017) and reported exactly
//! as the filesystem stores them.
//!
//! ```no_run
//! use fstudio_search::*;
//! let q = SearchQuery { name: "invoice".into(), ..SearchQuery::in_folder("/Users/me/Documents") };
//! let (handle, events) = start(q, Some(std::sync::Arc::new(Mdfind))).unwrap();
//! for e in events {
//!     match e {
//!         SearchEvent::Hits(hits) => for h in hits { println!("{}", h.path.display()) },
//!         SearchEvent::Finished(s) => { println!("{} found", s.found); break }
//!         _ => {}
//!     }
//! }
//! # drop(handle);
//! ```

mod cancel;
mod content;
mod kind;
mod matcher;
mod query;
mod run;
mod spotlight;
pub mod walk;

pub use cancel::CancelToken;
pub use content::{CONTENT_SNIFF_BYTES, content_matches};
pub use kind::{Kind, kind_of};
pub use matcher::{Matcher, glob_to_regex};
pub use query::{DateField, NameMode, QueryError, SearchQuery, THIS_MAC_SKIP};
pub use run::{Hit, Progress, SearchEvent, SearchHandle, Source, Summary, excluded_by_path, start};
pub use spotlight::{IndexQuery, IndexSource, Mdfind, NoIndex, escape_md, spotlight_query};
