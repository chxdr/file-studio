//! Turning typed or pasted text into a location (HANDS-ON §8).
//!
//! The pipeline, in order: trim and take the first line; strip one pair of
//! quotes; convert `file://` URLs; unescape Terminal-style backslashes (only if the
//! literal text is not an existing path); expand `~`, `~user`, `$VAR`, `${VAR}`;
//! resolve against the cwd; normalise lexically; then look each component up
//! case- and normalisation-insensitively so the result carries the on-disk spelling.
//!
//! **The disk is touched only after the text has been cleaned** (the one early
//! check, for step 4, runs on the quote-stripped literal text and only when it
//! contains a backslash). A leading quote can therefore never be mistaken for
//! something else, and nothing here can panic on odd input.

use std::path::{Path, PathBuf};

use crate::probe::{FsProbe, ProbeKind, StdProbe};

type EnvFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
type UserHomeFn = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// Everything parsing needs from the outside world.
pub struct ParseContext {
    /// The pane's current folder; relative input resolves against it.
    pub cwd: PathBuf,
    /// The user's home folder (`~`).
    pub home: PathBuf,
    /// Looks up an environment variable (`$VAR`).
    pub env: EnvFn,
    /// Looks up another user's home folder (`~name`).
    pub user_home: UserHomeFn,
    /// The filesystem probe.
    pub probe: Box<dyn FsProbe>,
}

impl ParseContext {
    /// A context with no environment variables and no other users.
    pub fn new(cwd: impl Into<PathBuf>, home: impl Into<PathBuf>, probe: Box<dyn FsProbe>) -> Self {
        Self {
            cwd: cwd.into(),
            home: home.into(),
            env: Box::new(|_| None),
            user_home: Box::new(|_| None),
            probe,
        }
    }

    /// Sets the environment lookup.
    pub fn with_env(mut self, f: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Box::new(f);
        self
    }

    /// Sets the `~user` lookup.
    pub fn with_user_home(
        mut self,
        f: impl Fn(&str) -> Option<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        self.user_home = Box::new(f);
        self
    }

    /// A context using the real environment, `$HOME` and the real disk.
    pub fn system(cwd: impl Into<PathBuf>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        Self::new(cwd, home, Box::new(StdProbe))
            .with_env(|k| std::env::var(k).ok())
            .with_user_home(|u| {
                if u.is_empty() || u.contains('/') || u.contains('\0') {
                    return None;
                }
                let p = PathBuf::from("/Users").join(u);
                p.is_dir().then_some(p)
            })
    }
}

/// Where the text points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A folder (symlinks to folders are reported by the path the user gave).
    Dir(PathBuf),
    /// A file: show `dir` and select `name` in it (never open it).
    File {
        /// The folder containing the file.
        dir: PathBuf,
        /// The file name as spelled on disk.
        name: String,
    },
    /// Nothing there. Offer "Create folder" for `path`; `nearest_existing` is the
    /// deepest folder that does exist.
    Missing {
        /// The full path as it would be created (existing part in on-disk spelling).
        path: PathBuf,
        /// The deepest existing folder above it.
        nearest_existing: PathBuf,
    },
    /// A remote URL (`sftp://`, `smb://`, `s3://`). Parsed only; not supported yet.
    Remote(String),
}

/// A cleanup that was applied, so the bar can show a "cleaned up paste" chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cleanup {
    /// Several lines were pasted; the first non-empty one was used.
    FirstLineOnly,
    /// NUL or other control characters were removed.
    RemovedControlChars,
    /// Surrounding quotes were removed.
    RemovedQuotes,
    /// A `file://` URL was converted to a path.
    FileUrl,
    /// Backslash escapes (`\ `, `\(`...) were removed.
    Unescaped,
    /// `~` or `~user` was expanded.
    ExpandedHome,
    /// `$VAR` or `${VAR}` was expanded.
    ExpandedVars,
    /// A relative path was resolved against the current folder.
    ResolvedRelative,
    /// `.`, `..` or repeated slashes were collapsed.
    Normalised,
    /// The path exists with different case or Unicode normalisation; the on-disk
    /// spelling is shown.
    OnDiskSpelling,
}

