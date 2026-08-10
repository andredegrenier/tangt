//! Raw syntax tree produced by the parser. Every field carries byte spans
//! into the source text so the rewrite engine can splice surgically.

use smol_str::SmolStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset <= self.end
    }
    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Diagnostic { span, message: message.into(), severity: Severity::Error }
    }
    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Diagnostic { span, message: message.into(), severity: Severity::Warning }
    }
}

/// An unvalidated date literal (`2026-08-10`); resolve turns it into a day
/// number and diagnoses impossible dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateLit {
    pub text: SmolStr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Days,
    Weeks,
}

impl Unit {
    pub fn days(self, n: i64) -> i64 {
        match self {
            Unit::Days => n,
            Unit::Weeks => n * 7,
        }
    }
}

/// `+ 2d` / `- 1w` following an `after` clause. `span` covers operator through
/// duration so the whole clause can be replaced or removed in one splice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LagClause {
    pub sign: i64,
    pub n: i64,
    pub unit: Unit,
    pub span: Span,
}

/// `after wf be [+ 2d]`. `span` covers the whole clause; `ids_end` is the byte
/// just past the last id (where a new id or lag gets appended).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfterClause {
    pub ids: Vec<(SmolStr, Span)>,
    pub lag: Option<LagClause>,
    pub span: Span,
    pub ids_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartField {
    Date(DateLit),
    After(AfterClause),
}

impl StartField {
    pub fn span(&self) -> Span {
        match self {
            StartField::Date(d) => d.span,
            StartField::After(a) => a.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurField {
    Duration { n: i64, unit: Unit, span: Span },
    Until(DateLit),
}

impl DurField {
    pub fn span(&self) -> Span {
        match self {
            DurField::Duration { span, .. } => *span,
            DurField::Until(d) => d.span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Crit,
    Done,
    Active,
}

impl Tag {
    pub fn from_word(word: &str) -> Option<Tag> {
        match word {
            "crit" => Some(Tag::Crit),
            "done" => Some(Tag::Done),
            "active" => Some(Tag::Active),
            _ => None,
        }
    }
}

/// `task "Label" : id, fields…` or `milestone "Label" : id, start`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskStmt {
    pub milestone: bool,
    pub label: String,
    pub label_span: Span,
    pub id: SmolStr,
    pub id_span: Span,
    pub start: Option<StartField>,
    pub dur: Option<DurField>,
    pub progress: Option<(u8, Span)>,
    /// Raw `#rrggbb` text including the hash.
    pub color: Option<(SmolStr, Span)>,
    pub tags: Vec<(Tag, Span)>,
    /// Keyword through last code token (excludes trailing comment/newline).
    pub stmt_span: Span,
    /// Byte offset where a new `, field` is inserted (after the last code
    /// token, before any trailing comment) — analog of gridflow's
    /// `insert_placement_at`.
    pub insert_field_at: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExcludeKind {
    Weekends,
    Date(DateLit),
    Range(DateLit, DateLit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisKind {
    #[default]
    Auto,
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // few statements per file; Box would ripple
pub enum Stmt {
    Title { value: String, span: Span, stmt_span: Span },
    DateFormat { value: SmolStr, span: Span, stmt_span: Span },
    Axis { value: AxisKind, span: Span, stmt_span: Span },
    Today { date: DateLit, stmt_span: Span },
    Exclude { kind: ExcludeKind, stmt_span: Span },
    Section { label: String, label_span: Span, stmt_span: Span },
    Task(TaskStmt),
}

impl Stmt {
    pub fn stmt_span(&self) -> Span {
        match self {
            Stmt::Title { stmt_span, .. }
            | Stmt::DateFormat { stmt_span, .. }
            | Stmt::Axis { stmt_span, .. }
            | Stmt::Today { stmt_span, .. }
            | Stmt::Exclude { stmt_span, .. }
            | Stmt::Section { stmt_span, .. } => *stmt_span,
            Stmt::Task(t) => t.stmt_span,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Ast {
    pub stmts: Vec<Stmt>,
    pub diagnostics: Vec<Diagnostic>,
}
