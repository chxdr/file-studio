//! Token templates: `{name}_{n:3}`, `{exif.date:%Y%m%d}-{exif.camera}`.
//!
//! | Token | Value |
//! |---|---|
//! | `{name}` | the name so far, without its extension |
//! | `{orig}` | the original name, without its extension |
//! | `{ext}` | the original extension (no dot) |
//! | `{parent}` | the enclosing folder's name |
//! | `{n}` `{n:3}` `{n:3:10}` `{n:3:10:5}` | a counter: padding, start, step (defaults 1, 1, 1) |
//! | `{date}` `{date:%Y-%m-%d}` | the modification date (default `%Y-%m-%d`) |
//! | `{created}` `{created:…}` | the creation date |
//! | `{exif.date}` `{exif.date:…}` | when the photo was taken |
//! | `{exif.camera}` `{exif.lens}` | camera and lens |
//! | `{{` `}}` | a literal brace |
//!
//! Values put into a name never contain `/` or `:` (they become `-`).

use crate::date::DateParts;
use crate::item::{Exif, ItemInfo};

/// Which file date a date token reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateSource {
    /// The modification date.
    Modified,
    /// The creation date.
    Created,
    /// EXIF's capture date.
    Exif,
}

/// One piece of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Text copied as it is.
    Lit(String),
    /// `{name}`.
    Name,
    /// `{orig}`.
    Orig,
    /// `{ext}`.
    Ext,
    /// `{parent}`.
    Parent,
    /// `{n:pad:start:step}`.
    Counter {
        /// Minimum digits.
        pad: u32,
        /// The first item's number.
        start: i64,
        /// Added for each next item.
        step: i64,
    },
    /// A date token and its format.
    Date(DateSource, String),
    /// `{exif.camera}`.
    Camera,
    /// `{exif.lens}`.
    Lens,
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Template {
    tokens: Vec<Token>,
}

/// Everything a template can read for one item.
pub struct RenderCtx<'a> {
    /// The item.
    pub item: &'a ItemInfo,
    /// The item's original name, split.
    pub orig: &'a crate::name::Parts,
    /// The name so far (without extension).
    pub stem: &'a str,
    /// The item's position in the batch, from 0.
    pub index: usize,
}

/// Warnings a render can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderNotes {
    /// An `{exif.*}` token had no value for this item.
    pub exif_missing: bool,
    /// An `{exif.*}` token was skipped because the item is an iCloud placeholder.
    pub placeholder: bool,
    /// EXIF isn't read yet.
    pub exif_pending: bool,
}

impl RenderNotes {
    /// Combines two sets of notes.
    pub fn merge(&mut self, o: RenderNotes) {
        self.exif_missing |= o.exif_missing;
        self.placeholder |= o.placeholder;
        self.exif_pending |= o.exif_pending;
    }
}

const DEFAULT_DATE: &str = "%Y-%m-%d";

/// Formats a sequence number with at least `pad` digits (`-` before a negative number).
pub fn counter(n: i64, pad: u32) -> String {
    let digits = format!("{:0width$}", n.unsigned_abs(), width = pad as usize);
    if n < 0 { format!("-{digits}") } else { digits }
}

