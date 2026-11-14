//! The preview pane's core side: its settings and the service that turns a
//! path into a [`fstudio_preview::Preview`] (M3).
//!
//! The service keeps a cache keyed by (path, mtime, size). Calls block on
//! disk, so the shell runs them on a background queue and cancels them with a
//! [`CancelToken`] when the cursor moves on. An iCloud placeholder is only
//! ever described, never read (F-016).

use std::path::Path;
use std::sync::Arc;

use fstudio_preview::{CancelToken, Preview, PreviewOptions, Previewer};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Settings → Previews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewSettings {
    /// The preview pane is shown. "Show or Hide Preview" flips this, so the
    /// pane comes back the way it was left.
    pub visible: bool,
    /// Most lines of text shown (10–2000).
    pub max_text_lines: u32,
    /// Colour code and markup by syntax.
    pub highlight: bool,
    /// Show the picture or thumbnail above the details.
    pub thumbnails: bool,
    /// The pane's width in points (220–640).
    pub width: u32,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            visible: false,
            max_text_lines: 200,
            highlight: true,
            thumbnails: true,
            width: 300,
        }
    }
}

impl PreviewSettings {
    /// Checks the ranges.
    pub fn validate(&self) -> Result<()> {
        if !(10..=2000).contains(&self.max_text_lines) {
            return Err(CoreError::InvalidSetting(
                "preview.max_text_lines must be 10–2000".into(),
            ));
        }
        if !(220..=640).contains(&self.width) {
            return Err(CoreError::InvalidSetting(
                "preview.width must be 220–640".into(),
            ));
        }
        Ok(())
    }

    /// The limits the preview crate is called with.
    pub fn options(&self) -> PreviewOptions {
        PreviewOptions {
            max_text_lines: self.max_text_lines,
            highlight: self.highlight,
            // Text over 32 MB gets an estimated line count rather than a scan.
            line_count_cap_bytes: 32 * 1024 * 1024,
            ..PreviewOptions::default()
        }
    }
}

/// Makes previews and remembers recent ones. One per engine.
#[derive(Debug, Default)]
pub struct PreviewService {
    previewer: Previewer,
}

impl PreviewService {
    /// A service with an empty cache.
    pub fn new() -> PreviewService {
        PreviewService::default()
    }

    /// The preview of `path` under `settings`. Blocks on disk; `None` when
    /// cancelled or the path can't be read at all.
    pub fn summarize(
        &self,
        path: &str,
        settings: &PreviewSettings,
        cancel: &CancelToken,
    ) -> Option<Arc<Preview>> {
        self.previewer
            .summarize(Path::new(path), &settings.options(), cancel)
    }

    /// The cached preview, if the file is unchanged since it was made. Does
    /// no content reads.
    pub fn cached(&self, path: &str, settings: &PreviewSettings) -> Option<Arc<Preview>> {
        self.previewer.cached(Path::new(path), &settings.options())
    }

    /// Forgets every cached preview.
    pub fn clear(&self) {
        self.previewer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn defaults_and_keys() {
        let s = Settings::default();
        assert!(!s.preview.visible);
        s.validate().unwrap();
        let t = s.with_key("preview.visible", "true").unwrap();
        assert!(t.preview.visible);
        let t = s.with_key("preview.max_text_lines", "50").unwrap();
        assert_eq!(t.preview.options().max_text_lines, 50);
        assert!(s.with_key("preview.max_text_lines", "5").is_err());
        assert!(s.with_key("preview.width", "100").is_err());
        // Settings saved before M3 have no preview group.
        let old = Settings::from_json(r#"{"version":1,"settings":{"view":{}}}"#).unwrap();
        assert_eq!(old.preview, PreviewSettings::default());
    }

    #[test]
    fn service_summarizes_and_caches() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, "hi\n").unwrap();
        let path = f.to_str().unwrap();
        let svc = PreviewService::new();
        let st = PreviewSettings::default();
        assert!(svc.cached(path, &st).is_none());
        let p = svc.summarize(path, &st, &CancelToken::new()).unwrap();
        assert_eq!(p.text.as_ref().unwrap().line_count, 1);
        assert!(svc.cached(path, &st).is_some());
        svc.clear();
        assert!(svc.cached(path, &st).is_none());
    }
}
