//! Throwaway M0 spike helpers. Not product code.
//! Every `unsafe` block carries a `// SAFETY:` comment; see also docs/SPIKES.md.
pub mod fsevents;
pub mod gen;
pub mod list;
pub mod sortkey;
pub mod stats;
pub mod sys;

use std::time::Instant;

pub fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
