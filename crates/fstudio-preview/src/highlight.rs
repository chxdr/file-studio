//! Syntax spans for the first lines of a text file. Highlighting is done by
//! `syntect`'s grammars only; colours are not chosen here. Each span carries a
//! [`TokenClass`] and the shell maps classes to its light or dark theme.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};

use crate::cancel::CancelToken;
use crate::kind::extension;
use crate::types::Span;

/// What kind of token a span is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenClass {
    /// `fn`, `let`, `if`, storage keywords.
    Keyword,
    /// String and character literals.
    String,
    /// Numbers.
    Number,
    /// Comments and doc comments.
    Comment,
    /// Function and method names.
    Function,
    /// Type and class names.
    Type,
    /// `true`, `nil`, named constants.
    Constant,
    /// Operators and punctuation.
    Punctuation,
    /// Markup tags.
    Tag,
    /// Attribute names and keys.
    Attribute,
    /// Markdown headings and similar.
    Heading,
    /// Variables and parameters worth tinting.
    Variable,
    /// Invalid input.
    Invalid,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

struct Rules(Vec<(Scope, TokenClass)>);

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        use TokenClass::*;
        // Most specific first; the first rule that is a prefix of a scope wins.
        let table: &[(&str, TokenClass)] = &[
            ("comment", Comment),
            ("punctuation.definition.comment", Comment),
            ("string", String),
            ("punctuation.definition.string", String),
            ("constant.numeric", Number),
            ("constant.character.escape", Constant),
            ("constant", Constant),
            ("entity.name.function", Function),
            ("support.function", Function),
            ("meta.function-call", Function),
            ("variable.function", Function),
            ("entity.name.type", Type),
            ("entity.name.class", Type),
            ("entity.name.struct", Type),
            ("entity.name.enum", Type),
            ("entity.name.trait", Type),
            ("entity.name.namespace", Type),
            ("support.type", Type),
            ("support.class", Type),
            ("storage.type", Keyword),
            ("storage.modifier", Keyword),
            ("keyword.operator", Punctuation),
            ("keyword", Keyword),
            ("storage", Keyword),
            ("entity.name.tag", Tag),
            ("entity.other.attribute-name", Attribute),
            ("meta.object-literal.key", Attribute),
            ("support.type.property-name", Attribute),
            ("markup.heading", Heading),
            ("entity.name.section", Heading),
            ("variable.parameter", Variable),
            ("variable.language", Keyword),
            ("invalid", Invalid),
            ("punctuation", Punctuation),
        ];
        Rules(
            table
                .iter()
                .filter_map(|(s, c)| Scope::new(s).ok().map(|s| (s, *c)))
                .collect(),
        )
    })
}

fn class_of(stack: &[Scope]) -> Option<TokenClass> {
    let rules = rules();
    // Innermost scope first. "support.type.property-name" must beat "support.type".
    for sc in stack.iter().rev() {
        if let Some((_, c)) = rules.0.iter().find(|(r, _)| r.is_prefix_of(*sc)) {
            return Some(*c);
        }
    }
    None
}

/// Chooses the grammar for a file name and first line.
fn syntax_for(name: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let ext = extension(name);
    // Grammars the default set lacks borrow the closest relative.
    let alias = match ext.as_str() {
        "ts" | "tsx" | "jsx" | "mjs" | "cjs" => "js",
        "swift" | "kt" | "kts" => "java",
        "toml" | "ini" | "conf" | "cfg" | "env" => "properties",
        "plist" | "svg" | "xmp" | "vue" => "xml",
        "zsh" | "bash" | "fish" => "sh",
        "hpp" | "cc" | "mm" => "cpp",
        "yml" => "yaml",
        "htm" => "html",
        "h" => "c",
        _ => "",
    };
    let by_name = match name {
        "Makefile" | "makefile" => set.find_syntax_by_name("Makefile"),
        "Dockerfile" => set.find_syntax_by_extension("sh"),
        _ => None,
    };
    by_name
        .or_else(|| set.find_syntax_by_extension(&ext))
        .or_else(|| {
            (!alias.is_empty())
                .then(|| set.find_syntax_by_extension(alias))
                .flatten()
        })
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// The language name for a file, if the highlighter knows it.
pub(crate) fn language_name(name: &str, first_line: &str) -> Option<String> {
    syntax_for(name, first_line).map(|s| s.name.clone())
}

/// Spans for each of `lines` (already cut to their display text). `None` when
/// no grammar fits or the work was cancelled; otherwise one `Vec<Span>` per
/// line, with offsets in UTF-16 code units.
pub(crate) fn spans(
    file_name: &str,
    lines: &[String],
    cancel: &CancelToken,
) -> Option<Vec<Vec<Span>>> {
    let first = lines.first().map(String::as_str).unwrap_or("");
    let syntax = syntax_for(file_name, first)?;
    let set = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if cancel.is_cancelled() {
            return None;
        }
        let with_nl = format!("{line}\n");
        // A parser error (a regex limit on a pathological line) leaves the
        // line plain rather than failing the preview.
        let Ok(ops) = state.parse_line(&with_nl, set) else {
            out.push(Vec::new());
            continue;
        };
        out.push(line_spans(&with_nl, line.len(), &ops, &mut stack));
    }
    Some(out)
}

