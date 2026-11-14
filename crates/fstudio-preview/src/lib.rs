#![warn(missing_docs)]
//! Preview summaries for the inline preview pane (milestone 3).
//!
//! [`Previewer::summarize`] turns one path into a [`Preview`]: size, dates,
//! tags and the full path for everything, plus EXIF for photos and camera
//! raws, codec / duration / frame rate for video, an encoding, line count
//! and highlighted first lines for text, and an item count for folders.
//!
//! Rules this crate keeps:
//! - **Never fetches.** An iCloud placeholder (`SF_DATALESS`, F-016) is
//!   described from its `stat` only; its content is never opened.
//! - **Bounded.** Photos read a few megabytes at most, video only the `moov`
//!   box, text only its first lines plus a capped line count, folders stop
//!   counting at a limit. Nothing here reads a whole large file.
//! - **Cancellable.** A [`CancelToken`] is checked between steps and inside
//!   the long loops, so a preview the user has scrolled past stops early.
//! - **Never panics on bad input.** A truncated or corrupt file gives a
//!   [`Preview`] with less in it (and a `note`), not an error.
//! - **Cached** per (path, modification time, size) in an LRU.
//!
//! Calls block on disk. The shell calls them from a background queue.

mod cache;
mod cancel;
mod highlight;
mod iso;
mod kind;
mod photo;
mod stat;
mod text;
mod types;
mod video;

pub use cache::PreviewCache;
pub use cancel::CancelToken;
pub use highlight::TokenClass;
pub use kind::{Category, classify};
pub use stat::FileStat;
pub use types::{
    FolderInfo, PhotoInfo, Preview, PreviewOptions, PreviewTag, Span, TextLine, TextPreview,
    VideoInfo,
};

use std::path::Path;
use std::sync::Arc;

/// Builds [`Preview`]s and remembers the recent ones.
#[derive(Debug)]
pub struct Previewer {
    cache: PreviewCache,
}

impl Default for Previewer {
    fn default() -> Self {
        Self::new()
    }
}

impl Previewer {
    /// A previewer whose cache holds the last 256 summaries.
    pub fn new() -> Previewer {
        Previewer {
            cache: PreviewCache::new(256),
        }
    }

    /// The summary of `path`, from the cache when the file is unchanged (same
    /// modification time and size) and the options are the same.
    ///
    /// Blocks on disk (a `stat`, then a bounded read). Returns `None` only
    /// when `cancel` fired before the work finished, or the path can't be
    /// `stat`ed at all. A cancelled summary is never cached.
    pub fn summarize(
        &self,
        path: &Path,
        opts: &PreviewOptions,
        cancel: &CancelToken,
    ) -> Option<Arc<Preview>> {
        let stat = FileStat::of(path).ok()?;
        self.summarize_with(path, stat, opts, cancel)
    }

    /// As [`Previewer::summarize`] with the `stat` already taken (tests use
    /// this to pretend a file is an iCloud placeholder).
    pub fn summarize_with(
        &self,
        path: &Path,
        stat: FileStat,
        opts: &PreviewOptions,
        cancel: &CancelToken,
    ) -> Option<Arc<Preview>> {
        let key = cache::Key::new(path, &stat, opts);
        if let Some(hit) = self.cache.get(&key) {
            return Some(hit);
        }
        let p = Arc::new(build(path, &stat, opts, cancel)?);
        if !cancel.is_cancelled() {
            self.cache.put(key, p.clone());
        }
        Some(p)
    }

    /// A cached summary, if there is one for the file as it is now. Never
    /// reads content, so it is cheap enough for the main thread.
    pub fn cached(&self, path: &Path, opts: &PreviewOptions) -> Option<Arc<Preview>> {
        let stat = FileStat::of(path).ok()?;
        self.cache.get(&cache::Key::new(path, &stat, opts))
    }

    /// Forgets every cached summary.
    pub fn clear(&self) {
        self.cache.clear();
    }
}

/// Builds the summary. `None` when cancelled.
fn build(
    path: &Path,
    stat: &FileStat,
    opts: &PreviewOptions,
    cancel: &CancelToken,
) -> Option<Preview> {
    let mut p = Preview::basic(path, stat);
    p.tags = types::tags_of(path);
    if cancel.is_cancelled() {
        return None;
    }
    // A placeholder is described from its stat alone (F-016).
    if stat.dataless {
        p.note = Some("Not downloaded from iCloud".into());
        return Some(p);
    }
    match p.category {
        Category::Folder | Category::Package => {
            p.folder = Some(types::folder_info(path, opts.folder_count_limit, cancel)?);
        }
        Category::Image | Category::Raw => {
            let (info, note) = photo::read(path, stat.size, opts, cancel);
            p.photo = info;
            p.note = note;
        }
        Category::Video => {
            let (info, note) = video::read(path, cancel);
            p.video = info;
            p.note = note;
        }
        Category::Text | Category::Unknown => {
            let (t, note) = text::read(path, stat.size, p.category == Category::Text, opts, cancel);
            if let Some(t) = t {
                // Content sniffing found text in a file with an unknown type.
                p.category = Category::Text;
                p.text = Some(t);
            }
            p.note = note;
        }
        Category::Audio | Category::Other => {}
    }
    if cancel.is_cancelled() {
        return None;
    }
    Some(p)
}
