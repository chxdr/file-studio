//! Chunk bookkeeping and hashing (proposal §3.6, §4.3). Pure: bytes in,
//! hashes out. A file is cut into chunks of `chunk_size` bytes (the last one
//! may be shorter); each chunk gets an xxh3-64, the whole file an xxh3-128
//! (the same file hash as Import Studio and File Studio's verify).

use xxhash_rust::xxh3::Xxh3;

/// Default chunk size: 64 MiB (decided 2026-11-13).
pub const DEFAULT_CHUNK_SIZE: u64 = 64 << 20;
/// Smallest allowed chunk size.
pub const MIN_CHUNK_SIZE: u64 = 1 << 20;
/// Largest allowed chunk size.
pub const MAX_CHUNK_SIZE: u64 = 256 << 20;

/// The hashes of a whole file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileHashes {
    /// xxh3-64 of each chunk, in order.
    pub chunks: Vec<u64>,
    /// xxh3-128 of the whole content.
    pub file: u128,
    /// Bytes hashed.
    pub len: u64,
}

impl FileHashes {
    /// The file hash as 32 lowercase hex digits.
    pub fn file_hex(&self) -> String {
        format!("{:032x}", self.file)
    }

    /// The index of the first chunk that differs from `other`, if any
    /// (a different chunk count counts as a difference at the shorter end).
    pub fn first_difference(&self, other: &FileHashes) -> Option<usize> {
        let n = self.chunks.len().min(other.chunks.len());
        (0..n)
            .find(|&i| self.chunks[i] != other.chunks[i])
            .or_else(|| (self.chunks.len() != other.chunks.len()).then_some(n))
            .or_else(|| (self.file != other.file || self.len != other.len).then_some(0))
    }
}

/// Number of chunks a file of `len` bytes has (zero-byte files have none).
pub fn chunk_count(len: u64, chunk_size: u64) -> u64 {
    len.div_ceil(chunk_size)
}

/// Feeds bytes in order and cuts them into chunk hashes.
pub struct ChunkHasher {
    chunk_size: u64,
    in_chunk: u64,
    chunk: Xxh3,
    file: Xxh3,
    done: FileHashes,
}

impl ChunkHasher {
    /// A hasher for chunks of `chunk_size` bytes (clamped to the allowed range).
    pub fn new(chunk_size: u64) -> ChunkHasher {
        ChunkHasher {
            chunk_size: chunk_size.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE),
            in_chunk: 0,
            chunk: Xxh3::new(),
            file: Xxh3::new(),
            done: FileHashes::default(),
        }
    }

    /// The chunk size in use.
    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    /// Bytes fed so far.
    pub fn len(&self) -> u64 {
        self.done.len
    }

    /// Whether nothing has been fed yet.
    pub fn is_empty(&self) -> bool {
        self.done.len == 0
    }

    /// Chunks completed so far.
    pub fn chunks_done(&self) -> usize {
        self.done.chunks.len()
    }

    /// Feeds bytes. Returns how many chunks this call completed.
    pub fn update(&mut self, mut bytes: &[u8]) -> usize {
        let before = self.done.chunks.len();
        while !bytes.is_empty() {
            let room = (self.chunk_size - self.in_chunk) as usize;
            let take = room.min(bytes.len());
            let (now, rest) = bytes.split_at(take);
            self.chunk.update(now);
            self.file.update(now);
            self.in_chunk += take as u64;
            self.done.len += take as u64;
            if self.in_chunk == self.chunk_size {
                self.done.chunks.push(self.chunk.digest());
                self.chunk.reset();
                self.in_chunk = 0;
            }
            bytes = rest;
        }
        self.done.chunks.len() - before
    }

    /// Feeds `n` zero bytes (a hole in a sparse file). Returns chunks completed.
    pub fn update_zeros(&mut self, mut n: u64) -> usize {
        static ZEROS: [u8; 1 << 16] = [0; 1 << 16];
        let mut done = 0;
        while n > 0 {
            let k = n.min(ZEROS.len() as u64) as usize;
            done += self.update(&ZEROS[..k]);
            n -= k as u64;
        }
        done
    }

    /// Finishes: the last partial chunk is hashed too.
    pub fn finish(mut self) -> FileHashes {
        if self.in_chunk > 0 {
            self.done.chunks.push(self.chunk.digest());
        }
        self.done.file = self.file.digest128();
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 253) as u8).collect()
    }

    #[test]
    fn chunk_counts_at_the_edges() {
        let c = 4 * MIB;
        for (len, want) in [
            (0, 0),
            (1, 1),
            (c - 1, 1),
            (c, 1),
            (c + 1, 2),
            ((2u64 << 30) + 1, 513),
        ] {
            assert_eq!(chunk_count(len, c), want, "len {len}");
        }
    }

    #[test]
    fn hashes_dont_depend_on_how_the_bytes_are_fed() {
        let d = data(5 * MIB as usize + 123);
        let whole = {
            let mut h = ChunkHasher::new(MIB);
            h.update(&d);
            h.finish()
        };
        for step in [1usize, 4095, 65536, 1 << 20, 3 << 20] {
            let mut h = ChunkHasher::new(MIB);
            for part in d.chunks(step) {
                h.update(part);
            }
            assert_eq!(h.finish(), whole, "step {step}");
        }
        assert_eq!(whole.chunks.len(), 6);
        assert_eq!(whole.len, d.len() as u64);
        assert_eq!(whole.file, xxhash_rust::xxh3::xxh3_128(&d));
        assert_eq!(
            whole.chunks[0],
            xxhash_rust::xxh3::xxh3_64(&d[..MIB as usize])
        );
    }

    #[test]
    fn update_reports_completed_chunks() {
        let mut h = ChunkHasher::new(MIB);
        assert_eq!(h.update(&data(MIB as usize - 1)), 0);
        assert_eq!(h.update(&[1, 2]), 1);
        assert_eq!(h.update_zeros(3 * MIB), 3);
        assert_eq!(h.chunks_done(), 4);
    }

    #[test]
    fn zeros_hash_like_written_zeros() {
        let mut a = ChunkHasher::new(MIB);
        a.update(&vec![0u8; 3 * MIB as usize + 5]);
        let mut b = ChunkHasher::new(MIB);
        b.update_zeros(3 * MIB + 5);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn empty_files_have_no_chunks_but_a_hash() {
        let h = ChunkHasher::new(MIB).finish();
        assert!(h.chunks.is_empty());
        assert_eq!(h.file, xxhash_rust::xxh3::xxh3_128(b""));
    }

    #[test]
    fn first_difference_finds_the_bad_chunk() {
        let d = data(3 * MIB as usize);
        let mut e = d.clone();
        e[MIB as usize + 10] ^= 1;
        let hash = |v: &[u8]| {
            let mut h = ChunkHasher::new(MIB);
            h.update(v);
            h.finish()
        };
        let (a, b) = (hash(&d), hash(&e));
        assert_eq!(a.first_difference(&a), None);
        assert_eq!(a.first_difference(&b), Some(1));
        assert_eq!(a.first_difference(&hash(&d[..MIB as usize])), Some(1));
    }

    #[test]
    fn chunk_size_is_clamped() {
        assert_eq!(ChunkHasher::new(1).chunk_size(), MIN_CHUNK_SIZE);
        assert_eq!(ChunkHasher::new(u64::MAX).chunk_size(), MAX_CHUNK_SIZE);
    }
}
