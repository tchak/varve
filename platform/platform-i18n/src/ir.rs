//! The compiled message IR — what [`compile`](crate::MessageTemplate::compile)
//! lowers the `ox_mf2_parser` CST into, and all that
//! [`format`](crate::MessageTemplate::format) ever walks.
//!
//! Q8's note "lower the CST to a small IR per message, don't re-walk
//! per format" is this module. Text runs are already unescaped, names
//! are already NFC-normalized (Q8: normalize before env lookup), and
//! declarations are an ordered list the formatter evaluates exactly
//! once, top to bottom (Q8: evaluate declarations once, in order).
//! Nothing borrows the source or the CST: a compiled template is
//! self-contained, `'static`, and shareable across threads.
//!
//! Invalid states are unrepresentable where the spike used runtime
//! checks: an [`Expr`] is either an operand (with an optional
//! annotation) or a bare function — never neither — and a
//! [`Body::Match`] carries the index of its all-`*` fallback variant,
//! whose existence compilation guarantees, so selection is total
//! without a panic path.

/// A whole message: declarations in source order, then the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ir {
    pub(crate) decls: Vec<Decl>,
    pub(crate) body: Body,
}

/// One `.input` or `.local` declaration. Both lower to the same
/// shape: a (NFC-normalized) name bound to an expression. `.input
/// {$x :number}` binds `x` to the annotated variable expression
/// itself; when the formatter evaluates it the environment does not
/// yet contain `x`, so the inner `$x` reads the caller's argument —
/// the spec's self-reference, without the spike's remove-self
/// recursion workaround.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decl {
    pub(crate) name: String,
    pub(crate) expr: Expr,
}

/// The message body: a plain pattern, or a `.match`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Body {
    Pattern(Pattern),
    Match {
        /// Selector variable names (NFC), in source order.
        selectors: Vec<String>,
        variants: Vec<Variant>,
        /// Index into `variants` of the first all-`*` variant.
        /// Compilation rejects a matcher without one
        /// (`CompileError::MissingFallbackVariant`), which is what
        /// makes format-time selection total.
        fallback_variant: usize,
    },
}

/// A pattern: interleaved text runs and placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pattern {
    pub(crate) parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    /// Literal text, escapes (`\{`, `\}`, `\|`, `\\`) already removed.
    Text(String),
    Placeholder(Expr),
}

/// An expression: an operand with an optional function annotation, or
/// a bare function call. The "neither" case is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expr {
    Operand {
        operand: Operand,
        func: Option<Func>,
    },
    Func(Func),
}

impl Expr {
    /// The MF2 fallback representation of this expression, used in
    /// the output when resolution fails at format time (missing
    /// argument, unknown function, bad operand): `{$var}`,
    /// `{|literal|}`, or `{:func}`.
    pub(crate) fn fallback(&self) -> String {
        match self {
            Expr::Operand {
                operand: Operand::Var(name),
                ..
            } => format!("{{${name}}}"),
            Expr::Operand {
                operand: Operand::Literal(value),
                ..
            } => format!("{{|{value}|}}"),
            Expr::Func(func) => format!("{{:{}}}", func.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Operand {
    /// `$name`, NFC-normalized.
    Var(String),
    /// A quoted or unquoted literal, unescaped.
    Literal(String),
}

/// A function annotation: `:name opt=val ...`. Name and option keys
/// are NFC-normalized; option order is preserved (later duplicates
/// win by applying last, matching source order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Func {
    pub(crate) name: String,
    pub(crate) options: Vec<(String, OptValue)>,
}

/// An option value: a literal, or a `$variable` resolved at format
/// time against the same environment as the pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptValue {
    Literal(String),
    Var(String),
}

/// One `.match` variant: keys (one per selector — compilation rejects
/// a mismatch) and the pattern to format when it wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Variant {
    pub(crate) keys: Vec<Key>,
    pub(crate) pattern: Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `*`.
    CatchAll,
    /// A literal key: an exact numeric key (`1`) or a plural category
    /// name (`one`, `many`, ...). Which one it is gets decided at
    /// match time against the resolved selector, as the spike did.
    Literal(String),
}