impl Cleanup {
    /// True for cleanups that changed what the user pasted in a way worth telling
    /// them about (the "Cleaned up paste: removed quotes" chip). Resolving
    /// relative paths, `~` and variables are ordinary typing, not paste cleanup.
    pub fn is_paste_cleanup(self) -> bool {
        matches!(
            self,
            Cleanup::FirstLineOnly
                | Cleanup::RemovedControlChars
                | Cleanup::RemovedQuotes
                | Cleanup::FileUrl
                | Cleanup::Unescaped
        )
    }

    /// Short human text for the chip, e.g. `removed quotes`.
    pub fn describe(self) -> &'static str {
        match self {
            Cleanup::FirstLineOnly => "used the first line",
            Cleanup::RemovedControlChars => "removed control characters",
            Cleanup::RemovedQuotes => "removed quotes",
            Cleanup::FileUrl => "converted file:// URL",
            Cleanup::Unescaped => "removed backslash escapes",
            Cleanup::ExpandedHome => "expanded ~",
            Cleanup::ExpandedVars => "expanded variables",
            Cleanup::ResolvedRelative => "resolved relative path",
            Cleanup::Normalised => "simplified . and ..",
            Cleanup::OnDiskSpelling => "used on-disk spelling",
        }
    }
}

/// The result of [`parse_location`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLocation {
    /// What the text points at.
    pub location: Location,
    /// The cleanups applied, in pipeline order, without repeats.
    pub cleanups: Vec<Cleanup>,
    /// The cleaned text (an absolute path, or the URL for remote locations)
    /// before the on-disk spelling was applied.
    pub cleaned: String,
}

impl ParsedLocation {
    /// The text for the "Cleaned up paste: ..." chip, or `None` if nothing worth
    /// showing happened.
    pub fn chip_text(&self) -> Option<String> {
        let parts: Vec<&str> = self
            .cleanups
            .iter()
            .filter(|c| c.is_paste_cleanup())
            .map(|c| c.describe())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(format!("Cleaned up paste: {}", parts.join(", ")))
        }
    }
}

const REMOTE_SCHEMES: &[&str] = &["sftp://", "smb://", "s3://"];
/// Private-use marker for a `$` or `~` the user escaped with a backslash.
const LITERAL_DOLLAR: char = '\u{E000}';
const LITERAL_TILDE: char = '\u{E001}';

