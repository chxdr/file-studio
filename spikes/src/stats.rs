pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub p95: f64,
}

/// Nearest-rank percentile on a sorted copy.
pub fn summarize(v: &[f64]) -> Summary {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    let rank = |p: f64| s[(((p * n as f64).ceil() as usize).max(1) - 1).min(n - 1)];
    Summary { n, min: s[0], median: rank(0.5), p95: rank(0.95) }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.2} / {:.2}", self.median, self.p95)
    }
}
