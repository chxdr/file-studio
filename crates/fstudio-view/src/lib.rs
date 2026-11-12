//! Pure view logic for File Studio's file lists. No I/O and no OS calls.
//!
//! * [`Listing`]: an immutable, versioned, sorted and filtered snapshot of a folder, with
//!   incremental [`Listing::apply`] and streaming [`ListingBuilder`].
//! * [`type_select`]: Finder-style type-ahead.
//! * [`Selection`]: cursor, anchor and selected rows, with Shift/Cmd semantics.
//! * [`format_size`] and [`format_date`]: display strings.

pub mod collation;
pub mod format;
pub mod listing;
pub mod options;
pub mod select;
pub mod selection;

pub use collation::{Collation, NaturalCollation, fold, natural_cmp};
pub use format::{
    Clock, DateFormat, DateOrder, SizeFormat, civil_from_days, format_date, format_size,
};
pub use listing::{Change, Listing, ListingBuilder, PARALLEL_THRESHOLD};
pub use options::{SortKey, ViewOptions, glob_match};
pub use select::type_select;
pub use selection::{Move, Selection};