/// Parses what the user typed or pasted. See the module docs for the pipeline.
pub fn parse_location(raw: &str, ctx: &ParseContext) -> ParsedLocation {
    let mut cleanups: Vec<Cleanup> = Vec::new();
    fn note(c: Cleanup, v: &mut Vec<Cleanup>) {
        if !v.contains(&c) {
            v.push(c);
        }
    }

    // 1. trim; first non-empty line.
    let mut text = raw.trim();
    if text.contains(['\n', '\r']) {
        text = text
            .split(['\n', '\r'])
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        note(Cleanup::FirstLineOnly, &mut cleanups);
    }
    let mut text: String = text.to_owned();
    if text
        .chars()
        .any(|c| c == '\0' || (c.is_control() && c != '\t'))
    {
        text.retain(|c| !(c == '\0' || (c.is_control() && c != '\t')));
        note(Cleanup::RemovedControlChars, &mut cleanups);
    }

    // 2. one pair of matching quotes.
    if let Some(inner) = strip_quotes(&text) {
        text = inner.to_owned();
        note(Cleanup::RemovedQuotes, &mut cleanups);
    }

    // Remote URLs are recognised after quote removal, and never probed.
    let lower = text.to_ascii_lowercase();
    if REMOTE_SCHEMES.iter().any(|s| lower.starts_with(s)) {
        return ParsedLocation {
            location: Location::Remote(text.clone()),
            cleanups,
            cleaned: text,
        };
    }

    // 3. file:// URLs.
    let mut from_url = false;
    match file_url_to_path(&text) {
        UrlResult::NotAUrl => {}
        UrlResult::Path(p) => {
            text = p;
            from_url = true;
            note(Cleanup::FileUrl, &mut cleanups);
        }
        UrlResult::OtherHost => {
            return ParsedLocation {
                location: Location::Remote(text.clone()),
                cleanups,
                cleaned: text,
            };
        }
    }

    if !from_url {
        // 4. backslash escapes, only when the literal text is not an existing path.
        if text.contains('\\') && !literal_exists(&text, ctx) {
            let un = unescape(&text);
            if un != text {
                text = un;
                note(Cleanup::Unescaped, &mut cleanups);
            }
        }
        // 5. ~, ~user, $VAR, ${VAR}.
        let (t, home, vars) = expand(&text, ctx);
        text = t;
        if home {
            note(Cleanup::ExpandedHome, &mut cleanups);
        }
        if vars {
            note(Cleanup::ExpandedVars, &mut cleanups);
        }
        text = text
            .replace(LITERAL_DOLLAR, "$")
            .replace(LITERAL_TILDE, "~");
    }

    // 6. relative to cwd.
    let cwd = ctx.cwd.to_string_lossy();
    let absolute = if text.starts_with('/') {
        text.clone()
    } else if text.is_empty() {
        cwd.to_string()
    } else {
        note(Cleanup::ResolvedRelative, &mut cleanups);
        format!("{}/{}", cwd.trim_end_matches('/'), text)
    };

    // 7. lexical normalisation.
    let normalised = normalise(&absolute);
    if normalised != absolute.trim_end_matches('/') && absolute != "/" {
        note(Cleanup::Normalised, &mut cleanups);
    }

    // On-disk spelling and kind.
    let resolved = resolve(&normalised, ctx.probe.as_ref());
    if resolved.respelled {
        note(Cleanup::OnDiskSpelling, &mut cleanups);
    }
    let location = resolved.into_location();
    ParsedLocation {
        location,
        cleanups,
        cleaned: normalised,
    }
}

fn strip_quotes(s: &str) -> Option<&str> {
    let mut it = s.chars();
    let first = it.next()?;
    let last = it.next_back()?;
    let pair = matches!(
        (first, last),
        ('"', '"') | ('\'', '\'') | ('\u{201C}', '\u{201D}') | ('\u{2018}', '\u{2019}')
    );
    pair.then(|| &s[first.len_utf8()..s.len() - last.len_utf8()])
}

enum UrlResult {
    NotAUrl,
    Path(String),
    OtherHost,
}

fn file_url_to_path(text: &str) -> UrlResult {
    let Some(head) = text.get(..5) else {
        return UrlResult::NotAUrl;
    };
    if !head.eq_ignore_ascii_case("file:") {
        return UrlResult::NotAUrl;
    }
    let rest = &text[5..];
    let path_part = if let Some(after) = rest.strip_prefix("//") {
        if after.starts_with('/') {
            after
        } else {
            let end = after.find('/').unwrap_or(after.len());
            let host = &after[..end];
            if host.eq_ignore_ascii_case("localhost") {
                if end == after.len() {
                    "/"
                } else {
                    &after[end..]
                }
            } else {
                return UrlResult::OtherHost;
            }
        }
    } else if rest.starts_with('/') {
        rest
    } else {
        return UrlResult::NotAUrl;
    };
    UrlResult::Path(percent_decode(path_part))
}

/// Decodes `%XX` escapes. Malformed escapes are kept as written; bytes that are not
/// valid UTF-8 become U+FFFD.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Does the quote-stripped text, taken literally, name an existing path?
fn literal_exists(text: &str, ctx: &ParseContext) -> bool {
    let abs = if text.starts_with('/') {
        text.to_owned()
    } else {
        format!(
            "{}/{}",
            ctx.cwd.to_string_lossy().trim_end_matches('/'),
            text
        )
    };
    let r = resolve(&normalise(&abs), ctx.probe.as_ref());
    r.missing.is_empty()
}

