//! A small LRU of finished summaries, keyed by (path, mtime, size, options).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::stat::FileStat;
use crate::types::{Preview, PreviewOptions};

/// What identifies a summary: the file as it is on disk plus the options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Key {
    path: PathBuf,
    mtime_ns: i64,
    size: u64,
    dataless: bool,
    opts: PreviewOptions,
}

impl Key {
    pub(crate) fn new(path: &Path, stat: &FileStat, opts: &PreviewOptions) -> Key {
        Key {
            path: path.to_path_buf(),
            mtime_ns: stat.mtime_ns,
            size: stat.size,
            dataless: stat.dataless,
            opts: opts.clone(),
        }
    }
}

/// The cache. Most recent last.
#[derive(Debug)]
pub struct PreviewCache {
    cap: usize,
    items: Mutex<VecDeque<(Key, Arc<Preview>)>>,
}

impl PreviewCache {
    /// A cache holding at most `cap` summaries.
    pub fn new(cap: usize) -> PreviewCache {
        PreviewCache {
            cap: cap.max(1),
            items: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn get(&self, key: &Key) -> Option<Arc<Preview>> {
        let mut items = self.items.lock();
        let i = items.iter().position(|(k, _)| k == key)?;
        let entry = items.remove(i)?;
        let hit = entry.1.clone();
        items.push_back(entry);
        Some(hit)
    }

    pub(crate) fn put(&self, key: Key, value: Arc<Preview>) {
        let mut items = self.items.lock();
        if let Some(i) = items.iter().position(|(k, _)| *k == key) {
            items.remove(i);
        }
        items.push_back((key, value));
        while items.len() > self.cap {
            items.pop_front();
        }
    }

    pub(crate) fn clear(&self) {
        self.items.lock().clear();
    }

    /// How many summaries are held.
    pub fn len(&self) -> usize {
        self.items.lock().len()
    }

    /// Is it empty?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
