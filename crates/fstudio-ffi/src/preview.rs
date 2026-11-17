//! The preview pane's exports (M3): a previewer object and plain mirror
//! records of `fstudio_core`'s preview types. Calls to `summarize` block on
//! disk; the shell makes them from a background queue and cancels them with
//! an [`FfiCancelToken`] when the cursor moves on.

use std::sync::Arc;

use fstudio_core as core;
use fstudio_core::{CancelToken, PreviewService};

use crate::FfiEngine;

/// A flag the shell sets to stop a preview that has been scrolled past.
#[derive(uniffi::Object, Default)]
pub struct FfiCancelToken {
    inner: CancelToken,
}

#[uniffi::export]
impl FfiCancelToken {
    /// A token that has not fired.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Asks the preview to stop. Any thread.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
    /// Has it been cancelled?
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

/// What kind of item a preview is about.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiPreviewCategory {
    Image,
    Raw,
    Video,
    Audio,
    Text,
    Folder,
    Package,
    Other,
    Unknown,
}

/// What kind of token a highlighted span is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiTokenClass {
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
    Constant,
    Punctuation,
    Tag,
    Attribute,
    Heading,
    Variable,
    Invalid,
}

/// A Finder tag.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiPreviewTag {
    pub name: String,
    /// Finder's colour index 0–7.
    pub color: u8,
}

/// Camera details, formatted for display.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiPhotoInfo {
    pub date_taken: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<String>,
    pub aperture: Option<String>,
    pub shutter: Option<String>,
    pub iso: Option<String>,
    pub exposure_comp: Option<String>,
    pub dimensions: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u16>,
}

/// Movie details, formatted for display.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiVideoInfo {
    pub codec: Option<String>,
    pub duration_secs: Option<f64>,
    pub duration: Option<String>,
    pub resolution: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub audio: Option<String>,
}

/// A coloured run in a line, in UTF-16 code units.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiSpan {
    pub start: u32,
    pub len: u32,
    pub class: FfiTokenClass,
}

/// One line of text with its coloured runs.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiTextLine {
    pub text: String,
    pub spans: Vec<FfiSpan>,
}

/// The start of a text file.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiTextPreview {
    pub encoding: String,
    pub language: Option<String>,
    pub line_count: u64,
    pub lines_exact: bool,
    pub lines: Vec<FfiTextLine>,
    pub truncated: bool,
}

/// What a folder holds.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiFolderInfo {
    pub items: u64,
    pub at_least: bool,
    pub unreadable: bool,
}

/// Everything the preview pane shows about one item.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiPreview {
    pub path: String,
    /// The name exactly as stored.
    pub name: String,
    pub category: FfiPreviewCategory,
    pub size: u64,
    pub modified_ns: i64,
    pub created_ns: Option<i64>,
    pub accessed_ns: Option<i64>,
    pub is_symlink: bool,
    /// An iCloud placeholder: nothing was read; offer Download.
    pub dataless: bool,
    pub tags: Vec<FfiPreviewTag>,
    pub photo: Option<FfiPhotoInfo>,
    pub video: Option<FfiVideoInfo>,
    pub text: Option<FfiTextPreview>,
    pub folder: Option<FfiFolderInfo>,
    /// Why there is little to show.
    pub note: Option<String>,
    // XMP culling additions
    /// Stars, label and pick/reject from the item's XMP sidecar.
    pub marks: Option<crate::types::FfiCullMarks>,
}

impl From<core::PreviewCategory> for FfiPreviewCategory {
    fn from(c: core::PreviewCategory) -> Self {
        use core::PreviewCategory as C;
        match c {
            C::Image => Self::Image,
            C::Raw => Self::Raw,
            C::Video => Self::Video,
            C::Audio => Self::Audio,
            C::Text => Self::Text,
            C::Folder => Self::Folder,
            C::Package => Self::Package,
            C::Other => Self::Other,
            C::Unknown => Self::Unknown,
        }
    }
}

impl From<core::TokenClass> for FfiTokenClass {
    fn from(c: core::TokenClass) -> Self {
        use core::TokenClass as T;
        match c {
            T::Keyword => Self::Keyword,
            T::String => Self::String,
            T::Number => Self::Number,
            T::Comment => Self::Comment,
            T::Function => Self::Function,
            T::Type => Self::Type,
            T::Constant => Self::Constant,
            T::Punctuation => Self::Punctuation,
            T::Tag => Self::Tag,
            T::Attribute => Self::Attribute,
            T::Heading => Self::Heading,
            T::Variable => Self::Variable,
            T::Invalid => Self::Invalid,
        }
    }
}

