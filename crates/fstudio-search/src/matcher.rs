//! The compiled filters. [`Matcher::matches_meta`] is pure (name, kind, size,
//! date); [`Matcher::matches`] adds the checks that need I/O (tags, content).

use std::path::Path;

use fstudio_model::{Entry, EntryKind};
use regex::{Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;

use crate::content::content_matches;
use crate::kind::{Kind, kind_of};
use crate::query::{DateField, NameMode, QueryError, SearchQuery};

#[derive(Debug)]
enum NameTest {
    Any,
    /// The folded needle.
    Contains(String),
    /// Matched against the NFC name, case-insensitively.
    Pattern(Regex),
}

/// A query's filters, compiled once and shared by every walker thread.
#[derive(Debug)]
pub struct Matcher {
    name: NameTest,
    kinds: Vec<Kind>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    date_field: DateField,
    from: Option<i64>,
    to: Option<i64>,
    tags: Vec<String>,
    content: Option<String>,
    content_max: u64,
    include_hidden: bool,
}

/// Turns a shell glob into an anchored regular expression: `*` is any run,
/// `?` one character, `[…]` a set (`[!…]` negated); everything else is
/// literal.
pub fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                // A set runs to the next ']' (a ']' first in the set is
                // literal); an unclosed '[' is a literal.
                let mut look = chars.clone();
                let neg = look.peek() == Some(&'!');
                if neg {
                    look.next();
                }
                let (mut set, mut closed) = (String::new(), false);
                for x in look.by_ref() {
                    if x == ']' && !set.is_empty() {
                        closed = true;
                        break;
                    }
                    set.push(x);
                }
                if !closed {
                    out.push_str(r"\[");
                    continue;
                }
                chars = look;
                out.push('[');
                if neg {
                    out.push('^');
                }
                for x in set.chars() {
                    if matches!(x, '\\' | '[' | ']' | '^' | '&' | '~') {
                        out.push('\\');
                    }
                    out.push(x);
                }
                out.push(']');
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push('$');
    out
}

fn nfc(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_ascii() {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(s.nfc().collect())
    }
}

fn pattern(src: &str) -> Result<Regex, QueryError> {
    RegexBuilder::new(&nfc(src))
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| QueryError::BadPattern(e.to_string()))
}

impl Matcher {
    /// Compiles `q`'s filters. Fails on a bad glob or regular expression.
    pub fn new(q: &SearchQuery) -> Result<Matcher, QueryError> {
        let name = if q.name.is_empty() {
            NameTest::Any
        } else {
            match q.name_mode {
                NameMode::Contains => NameTest::Contains(fstudio_view::fold(&q.name)),
                NameMode::Glob => NameTest::Pattern(pattern(&glob_to_regex(&q.name))?),
                NameMode::Regex => NameTest::Pattern(pattern(&q.name)?),
            }
        };
        Ok(Matcher {
            name,
            kinds: q.kinds.clone(),
            min_size: q.min_size,
            max_size: q.max_size,
            date_field: q.date_field,
            from: q.date_from_ns,
            to: q.date_to_ns,
            tags: q.tags.iter().map(|t| fstudio_view::fold(t)).collect(),
            content: q
                .content
                .as_deref()
                .filter(|c| !c.is_empty())
                .map(fstudio_view::fold),
            content_max: q.content_max_bytes,
            include_hidden: q.include_hidden,
        })
    }

    /// Does the name alone match?
    pub fn name_matches(&self, name: &str) -> bool {
        match &self.name {
            NameTest::Any => true,
            NameTest::Contains(n) => fstudio_view::fold(name).contains(n.as_str()),
            NameTest::Pattern(r) => r.is_match(&nfc(name)),
        }
    }

    /// True when hidden items are skipped and `e` is one.
    pub fn hides(&self, e: &Entry) -> bool {
        !self.include_hidden && e.is_hidden()
    }

    /// The checks that need no I/O: hidden, name, kind, size and date.
    pub fn matches_meta(&self, e: &Entry) -> bool {
        if self.hides(e) || !self.name_matches(&e.name) {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&kind_of(e)) {
            return false;
        }
        if self.min_size.is_some() || self.max_size.is_some() {
            if e.kind == EntryKind::Dir {
                return false;
            }
            if self.min_size.is_some_and(|m| e.size < m)
                || self.max_size.is_some_and(|m| e.size > m)
            {
                return false;
            }
        }
        let date = match self.date_field {
            DateField::Modified => e.mtime_ns,
            DateField::Created => e.btime_ns.unwrap_or(e.mtime_ns),
        };
        if self.from.is_some_and(|f| date < f) || self.to.is_some_and(|t| date > t) {
            return false;
        }
        if self.content.is_some() && e.kind != EntryKind::File {
            return false;
        }
        true
    }

    /// True when [`Matcher::matches`] may touch the disk beyond `matches_meta`.
    pub fn needs_io(&self) -> bool {
        !self.tags.is_empty() || self.content.is_some()
    }

