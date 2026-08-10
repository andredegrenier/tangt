//! Hand-written recursive descent parser with statement-level error recovery:
//! a parse error inside one line emits a diagnostic and skips to the next
//! line, so a half-typed statement never blanks the rest of the chart.
//!
//! The grammar is line-oriented: one statement per line, `//` comments.
//! Task fields are lexically distinct (date / `after` clause / duration /
//! `until` / percent / color / tag), so they may appear in any order and each
//! gets a precise byte span for the rewrite engine.

use crate::ast::*;
use crate::lexer::{lex, Lexeme, Token};
use smol_str::SmolStr;

pub fn parse(src: &str) -> Ast {
    Parser::new(src).parse_file()
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Lexeme>,
    pos: usize,
    /// End byte of the most recently consumed non-comment token.
    last_code_end: usize,
    diags: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser { src, toks: lex(src), pos: 0, last_code_end: 0, diags: Vec::new() }
    }

    // ---- token helpers -------------------------------------------------

    /// Next lexeme on this line, transparently skipping comments.
    fn peek(&self) -> Option<Lexeme> {
        self.toks[self.pos..].iter().copied().find(|l| l.token != Token::Comment)
    }

    fn bump(&mut self) -> Option<Lexeme> {
        while let Some(l) = self.toks.get(self.pos).copied() {
            self.pos += 1;
            if l.token != Token::Comment {
                if l.token != Token::Newline {
                    self.last_code_end = l.end;
                }
                return Some(l);
            }
        }
        None
    }

    fn text(&self, l: Lexeme) -> &'a str {
        &self.src[l.start..l.end]
    }

    fn span_of(&self, l: Lexeme) -> Span {
        Span::new(l.start, l.end)
    }

    fn expect(&mut self, tok: Token, what: &str) -> Result<Lexeme, ()> {
        match self.peek() {
            Some(l) if l.token == tok => {
                self.bump();
                Ok(l)
            }
            found => {
                let (span, got) = match found {
                    Some(l) => (self.span_of(l), self.text(l).to_string()),
                    None => (Span::new(self.src.len(), self.src.len()), "end of file".into()),
                };
                self.diags.push(Diagnostic::error(
                    span,
                    format!("expected {what}, found `{got}`"),
                ));
                Err(())
            }
        }
    }

    fn at_line_end(&self) -> bool {
        matches!(self.peek().map(|l| l.token), None | Some(Token::Newline))
    }

    fn skip_separators(&mut self) {
        while let Some(l) = self.toks.get(self.pos).copied() {
            match l.token {
                Token::Newline | Token::Comment => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Error recovery: skip past the rest of the current line.
    fn sync(&mut self) {
        while let Some(l) = self.toks.get(self.pos).copied() {
            self.pos += 1;
            if l.token == Token::Newline {
                return;
            }
        }
    }

    // ---- entry ---------------------------------------------------------

    fn parse_file(mut self) -> Ast {
        let mut stmts = Vec::new();
        loop {
            self.skip_separators();
            let Some(first) = self.peek() else { break };
            let word = if first.token == Token::Ident { self.text(first) } else { "" };
            let result = match word {
                "title" => self.parse_title(first),
                "dateformat" => self.parse_dateformat(first),
                "axis" => self.parse_axis(first),
                "today" => self.parse_today(first),
                "exclude" => self.parse_exclude(first),
                "section" => self.parse_section(first),
                "task" => self.parse_task(first, false),
                "milestone" => self.parse_task(first, true),
                _ => {
                    self.diags.push(Diagnostic::error(
                        self.span_of(first),
                        format!(
                            "expected a statement (task, milestone, section, title, axis, today, exclude), found `{}`",
                            self.text(first)
                        ),
                    ));
                    Err(())
                }
            };
            match result {
                Ok(stmt) => {
                    if !self.at_line_end() {
                        if let Some(l) = self.peek() {
                            self.diags.push(Diagnostic::error(
                                self.span_of(l),
                                format!("unexpected `{}` after statement", self.text(l)),
                            ));
                        }
                        self.sync();
                    }
                    stmts.push(stmt);
                }
                Err(()) => self.sync(),
            }
        }
        Ast { stmts, diagnostics: self.diags }
    }

    // ---- statements ----------------------------------------------------

    fn parse_title(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        let s = self.expect(Token::Str, "a quoted title")?;
        Ok(Stmt::Title {
            value: unquote(self.text(s)),
            span: self.span_of(s),
            stmt_span: Span::new(kw.start, self.last_code_end),
        })
    }

    fn parse_dateformat(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        let w = self.expect(Token::Ident, "a date format name (iso)")?;
        let value = self.text(w);
        if value != "iso" {
            self.diags.push(Diagnostic::warning(
                self.span_of(w),
                format!("unknown date format `{value}` — only `iso` (YYYY-MM-DD) is supported"),
            ));
        }
        Ok(Stmt::DateFormat {
            value: SmolStr::new(value),
            span: self.span_of(w),
            stmt_span: Span::new(kw.start, self.last_code_end),
        })
    }

    fn parse_axis(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        let w = self.expect(Token::Ident, "an axis density (auto, day, week, month)")?;
        let value = match self.text(w) {
            "auto" => AxisKind::Auto,
            "day" => AxisKind::Day,
            "week" => AxisKind::Week,
            "month" => AxisKind::Month,
            other => {
                self.diags.push(Diagnostic::error(
                    self.span_of(w),
                    format!("unknown axis density `{other}` (expected auto, day, week, month)"),
                ));
                AxisKind::Auto
            }
        };
        Ok(Stmt::Axis {
            value,
            span: self.span_of(w),
            stmt_span: Span::new(kw.start, self.last_code_end),
        })
    }

    fn parse_today(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        let d = self.expect(Token::Date, "a date (YYYY-MM-DD)")?;
        Ok(Stmt::Today {
            date: self.date_lit(d),
            stmt_span: Span::new(kw.start, self.last_code_end),
        })
    }

    fn parse_exclude(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        let kind = match self.peek() {
            Some(l) if l.token == Token::Ident && self.text(l) == "weekends" => {
                self.bump();
                ExcludeKind::Weekends
            }
            Some(l) if l.token == Token::Date => {
                self.bump();
                let first = self.date_lit(l);
                if matches!(self.peek().map(|l| l.token), Some(Token::DotDot)) {
                    self.bump();
                    let second = self.expect(Token::Date, "the end date of the range")?;
                    ExcludeKind::Range(first, self.date_lit(second))
                } else {
                    ExcludeKind::Date(first)
                }
            }
            found => {
                let span = found.map_or(Span::new(self.src.len(), self.src.len()), |l| {
                    self.span_of(l)
                });
                self.diags.push(Diagnostic::error(
                    span,
                    "expected `weekends`, a date, or a date range after `exclude`",
                ));
                return Err(());
            }
        };
        Ok(Stmt::Exclude { kind, stmt_span: Span::new(kw.start, self.last_code_end) })
    }

    fn parse_section(&mut self, kw: Lexeme) -> Result<Stmt, ()> {
        self.bump();
        // Either one quoted string or bare words to end of line.
        if let Some(l) = self.peek() {
            if l.token == Token::Str {
                self.bump();
                return Ok(Stmt::Section {
                    label: unquote(self.text(l)),
                    label_span: self.span_of(l),
                    stmt_span: Span::new(kw.start, self.last_code_end),
                });
            }
        }
        let mut first: Option<Lexeme> = None;
        let mut last: Option<Lexeme> = None;
        while let Some(l) = self.peek() {
            if l.token == Token::Newline {
                break;
            }
            self.bump();
            first.get_or_insert(l);
            last = Some(l);
        }
        let (Some(first), Some(last)) = (first, last) else {
            self.diags.push(Diagnostic::error(
                Span::new(kw.start, kw.end),
                "section needs a name",
            ));
            return Err(());
        };
        let label_span = Span::new(first.start, last.end);
        Ok(Stmt::Section {
            label: self.src[first.start..last.end].to_string(),
            label_span,
            stmt_span: Span::new(kw.start, self.last_code_end),
        })
    }

    fn parse_task(&mut self, kw: Lexeme, milestone: bool) -> Result<Stmt, ()> {
        self.bump();
        let label_tok = self.expect(Token::Str, "a quoted label")?;
        self.expect(Token::Colon, "`:`")?;
        let id_tok = self.expect(Token::Ident, "a task id")?;
        let id_text = self.text(id_tok);
        if crate::lexer::is_keyword(id_text) {
            self.diags.push(Diagnostic::error(
                self.span_of(id_tok),
                format!("`{id_text}` is a reserved word and cannot be a task id"),
            ));
        }

        let mut task = TaskStmt {
            milestone,
            label: unquote(self.text(label_tok)),
            label_span: self.span_of(label_tok),
            id: SmolStr::new(id_text),
            id_span: self.span_of(id_tok),
            start: None,
            dur: None,
            progress: None,
            color: None,
            tags: Vec::new(),
            stmt_span: Span::new(kw.start, 0),
            insert_field_at: 0,
        };

        while matches!(self.peek().map(|l| l.token), Some(Token::Comma)) {
            self.bump();
            self.parse_field(&mut task)?;
        }

        if milestone {
            if let Some(dur) = &task.dur {
                self.diags.push(Diagnostic::warning(
                    dur.span(),
                    "milestones have no duration — this field is ignored",
                ));
            }
            if let Some((_, span)) = &task.progress {
                self.diags.push(Diagnostic::warning(
                    *span,
                    "milestones have no progress — this field is ignored",
                ));
            }
        }

        task.stmt_span = Span::new(kw.start, self.last_code_end);
        task.insert_field_at = self.last_code_end;
        Ok(Stmt::Task(task))
    }

    fn parse_field(&mut self, task: &mut TaskStmt) -> Result<(), ()> {
        let Some(l) = self.peek() else {
            self.diags.push(Diagnostic::error(
                Span::new(self.src.len(), self.src.len()),
                "expected a task field after `,`",
            ));
            return Err(());
        };
        match l.token {
            Token::Date => {
                self.bump();
                let lit = self.date_lit(l);
                self.set_start(task, StartField::Date(lit), self.span_of(l));
            }
            Token::Ident if self.text(l) == "after" => {
                let clause = self.parse_after(l)?;
                let span = clause.span;
                self.set_start(task, StartField::After(clause), span);
            }
            Token::Ident if self.text(l) == "until" => {
                self.bump();
                let d = self.expect(Token::Date, "a date after `until`")?;
                self.set_dur(task, DurField::Until(self.date_lit(d)), self.span_of(d));
            }
            Token::Duration => {
                self.bump();
                let (n, unit) = parse_duration(self.text(l));
                self.set_dur(
                    task,
                    DurField::Duration { n, unit, span: self.span_of(l) },
                    self.span_of(l),
                );
            }
            Token::Percent => {
                self.bump();
                let digits = &self.text(l)[..self.text(l).len() - 1];
                let value: u64 = digits.parse().unwrap_or(0);
                if value > 100 {
                    self.diags.push(Diagnostic::warning(
                        self.span_of(l),
                        "progress above 100% is clamped",
                    ));
                }
                if task.progress.is_some() {
                    self.diags.push(Diagnostic::error(
                        self.span_of(l),
                        "duplicate progress field",
                    ));
                }
                task.progress = Some((value.min(100) as u8, self.span_of(l)));
            }
            Token::Color => {
                let text = self.text(l);
                let digits = text.len() - 1;
                if digits != 6 && digits != 3 {
                    self.diags.push(Diagnostic::error(
                        self.span_of(l),
                        "colors are `#rgb` or `#rrggbb`",
                    ));
                }
                if task.color.is_some() {
                    self.diags.push(Diagnostic::error(self.span_of(l), "duplicate color field"));
                }
                task.color = Some((SmolStr::new(text), self.span_of(l)));
                self.bump();
            }
            Token::Ident => {
                let word = self.text(l);
                match Tag::from_word(word) {
                    Some(tag) => {
                        task.tags.push((tag, self.span_of(l)));
                        self.bump();
                    }
                    None => {
                        self.diags.push(Diagnostic::error(
                            self.span_of(l),
                            format!(
                                "unknown task field `{word}` (expected a date, `after`, a duration, `until`, `N%`, `#color`, or a tag)"
                            ),
                        ));
                        return Err(());
                    }
                }
            }
            _ => {
                self.diags.push(Diagnostic::error(
                    self.span_of(l),
                    format!("unexpected `{}` in task fields", self.text(l)),
                ));
                return Err(());
            }
        }
        Ok(())
    }

    /// `after id+ [(+|-) Nd]` — caller has peeked the `after` keyword.
    fn parse_after(&mut self, kw: Lexeme) -> Result<AfterClause, ()> {
        self.bump();
        let mut ids = Vec::new();
        while let Some(l) = self.peek() {
            if l.token == Token::Ident && !crate::lexer::is_keyword(self.text(l)) {
                self.bump();
                ids.push((SmolStr::new(self.text(l)), self.span_of(l)));
            } else {
                break;
            }
        }
        if ids.is_empty() {
            self.diags.push(Diagnostic::error(
                Span::new(kw.start, kw.end),
                "`after` needs at least one task id",
            ));
            return Err(());
        }
        let ids_end = ids.last().unwrap().1.end;
        let mut lag = None;
        if let Some(op) = self.peek() {
            if matches!(op.token, Token::Plus | Token::Minus) {
                self.bump();
                let d = self.expect(Token::Duration, "a lag duration like `2d`")?;
                let (n, unit) = parse_duration(self.text(d));
                lag = Some(LagClause {
                    sign: if op.token == Token::Plus { 1 } else { -1 },
                    n,
                    unit,
                    span: Span::new(op.start, d.end),
                });
            }
        }
        let end = lag.as_ref().map_or(ids_end, |l| l.span.end);
        Ok(AfterClause { ids, lag, span: Span::new(kw.start, end), ids_end })
    }

    // ---- small helpers -------------------------------------------------

    fn set_start(&mut self, task: &mut TaskStmt, field: StartField, span: Span) {
        if task.start.is_some() {
            self.diags.push(Diagnostic::error(
                span,
                "task already has a start (date or `after`)",
            ));
        } else {
            task.start = Some(field);
        }
    }

    fn set_dur(&mut self, task: &mut TaskStmt, field: DurField, span: Span) {
        if task.dur.is_some() {
            self.diags.push(Diagnostic::error(
                span,
                "task already has a duration or `until` end",
            ));
        } else {
            task.dur = Some(field);
        }
    }

    fn date_lit(&self, l: Lexeme) -> DateLit {
        DateLit { text: SmolStr::new(self.text(l)), span: self.span_of(l) }
    }
}

fn parse_duration(text: &str) -> (i64, Unit) {
    let unit = match text.as_bytes()[text.len() - 1] {
        b'w' => Unit::Weeks,
        _ => Unit::Days,
    };
    let n: i64 = text[..text.len() - 1].parse().unwrap_or(0);
    (n, unit)
}

/// Longest label we keep. The renderer measures and lays out labels several
/// times per task per frame, so an absurdly long one is a per-frame cost.
const MAX_LABEL_CHARS: usize = 512;

fn unquote(raw: &str) -> String {
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if out.chars().count() >= MAX_LABEL_CHARS {
            break;
        }
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Quote and escape a label for writing back into source text.
pub fn quote(label: &str) -> String {
    let mut out = String::with_capacity(label.len() + 2);
    out.push('"');
    for c in label.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Ast {
        let ast = parse(src);
        assert!(
            ast.diagnostics.iter().all(|d| d.severity != Severity::Error),
            "unexpected errors: {:?}",
            ast.diagnostics
        );
        ast
    }

    fn task(ast: &Ast, i: usize) -> &TaskStmt {
        let tasks: Vec<_> = ast
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Task(t) => Some(t),
                _ => None,
            })
            .collect();
        tasks[i]
    }

    #[test]
    fn full_task_line() {
        let src = r##"task "Visual design" : vis, after wf be + 2d, 10d, #4fa3ff, 60%, crit"##;
        let ast = parse_ok(src);
        let t = task(&ast, 0);
        assert_eq!(t.label, "Visual design");
        assert_eq!(t.id, "vis");
        let StartField::After(after) = t.start.as_ref().unwrap() else {
            panic!("expected after clause")
        };
        assert_eq!(after.ids.len(), 2);
        assert_eq!(after.ids[0].0, "wf");
        assert_eq!(after.ids[1].0, "be");
        let lag = after.lag.as_ref().unwrap();
        assert_eq!((lag.sign, lag.n, lag.unit), (1, 2, Unit::Days));
        let DurField::Duration { n, unit, .. } = t.dur.as_ref().unwrap() else {
            panic!("expected duration")
        };
        assert_eq!((*n, *unit), (10, Unit::Days));
        assert_eq!(t.progress.unwrap().0, 60);
        assert_eq!(t.color.as_ref().unwrap().0, "#4fa3ff");
        assert_eq!(t.tags.len(), 1);
    }

    #[test]
    fn spans_are_byte_accurate() {
        let src = r##"task "A" : a, 2026-01-05, 5d"##;
        let ast = parse_ok(src);
        let t = task(&ast, 0);
        assert_eq!(&src[t.label_span.start..t.label_span.end], r#""A""#);
        let start_span = t.start.as_ref().unwrap().span();
        assert_eq!(&src[start_span.start..start_span.end], "2026-01-05");
        let dur_span = t.dur.as_ref().unwrap().span();
        assert_eq!(&src[dur_span.start..dur_span.end], "5d");
        assert_eq!(t.insert_field_at, src.len());
    }

    #[test]
    fn insert_field_at_precedes_trailing_comment() {
        let src = "task \"A\" : a, 5d   // note\n";
        let ast = parse_ok(src);
        let t = task(&ast, 0);
        assert_eq!(t.insert_field_at, src.find("   //").unwrap());
    }

    #[test]
    fn broken_line_keeps_rest_of_file() {
        let src = "task \"A\" : a, zzz\ntask \"B\" : b, 3d\n";
        let ast = parse(src);
        assert!(ast.diagnostics.iter().any(|d| d.severity == Severity::Error));
        let tasks: Vec<_> = ast
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Task(t) => Some(t.id.clone()),
                _ => None,
            })
            .collect();
        assert!(tasks.contains(&SmolStr::new("b")));
    }

    #[test]
    fn directives_and_sections() {
        let src = "title \"Plan\"\naxis week\ntoday 2026-08-10\nexclude weekends\nexclude 2026-12-24..2027-01-02\nsection Design work\n";
        let ast = parse_ok(src);
        assert_eq!(ast.stmts.len(), 6);
        match &ast.stmts[5] {
            Stmt::Section { label, .. } => assert_eq!(label, "Design work"),
            other => panic!("expected section, got {other:?}"),
        }
    }

    #[test]
    fn milestone_duration_warns() {
        let src = "milestone \"Ship\" : ship, 2026-09-01, 5d\n";
        let ast = parse(src);
        assert!(ast
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("milestone")));
    }

    #[test]
    fn quote_roundtrip() {
        assert_eq!(quote(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
    }
}
