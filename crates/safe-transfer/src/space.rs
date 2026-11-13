//! Free-space arithmetic (proposal §10). Pure.

/// The smallest safety margin: 100 MB.
pub const MIN_MARGIN: u64 = 100_000_000;

/// Bytes a file of `len` bytes takes on a disk with `block`-byte allocation
/// units, plus one more unit for an AppleDouble `._` file when `appledouble`.
pub fn on_disk(len: u64, block: u64, appledouble: bool) -> u64 {
    let block = block.max(1);
    len.div_ceil(block) * block + if appledouble { block } else { 0 }
}

/// The safety margin for a job needing `need` bytes: the larger of 100 MB
/// and 1 % of the job.
pub fn margin(need: u64) -> u64 {
    MIN_MARGIN.max(need / 100)
}

/// Whether `need` bytes plus the margin fit in `free`.
pub fn fits(need: u64, free: u64) -> bool {
    need.checked_add(margin(need)).is_some_and(|n| n <= free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_and_appledouble() {
        assert_eq!(on_disk(0, 131_072, false), 0);
        assert_eq!(on_disk(1, 131_072, false), 131_072);
        assert_eq!(on_disk(131_072, 131_072, true), 262_144);
        assert_eq!(on_disk(50_000_000, 4096, false), 50_003_968);
        assert_eq!(on_disk(10, 0, false), 10);
    }

    #[test]
    fn margins() {
        assert_eq!(margin(1_000), MIN_MARGIN);
        assert_eq!(margin(48_000_000_000), 480_000_000);
        assert!(fits(1_000, 200_000_000));
        assert!(!fits(1_000, 100_000_000));
        assert!(!fits(u64::MAX, u64::MAX));
    }
}
