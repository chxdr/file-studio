//! EXIF for photos and camera raws, read with a bounded number of bytes.
//!
//! - JPEG, HEIC/HEIF/AVIF, PNG, WebP: `kamadak-exif` walks the container
//!   and reads only the metadata segment.
//! - TIFF-based raws (ARW, NEF, DNG, CR2, …): the EXIF directories sit near
//!   the start, so a prefix of the file is read (1 MiB, then 8, then 64 MiB
//!   only if the first reads did not hold them).
//! - Canon CR3: an ISO box file; the EXIF lives in the `CMT1`/`CMT2` boxes
//!   inside Canon's `uuid` box of `moov`, read from there.
//! - Fujifilm RAF: the embedded JPEG's EXIF.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use exif::{Exif, In, Reader, Tag, Value};

use crate::cancel::CancelToken;
use crate::iso;
use crate::kind::{Category, classify};
use crate::types::{PhotoInfo, PreviewOptions};
use crate::video;

/// Canon's `uuid` box id inside `moov` of a CR3.
const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];

/// Prefix sizes tried for TIFF-based raws.
const PREFIXES: [u64; 3] = [1 << 20, 8 << 20, 64 << 20];

/// Reads photo facts. The note says why there are none.
pub(crate) fn read(
    path: &Path,
    size: u64,
    opts: &PreviewOptions,
    cancel: &CancelToken,
) -> (Option<PhotoInfo>, Option<String>) {
    if opts.max_photo_bytes > 0 && size > opts.max_photo_bytes {
        return (None, Some("Too large to read metadata from".into()));
    }
    let Ok(mut f) = File::open(path) else {
        return (None, Some("Can't read this file".into()));
    };
    let is_raw = classify(path, false) == Category::Raw;
    let (exifs, dims) = match load(&mut f, size, cancel) {
        Some(x) => x,
        None => return (None, None),
    };
    if cancel.is_cancelled() {
        return (None, None);
    }
    let mut info = from_exif(&exifs, is_raw);
    if info.width.is_none()
        && let Some((w, h)) = dims
    {
        set_dims(&mut info, w, h);
    }
    if info.is_empty() {
        let note = if exifs.is_empty() {
            "No camera metadata in this file"
        } else {
            "No camera details found"
        };
        (None, Some(note.into()))
    } else {
        (Some(info), None)
    }
}

fn set_dims(info: &mut PhotoInfo, w: u32, h: u32) {
    if w > 0 && h > 0 {
        info.width = Some(w);
        info.height = Some(h);
        info.dimensions = Some(format!("{w} × {h}"));
    }
}

/// Finds the EXIF blocks of the file (more than one for CR3) and, if the
/// format gives them cheaply, the pixel size.
/// EXIF blocks and, when cheaply known, the pixel size.
type Loaded = (Vec<Exif>, Option<(u32, u32)>);

fn load(f: &mut File, size: u64, cancel: &CancelToken) -> Option<Loaded> {
    let mut head = [0u8; 16];
    let n = read_at(f, 0, &mut head);
    let head = &head[..n];
    if head.starts_with(&[0xff, 0xd8]) {
        let dims = jpeg_dims(f);
        return Some((container(f), dims));
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some((tiff_prefix(f, size, cancel)?, None));
    }
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return Some((raf(f, size), None));
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        if &head[8..12] == b"crx " {
            return Some(cr3(f, size));
        }
        return Some((container(f), None));
    }
    if head.starts_with(b"\x89PNG") {
        let mut ihdr = [0u8; 24];
        let dims = (read_at(f, 0, &mut ihdr) == 24).then(|| {
            (
                u32::from_be_bytes([ihdr[16], ihdr[17], ihdr[18], ihdr[19]]),
                u32::from_be_bytes([ihdr[20], ihdr[21], ihdr[22], ihdr[23]]),
            )
        });
        return Some((container(f), dims));
    }
    // WebP and anything else kamadak-exif may know.
    Some((container(f), None))
}

/// Reads up to `buf.len()` bytes at `at`; returns how many arrived.
fn read_at(f: &mut File, at: u64, buf: &mut [u8]) -> usize {
    if f.seek(SeekFrom::Start(at)).is_err() {
        return 0;
    }
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    got
}