    /// Every check. Tags are read from the item's extended attributes;
    /// content is read only for text files within the size cap that are not
    /// iCloud placeholders (F-016).
    pub fn matches(&self, path: &Path, e: &Entry) -> bool {
        if !self.matches_meta(e) {
            return false;
        }
        if !self.tags.is_empty() {
            let Ok(have) = fstudio_fs::finder_tags(path) else {
                return false;
            };
            let have: Vec<String> = have.iter().map(|t| fstudio_view::fold(&t.name)).collect();
            if !self.tags.iter().all(|t| have.contains(t)) {
                return false;
            }
        }
        if let Some(needle) = &self.content
            && !content_matches(path, e, needle, self.content_max)
        {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: 100,
            alloc_size: None,
            mtime_ns: 1_000,
            btime_ns: Some(500),
            mode: 0o644,
            flags: 0,
            inode: 1,
            dev: 1,
            is_package: false,
        }
    }

    fn m(f: impl FnOnce(&mut SearchQuery)) -> Matcher {
        let mut q = SearchQuery::in_folder("/x");
        f(&mut q);
        Matcher::new(&q).unwrap()
    }

    #[test]
    fn contains_is_nfc_and_case_folded() {
        let x = m(|q| q.name = "CAFÉ".into());
        assert!(x.matches_meta(&e("Cafe\u{301} menu.txt"))); // NFD on disk
        assert!(x.matches_meta(&e("my café.txt")));
        assert!(!x.matches_meta(&e("cafe.txt")));
        let nfd = m(|q| q.name = "cafe\u{301}".into());
        assert!(nfd.matches_meta(&e("Caf\u{e9}.txt")));
    }

    #[test]
    fn glob_and_regex() {
        let g = m(|q| {
            q.name = "img_????.JPG".into();
            q.name_mode = NameMode::Glob;
        });
        assert!(g.matches_meta(&e("IMG_0001.jpg")));
        assert!(!g.matches_meta(&e("IMG_01.jpg")));
        assert!(!g.matches_meta(&e("xIMG_0001.jpg")));
        let set = m(|q| {
            q.name = "[!a]*.txt".into();
            q.name_mode = NameMode::Glob;
        });
        assert!(set.matches_meta(&e("b.txt")));
        assert!(!set.matches_meta(&e("a.txt")));
        assert!(!set.matches_meta(&e("A.txt")));
        let lit = m(|q| {
            q.name = "a+b (1).txt".into();
            q.name_mode = NameMode::Glob;
        });
        assert!(lit.matches_meta(&e("A+B (1).txt")));
        let r = m(|q| {
            q.name = r"^dsc\d{4}\.(arw|jpg)$".into();
            q.name_mode = NameMode::Regex;
        });
        assert!(r.matches_meta(&e("DSC0042.ARW")));
        assert!(!r.matches_meta(&e("DSC0042.png")));
        let accent = m(|q| {
            q.name = "^café".into();
            q.name_mode = NameMode::Regex;
        });
        assert!(accent.matches_meta(&e("Cafe\u{301}.txt")));
        let mut q = SearchQuery::in_folder("/x");
        q.name = "(".into();
        q.name_mode = NameMode::Regex;
        assert!(matches!(Matcher::new(&q), Err(QueryError::BadPattern(_))));
    }

    #[test]
    fn glob_translation() {
        assert_eq!(glob_to_regex("*.png"), r"^.*\.png$");
        assert_eq!(glob_to_regex("[ab]?"), "^[ab].$");
        assert_eq!(glob_to_regex("[!x]"), "^[^x]$");
        assert_eq!(glob_to_regex("a["), r"^a\[$");
        assert_eq!(glob_to_regex("[]x]"), r"^[\]x]$");
    }

    #[test]
    fn size_date_hidden_kind() {
        let s = m(|q| {
            q.min_size = Some(50);
            q.max_size = Some(100);
        });
        assert!(s.matches_meta(&e("a")));
        let mut big = e("a");
        big.size = 101;
        assert!(!s.matches_meta(&big));
        let mut dir = e("d");
        dir.kind = EntryKind::Dir;
        assert!(!s.matches_meta(&dir));

        let d = m(|q| {
            q.date_from_ns = Some(900);
            q.date_to_ns = Some(1_000);
        });
        assert!(d.matches_meta(&e("a")));
        let c = m(|q| {
            q.date_field = DateField::Created;
            q.date_from_ns = Some(900);
        });
        assert!(!c.matches_meta(&e("a")));

        let h = m(|_| {});
        assert!(!h.matches_meta(&e(".hidden")));
        let h2 = m(|q| q.include_hidden = true);
        assert!(h2.matches_meta(&e(".hidden")));

        let k = m(|q| q.kinds = vec![Kind::Image, Kind::Folder]);
        assert!(k.matches_meta(&e("a.png")));
        assert!(k.matches_meta(&dir));
        assert!(!k.matches_meta(&e("a.txt")));
    }
}