/// Removes shell-style backslash escapes. `\$` and `\~` become private markers so
/// they survive expansion as literal characters. A trailing lone backslash stays.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('$') => out.push(LITERAL_DOLLAR),
                Some('~') => out.push(LITERAL_TILDE),
                Some(n) => out.push(n),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Expands `~`, `~user`, `$VAR`, `${VAR}`. Unknown names are left as written.
/// Returns (text, home_expanded, vars_expanded).
pub(crate) fn expand(s: &str, ctx: &ParseContext) -> (String, bool, bool) {
    let mut home_done = false;
    let mut vars_done = false;
    let mut text = s.to_owned();
    if text.starts_with('~') {
        let end = text.find('/').unwrap_or(text.len());
        let user = &text[1..end];
        let base = if user.is_empty() {
            Some(ctx.home.clone())
        } else {
            (ctx.user_home)(user)
        };
        if let Some(b) = base {
            let b = b.to_string_lossy().trim_end_matches('/').to_owned();
            text = format!("{}{}", b, &text[end..]);
            if text.is_empty() {
                text.push('/');
            }
            home_done = true;
        }
    }
    if text.contains('$') {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '$' {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            // `${NAME}`
            if chars.get(i + 1) == Some(&'{')
                && let Some(close) = chars[i + 2..].iter().position(|&c| c == '}')
            {
                let name: String = chars[i + 2..i + 2 + close].iter().collect();
                if valid_var(&name)
                    && let Some(v) = (ctx.env)(&name)
                {
                    out.push_str(&v);
                    vars_done = true;
                    i += close + 3;
                    continue;
                }
            }
            // `$NAME`
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            if valid_var(&name)
                && let Some(v) = (ctx.env)(&name)
            {
                out.push_str(&v);
                vars_done = true;
                i = j;
                continue;
            }
            out.push('$');
            i += 1;
        }
        text = out;
    }
    (text, home_done, vars_done)
}

fn valid_var(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Collapses `.`, `..` and repeated slashes without touching the disk. `..` never
/// climbs above `/`. The result is absolute, with no trailing slash (except `/`).
pub fn normalise(abs: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in abs.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        "/".to_owned()
    } else {
        let mut s = String::with_capacity(abs.len());
        for p in parts {
            s.push('/');
            s.push_str(p);
        }
        s
    }
}

struct Resolved {
    /// The existing part, in on-disk spelling.
    existing: PathBuf,
    /// The kind of `existing`.
    kind: ProbeKind,
    /// Components that were not found.
    missing: Vec<String>,
    /// Some component's on-disk spelling differs from what was typed.
    respelled: bool,
}

impl Resolved {
    fn into_location(self) -> Location {
        if self.missing.is_empty() {
            return match self.kind {
                ProbeKind::Dir => Location::Dir(self.existing),
                ProbeKind::File => {
                    let name = self
                        .existing
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let dir = self
                        .existing
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("/"));
                    Location::File { dir, name }
                }
            };
        }
        let mut path = self.existing.clone();
        for m in &self.missing {
            path.push(m);
        }
        let nearest = if self.kind == ProbeKind::Dir {
            self.existing
        } else {
            self.existing
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"))
        };
        Location::Missing {
            path,
            nearest_existing: nearest,
        }
    }
}

/// Walks the components of an absolute, normalised path, correcting the spelling of
/// each existing one. Stops at the first component that does not exist, or that is
/// below a file.
fn resolve(path: &str, probe: &dyn FsProbe) -> Resolved {
    let mut existing = PathBuf::from("/");
    let mut kind = ProbeKind::Dir;
    let mut respelled = false;
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let mut idx = 0;
    while idx < comps.len() {
        if kind == ProbeKind::File {
            break;
        }
        match probe.lookup(&existing, comps[idx]) {
            Some((actual, k)) => {
                if actual != comps[idx] {
                    respelled = true;
                }
                existing.push(&actual);
                kind = k;
                idx += 1;
            }
            None => break,
        }
    }
    Resolved {
        existing,
        kind,
        missing: comps[idx..].iter().map(|s| (*s).to_owned()).collect(),
        respelled,
    }
}
