//! A flag a preview checks so it can stop early.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cancels the preview it was passed to. Cloning shares the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not fired.
    pub fn new() -> CancelToken {
        CancelToken::default()
    }
    /// Asks the work to stop. Safe from any thread.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    /// Has [`CancelToken::cancel`] been called?
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
