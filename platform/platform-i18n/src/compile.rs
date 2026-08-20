//! CST → IR lowering: the compile half of the Q8 split.
//!
//! Parsing is `ox_mf2_parser`'s job (spec-final MF2, CST layer — its
//! `SemanticModel` lowering is lint-only and carries no option
//! values, so we lower from the CST ourselves; Q8's finding). This
//! module walks the CST exactly once per message and emits the
//! [`ir`] types; the CST is dropped before `compile` returns.
//!
//! Everything the spec calls a *data model error* is rejected here,
//! not at format time: syntax diagnostics, duplicate declarations,
//! variant-key/selector arity mismatches, and a matcher without an
//! all-`*` fallback variant. Format time then only ever sees
//! *resolution* errors, which warn-and-fall-back (see
//! [`format`](crate::MessageTemplate::format)).

use icu::normalizer::ComposingNormalizer;
use ox_mf2_parser::{CstChild, CstNodeView, CstView, SyntaxKind, parse_message};

use crate::CompileError;
use crate::ir::{Body, Decl, Expr, Func, Ir, Key, Operand, OptValue, Part, Pattern, Variant};

/// Parse and lower one MF2 source message.
pub(crate) fn compile(source: &str) -> Result<Ir, CompileError> {
    let parsed = parse_message(source).map_err(|e| CompileError::Syntax(vec![format!("{e:?}")]))?;
    let result = parsed.result();
    if !result.diagnostics.is_empty() {
        return Err(CompileError::Syntax(
            result
                .diagnostics
                .iter()
                .map(|d| {
                    format!(
                        "{:?} at {}..{}: {}",
                        d.code, d.span.start, d.span.end, d.message
                    )
                })
                .collect(),
        ));
    }
    let view = CstView::new(parsed.sources(), result.source, &result.cst);
    let root = view
        .root()
        .ok_or_else(|| CompileError::Syntax(vec!["no root".into()]))?;
    Lowerer { view }.lower_root(root)
}

struct Lowerer<'a> {
    view: CstView<'a>,
}