/// EXIF from a container kamadak-exif understands (JPEG, HEIF, PNG, WebP).
fn container(f: &mut File) -> Vec<Exif> {
    if f.seek(SeekFrom::Start(0)).is_err() {
        return Vec::new();
    }
    Reader::new()
        .read_from_container(&mut BufReader::new(&mut *f))
        .map(|e| vec![e])
        .unwrap_or_default()
}

/// EXIF from a TIFF-based raw: the smallest prefix that holds it.
fn tiff_prefix(f: &mut File, size: u64, cancel: &CancelToken) -> Option<Vec<Exif>> {
    for (i, cap) in PREFIXES.iter().enumerate() {
        if cancel.is_cancelled() {
            return None;
        }
        let want = size.min(*cap);
        let mut buf = vec![0u8; usize::try_from(want).ok()?];
        let got = read_at(f, 0, &mut buf);
        buf.truncate(got);
        let whole = got as u64 >= size;
        match Reader::new().read_raw(buf) {
            Ok(e) if e.fields().next().is_some() => {
                // A short read can hide the Exif directory (it is then simply
                // missing, not an error): ask for more unless this is all.
                if whole || e.get_field(Tag::ExposureTime, In::PRIMARY).is_some() {
                    return Some(vec![e]);
                }
                if i == PREFIXES.len() - 1 {
                    return Some(vec![e]);
                }
            }
            Ok(_) | Err(_) => {
                if whole {
                    return Some(Vec::new());
                }
            }
        }
        if whole {
            break;
        }
    }
    Some(Vec::new())
}

/// Fujifilm RAF: a header points at an embedded JPEG.
fn raf(f: &mut File, size: u64) -> Vec<Exif> {
    let mut h = [0u8; 100];
    if read_at(f, 0, &mut h) < 92 {
        return Vec::new();
    }
    let off = u64::from(u32::from_be_bytes([h[84], h[85], h[86], h[87]]));
    let len = u64::from(u32::from_be_bytes([h[88], h[89], h[90], h[91]]));
    if off == 0 || off >= size {
        return Vec::new();
    }
    let take = len.min(size - off).min(1 << 20);
    let mut buf = vec![0u8; take as usize];
    let got = read_at(f, off, &mut buf);
    buf.truncate(got);
    Reader::new()
        .read_from_container(&mut Cursor::new(buf))
        .map(|e| vec![e])
        .unwrap_or_default()
}

/// Canon CR3: `moov` › `uuid`(Canon) › `CMT1` (IFD0), `CMT2` (Exif IFD).
fn cr3(f: &mut File, size: u64) -> Loaded {
    let top = iso::children(f, 0, size);
    let Some(moov) = top.iter().find(|b| &b.kind == b"moov").copied() else {
        return (Vec::new(), None);
    };
    let mut out = Vec::new();
    for u in iso::children(f, moov.payload, moov.end)
        .into_iter()
        .filter(|b| &b.kind == b"uuid")
    {
        let Some(id) = iso::payload(f, &u, 16) else {
            continue;
        };
        if id != CANON_UUID {
            continue;
        }
        for b in iso::children(f, u.payload + 16, u.end) {
            if (&b.kind == b"CMT1" || &b.kind == b"CMT2")
                && let Some(bytes) = iso::payload(f, &b, 1 << 20)
                && let Ok(e) = Reader::new().read_raw(bytes)
            {
                out.push(e);
            }
        }
    }
    // The raw image is the biggest `CRAW` track.
    let dims = video::tracks(f, &moov)
        .into_iter()
        .filter(|t| &t.format == b"CRAW" || &t.handler == b"vide")
        .max_by_key(|t| u64::from(t.width) * u64::from(t.height))
        .filter(|t| t.width > 0 && t.height > 0)
        .map(|t| (t.width, t.height));
    (out, dims)
}

