//! The Spotlight source.
//!
//! On macOS it runs the system's `/usr/bin/mdfind` with NUL-separated
//! output. That needs no `unsafe` code, works the same from the app, the CLI
//! and tests, and is cancelled by killing the process. The query text is
//! built here ([`spotlight_query`]) as a pure function. Linux and Windows
//! shells can plug in their own index (`plocate`, Windows Search) through
//! [`IndexSource`].

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::cancel::CancelToken;
use crate::query::{NameMode, SearchQuery};

/// What to ask an index for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexQuery {
    /// Limit to these folders; empty means everywhere.
    pub roots: Vec<PathBuf>,
    /// The Spotlight query text (`kMDItemFSName == "*x*"cd`).
    pub query: String,
}

/// A fast but possibly stale source of candidate paths. Every candidate is
/// re-checked on disk, so an index may over-report freely.
pub trait IndexSource: Send + Sync {
    /// Streams candidates to `out` until done or `cancel` fires; must return
    /// promptly after cancel. An `Err` says why the index couldn't be asked
    /// (the walk still runs).
    fn candidates(
        &self,
        q: &IndexQuery,
        cancel: &CancelToken,
        out: &mut dyn FnMut(PathBuf),
    ) -> Result<(), String>;
}

/// No index: the walk alone.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoIndex;

impl IndexSource for NoIndex {
    fn candidates(
        &self,
        _: &IndexQuery,
        _: &CancelToken,
        _: &mut dyn FnMut(PathBuf),
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Escapes text for a quoted Spotlight string: `\`, `"` and `*`
/// (Spotlight's wildcard) are backslash-escaped.
pub fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '*') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A glob for Spotlight's own wildcard: `*` stays, `?` and sets become `*`
/// (looser; results are re-checked).
fn glob_for_md(glob: &str) -> String {
    let mut out = String::new();
    let mut in_set = false;
    for c in glob.chars() {
        match c {
            '[' if !in_set => {
                in_set = true;
                out.push('*');
            }
            ']' if in_set => in_set = false,
            _ if in_set => {}
            '*' | '?' => out.push('*'),
            c => out.push_str(&escape_md(&c.to_string())),
        }
    }
    // Collapse runs of '*'.
    let mut s = String::new();
    for c in out.chars() {
        if !(c == '*' && s.ends_with('*') && !s.ends_with("\\*")) {
            s.push(c);
        }
    }
    s
}

/// The Spotlight query for `q`, or `None` when the index can't narrow it
/// (no name, tags or content text, or a regular expression with nothing
/// else).
/// `c` and `d` make it case- and diacritic-insensitive, like F-017.
pub fn spotlight_query(q: &SearchQuery) -> Option<String> {
    let mut parts = Vec::new();
    if !q.name.is_empty() {
        match q.name_mode {
            NameMode::Contains => {
                parts.push(format!("kMDItemFSName == \"*{}*\"cd", escape_md(&q.name)))
            }
            NameMode::Glob => {
                parts.push(format!("kMDItemFSName == \"{}\"cd", glob_for_md(&q.name)))
            }
            NameMode::Regex => {}
        }
    }
    for t in q.tags.iter().filter(|t| !t.is_empty()) {
        parts.push(format!("kMDItemUserTags == \"{}\"cd", escape_md(t)));
    }
    if let Some(c) = q.content.as_deref().filter(|c| !c.is_empty()) {
        parts.push(format!("kMDItemTextContent == \"*{}*\"cd", escape_md(c)));
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" && "))
}

/// Spotlight through `/usr/bin/mdfind`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Mdfind;

const MDFIND: &str = "/usr/bin/mdfind";

impl IndexSource for Mdfind {
    fn candidates(
        &self,
        q: &IndexQuery,
        cancel: &CancelToken,
        out: &mut dyn FnMut(PathBuf),
    ) -> Result<(), String> {
        if !std::path::Path::new(MDFIND).exists() {
            return Err("Spotlight isn’t available".into());
        }
        let mut cmd = Command::new(MDFIND);
        cmd.arg("-0");
        for r in &q.roots {
            cmd.arg("-onlyin").arg(r);
        }
        cmd.arg(&q.query)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("no output")?;
        let child = Arc::new(Mutex::new(child));
        let killer = child.clone();
        cancel.on_cancel(move || {
            let _ = killer.lock().kill();
        });
        let mut r = BufReader::with_capacity(64 * 1024, stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(0, &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if cancel.is_cancelled() {
                        break;
                    }
                    if buf.last() == Some(&0) {
                        buf.pop();
                    }
                    if buf.is_empty() {
                        continue;
                    }
                    use std::os::unix::ffi::OsStrExt;
                    out(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)));
                }
                Err(_) => break,
            }
        }
        if cancel.is_cancelled() {
            let _ = child.lock().kill();
        }
        let _ = child.lock().wait();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries() {
        let mut q = SearchQuery::in_folder("/x");
        assert_eq!(spotlight_query(&q), None);
        q.name = "a\"b*c".into();
        assert_eq!(
            spotlight_query(&q).unwrap(),
            r#"kMDItemFSName == "*a\"b\*c*"cd"#
        );
        q.name = "IMG_??[0-9]*.jpg".into();
        q.name_mode = NameMode::Glob;
        assert_eq!(
            spotlight_query(&q).unwrap(),
            r#"kMDItemFSName == "IMG_*.jpg"cd"#
        );
        q.name_mode = NameMode::Regex;
        assert_eq!(spotlight_query(&q), None);
        q.content = Some("hello".into());
        assert_eq!(
            spotlight_query(&q).unwrap(),
            r#"kMDItemTextContent == "*hello*"cd"#
        );
    }

    #[test]
    fn tag_queries() {
        // A sidebar tag click: tags alone narrow the index.
        let mut q = SearchQuery::this_mac();
        q.tags = vec!["Red".into()];
        assert_eq!(
            spotlight_query(&q).unwrap(),
            r#"kMDItemUserTags == "Red"cd"#
        );
        q.tags = vec!["Work \"A\"*".into(), String::new(), "Blue".into()];
        q.name = "x".into();
        assert_eq!(
            spotlight_query(&q).unwrap(),
            r#"kMDItemFSName == "*x*"cd && kMDItemUserTags == "Work \"A\"\*"cd && kMDItemUserTags == "Blue"cd"#
        );
    }

    #[test]
    fn mdfind_on_an_empty_folder_returns_cleanly() {
        let d = tempfile::tempdir().unwrap();
        let mut n = 0;
        let r = Mdfind.candidates(
            &IndexQuery {
                roots: vec![d.path().to_path_buf()],
                query: "kMDItemFSName == \"*zzqq-nothing*\"cd".into(),
            },
            &CancelToken::new(),
            &mut |_| n += 1,
        );
        if std::path::Path::new(MDFIND).exists() {
            assert!(r.is_ok());
        }
        assert_eq!(n, 0);
    }

    #[test]
    fn mdfind_cancel_returns() {
        let t = CancelToken::new();
        t.cancel();
        let start = std::time::Instant::now();
        let _ = Mdfind.candidates(
            &IndexQuery {
                roots: vec![],
                query: "kMDItemFSName == \"*e*\"cd".into(),
            },
            &t,
            &mut |_| {},
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