/// Turns one line's scope operations into merged spans.
fn line_spans(
    text: &str,
    visible_len: usize,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut prev = 0usize;
    let mut cur = class_of(stack.as_slice());
    let mut utf16_at = 0u32;
    let mut byte_at = 0usize;
    let mut emit = |from: usize, to: usize, class: Option<TokenClass>, spans: &mut Vec<Span>| {
        let to = to.min(visible_len);
        if from >= to {
            return;
        }
        // Advance the UTF-16 cursor to `from`.
        utf16_at += text[byte_at..from].encode_utf16().count() as u32;
        byte_at = from;
        let len = text[from..to].encode_utf16().count() as u32;
        let start = utf16_at;
        utf16_at += len;
        byte_at = to;
        let Some(class) = class else { return };
        if let Some(last) = spans.last_mut()
            && last.class == class
            && last.start + last.len == start
        {
            last.len += len;
        } else {
            spans.push(Span { start, len, class });
        }
    };
    for (pos, op) in ops {
        let pos = (*pos).min(text.len());
        if pos > prev {
            emit(prev, pos, cur, &mut spans);
            prev = pos;
        }
        // An op that can't apply (unbalanced pop) is ignored.
        if stack.apply(op).is_ok() {
            cur = class_of(stack.as_slice());
        }
    }
    emit(prev, text.len(), cur, &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn rust_gets_keyword_string_comment_and_number_spans() {
        let src = lines("fn main() {\n    let x = \"hé\"; // note\n    let n = 42;\n}");
        let sp = spans("main.rs", &src, &CancelToken::new()).unwrap();
        let has = |line: usize, class: TokenClass| sp[line].iter().any(|s| s.class == class);
        assert!(has(0, TokenClass::Keyword));
        assert!(has(1, TokenClass::String));
        assert!(has(1, TokenClass::Comment));
        assert!(has(2, TokenClass::Number));
        // Spans stay inside the line and don't overlap (UTF-16 units; é is one).
        for (i, l) in src.iter().enumerate() {
            let n = l.encode_utf16().count() as u32;
            let mut end = 0;
            for s in &sp[i] {
                assert!(s.start >= end && s.start + s.len <= n, "line {i}: {s:?}");
                end = s.start + s.len;
            }
        }
    }

    #[test]
    fn grammar_choice() {
        assert_eq!(language_name("a.rs", "").as_deref(), Some("Rust"));
        assert!(language_name("a.swift", "").is_some());
        assert!(language_name("a.ts", "").is_some());
        assert!(language_name("a.toml", "").is_some());
        assert!(language_name("notes.txt", "hello").is_none());
        assert!(spans("notes.txt", &lines("hello"), &CancelToken::new()).is_none());
    }

    #[test]
    fn cancelled_gives_none() {
        let c = CancelToken::new();
        c.cancel();
        assert!(spans("a.rs", &lines("fn x() {}"), &c).is_none());
    }

    #[test]
    fn multi_line_comment_state_carries_over() {
        let src = lines("/* start\nmiddle\nend */ int x;");
        let sp = spans("a.c", &src, &CancelToken::new()).unwrap();
        assert!(sp[1].iter().any(|s| s.class == TokenClass::Comment));
    }
}