/// Pixel size from the first start-of-frame marker of a JPEG, found by
/// hopping over segments (no pixel data is read).
fn jpeg_dims(f: &mut File) -> Option<(u32, u32)> {
    let mut pos = 2u64;
    for _ in 0..256 {
        let mut h = [0u8; 4];
        if read_at(f, pos, &mut h) < 4 || h[0] != 0xff {
            return None;
        }
        let marker = h[1];
        if marker == 0xff {
            pos += 1;
            continue;
        }
        let len = u64::from(u16::from_be_bytes([h[2], h[3]]));
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let mut s = [0u8; 9];
            if read_at(f, pos + 4, &mut s) < 5 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([s[1], s[2]]));
            let width = u32::from(u16::from_be_bytes([s[3], s[4]]));
            return Some((width, height));
        }
        if marker == 0xda || marker == 0xd9 || len < 2 {
            return None;
        }
        pos += 2 + len;
    }
    None
}

// ───────────────────────── field extraction ─────────────────────────

fn field(exifs: &[Exif], tag: Tag) -> Option<&exif::Field> {
    exifs.iter().find_map(|e| e.get_field(tag, In::PRIMARY))
}

fn ascii(exifs: &[Exif], tag: Tag) -> Option<String> {
    let f = field(exifs, tag)?;
    let Value::Ascii(v) = &f.value else {
        return None;
    };
    let s = String::from_utf8_lossy(v.first()?).into_owned();
    let s = s
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string();
    (!s.is_empty()).then_some(s)
}

fn rational(exifs: &[Exif], tag: Tag) -> Option<(u32, u32)> {
    match &field(exifs, tag)?.value {
        Value::Rational(v) => v.first().map(|r| (r.num, r.denom)),
        _ => None,
    }
}

fn uint(exifs: &[Exif], tag: Tag) -> Option<u32> {
    field(exifs, tag)?.value.get_uint(0)
}

/// Builds the display record from EXIF blocks.
#[allow(clippy::field_reassign_with_default)] // built field by field from many tags
pub(crate) fn from_exif(exifs: &[Exif], is_raw: bool) -> PhotoInfo {
    let mut p = PhotoInfo::default();
    p.date_taken = [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime]
        .into_iter()
        .find_map(|t| ascii(exifs, t))
        .map(|s| format_date(&s));
    p.camera = camera(ascii(exifs, Tag::Make), ascii(exifs, Tag::Model));
    // Some cameras write "----" when there is no lens data.
    p.lens = ascii(exifs, Tag::LensModel).filter(|l| l.chars().any(char::is_alphanumeric));
    p.focal_length = rational(exifs, Tag::FocalLength)
        .filter(|(n, d)| *d != 0 && *n != 0)
        .map(|(n, d)| {
            let mm = f64::from(n) / f64::from(d);
            let base = format!("{} mm", trim_num(mm, 1));
            match uint(exifs, Tag::FocalLengthIn35mmFilm) {
                Some(eq) if eq > 0 && (f64::from(eq) - mm).abs() >= 1.0 => {
                    format!("{base} ({eq} mm equiv.)")
                }
                _ => base,
            }
        });
    p.aperture = rational(exifs, Tag::FNumber)
        .filter(|(n, d)| *d != 0 && *n != 0)
        .map(|(n, d)| format!("ƒ/{}", trim_num(f64::from(n) / f64::from(d), 1)));
    p.shutter = rational(exifs, Tag::ExposureTime).and_then(|(n, d)| format_shutter(n, d));
    p.iso = uint(exifs, Tag::PhotographicSensitivity)
        .filter(|v| *v > 0 && *v < 65535)
        .or_else(|| uint(exifs, Tag::ISOSpeed).filter(|v| *v > 0))
        .map(|v| format!("ISO {v}"));
    p.exposure_comp = match field(exifs, Tag::ExposureBiasValue).map(|f| &f.value) {
        Some(Value::SRational(v)) => v.first().filter(|r| r.denom != 0).map(|r| {
            let ev = f64::from(r.num) / f64::from(r.denom);
            if ev == 0.0 {
                "0 EV".to_string()
            } else {
                format!("{ev:+.1} EV").replace(".0 ", " ")
            }
        }),
        _ => None,
    };
    p.orientation = uint(exifs, Tag::Orientation)
        .filter(|o| (1..=8).contains(o))
        .map(|o| o as u16);
    let w = uint(exifs, Tag::PixelXDimension)
        .or_else(|| (!is_raw).then(|| uint(exifs, Tag::ImageWidth)).flatten());
    let h = uint(exifs, Tag::PixelYDimension)
        .or_else(|| (!is_raw).then(|| uint(exifs, Tag::ImageLength)).flatten());
    if let (Some(w), Some(h)) = (w, h) {
        set_dims(&mut p, w, h);
    }
    p
}

