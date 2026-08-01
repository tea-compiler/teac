//! Parsing of TeaLang expression rules.
//!
//! This submodule implements the [`ParseContext`] methods
//! that lower expression-oriented parse-tree nodes into the corresponding AST
//! types: right-hand values, Boolean expressions (`||`, `&&`, `!`, and
//! comparisons), arithmetic expressions (the `+`/`-` and `*`/`/` precedence
//! layers), and expression units (literals, parenthesised expressions,
//! function calls, references, and left-value access chains).

use crate::ast;

use super::common::{get_pos, grammar_error, parse_num, Pair, ParseResult, Rule};
use super::ParseContext;

impl<'a> ParseContext<'a> {
    pub(crate) fn parse_right_val_list(&self, pair: Pair) -> ParseResult<Vec<ast::RightVal>> {
        let mut vals = Vec::new();
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::right_val {
                vals.push(*self.parse_right_val(inner)?);
            }
        }
        Ok(vals)
    }

    /// Parses a `right_val` node into a boxed [`ast::RightVal`].
    ///
    /// A right-hand-side value is either a Boolean expression (`bool_expr`) or
    /// an arithmetic expression (`arith_expr`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither is found.
    pub(crate) fn parse_right_val(&self, pair: Pair) -> ParseResult<Box<ast::RightVal>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::bool_expr => {
                    return Ok(Box::new(ast::RightVal {
                        inner: ast::RightValInner::BoolExpr(self.parse_bool_expr(inner)?),
                    }));
                }
                Rule::arith_expr => {
                    return Ok(Box::new(ast::RightVal {
                        inner: ast::RightValInner::ArithExpr(self.parse_arith_expr(inner)?),
                    }));
                }
                _ => {}
            }
        }

        Err(grammar_error("right_val", &pair_for_error))
    }

    /// Parses a `bool_expr` node into a boxed [`ast::BoolExpr`].
    ///
    /// A Boolean expression is a sequence of `bool_and_term` nodes optionally
    /// combined with `||` operators, lowered into a left-associative tree of
    /// [`ast::BoolBiOpExpr`] nodes with [`ast::BoolBiOp::Or`].
    pub(crate) fn parse_bool_expr(&self, pair: Pair) -> ParseResult<Box<ast::BoolExpr>> {
        let pair_for_error = pair.clone();
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        if inner_pairs.is_empty() {
            return Err(grammar_error("bool_expr", &pair_for_error));
        }

        let mut expr = self.parse_bool_and_term(inner_pairs[0].clone())?;

        let mut i = 1;
        while i < inner_pairs.len() {
            if inner_pairs[i].as_rule() == Rule::op_or {
                let right = self.parse_bool_and_term(
                    inner_pairs
                        .get(i + 1)
                        .ok_or_else(|| grammar_error("bool_expr.operand", &pair_for_error))?
                        .clone(),
                )?;
                expr = Box::new(ast::BoolExpr {
                    pos: expr.pos,
                    inner: ast::BoolExprInner::BoolBiOpExpr(Box::new(ast::BoolBiOpExpr {
                        op: ast::BoolBiOp::Or,
                        left: expr,
                        right,
                    })),
                });
                i += 2;
            } else {
                i += 1;
            }
        }

        Ok(expr)
    }

    /// Parses a `bool_and_term` node into a boxed [`ast::BoolExpr`].
    ///
    /// A Boolean AND term is a sequence of `bool_unit_atom` nodes optionally
    /// combined with `&&` operators, lowered into a left-associative tree of
    /// [`ast::BoolBiOpExpr`] nodes with [`ast::BoolBiOp::And`].
    fn parse_bool_and_term(&self, pair: Pair) -> ParseResult<Box<ast::BoolExpr>> {
        let pair_for_error = pair.clone();
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        if inner_pairs.is_empty() {
            return Err(grammar_error("bool_and_term", &pair_for_error));
        }

        let first_unit = self.parse_bool_unit_atom(inner_pairs[0].clone())?;
        let mut expr = Box::new(ast::BoolExpr {
            pos: first_unit.pos,
            inner: ast::BoolExprInner::BoolUnit(first_unit),
        });

        let mut i = 1;
        while i < inner_pairs.len() {
            if inner_pairs[i].as_rule() == Rule::op_and {
                let right_unit = self.parse_bool_unit_atom(
                    inner_pairs
                        .get(i + 1)
                        .ok_or_else(|| grammar_error("bool_and_term.operand", &pair_for_error))?
                        .clone(),
                )?;
                let right_expr = Box::new(ast::BoolExpr {
                    pos: right_unit.pos,
                    inner: ast::BoolExprInner::BoolUnit(right_unit),
                });

                expr = Box::new(ast::BoolExpr {
                    pos: expr.pos,
                    inner: ast::BoolExprInner::BoolBiOpExpr(Box::new(ast::BoolBiOpExpr {
                        op: ast::BoolBiOp::And,
                        left: expr,
                        right: right_expr,
                    })),
                });
                i += 2;
            } else {
                i += 1;
            }
        }

        Ok(expr)
    }

    /// Parses a `bool_unit_atom` node into a boxed [`ast::BoolUnit`].
    ///
    /// Handles three cases:
    /// 1. A prefixed `!` (NOT) operator followed by a nested `bool_unit_atom`.
    /// 2. A parenthesised Boolean expression (`bool_unit_paren`).
    /// 3. A comparison expression (`bool_comparison`).
    fn parse_bool_unit_atom(&self, pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
        let pair_for_error = pair.clone();
        let pos = get_pos(&pair);
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        // `! <bool_unit_atom>` — unary NOT operator.
        if inner_pairs.len() == 2 && inner_pairs[0].as_rule() == Rule::op_not {
            let cond = self.parse_bool_unit_atom(inner_pairs[1].clone())?;
            return Ok(Box::new(ast::BoolUnit {
                pos,
                inner: ast::BoolUnitInner::BoolUOpExpr(Box::new(ast::BoolUOpExpr {
                    op: ast::BoolUOp::Not,
                    cond,
                })),
            }));
        }

        for inner in inner_pairs {
            match inner.as_rule() {
                Rule::bool_unit_paren => {
                    return self.parse_bool_unit_paren(inner);
                }
                Rule::bool_comparison => {
                    return self.parse_bool_comparison(inner);
                }
                _ => {}
            }
        }

        Err(grammar_error("bool_unit_atom", &pair_for_error))
    }

    /// Parses a `bool_unit_paren` node into a boxed [`ast::BoolUnit`].
    ///
    /// After stripping the surrounding parentheses, the inner content is
    /// either:
    /// * A single `bool_expr` — wrapped as [`ast::BoolUnitInner::BoolExpr`].
    /// * A comparison triple `(expr op expr)` — delegated to
    ///   [`Self::parse_comparison_pair_triple`].
    fn parse_bool_unit_paren(&self, pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
        let pair_for_error = pair.clone();
        let pos = get_pos(&pair);
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        let filtered: Vec<_> = inner_pairs
            .into_iter()
            .filter(|p| p.as_rule() != Rule::lparen && p.as_rule() != Rule::rparen)
            .collect();

        if filtered.len() == 1 && filtered[0].as_rule() == Rule::bool_expr {
            return Ok(Box::new(ast::BoolUnit {
                pos,
                inner: ast::BoolUnitInner::BoolExpr(self.parse_bool_expr(filtered[0].clone())?),
            }));
        }

        self.parse_comparison_pair_triple(pos, &filtered, "bool_unit_paren", &pair_for_error)
    }

    /// Parses a `bool_comparison` node (`expr op expr`, exactly three
    /// children) into a boxed [`ast::BoolUnit`].
    fn parse_bool_comparison(&self, pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
        let pair_for_error = pair.clone();
        let pos = get_pos(&pair);
        let inner_pairs: Vec<_> = pair.into_inner().collect();
        self.parse_comparison_pair_triple(pos, &inner_pairs, "bool_comparison", &pair_for_error)
    }

    /// Builds a comparison [`ast::BoolUnit`] from `pairs`, which must be laid
    /// out as `[left_expr, comp_op, right_expr]`.
    ///
    /// Returns [`Error::Grammar`](super::common::Error::Grammar), labelled
    /// with `context`, when the slice does not have exactly three elements.
    fn parse_comparison_pair_triple(
        &self,
        pos: usize,
        pairs: &[Pair],
        context: &'static str,
        pair_for_error: &Pair<'_>,
    ) -> ParseResult<Box<ast::BoolUnit>> {
        if pairs.len() != 3 {
            return Err(grammar_error(context, pair_for_error));
        }

        self.parse_comparison_to_bool_unit(
            pos,
            pairs[0].clone(),
            pairs[1].clone(),
            pairs[2].clone(),
        )
    }

    /// Builds a comparison [`ast::BoolUnit`]
    /// ([`ast::BoolUnitInner::ComExpr`]) from pairs for the left operand, the
    /// comparison operator, and the right operand.
    fn parse_comparison_to_bool_unit(
        &self,
        pos: usize,
        left_pair: Pair,
        op_pair: Pair,
        right_pair: Pair,
    ) -> ParseResult<Box<ast::BoolUnit>> {
        let left = self.parse_expr_unit(left_pair)?;
        let op = self.parse_comp_op(op_pair)?;
        let right = self.parse_expr_unit(right_pair)?;

        Ok(Box::new(ast::BoolUnit {
            pos,
            inner: ast::BoolUnitInner::ComExpr(Box::new(ast::ComExpr { op, left, right })),
        }))
    }

    /// Maps a `comp_op` node onto one of the six comparison operators: `<`,
    /// `>`, `<=`, `>=`, `==`, `!=`.  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if no known operator
    /// token is found.
    fn parse_comp_op(&self, pair: Pair) -> ParseResult<ast::ComOp> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::op_lt => return Ok(ast::ComOp::Lt),
                Rule::op_gt => return Ok(ast::ComOp::Gt),
                Rule::op_le => return Ok(ast::ComOp::Le),
                Rule::op_ge => return Ok(ast::ComOp::Ge),
                Rule::op_eq => return Ok(ast::ComOp::Eq),
                Rule::op_ne => return Ok(ast::ComOp::Ne),
                _ => {}
            }
        }
        Err(grammar_error("comp_op", &pair_for_error))
    }

    /// Parses an `arith_expr` node into a boxed [`ast::ArithExpr`].
    ///
    /// An arithmetic expression is a sequence of `arith_term` nodes optionally
    /// combined with additive operators (`+`, `-`), lowered into a
    /// left-associative tree of [`ast::ArithBiOpExpr`] nodes.
    pub(crate) fn parse_arith_expr(&self, pair: Pair) -> ParseResult<Box<ast::ArithExpr>> {
        let pair_for_error = pair.clone();
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        if inner_pairs.is_empty() {
            return Err(grammar_error("arith_expr", &pair_for_error));
        }

        let mut expr = self.parse_arith_term(inner_pairs[0].clone())?;

        let mut i = 1;
        while i < inner_pairs.len() {
            if inner_pairs[i].as_rule() == Rule::arith_add_op {
                let op = self.parse_arith_add_op(inner_pairs[i].clone())?;
                let right = self.parse_arith_term(
                    inner_pairs
                        .get(i + 1)
                        .ok_or_else(|| grammar_error("arith_expr.operand", &pair_for_error))?
                        .clone(),
                )?;

                expr = Box::new(ast::ArithExpr {
                    pos: expr.pos,
                    inner: ast::ArithExprInner::ArithBiOpExpr(Box::new(ast::ArithBiOpExpr {
                        op,
                        left: expr,
                        right,
                    })),
                });
                i += 2;
            } else {
                i += 1;
            }
        }

        Ok(expr)
    }

    /// Parses an `arith_term` node into a boxed [`ast::ArithExpr`].
    ///
    /// An arithmetic term is a sequence of `expr_unit` nodes optionally
    /// combined with multiplicative operators (`*`, `/`), lowered into a
    /// left-associative tree of [`ast::ArithBiOpExpr`] nodes.
    fn parse_arith_term(&self, pair: Pair) -> ParseResult<Box<ast::ArithExpr>> {
        let pair_for_error = pair.clone();
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        if inner_pairs.is_empty() {
            return Err(grammar_error("arith_term", &pair_for_error));
        }

        let first_unit = self.parse_expr_unit(inner_pairs[0].clone())?;
        let mut expr = Box::new(ast::ArithExpr {
            pos: first_unit.pos,
            inner: ast::ArithExprInner::ExprUnit(first_unit),
        });

        let mut i = 1;
        while i < inner_pairs.len() {
            if inner_pairs[i].as_rule() == Rule::arith_mul_op {
                let op = self.parse_arith_mul_op(inner_pairs[i].clone())?;
                let right_unit = self.parse_expr_unit(
                    inner_pairs
                        .get(i + 1)
                        .ok_or_else(|| grammar_error("arith_term.operand", &pair_for_error))?
                        .clone(),
                )?;
                let right = Box::new(ast::ArithExpr {
                    pos: right_unit.pos,
                    inner: ast::ArithExprInner::ExprUnit(right_unit),
                });

                expr = Box::new(ast::ArithExpr {
                    pos: expr.pos,
                    inner: ast::ArithExprInner::ArithBiOpExpr(Box::new(ast::ArithBiOpExpr {
                        op,
                        left: expr,
                        right,
                    })),
                });
                i += 2;
            } else {
                i += 1;
            }
        }

        Ok(expr)
    }

    /// Maps an `arith_add_op` node onto the matching additive
    /// [`ast::ArithBiOp`] variant (`+` or `-`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither token is
    /// found.
    fn parse_arith_add_op(&self, pair: Pair) -> ParseResult<ast::ArithBiOp> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::op_add => return Ok(ast::ArithBiOp::Add),
                Rule::op_sub => return Ok(ast::ArithBiOp::Sub),
                _ => {}
            }
        }
        Err(grammar_error("arith_add_op", &pair_for_error))
    }

    /// Maps an `arith_mul_op` node onto the matching multiplicative
    /// [`ast::ArithBiOp`] variant (`*` or `/`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither token is
    /// found.
    fn parse_arith_mul_op(&self, pair: Pair) -> ParseResult<ast::ArithBiOp> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::op_mul => return Ok(ast::ArithBiOp::Mul),
                Rule::op_div => return Ok(ast::ArithBiOp::Div),
                _ => {}
            }
        }
        Err(grammar_error("arith_mul_op", &pair_for_error))
    }

    /// Parses an `expr_unit` node into a boxed [`ast::ExprUnit`].
    ///
    /// An expression unit is the atomic building block of arithmetic
    /// expressions:
    /// 1. Negated integer literal: `-<num>`.
    /// 2. Parenthesised arithmetic expression: `(<arith_expr>)`.
    /// 3. Function call: `<fn_call>`.
    /// 4. Plain integer literal: `<num>`.
    /// 5. Reference: `&<identifier>`.
    /// 6. Identifier with optional field/index suffixes (left-value chain).
    ///
    /// Returns [`Error::Grammar`](super::common::Error::Grammar) if none of
    /// the forms matches.
    pub(crate) fn parse_expr_unit(&self, pair: Pair) -> ParseResult<Box<ast::ExprUnit>> {
        let pair_for_error = pair.clone();
        let pos = get_pos(&pair);
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        let filtered: Vec<_> = inner_pairs
            .iter()
            .filter(|p| !matches!(p.as_rule(), Rule::lparen | Rule::rparen))
            .cloned()
            .collect();

        // `-<num>` — negated integer literal.
        if filtered.len() == 2
            && filtered[0].as_rule() == Rule::op_sub
            && filtered[1].as_rule() == Rule::num
        {
            let num = parse_num(filtered[1].clone())?;
            return Ok(Box::new(ast::ExprUnit {
                pos,
                inner: ast::ExprUnitInner::Num(-num),
            }));
        }

        // `(<arith_expr>)` — parenthesised arithmetic expression.
        if filtered.len() == 1 && filtered[0].as_rule() == Rule::arith_expr {
            return Ok(Box::new(ast::ExprUnit {
                pos,
                inner: ast::ExprUnitInner::ArithExpr(self.parse_arith_expr(filtered[0].clone())?),
            }));
        }

        // `<fn_call>` — a function or method call.
        if !filtered.is_empty() && filtered[0].as_rule() == Rule::fn_call {
            return Ok(Box::new(ast::ExprUnit {
                pos,
                inner: ast::ExprUnitInner::FnCall(self.parse_fn_call(filtered[0].clone())?),
            }));
        }

        // `<num>` — plain integer literal.
        if filtered.len() == 1 && filtered[0].as_rule() == Rule::num {
            let num = parse_num(filtered[0].clone())?;
            return Ok(Box::new(ast::ExprUnit {
                pos,
                inner: ast::ExprUnitInner::Num(num),
            }));
        }

        // `&<identifier>` — a reference to a variable.
        if filtered.len() == 2
            && filtered[0].as_rule() == Rule::ampersand
            && filtered[1].as_rule() == Rule::identifier
        {
            let id = filtered[1].as_str().to_string();
            return Ok(Box::new(ast::ExprUnit {
                pos,
                inner: ast::ExprUnitInner::Reference(id),
            }));
        }

        // `<identifier> (<expr_suffix>)*` — variable or field/index access.
        if !inner_pairs.is_empty() && inner_pairs[0].as_rule() == Rule::identifier {
            let id = inner_pairs[0].as_str().to_string();

            let mut base = Box::new(ast::LeftVal {
                pos,
                inner: ast::LeftValInner::Id(id),
            });

            let mut i = 1;
            while i < inner_pairs.len() {
                match inner_pairs[i].as_rule() {
                    Rule::expr_suffix => {
                        base = self.parse_expr_suffix(base, inner_pairs[i].clone())?;
                        i += 1;
                    }
                    _ => break,
                }
            }

            return left_val_to_expr_unit(*base);
        }

        Err(grammar_error("expr_unit", &pair_for_error))
    }

    /// Parses an `index_expr` node into a boxed [`ast::IndexExpr`].
    ///
    /// An index expression is either a numeric literal (`arr[0]`) or an
    /// identifier (`arr[i]`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither is found.
    pub(crate) fn parse_index_expr(&self, pair: Pair) -> ParseResult<Box<ast::IndexExpr>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::num => {
                    let num = parse_num(inner)? as usize;
                    return Ok(Box::new(ast::IndexExpr {
                        inner: ast::IndexExprInner::Num(num),
                    }));
                }
                Rule::identifier => {
                    return Ok(Box::new(ast::IndexExpr {
                        inner: ast::IndexExprInner::Id(inner.as_str().to_string()),
                    }));
                }
                _ => {}
            }
        }
        Err(grammar_error("index_expr", &pair_for_error))
    }

    /// Parses a `fn_call` node into a boxed [`ast::FnCall`].
    ///
    /// Dispatches to either [`Self::parse_module_prefixed_call`] (for calls
    /// like `module::func(...)`) or [`Self::parse_local_call`] (for calls like
    /// `func(...)`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither child is
    /// found.
    pub(crate) fn parse_fn_call(&self, pair: Pair) -> ParseResult<Box<ast::FnCall>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::module_prefixed_call => {
                    return self.parse_module_prefixed_call(inner);
                }
                Rule::local_call => {
                    return self.parse_local_call(inner);
                }
                _ => {}
            }
        }
        Err(grammar_error("fn_call", &pair_for_error))
    }

    /// Parses a `module_prefixed_call` node (`mod1::mod2::func(args)`) into a
    /// boxed [`ast::FnCall`].
    ///
    /// The last `identifier` child is the function name; the preceding ones
    /// form the module path.
    fn parse_module_prefixed_call(&self, pair: Pair) -> ParseResult<Box<ast::FnCall>> {
        let inner_pairs: Vec<_> = pair.into_inner().collect();
        let mut idents: Vec<String> = Vec::new();
        let mut vals = Vec::new();

        for inner in &inner_pairs {
            match inner.as_rule() {
                Rule::identifier => idents.push(inner.as_str().to_string()),
                Rule::right_val_list => vals = self.parse_right_val_list(inner.clone())?,
                _ => {}
            }
        }

        let name = idents.pop().unwrap_or_default();
        let module_prefix = if idents.is_empty() {
            None
        } else {
            Some(idents.join("::"))
        };

        Ok(Box::new(ast::FnCall {
            module_prefix,
            name,
            vals,
        }))
    }

    /// Parses a `local_call` node (`func(args)`, no module prefix) into a
    /// boxed [`ast::FnCall`].
    fn parse_local_call(&self, pair: Pair) -> ParseResult<Box<ast::FnCall>> {
        let mut name = String::new();
        let mut vals = Vec::new();

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => name = inner.as_str().to_string(),
                Rule::right_val_list => vals = self.parse_right_val_list(inner)?,
                _ => {}
            }
        }

        Ok(Box::new(ast::FnCall {
            module_prefix: None,
            name,
            vals,
        }))
    }

    /// Parses a `left_val` node into a boxed [`ast::LeftVal`].
    ///
    /// A left-hand-side value starts with an identifier and may be followed by
    /// zero or more `expr_suffix` nodes representing field access (`.field`) or
    /// array indexing (`[idx]`).
    ///
    /// Returns [`Error::Grammar`](super::common::Error::Grammar) if the node
    /// contains no children.
    pub(crate) fn parse_left_val(&self, pair: Pair) -> ParseResult<Box<ast::LeftVal>> {
        let pair_for_error = pair.clone();
        let pos = get_pos(&pair);
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        if inner_pairs.is_empty() {
            return Err(grammar_error("left_val", &pair_for_error));
        }

        // The first child is always the root identifier.
        let id = inner_pairs[0].as_str().to_string();

        let mut base = Box::new(ast::LeftVal {
            pos,
            inner: ast::LeftValInner::Id(id),
        });

        let mut i = 1;
        while i < inner_pairs.len() {
            match inner_pairs[i].as_rule() {
                Rule::expr_suffix => {
                    base = self.parse_expr_suffix(base, inner_pairs[i].clone())?;
                    i += 1;
                }
                _ => break,
            }
        }

        Ok(base)
    }

    /// Applies a single `expr_suffix` to a `base` left-value, producing a new
    /// [`ast::LeftVal`].
    ///
    /// An `expr_suffix` is either:
    /// * An array index: `[<index_expr>]` → [`ast::LeftValInner::ArrayExpr`].
    /// * A field access: `.<identifier>`  → [`ast::LeftValInner::MemberExpr`].
    ///
    /// If no recognised suffix token is found, `base` returns unchanged.
    pub(crate) fn parse_expr_suffix(
        &self,
        base: Box<ast::LeftVal>,
        suffix: Pair,
    ) -> ParseResult<Box<ast::LeftVal>> {
        let pos = base.pos;

        for inner in suffix.into_inner() {
            match inner.as_rule() {
                Rule::lbracket | Rule::rbracket | Rule::dot => continue,
                Rule::index_expr => {
                    let idx = self.parse_index_expr(inner)?;
                    return Ok(Box::new(ast::LeftVal {
                        pos,
                        inner: ast::LeftValInner::ArrayExpr(Box::new(ast::ArrayExpr {
                            arr: base,
                            idx,
                        })),
                    }));
                }
                Rule::identifier => {
                    let member_id = inner.as_str().to_string();
                    return Ok(Box::new(ast::LeftVal {
                        pos,
                        inner: ast::LeftValInner::MemberExpr(Box::new(ast::MemberExpr {
                            struct_id: base,
                            member_id,
                        })),
                    }));
                }
                _ => {}
            }
        }

        Ok(base)
    }
}

/// Converts a [`ast::LeftVal`] into the corresponding [`ast::ExprUnit`] variant.
///
/// An identifier (or field/index access chain) is parsed as a left-value
/// first; this conversion applies when it turns out to sit on the right-hand
/// side of an expression.  Infallible for the three [`ast::LeftValInner`]
/// variants.
fn left_val_to_expr_unit(lval: ast::LeftVal) -> ParseResult<Box<ast::ExprUnit>> {
    let pos = lval.pos;

    match &lval.inner {
        ast::LeftValInner::Id(id) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::Id(id.clone()),
        })),
        ast::LeftValInner::ArrayExpr(arr_expr) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::ArrayExpr(arr_expr.clone()),
        })),
        ast::LeftValInner::MemberExpr(mem_expr) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::MemberExpr(mem_expr.clone()),
        })),
    }
}
