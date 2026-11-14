//! Reads the EXIF values the `{exif.*}` tokens use, with `kamadak-exif`
//! (BSD-2-Clause).
//!
//! iCloud placeholders (`SF_DATALESS`) are never opened, so reading EXIF
//! never downloads a file (F-016): they come back as
//! [`Exif::Placeholder`]. TIFF-based files (TIFF, DNG and most raws) are
//! read only up to their first 4 MiB, where their EXIF lives, instead of
//! whole.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use exif::{In, Reader, Tag, Value};

use crate::date::DateParts;
use crate::item::{Exif, ExifInfo};

/// `SF_DATALESS`: the file's content is in the cloud, not on disk.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// How much of a TIFF-based file is read.
const TIFF_HEAD: u64 = 4 << 20;

/// Extensions worth opening. Anything else is [`Exif::Missing`] without a read.
const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "tif", "tiff", "heic", "heif", "hif", "avif", "png", "webp", "dng",
    "cr2", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "pef", "srw", "erf", "3fr", "iiq",
    "mos", "raw",
];

/// Whether a file is an iCloud placeholder (its `st_flags` has `SF_DATALESS`).
/// `lstat` never downloads.
pub fn is_placeholder(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.st_flags() & SF_DATALESS != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// Reads one file's EXIF. Never reads a placeholder, a folder or a
/// non-image (by extension).
pub fn read(path: &Path) -> Exif {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return Exif::Missing;
    }
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return Exif::Missing;
    };
    if !md.is_file() {
        return Exif::Missing;
    }
    if is_placeholder(path) {
        return Exif::Placeholder;
    }
    match read_exif(path) {
        Some(e) => {
            let info = info_of(&e);
            if info == ExifInfo::default() {
                Exif::Missing
            } else {
                Exif::Loaded(info)
            }
        }
        None => Exif::Missing,
    }
}

fn read_exif(path: &Path) -> Option<exif::Exif> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 4];
    f.read_exact(&mut head).ok()?;
    let reader = {
        let mut r = Reader::new();
        r.continue_on_error(true);
        r
    };
    let ok = |r: Result<exif::Exif, exif::Error>| match r {
        Ok(e) => Some(e),
        Err(exif::Error::PartialResult(p)) => Some(p.into_inner().0),
        Err(_) => None,
    };
    let tiff = head == *b"II*\0" || head == *b"MM\0*";
    if tiff {
        // kamadak-exif reads a whole TIFF into memory; a raw's EXIF is near
        // its start, so read only the head.
        let mut buf = head.to_vec();
        f.take(TIFF_HEAD).read_to_end(&mut buf).ok()?;
        return ok(reader.read_raw(buf));
    }
    let f = File::open(path).ok()?;
    ok(reader.read_from_container(&mut BufReader::new(f)))
}