/// `2026:11:11 14:03:22` → `2026-11-11 14:03:22`; anything else unchanged.
pub(crate) fn format_date(s: &str) -> String {
    let b = s.as_bytes();
    let shaped = b.len() >= 19
        && b[4] == b':'
        && b[7] == b':'
        && b[10] == b' '
        && b[..4].iter().all(u8::is_ascii_digit);
    if shaped {
        format!("{}-{}-{}{}", &s[..4], &s[5..7], &s[8..10], &s[10..19])
    } else {
        s.to_string()
    }
}

/// Make and model without repeating the make.
pub(crate) fn camera(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(mk), Some(md)) => {
            let first = mk.split_whitespace().next().unwrap_or("").to_lowercase();
            if !first.is_empty() && md.to_lowercase().starts_with(&first) {
                Some(md)
            } else {
                Some(format!("{mk} {md}"))
            }
        }
        (None, Some(m)) | (Some(m), None) => Some(m),
        (None, None) => None,
    }
}

/// `1/250 s`, `2 s`, `1.3 s`.
pub(crate) fn format_shutter(n: u32, d: u32) -> Option<String> {
    if n == 0 || d == 0 {
        return None;
    }
    let secs = f64::from(n) / f64::from(d);
    Some(if secs >= 1.0 {
        format!("{} s", trim_num(secs, 1))
    } else {
        format!("1/{} s", (1.0 / secs).round() as u64)
    })
}