impl From<&core::Preview> for FfiPreview {
    fn from(p: &core::Preview) -> Self {
        FfiPreview {
            path: p.path.clone(),
            name: p.name.clone(),
            category: p.category.into(),
            size: p.size,
            modified_ns: p.modified_ns,
            created_ns: p.created_ns,
            accessed_ns: p.accessed_ns,
            is_symlink: p.is_symlink,
            dataless: p.dataless,
            tags: p
                .tags
                .iter()
                .map(|t| FfiPreviewTag {
                    name: t.name.clone(),
                    color: t.color,
                })
                .collect(),
            photo: p.photo.as_ref().map(|i| FfiPhotoInfo {
                date_taken: i.date_taken.clone(),
                camera: i.camera.clone(),
                lens: i.lens.clone(),
                focal_length: i.focal_length.clone(),
                aperture: i.aperture.clone(),
                shutter: i.shutter.clone(),
                iso: i.iso.clone(),
                exposure_comp: i.exposure_comp.clone(),
                dimensions: i.dimensions.clone(),
                width: i.width,
                height: i.height,
                orientation: i.orientation,
            }),
            video: p.video.as_ref().map(|v| FfiVideoInfo {
                codec: v.codec.clone(),
                duration_secs: v.duration_secs,
                duration: v.duration.clone(),
                resolution: v.resolution.clone(),
                width: v.width,
                height: v.height,
                frame_rate: v.frame_rate.clone(),
                audio: v.audio.clone(),
            }),
            text: p.text.as_ref().map(|t| FfiTextPreview {
                encoding: t.encoding.clone(),
                language: t.language.clone(),
                line_count: t.line_count,
                lines_exact: t.lines_exact,
                lines: t
                    .lines
                    .iter()
                    .map(|l| FfiTextLine {
                        text: l.text.clone(),
                        spans: l
                            .spans
                            .iter()
                            .map(|s| FfiSpan {
                                start: s.start,
                                len: s.len,
                                class: s.class.into(),
                            })
                            .collect(),
                    })
                    .collect(),
                truncated: t.truncated,
            }),
            folder: p.folder.map(|f| FfiFolderInfo {
                items: f.items,
                at_least: f.at_least,
                unreadable: f.unreadable,
            }),
            note: p.note.clone(),
            marks: None,
        }
    }
}

/// Makes previews for the preview pane. One per app; it remembers recent
/// ones by (path, modification time, size).
#[derive(uniffi::Object)]
pub struct FfiPreviewer {
    engine: Arc<FfiEngine>,
    service: PreviewService,
}

#[uniffi::export]
impl FfiPreviewer {
    /// A previewer that reads its limits from the engine's `preview.*` settings.
    #[uniffi::constructor]
    pub fn new(engine: Arc<FfiEngine>) -> Arc<Self> {
        Arc::new(FfiPreviewer {
            engine,
            service: PreviewService::new(),
        })
    }

    /// The preview of `path`. **Blocks on disk**: call it from a background
    /// queue. `None` when `cancel` fired first or the path can't be read.
    pub fn summarize(&self, path: String, cancel: Arc<FfiCancelToken>) -> Option<FfiPreview> {
        let settings = self.engine.inner.settings().preview;
        let p = self.service.summarize(&path, &settings, &cancel.inner)?;
        let mut out = FfiPreview::from(&*p);
        out.marks = self.engine.inner.cull_marks(&path).map(Into::into);
        Some(out)
    }

    /// The preview if it is already cached and the file is unchanged. Reads
    /// no content, so the main thread may call it.
    pub fn cached(&self, path: String) -> Option<FfiPreview> {
        let settings = self.engine.inner.settings().preview;
        let mut out = self
            .service
            .cached(&path, &settings)
            .map(|p| FfiPreview::from(&*p))?;
        out.marks = self.engine.inner.cull_marks_cached(&path).map(Into::into);
        Some(out)
    }

    /// Forgets every cached preview.
    pub fn clear(&self) {
        self.service.clear();
    }
}
