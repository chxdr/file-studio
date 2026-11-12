//! The core facade: `Engine`, the workspace model (windows → panes → tabs),
//! events and settings. Built by the coordinator after the M1 crates merge.
//! See docs/PROPOSAL.md §4.

/// The core's version, for the shells' About box and the CLI.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