/// `2.80` → `2.8`, `35.0` → `35`.
pub(crate) fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exif::experimental::Writer;
    use exif::{Field, Rational, SRational};

    /// A TIFF block with the given fields (little-endian).
    pub(crate) fn exif_tiff(fields: &[Field]) -> Vec<u8> {
        let mut w = Writer::new();
        for f in fields {
            w.push_field(f);
        }
        let mut out = Cursor::new(Vec::new());
        w.write(&mut out, true).unwrap();
        out.into_inner()
    }

    pub(crate) fn sample_fields() -> Vec<Field> {
        let f = |tag, value| Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        };
        vec![
            f(Tag::Make, Value::Ascii(vec![b"SONY".to_vec()])),
            f(Tag::Model, Value::Ascii(vec![b"ILCE-7M4".to_vec()])),
            f(
                Tag::DateTimeOriginal,
                Value::Ascii(vec![b"2026:11:11 14:03:22".to_vec()]),
            ),
            f(
                Tag::LensModel,
                Value::Ascii(vec![b"FE 35mm F1.4 GM".to_vec()]),
            ),
            f(
                Tag::FocalLength,
                Value::Rational(vec![Rational { num: 35, denom: 1 }]),
            ),
            f(
                Tag::FNumber,
                Value::Rational(vec![Rational { num: 28, denom: 10 }]),
            ),
            f(
                Tag::ExposureTime,
                Value::Rational(vec![Rational { num: 1, denom: 250 }]),
            ),
            f(Tag::PhotographicSensitivity, Value::Short(vec![400])),
            f(
                Tag::ExposureBiasValue,
                Value::SRational(vec![SRational { num: 7, denom: 10 }]),
            ),
            f(Tag::PixelXDimension, Value::Long(vec![7008])),
            f(Tag::PixelYDimension, Value::Long(vec![4672])),
        ]
    }

    /// A JPEG: SOI, APP1 with the EXIF, a SOF0 for 640×480, EOI.
    pub(crate) fn sample_jpeg() -> Vec<u8> {
        let tiff = exif_tiff(&sample_fields());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(&tiff);
        let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
        j.extend(((app1.len() + 2) as u16).to_be_bytes());
        j.extend(app1);
        j.extend([0xff, 0xc0, 0, 11, 8, 0x01, 0xe0, 0x02, 0x80, 1, 1, 0x11, 0]);
        j.extend([0xff, 0xd9]);
        j
    }

    fn check_sample(p: &PhotoInfo) {
        assert_eq!(p.camera.as_deref(), Some("SONY ILCE-7M4"));
        assert_eq!(p.date_taken.as_deref(), Some("2026-11-11 14:03:22"));
        assert_eq!(p.lens.as_deref(), Some("FE 35mm F1.4 GM"));
        assert_eq!(p.focal_length.as_deref(), Some("35 mm"));
        assert_eq!(p.aperture.as_deref(), Some("ƒ/2.8"));
        assert_eq!(p.shutter.as_deref(), Some("1/250 s"));
        assert_eq!(p.iso.as_deref(), Some("ISO 400"));
        assert_eq!(p.exposure_comp.as_deref(), Some("+0.7 EV"));
        assert_eq!(p.dimensions.as_deref(), Some("7008 × 4672"));
    }

    fn run(bytes: &[u8], name: &str) -> (Option<PhotoInfo>, Option<String>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        read(
            &path,
            bytes.len() as u64,
            &PreviewOptions::default(),
            &CancelToken::new(),
        )
    }

    #[test]
    fn jpeg_exif() {
        let (p, note) = run(&sample_jpeg(), "a.jpg");
        assert!(note.is_none());
        check_sample(&p.unwrap());
    }

    #[test]
    fn tiff_based_raw_exif() {
        let (p, _) = run(&exif_tiff(&sample_fields()), "DSC0001.ARW");
        check_sample(&p.unwrap());
    }

    #[test]
    fn cr3_exif_from_cmt_boxes() {
        use crate::iso::testutil::make_box;
        let tiff = exif_tiff(&sample_fields());
        let mut uuid = CANON_UUID.to_vec();
        uuid.extend(make_box(b"CMT1", &tiff));
        let moov = make_box(b"moov", &make_box(b"uuid", &uuid));
        let mut f = make_box(b"ftyp", b"crx \0\0\0\x01crx isom");
        f.extend(moov);
        let (p, _) = run(&f, "IMG_0001.CR3");
        check_sample(&p.unwrap());
    }

    #[test]
    fn jpeg_without_exif_still_gets_pixel_size_from_the_frame_header() {
        let j = vec![
            0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0x01, 0xe0, 0x02, 0x80, 1, 1, 0x11, 0, 0xff, 0xd9,
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.jpg");
        std::fs::write(&path, &j).unwrap();
        let mut f = File::open(&path).unwrap();
        assert_eq!(jpeg_dims(&mut f), Some((640, 480)));
    }

    #[test]
    fn corrupt_and_truncated_files_never_panic() {
        let good = sample_jpeg();
        for cut in [0, 1, 2, 3, 10, 40, good.len() / 2, good.len() - 1] {
            let (_p, _n) = run(&good[..cut], "t.jpg");
        }
        let tiff = exif_tiff(&sample_fields());
        for cut in [0, 4, 8, 20, tiff.len() / 2, tiff.len() - 1] {
            let _ = run(&tiff[..cut], "t.nef");
        }
        let garbage: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 251) as u8).collect();
        for name in ["g.jpg", "g.heic", "g.cr3", "g.dng", "g.raf", "g.png"] {
            let _ = run(&garbage, name);
        }
        let mut lying = b"II*\0".to_vec();
        lying.extend([0xff; 64]);
        let _ = run(&lying, "lying.dng");
        let (p, note) = run(b"", "empty.jpg");
        assert!(p.is_none() && note.is_some());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_shutter(1, 8000).as_deref(), Some("1/8000 s"));
        assert_eq!(format_shutter(13, 10).as_deref(), Some("1.3 s"));
        assert_eq!(format_shutter(2, 1).as_deref(), Some("2 s"));
        assert_eq!(format_shutter(0, 1), None);
        assert_eq!(
            camera(Some("NIKON CORPORATION".into()), Some("NIKON Z 6".into())).as_deref(),
            Some("NIKON Z 6")
        );
        assert_eq!(
            camera(Some("Canon".into()), Some("EOS R5".into())).as_deref(),
            Some("Canon EOS R5")
        );
        assert_eq!(format_date("not a date"), "not a date");
        assert_eq!(trim_num(2.0, 1), "2");
    }
}
