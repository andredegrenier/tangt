//! logos-based lexer. Byte spans on every token; the same token stream feeds
//! the parser and the editor's syntax highlighter.
//!
//! Comments are `//` to end of line (`#` is reserved for hex colors).
//! Dates, durations, and percentages are their own tokens so every task field
//! is lexically distinct — the parser records one byte span per field and the
//! rewrite engine splices exactly that span.

use logos::Logos;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\r]+")]
pub enum Token {
    /// `2026-08-10` — must stay a single token so it never fights `Int`/`Minus`.
    #[regex(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")]
    Date,
    /// `5d`, `2w`
    #[regex(r"[0-9]+[dw]")]
    Duration,
    /// `60%`
    #[regex(r"[0-9]+%")]
    Percent,
    #[regex(r"[0-9]+")]
    Int,
    #[regex(r"[A-Za-z_][A-Za-z0-9_-]*")]
    Ident,
    #[regex(r#""([^"\\\n]|\\.)*""#)]
    Str,
    #[regex(r"#[0-9a-fA-F]+")]
    Color,
    #[regex(r"//[^\n]*", allow_greedy = true)]
    Comment,

    #[token("..")]
    DotDot,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("\n")]
    Newline,
}

/// Keywords, classified from `Ident` in the parser. Kept in one place so the
/// editor highlighter agrees with the parser.
pub const KEYWORDS: &[&str] = &[
    "task",
    "milestone",
    "section",
    "after",
    "until",
    "title",
    "dateformat",
    "axis",
    "today",
    "exclude",
    "weekends",
];

/// Tag keywords (also highlighted as keywords).
pub const TAGS: &[&str] = &["crit", "done", "active"];

pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word) || TAGS.contains(&word)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lexeme {
    pub token: Token,
    pub start: usize,
    pub end: usize,
}

/// Lex the whole input. Unrecognized bytes are skipped; the parser reports
/// errors at the statement level, so lost bytes surface as "unexpected token"
/// on whatever follows. Returns lexemes including comments and newlines; the
/// parser filters as needed, the highlighter uses everything.
pub fn lex(src: &str) -> Vec<Lexeme> {
    let mut out = Vec::new();
    let mut lexer = Token::lexer(src);
    while let Some(result) = lexer.next() {
        let span = lexer.span();
        if let Ok(token) = result {
            out.push(Lexeme { token, start: span.start, end: span.end });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Token> {
        lex(src).into_iter().map(|l| l.token).collect()
    }

    #[test]
    fn task_line() {
        assert_eq!(
            kinds(r#"task "Visual design" : vis, after wf + 2d, 10d, #4fa3ff, 60%"#),
            vec![
                Token::Ident,
                Token::Str,
                Token::Colon,
                Token::Ident,
                Token::Comma,
                Token::Ident,
                Token::Ident,
                Token::Plus,
                Token::Duration,
                Token::Comma,
                Token::Duration,
                Token::Comma,
                Token::Color,
                Token::Comma,
                Token::Percent,
            ]
        );
    }

    #[test]
    fn date_is_one_token() {
        assert_eq!(kinds("2026-08-10"), vec![Token::Date]);
        assert_eq!(
            kinds("exclude 2026-12-24..2027-01-02"),
            vec![Token::Ident, Token::Date, Token::DotDot, Token::Date]
        );
    }

    #[test]
    fn duration_vs_int() {
        assert_eq!(kinds("5d 2w 7"), vec![Token::Duration, Token::Duration, Token::Int]);
    }

    #[test]
    fn colors_vs_comments() {
        assert_eq!(kinds("#ff6b35"), vec![Token::Color]);
        assert_eq!(kinds("// #ff6b35 in a comment"), vec![Token::Comment]);
    }

    #[test]
    fn spans_are_byte_accurate() {
        let ls = lex("wf 2026-01-05 5d");
        assert_eq!((ls[0].start, ls[0].end), (0, 2));
        assert_eq!((ls[1].start, ls[1].end), (3, 13));
        assert_eq!((ls[2].start, ls[2].end), (14, 16));
    }
}
