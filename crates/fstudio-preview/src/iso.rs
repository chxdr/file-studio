//! A tiny, bounded reader for ISO base media boxes (MP4, MOV, HEIF, CR3).
//! It only ever reads box headers and the small payloads asked for, by
//! seeking, so a 50 GB movie costs a handful of reads.

use std::io::{Read, Seek, SeekFrom};

/// The most boxes listed inside one parent (a corrupt file can't loop forever).
const MAX_CHILDREN: usize = 4096;

/// One box's place in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoxRef {
    /// The four-character type.
    pub kind: [u8; 4],
    /// Offset of the box's first byte.
    pub start: u64,
    /// Offset of the payload (after the size and type, and the 64-bit size).
    pub payload: u64,
    /// Offset just past the box.
    pub end: u64,
}

impl BoxRef {
    /// Payload length in bytes.
    pub fn payload_len(&self) -> u64 {
        self.end - self.payload
    }
}

/// Reads the box header at `pos`, requiring the box to fit before `limit`.
pub(crate) fn read_box<R: Read + Seek>(r: &mut R, pos: u64, limit: u64) -> Option<BoxRef> {
    if pos.checked_add(8)? > limit {
        return None;
    }
    r.seek(SeekFrom::Start(pos)).ok()?;
    let mut h = [0u8; 8];
    r.read_exact(&mut h).ok()?;
    let size32 = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
    let kind = [h[4], h[5], h[6], h[7]];
    let (hdr, size) = match size32 {
        0 => (8, limit - pos),
        1 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).ok()?;
            (16, u64::from_be_bytes(b))
        }
        n => (8, n),
    };
    if size < hdr {
        return None;
    }
    let end = pos.checked_add(size)?;
    if end > limit {
        return None;
    }
    Some(BoxRef {
        kind,
        start: pos,
        payload: pos + hdr,
        end,
    })
}

/// The boxes directly inside `start..end`, skipping `skip` bytes first
/// (for boxes with a fixed header before their children, like `stsd`).
pub(crate) fn children<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> Vec<BoxRef> {
    let mut out = Vec::new();
    let mut pos = start;
    while out.len() < MAX_CHILDREN {
        let Some(b) = read_box(r, pos, end) else {
            break;
        };
        pos = b.end;
        out.push(b);
        if pos >= end {
            break;
        }
    }
    out
}

/// The first child of `parent` with this type.
pub(crate) fn find<R: Read + Seek>(r: &mut R, parent: &BoxRef, kind: &[u8; 4]) -> Option<BoxRef> {
    children(r, parent.payload, parent.end)
        .into_iter()
        .find(|b| &b.kind == kind)
}

/// The first `max` bytes of a box's payload.
pub(crate) fn payload<R: Read + Seek>(r: &mut R, b: &BoxRef, max: u64) -> Option<Vec<u8>> {
    let len = b.payload_len().min(max);
    r.seek(SeekFrom::Start(b.payload)).ok()?;
    let mut v = vec![0u8; usize::try_from(len).ok()?];
    r.read_exact(&mut v).ok()?;
    Some(v)
}

/// Big-endian reads that return `None` instead of panicking.
pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
pub(crate) fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
pub(crate) fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

#[cfg(test)]
pub(crate) mod testutil {
    /// Builds a box: size, type, payload.
    pub fn make_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::make_box;
    use super::*;
    use std::io::Cursor;

    #[test]
    fn walks_children_and_rejects_bad_sizes() {
        let inner = make_box(b"free", &[1, 2, 3]);
        let outer = make_box(b"moov", &inner);
        let mut c = Cursor::new(outer.clone());
        let top = children(&mut c, 0, outer.len() as u64);
        assert_eq!(top.len(), 1);
        assert_eq!(&top[0].kind, b"moov");
        let kids = children(&mut c, top[0].payload, top[0].end);
        assert_eq!(&kids[0].kind, b"free");
        // A box claiming to be bigger than the file is refused.
        let mut bad = outer.clone();
        bad[3] = 0xff;
        let mut c = Cursor::new(bad.clone());
        assert!(children(&mut c, 0, bad.len() as u64).is_empty());
        // A box smaller than its own header is refused (no infinite loop).
        let tiny = [0, 0, 0, 4, b'a', b'b', b'c', b'd'];
        let mut c = Cursor::new(tiny.to_vec());
        assert!(children(&mut c, 0, 8).is_empty());
    }
}