fn ascii(e: &exif::Exif, tag: Tag) -> Option<String> {
    let f = e.get_field(tag, In::PRIMARY)?;
    match &f.value {
        Value::Ascii(v) => {
            let s: String = v
                .iter()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            let s = s
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

/// The camera's name: the model, with the make in front unless the model
/// already starts with it ("Canon EOS R5", "NIKON Z 9", "FUJIFILM X100V").
pub fn camera_name(make: Option<&str>, model: Option<&str>) -> Option<String> {
    match (make.map(str::trim), model.map(str::trim)) {
        (_, Some(m)) if !m.is_empty() => {
            let first = make
                .and_then(|mk| mk.split_whitespace().next())
                .unwrap_or("");
            if first.is_empty() || m.to_lowercase().starts_with(&first.to_lowercase()) {
                Some(m.to_string())
            } else {
                Some(format!("{} {m}", make.unwrap_or("").trim()))
            }
        }
        (Some(mk), _) if !mk.is_empty() => Some(mk.to_string()),
        _ => None,
    }
}

fn info_of(e: &exif::Exif) -> ExifInfo {
    let date = [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime]
        .into_iter()
        .find_map(|t| ascii(e, t).and_then(|s| DateParts::parse_exif(&s)));
    let make = ascii(e, Tag::Make);
    let model = ascii(e, Tag::Model);
    ExifInfo {
        date,
        camera: camera_name(make.as_deref(), model.as_deref()),
        lens: ascii(e, Tag::LensModel),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal little-endian TIFF with Make, Model, DateTime and an EXIF
    /// IFD holding DateTimeOriginal and LensModel.
    pub fn tiff_with_exif(make: &str, model: &str, date: &str, lens: &str) -> Vec<u8> {
        fn z(s: &str) -> Vec<u8> {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        }
        let (make, model, date, lens) = (z(make), z(model), z(date), z(lens));
        // Layout: header(8) | IFD0 (2 + 4*12 + 4) | EXIF IFD (2 + 2*12 + 4) | strings
        let ifd0 = 8u32;
        let ifd0_len = 2 + 4 * 12 + 4;
        let exif_ifd = ifd0 + ifd0_len;
        let exif_len = 2 + 2 * 12 + 4;
        let mut data_off = exif_ifd + exif_len;
        let mut strings = Vec::new();
        let mut place = |s: &Vec<u8>| {
            let off = data_off;
            strings.extend_from_slice(s);
            data_off += s.len() as u32;
            off
        };
        let (o_make, o_model, o_date, o_date2, o_lens) = (
            place(&make),
            place(&model),
            place(&date),
            place(&date),
            place(&lens),
        );
        let mut b = Vec::new();
        b.extend_from_slice(b"II*\0");
        b.extend_from_slice(&ifd0.to_le_bytes());
        let entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val: u32| {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&val.to_le_bytes());
        };
        b.extend_from_slice(&4u16.to_le_bytes());
        entry(&mut b, 0x010F, 2, make.len() as u32, o_make);
        entry(&mut b, 0x0110, 2, model.len() as u32, o_model);
        entry(&mut b, 0x0132, 2, date.len() as u32, o_date);
        entry(&mut b, 0x8769, 4, 1, exif_ifd);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut b, 0x9003, 2, date.len() as u32, o_date2);
        entry(&mut b, 0xA434, 2, lens.len() as u32, o_lens);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&strings);
        b
    }

    #[test]
    fn reads_a_tiff() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("shot.dng");
        std::fs::write(
            &p,
            tiff_with_exif(
                "Canon",
                "Canon EOS R5",
                "2024:06:01 14:03:22",
                "RF24-70mm F2.8",
            ),
        )
        .unwrap();
        let Exif::Loaded(i) = read(&p) else {
            panic!("no exif: {:?}", read(&p));
        };
        assert_eq!(i.camera.as_deref(), Some("Canon EOS R5"));
        assert_eq!(i.lens.as_deref(), Some("RF24-70mm F2.8"));
        assert_eq!(i.date.unwrap().format("%F %H%M%S"), "2024-06-01 140322");
    }

    #[test]
    fn non_images_and_garbage_are_missing() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("notes.txt");
        std::fs::write(&t, "hello").unwrap();
        assert_eq!(read(&t), Exif::Missing);
        let j = d.path().join("broken.jpg");
        std::fs::write(&j, b"\xff\xd8 not really").unwrap();
        assert_eq!(read(&j), Exif::Missing);
        assert_eq!(read(&d.path().join("gone.jpg")), Exif::Missing);
        assert_eq!(read(d.path()), Exif::Missing);
    }

    #[test]
    fn camera_names() {
        assert_eq!(
            camera_name(Some("NIKON CORPORATION"), Some("NIKON Z 9")).as_deref(),
            Some("NIKON Z 9")
        );
        assert_eq!(
            camera_name(Some("FUJIFILM"), Some("X100V")).as_deref(),
            Some("FUJIFILM X100V")
        );
        assert_eq!(camera_name(Some("Apple"), None).as_deref(), Some("Apple"));
        assert_eq!(camera_name(None, None), None);
    }
}