/// Replaces characters that can't be in a name.
pub fn clean_value(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| if c == '/' || c == ':' { '-' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

fn parse_token(body: &str) -> Result<Token, String> {
    let (head, arg) = match body.split_once(':') {
        Some((h, a)) => (h, Some(a)),
        None => (body, None),
    };
    let no_arg = |t: Token| match arg {
        None => Ok(t),
        Some(_) => Err(format!("“{{{head}}}” takes no options")),
    };
    let fmt = |a: Option<&str>| a.unwrap_or(DEFAULT_DATE).to_string();
    match head.trim() {
        "name" => no_arg(Token::Name),
        "orig" => no_arg(Token::Orig),
        "ext" => no_arg(Token::Ext),
        "parent" => no_arg(Token::Parent),
        "exif.camera" => no_arg(Token::Camera),
        "exif.lens" => no_arg(Token::Lens),
        "date" => Ok(Token::Date(DateSource::Modified, fmt(arg))),
        "created" => Ok(Token::Date(DateSource::Created, fmt(arg))),
        "exif.date" => Ok(Token::Date(DateSource::Exif, fmt(arg))),
        "n" => {
            let parts: Vec<&str> = arg.map(|a| a.split(':').collect()).unwrap_or_default();
            if parts.len() > 3 {
                return Err("“{n}” takes at most padding, start and step: {n:3:1:1}".into());
            }
            let num = |i: usize, def: i64| -> Result<i64, String> {
                match parts.get(i).map(|s| s.trim()) {
                    None | Some("") => Ok(def),
                    Some(s) => s
                        .parse::<i64>()
                        .map_err(|_| format!("“{s}” in {{n:…}} isn't a number")),
                }
            };
            let pad = num(0, 1)?;
            if !(0..=12).contains(&pad) {
                return Err("{n} padding must be 0 to 12".into());
            }
            Ok(Token::Counter {
                pad: pad as u32,
                start: num(1, 1)?,
                step: num(2, 1)?,
            })
        }
        other => Err(format!("unknown token “{{{other}}}”")),
    }
}

impl Template {
    /// Parses a template. `Err` says what is wrong, in words.
    pub fn parse(s: &str) -> Result<Template, String> {
        let mut tokens = Vec::new();
        let mut lit = String::new();
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '{' if chars.peek().map(|p| p.1) == Some('{') => {
                    chars.next();
                    lit.push('{');
                }
                '}' if chars.peek().map(|p| p.1) == Some('}') => {
                    chars.next();
                    lit.push('}');
                }
                '{' => {
                    let rest = &s[i + 1..];
                    let Some(end) = rest.find('}') else {
                        return Err("a “{” has no matching “}”".into());
                    };
                    let body = &rest[..end];
                    if body.contains('{') {
                        return Err("a “{” has no matching “}”".into());
                    }
                    if !lit.is_empty() {
                        tokens.push(Token::Lit(std::mem::take(&mut lit)));
                    }
                    tokens.push(parse_token(body)?);
                    // Skip the token's characters.
                    let stop = i + 1 + end;
                    while chars.peek().is_some_and(|p| p.0 <= stop) {
                        chars.next();
                    }
                }
                '}' => return Err("a “}” has no matching “{” (write “}}” for a brace)".into()),
                c => lit.push(c),
            }
        }
        if !lit.is_empty() {
            tokens.push(Token::Lit(lit));
        }
        Ok(Template { tokens })
    }

    /// The parsed pieces.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// Whether any token needs EXIF.
    pub fn uses_exif(&self) -> bool {
        self.tokens.iter().any(|t| {
            matches!(
                t,
                Token::Camera | Token::Lens | Token::Date(DateSource::Exif, _)
            )
        })
    }

    /// Whether the template is plain text with no tokens.
    pub fn is_literal(&self) -> bool {
        self.tokens.iter().all(|t| matches!(t, Token::Lit(_)))
    }

    /// Renders the template for one item.
    pub fn render(&self, cx: &RenderCtx<'_>, notes: &mut RenderNotes) -> String {
        let mut out = String::new();
        for t in &self.tokens {
            match t {
                Token::Lit(s) => out.push_str(s),
                Token::Name => out.push_str(cx.stem),
                Token::Orig => out.push_str(&cx.orig.stem),
                Token::Ext => out.push_str(&cx.orig.ext),
                Token::Parent => out.push_str(&clean_value(&cx.item.parent_name)),
                Token::Counter { pad, start, step } => {
                    let n = start.saturating_add(step.saturating_mul(cx.index as i64));
                    out.push_str(&counter(n, *pad));
                }
                Token::Date(src, fmt) => {
                    let d: Option<DateParts> = match src {
                        DateSource::Modified => cx.item.modified,
                        DateSource::Created => cx.item.created,
                        DateSource::Exif => exif_value(cx.item, notes, |i| i.date),
                    };
                    if let Some(d) = d {
                        out.push_str(&clean_value(&d.format(fmt)));
                    }
                }
                Token::Camera => {
                    if let Some(v) = exif_value(cx.item, notes, |i| i.camera.clone()) {
                        out.push_str(&clean_value(&v));
                    }
                }
                Token::Lens => {
                    if let Some(v) = exif_value(cx.item, notes, |i| i.lens.clone()) {
                        out.push_str(&clean_value(&v));
                    }
                }
            }
        }
        out
    }
}

