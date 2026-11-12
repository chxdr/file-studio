//! How a listing is sorted and filtered.

use crate::collation::fold;

/// What to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// Natural, case- and normalisation-insensitive name order.
    Name,
    /// Logical size (folders count as 0 and fall back to name order).
    Size,
    /// Modification time.
    Modified,
    /// Creation time (modification time when the filesystem has none).
    Created,
    /// Kind: folders, packages, links, then files grouped by extension.
    Kind,
    /// The file extension, then name.
    Extension,
}

/// Everything that decides which rows are shown and in what order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewOptions {
    /// The column sorted by.
    pub sort_key: SortKey,
    /// Sort from smallest/oldest/A to largest/newest/Z when true.
    pub ascending: bool,
    /// Keep folders above files regardless of direction.
    pub folders_first: bool,
    /// Include dot-files and items flagged hidden.
    pub show_hidden: bool,
    /// Keep only names matching this text: a case- and normalisation-insensitive substring,
    /// or a whole-name glob (`*` and `?`) when the text contains a wildcard. Empty keeps everything.
    pub filter: String,
    /// Treat packages (`.app`, …) as files for folders-first and Kind, as Finder does.
    pub packages_as_files: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            sort_key: SortKey::Name,
            ascending: true,
            folders_first: true,
            show_hidden: false,
            filter: String::new(),
            packages_as_files: true,
        }
    }
}

/// A compiled filter.
#[derive(Debug, Clone)]
pub(crate) enum Filter {
    None,
    Substring(String),
    Glob(String),
}

impl Filter {
    pub(crate) fn compile(text: &str) -> Filter {
        if text.is_empty() {
            Filter::None
        } else if text.contains('*') || text.contains('?') {
            Filter::Glob(fold(text))
        } else {
            Filter::Substring(fold(text))
        }
    }

    /// Tests a *folded* name.
    pub(crate) fn matches(&self, folded_name: &str) -> bool {
        match self {
            Filter::None => true,
            Filter::Substring(s) => folded_name.contains(s.as_str()),
            Filter::Glob(g) => glob_match_folded(g, folded_name),
        }
    }
}

/// Matches `text` against a glob made of `*` (any run) and `?` (any one character), over the
/// whole text. Both sides are compared case- and normalisation-insensitively.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_folded(&fold(pattern), &fold(text))
}

pub(crate) fn glob_match_folded(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*.png", "Photo.PNG"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b", "abc"));
        assert!(glob_match("caf\u{e9}*", "Cafe\u{301}.txt"));
    }

    #[test]
    fn filters() {
        assert!(Filter::compile("").matches("x"));
        assert!(Filter::compile("OTO").matches("photo"));
        assert!(!Filter::compile("oto?").matches("photo"));
        assert!(Filter::compile("ph*o").matches("photo"));
    }
}
