//! "Copy Path" in every style (`PathStyle`).

use std::path::Path;

use fstudio_model::PathStyle;

/// Writes `path` in the requested style.
///
/// - `Posix`: as is.
/// - `Escaped`: Terminal style, a backslash before every character the shell
///   would treat specially (`My\ Folder`).
/// - `HomeRelative`: `~` or `~/rest` when the path is the home folder or inside
///   it (compared by whole components), otherwise as is.
/// - `FileUrl`: `file://` plus the percent-encoded path. A trailing slash is kept
///   if the input has one, so pass `/Users/me/Folder/` for a folder.
/// - `Quoted`: single-quoted for a shell, with embedded `'` written as `'\''`.
pub fn copy_path_as(path: &Path, style: PathStyle, home: &Path) -> String {
    let s = path.to_string_lossy();
    match style {
        PathStyle::Posix => s.into_owned(),
        PathStyle::Escaped => escape(&s),
        PathStyle::HomeRelative => home_relative(&s, &home.to_string_lossy()),
        PathStyle::FileUrl => format!("file://{}", percent_encode_path(&s)),
        PathStyle::Quoted => format!("'{}'", s.replace('\'', "'\\''")),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        let safe = c.is_alphanumeric()
            || matches!(c, '/' | '.' | '_' | '-' | ',' | ':' | '+' | '@' | '%' | '=');
        if !safe && c.is_ascii() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn home_relative(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_owned();
    }
    if path == home {
        return "~".to_owned();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// Percent-encodes a path for a `file://` URL: letters, digits and `-._~/` stay,
/// every other byte becomes `%XX`.
pub fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