fn exif_value<T>(
    item: &ItemInfo,
    notes: &mut RenderNotes,
    get: impl Fn(&crate::item::ExifInfo) -> Option<T>,
) -> Option<T> {
    match &item.exif {
        Exif::Loaded(i) => {
            let v = get(i);
            if v.is_none() {
                notes.exif_missing = true;
            }
            v
        }
        Exif::Missing => {
            notes.exif_missing = true;
            None
        }
        Exif::Placeholder => {
            notes.placeholder = true;
            None
        }
        Exif::NotLoaded => {
            notes.exif_pending = true;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ExifInfo;
    use crate::name::split;

    fn render(t: &str, item: &ItemInfo, index: usize) -> (String, RenderNotes) {
        let orig = split(&item.name, item.is_dir);
        let tpl = Template::parse(t).unwrap();
        let mut notes = RenderNotes::default();
        let s = tpl.render(
            &RenderCtx {
                item,
                orig: &orig,
                stem: &orig.stem,
                index,
            },
            &mut notes,
        );
        (s, notes)
    }

    #[test]
    fn basic_tokens() {
        let mut it = ItemInfo::named("IMG_0001.JPG");
        it.parent_name = "Trip: Day 1".into();
        assert_eq!(
            render("{parent}-{name}.{ext}", &it, 0).0,
            "Trip- Day 1-IMG_0001.JPG"
        );
        assert_eq!(render("{{x}} {orig}", &it, 0).0, "{x} IMG_0001");
    }

    #[test]
    fn counters() {
        let it = ItemInfo::named("a.txt");
        assert_eq!(render("{n}", &it, 0).0, "1");
        assert_eq!(render("{n:3}", &it, 4).0, "005");
        assert_eq!(render("{n:2:10:5}", &it, 2).0, "20");
        assert_eq!(render("{n:0:-1}", &it, 0).0, "-1");
        assert_eq!(render("{n::0}", &it, 0).0, "0");
    }

    #[test]
    fn dates() {
        let mut it = ItemInfo::named("a.txt");
        it.modified = DateParts::parse_exif("2024:01:02 03:04:05");
        it.created = DateParts::parse_exif("2020:05:06 07:08:09");
        assert_eq!(render("{date}", &it, 0).0, "2024-01-02");
        assert_eq!(render("{date:%H:%M}", &it, 0).0, "03-04");
        assert_eq!(render("{created:%Y}", &it, 0).0, "2020");
    }

    #[test]
    fn exif_tokens() {
        let mut it = ItemInfo::named("a.jpg");
        it.exif = Exif::Loaded(ExifInfo {
            date: DateParts::parse_exif("2021:07:08 09:10:11"),
            camera: Some("X100V".into()),
            lens: None,
        });
        let (s, n) = render("{exif.date:%Y%m%d}_{exif.camera}{exif.lens}", &it, 0);
        assert_eq!(s, "20210708_X100V");
        assert!(n.exif_missing);
        it.exif = Exif::Placeholder;
        let (s, n) = render("{exif.camera}x", &it, 0);
        assert_eq!(s, "x");
        assert!(n.placeholder);
        it.exif = Exif::NotLoaded;
        assert!(render("{exif.lens}", &it, 0).1.exif_pending);
    }

    #[test]
    fn errors() {
        assert!(Template::parse("{nope}").is_err());
        assert!(Template::parse("{name").is_err());
        assert!(Template::parse("name}").is_err());
        assert!(Template::parse("{name:x}").is_err());
        assert!(Template::parse("{n:a}").is_err());
        assert!(Template::parse("{n:99}").is_err());
        assert!(Template::parse("{exif.date}").unwrap().uses_exif());
        assert!(!Template::parse("{date}").unwrap().uses_exif());
        assert!(Template::parse("plain").unwrap().is_literal());
    }
}
