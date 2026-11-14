//! A cancel flag that can also run clean-up hooks (killing `mdfind`, waking
//! idle walker threads) the moment it is set.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    hooks: Mutex<Vec<Hook>>,
}

/// Shared between a search and whoever may stop it. Cloning shares the flag.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Inner>);

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CancelToken")
            .field(&self.is_cancelled())
            .finish()
    }
}

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the flag and runs every hook once. Later calls do nothing.
    pub fn cancel(&self) {
        if self.0.flag.swap(true, Ordering::SeqCst) {
            return;
        }
        let hooks = std::mem::take(&mut *self.0.hooks.lock());
        for h in hooks {
            h();
        }
    }

    /// True once [`CancelToken::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Runs `f` when the token is cancelled, or right away if it already is.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut hooks = self.0.hooks.lock();
        if self.is_cancelled() {
            drop(hooks);
            f();
        } else {
            hooks.push(Box::new(f));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn hooks_run_once_and_late_hooks_run_now() {
        let t = CancelToken::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        t.on_cancel(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!t.is_cancelled());
        t.cancel();
        t.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        let n3 = n.clone();
        t.on_cancel(move || {
            n3.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(n.load(Ordering::SeqCst), 11);
    }
}
