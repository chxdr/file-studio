//! Fuzz-style: random and nasty strings must never panic, against both a fake and
//! the real filesystem (a temp folder).

use fstudio_path::testing::MemFs;
use fstudio_path::{
    Completer, CompleterConfig, Frecency, FrecencyConfig, ParseContext, StdDirSource, StdProbe,
    parse_location,
};
use std::sync::Arc;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

const PIECES: &[&str] = &[
    "/",
    "//",
    "~",
    "~~",
    "~me",
    "$",
    "${",
    "}",
    "$HOME",
    "${HOME}",
    "..",
    ".",
    "\\",
    "\\ ",
    "\"",
    "'",
    "\u{201C}",
    "file://",
    "file:///",
    "file://localhost/",
    "file:",
    "%",
    "%2",
    "%zz",
    "%00",
    "%FF",
    "%E2%82",
    "%20",
    "sftp://",
    "smb://",
    "s3://",
    "\0",
    "\n",
    "\r\n",
    "\t",
    " ",
    "a",
    "Z",
    "é",
    "e\u{301}",
    "ß",
    "İ",
    "日本",
    "🙂",
    "\u{200B}",
    "\u{202E}",
    "Pictures",
    "Users",
    "me",
    "x y",
    "\u{FFFF}",
    "\u{10FFFF}",
];

fn random_text(rng: &mut Rng) -> String {
    let n = rng.next() % 12;
    (0..n).map(|_| *rng.pick(PIECES)).collect()
}

#[test]
fn parse_never_panics_on_random_strings() {
    let fake = ParseContext::new(
        "/Users/me",
        "/Users/me",
        Box::new(MemFs::new().dir("/Users/me/Pictures")),
    )
    .with_env(|k| (k == "HOME").then(|| "/Users/me".to_owned()));
    let real = ParseContext::system(std::env::temp_dir());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let s = random_text(&mut rng);
        let _ = parse_location(&s, &fake);
        let _ = parse_location(&s, &real);
    }
}

#[test]
fn parse_never_panics_on_random_bytes() {
    let ctx = ParseContext::new("/", "/", Box::new(MemFs::new()));
    let mut rng = Rng(12345);
    for _ in 0..10_000 {
        let len = (rng.next() % 64) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let s = String::from_utf8_lossy(&bytes);
        let _ = parse_location(&s, &ctx);
    }
}

#[test]
fn nasty_fixed_inputs() {
    let ctx = ParseContext::system("/tmp");
    for s in [
        "",
        " ",
        "\n",
        "\0",
        "~",
        "~~",
        "\"",
        "'",
        "\"\"",
        "''",
        "\\",
        "file://",
        "file:///%",
        "file://%00",
        "file:///\u{0}",
        "file://localhost%2Fx",
        &"a".repeat(4096),
        &format!("file:///{}", "%41".repeat(2000)),
        "/Volumes/DoesNotExist/x",
        "$",
        "${",
        "${}",
        "$1",
        "$$",
        "~/",
        "~/..",
        "/..",
        "/./",
        "////",
    ] {
        let _ = parse_location(s, &ctx);
    }
}

#[test]
fn completion_never_panics() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Alpha/Beta")).unwrap();
    std::fs::create_dir_all(dir.path().join("Caf\u{e9}")).unwrap();
    let ctx = ParseContext::new(dir.path(), dir.path(), Box::new(StdProbe));
    let f = Frecency::in_memory(FrecencyConfig::default());
    f.visit(&dir.path().join("Alpha/Beta"));
    let c = Completer::new(Arc::new(StdDirSource::new()), f, CompleterConfig::default());
    let mut rng = Rng(777);
    for _ in 0..5_000 {
        let s = random_text(&mut rng);
        let cur = (rng.next() % (s.len() as u64 + 3)) as usize;
        let _ = c.complete(&s, cur, &ctx);
    }
}