impl<'a> Lowerer<'a> {
    fn lower_root(&self, root: CstNodeView<'a>) -> Result<Ir, CompileError> {
        let message = child_nodes(root)
            .find(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::SimpleMessage | SyntaxKind::ComplexMessage
                )
            })
            .ok_or_else(|| CompileError::Unsupported("empty root".into()))?;
        match message.kind() {
            SyntaxKind::SimpleMessage => {
                let pattern = self.expect_child(message, SyntaxKind::Pattern)?;
                Ok(Ir {
                    decls: Vec::new(),
                    body: Body::Pattern(self.lower_pattern(pattern)?),
                })
            }
            SyntaxKind::ComplexMessage => self.lower_complex(message),
            _ => unreachable!("find() filtered on these two kinds"),
        }
    }

    fn lower_complex(&self, message: CstNodeView<'a>) -> Result<Ir, CompileError> {
        let mut decls: Vec<Decl> = Vec::new();
        let mut body: Option<Body> = None;
        for child in child_nodes(message) {
            match child.kind() {
                SyntaxKind::InputDeclaration => {
                    // `.input {$x :fn ...}` binds $x to its own
                    // annotated expression. Evaluated in an
                    // environment that does not yet contain $x, the
                    // inner $x reads the caller's argument — the
                    // spec's semantics, by ordering alone.
                    let placeholder = self.expect_child(child, SyntaxKind::Placeholder)?;
                    let expr_node =
                        self.expect_child(placeholder, SyntaxKind::VariableExpression)?;
                    let var = self.expect_child(expr_node, SyntaxKind::Variable)?;
                    let name = self.variable_name(var)?;
                    push_decl(&mut decls, name, self.lower_expression(expr_node)?)?;
                }
                SyntaxKind::LocalDeclaration => {
                    // `.local $x = {expr}`.
                    let var = self.expect_child(child, SyntaxKind::Variable)?;
                    let name = self.variable_name(var)?;
                    let placeholder = self.expect_child(child, SyntaxKind::Placeholder)?;
                    let expr_node = child_nodes(placeholder)
                        .find(|n| is_expression_kind(n.kind()))
                        .ok_or_else(|| {
                            CompileError::Unsupported("empty local declaration".into())
                        })?;
                    push_decl(&mut decls, name, self.lower_expression(expr_node)?)?;
                }
                SyntaxKind::ComplexBody => {
                    for b in child_nodes(child) {
                        match b.kind() {
                            SyntaxKind::QuotedPattern => {
                                let pattern = self.expect_child(b, SyntaxKind::Pattern)?;
                                body = Some(Body::Pattern(self.lower_pattern(pattern)?));
                            }
                            SyntaxKind::Matcher => body = Some(self.lower_matcher(b)?),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let body =
            body.ok_or_else(|| CompileError::Unsupported("complex message without body".into()))?;
        Ok(Ir { decls, body })
    }

    fn lower_matcher(&self, matcher: CstNodeView<'a>) -> Result<Body, CompileError> {
        let mut selectors: Vec<String> = Vec::new();
        for sel in child_nodes(matcher).filter(|n| n.kind() == SyntaxKind::Selector) {
            let var = self.expect_child(sel, SyntaxKind::Variable)?;
            selectors.push(self.variable_name(var)?);
        }
        if selectors.is_empty() {
            return Err(CompileError::Unsupported(
                "matcher without selectors".into(),
            ));
        }
        let mut variants: Vec<Variant> = Vec::new();
        let mut fallback_variant: Option<usize> = None;
        for v in child_nodes(matcher).filter(|n| n.kind() == SyntaxKind::Variant) {
            let keys = child_nodes(v)
                .filter(|n| matches!(n.kind(), SyntaxKind::VariantKey | SyntaxKind::CatchAllKey))
                .map(|k| self.lower_key(k))
                .collect::<Result<Vec<Key>, CompileError>>()?;
            if keys.len() != selectors.len() {
                return Err(CompileError::VariantKeyMismatch {
                    selectors: selectors.len(),
                    keys: keys.len(),
                });
            }
            let quoted = self.expect_child(v, SyntaxKind::QuotedPattern)?;
            let pattern = self.lower_pattern(self.expect_child(quoted, SyntaxKind::Pattern)?)?;
            if fallback_variant.is_none() && keys.iter().all(|k| matches!(k, Key::CatchAll)) {
                fallback_variant = Some(variants.len());
            }
            variants.push(Variant { keys, pattern });
        }
        let fallback_variant = fallback_variant.ok_or(CompileError::MissingFallbackVariant)?;
        Ok(Body::Match {
            selectors,
            variants,
            fallback_variant,
        })
    }

    fn lower_key(&self, key: CstNodeView<'a>) -> Result<Key, CompileError> {
        if key.kind() == SyntaxKind::CatchAllKey {
            return Ok(Key::CatchAll);
        }
        let lit = child_nodes(key)
            .find(|n| is_literal_kind(n.kind()))
            .ok_or_else(|| CompileError::Unsupported("variant key without literal".into()))?;
        Ok(Key::Literal(self.literal_content(lit)))
    }

    fn lower_pattern(&self, pattern: CstNodeView<'a>) -> Result<Pattern, CompileError> {
        let mut parts: Vec<Part> = Vec::new();
        for child in pattern.children() {
            match child {
                CstChild::Node(node) => match node.kind() {
                    SyntaxKind::Text => {
                        let text = self.text_content(node);
                        // Merge adjacent runs so the IR is minimal.
                        if let Some(Part::Text(prev)) = parts.last_mut() {
                            prev.push_str(&text);
                        } else {
                            parts.push(Part::Text(text));
                        }
                    }
                    SyntaxKind::Placeholder => {
                        let expr_node = child_nodes(node)
                            .find(|n| is_expression_kind(n.kind()))
                            .ok_or_else(|| {
                                CompileError::Unsupported(
                                    "markup placeholders are out of scope".into(),
                                )
                            })?;
                        parts.push(Part::Placeholder(self.lower_expression(expr_node)?));
                    }
                    kind => {
                        return Err(CompileError::Unsupported(format!("pattern child {kind:?}")));
                    }
                },
                CstChild::Token(_) => {}
            }
        }
        Ok(Pattern { parts })
    }

    fn lower_expression(&self, expr: CstNodeView<'a>) -> Result<Expr, CompileError> {
        let operand = match expr.kind() {
            SyntaxKind::VariableExpression => {
                let var = self.expect_child(expr, SyntaxKind::Variable)?;
                Some(Operand::Var(self.variable_name(var)?))
            }
            SyntaxKind::LiteralExpression => {
                let lit = child_nodes(expr)
                    .find(|n| is_literal_kind(n.kind()))
                    .ok_or_else(|| {
                        CompileError::Unsupported("literal expression without literal".into())
                    })?;
                Some(Operand::Literal(self.literal_content(lit)))
            }
            SyntaxKind::FunctionExpression => None,
            kind => return Err(CompileError::Unsupported(format!("expression {kind:?}"))),
        };
        let func = child_nodes(expr)
            .find(|n| n.kind() == SyntaxKind::Function)
            .map(|f| self.lower_function(f))
            .transpose()?;
        match (operand, func) {
            (Some(operand), func) => Ok(Expr::Operand { operand, func }),
            (None, Some(func)) => Ok(Expr::Func(func)),
            (None, None) => Err(CompileError::Unsupported(
                "function expression without function".into(),
            )),
        }
    }

    fn lower_function(&self, function: CstNodeView<'a>) -> Result<Func, CompileError> {
        let name_node = self.expect_child(function, SyntaxKind::Identifier)?;
        let name = nfc(self.view.source_slice(name_node.span()));
        let mut options: Vec<(String, OptValue)> = Vec::new();
        for opt in child_nodes(function).filter(|n| n.kind() == SyntaxKind::Option) {
            let key_node = self.expect_child(opt, SyntaxKind::Identifier)?;
            let key = nfc(self.view.source_slice(key_node.span()));
            let value = self.lower_option_value(opt)?;
            options.push((key, value));
        }
        Ok(Func { name, options })
    }

    fn lower_option_value(&self, opt: CstNodeView<'a>) -> Result<OptValue, CompileError> {
        // skip(1): the first node child is the option's key Identifier.
        for node in child_nodes(opt).skip(1) {
            match node.kind() {
                SyntaxKind::QuotedLiteral | SyntaxKind::UnquotedLiteral => {
                    return Ok(OptValue::Literal(self.literal_content(node)));
                }
                SyntaxKind::Variable => {
                    return Ok(OptValue::Var(self.variable_name(node)?));
                }
                _ => {}
            }
        }
        Err(CompileError::Unsupported("option without value".into()))
    }

    // ── CST helpers (ported from the Q8 spike) ──────────────────────────

    fn expect_child(
        &self,
        node: CstNodeView<'a>,
        kind: SyntaxKind,
    ) -> Result<CstNodeView<'a>, CompileError> {
        child_nodes(node)
            .find(|n| n.kind() == kind)
            .ok_or_else(|| CompileError::Unsupported(format!("{:?} without {kind:?}", node.kind())))
    }

    /// `$name` → NFC-normalized name (Q8: normalize before env lookup).
    fn variable_name(&self, var: CstNodeView<'a>) -> Result<String, CompileError> {
        let name = self.expect_child(var, SyntaxKind::Name)?;
        Ok(nfc(self.view.source_slice(name.span())))
    }

    /// Text node content with `\{`, `\}`, `\|`, `\\` unescaped.
    fn text_content(&self, text: CstNodeView<'a>) -> String {
        let mut out = String::new();
        for token in text.tokens() {
            let slice = self.view.source_slice(token.span());
            match token.kind() {
                SyntaxKind::EscapeToken => out.push_str(slice.trim_start_matches('\\')),
                _ => out.push_str(slice),
            }
        }
        out
    }

    /// Literal content: quoted literals lose their pipes and escapes,
    /// unquoted literals are taken verbatim.
    fn literal_content(&self, lit: CstNodeView<'a>) -> String {
        match lit.kind() {
            SyntaxKind::QuotedLiteral => {
                let mut out = String::new();
                for token in lit.tokens() {
                    let slice = self.view.source_slice(token.span());
                    match token.kind() {
                        SyntaxKind::PipeToken => {}
                        SyntaxKind::EscapeToken => out.push_str(slice.trim_start_matches('\\')),
                        _ => out.push_str(slice),
                    }
                }
                out
            }
            _ => self.view.source_slice(lit.span()).to_owned(),
        }
    }
}

fn push_decl(decls: &mut Vec<Decl>, name: String, expr: Expr) -> Result<(), CompileError> {
    // Spec data model error: a name may be declared only once.
    if decls.iter().any(|d| d.name == name) {
        return Err(CompileError::DuplicateDeclaration(name));
    }
    decls.push(Decl { name, expr });
    Ok(())
}

fn is_expression_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::VariableExpression
            | SyntaxKind::LiteralExpression
            | SyntaxKind::FunctionExpression
    )
}

fn is_literal_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::QuotedLiteral | SyntaxKind::UnquotedLiteral
    )
}

/// Iterate node (not token) children.
fn child_nodes<'a>(node: CstNodeView<'a>) -> impl Iterator<Item = CstNodeView<'a>> {
    node.children().filter_map(|c| match c {
        CstChild::Node(n) => Some(n),
        CstChild::Token(_) => None,
    })
}

/// NFC-normalize a name from the CST. The normalizer constructors are
/// `const` over compiled data — constructing one here is free.
pub(crate) fn nfc(s: &str) -> String {
    ComposingNormalizer::new_nfc().normalize(s).into_owned()
}
