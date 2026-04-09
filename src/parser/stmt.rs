use crate::ast;

use super::ParseContext;
use super::common::{ParseResult, Pair, Rule, get_pos, grammar_error};

impl<'a> ParseContext<'a> {
    /// Parses a `code_block_stmt` node into a boxed [`ast::CodeBlockStmt`].
    ///
    /// Dispatches to the appropriate statement parser depending on the inner
    /// rule:
    /// * `var_decl_stmt`  → [`Self::parse_var_decl_stmt`]
    /// * `assignment_stmt` → [`Self::parse_assignment_stmt`]
    /// * `call_stmt`       → [`Self::parse_call_stmt`]
    /// * `if_stmt`         → [`Self::parse_if_stmt`]
    /// * `while_stmt`      → [`Self::parse_while_stmt`]
    /// * `return_stmt`     → [`Self::parse_return_stmt`]
    /// * `continue_stmt`   → wraps a unit [`ast::ContinueStmt`]
    /// * `break_stmt`      → wraps a unit [`ast::BreakStmt`]
    /// * `null_stmt`       → wraps a unit [`ast::NullStmt`]
    ///
    /// Returns [`Error::Grammar`] if no recognisable inner rule is found.
    ///
    /// # Arguments
    /// * `pair` – the `code_block_stmt` parse-tree node.
    pub(crate) fn parse_code_block_stmt(&self, pair: Pair) -> ParseResult<Box<ast::CodeBlockStmt>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::var_decl_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::VarDecl(self.parse_var_decl_stmt(inner)?),
                    }));
                }
                Rule::assignment_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Assignment(
                            self.parse_assignment_stmt(inner)?,
                        ),
                    }));
                }
                Rule::call_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Call(self.parse_call_stmt(inner)?),
                    }));
                }
                Rule::if_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::If(self.parse_if_stmt(inner)?),
                    }));
                }
                Rule::while_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::While(self.parse_while_stmt(inner)?),
                    }));
                }
                Rule::return_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Return(self.parse_return_stmt(inner)?),
                    }));
                }
                // `continue` and `break` carry no additional data.
                Rule::continue_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Continue(Box::new(ast::ContinueStmt {})),
                    }));
                }
                Rule::break_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Break(Box::new(ast::BreakStmt {})),
                    }));
                }
                // A null statement is a bare semicolon; nothing to parse.
                Rule::null_stmt => {
                    return Ok(Box::new(ast::CodeBlockStmt {
                        inner: ast::CodeBlockStmtInner::Null(Box::new(ast::NullStmt {})),
                    }));
                }
                _ => {}
            }
        }

        Err(grammar_error("code_block_stmt", &pair_for_error))
    }

    /// Parses an `assignment_stmt` node into a boxed [`ast::AssignmentStmt`].
    ///
    /// An assignment has the form `left_val = right_val;`.  Both operands are
    /// required; [`Error::Grammar`] is returned if either is absent.
    ///
    /// # Arguments
    /// * `pair` – the `assignment_stmt` parse-tree node.
    fn parse_assignment_stmt(&self, pair: Pair) -> ParseResult<Box<ast::AssignmentStmt>> {
        let pair_for_error = pair.clone();
        let mut left_val = None;
        let mut right_val = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::left_val => left_val = Some(self.parse_left_val(inner)?),
                Rule::right_val => right_val = Some(self.parse_right_val(inner)?),
                _ => {}
            }
        }

        Ok(Box::new(ast::AssignmentStmt {
            left_val: left_val
                .ok_or_else(|| grammar_error("assignment.left_val", &pair_for_error))?,
            right_val: right_val
                .ok_or_else(|| grammar_error("assignment.right_val", &pair_for_error))?,
        }))
    }

    /// Parses a `call_stmt` node into a boxed [`ast::CallStmt`].
    ///
    /// A call statement is a standalone function call used for its side
    /// effects: `func(args);`.  Returns [`Error::Grammar`] if the expected
    /// `fn_call` child is absent.
    ///
    /// # Arguments
    /// * `pair` – the `call_stmt` parse-tree node.
    fn parse_call_stmt(&self, pair: Pair) -> ParseResult<Box<ast::CallStmt>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::fn_call {
                return Ok(Box::new(ast::CallStmt {
                    fn_call: self.parse_fn_call(inner)?,
                }));
            }
        }

        Err(grammar_error("call_stmt", &pair_for_error))
    }

    /// Parses a `return_stmt` node into a boxed [`ast::ReturnStmt`].
    ///
    /// The return value is optional: `return;` and `return expr;` are both
    /// valid.  When present, the expression is parsed as a `right_val`.
    ///
    /// # Arguments
    /// * `pair` – the `return_stmt` parse-tree node.
    fn parse_return_stmt(&self, pair: Pair) -> ParseResult<Box<ast::ReturnStmt>> {
        let mut val = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::right_val {
                val = Some(self.parse_right_val(inner)?);
            }
        }

        Ok(Box::new(ast::ReturnStmt { val }))
    }

    /// Parses an `if_stmt` node into a boxed [`ast::IfStmt`].
    ///
    /// An `if` statement has the form:
    /// ```text
    /// if <bool_expr> { <body> } [else { <else_body> }]
    /// ```
    /// The condition is parsed as a `bool_expr` wrapped in a `BoolUnit`.
    /// Body statements are collected into `if_stmts`; once the `else` keyword
    /// token is encountered subsequent `code_block_stmt` nodes are collected
    /// into `else_stmts`.
    ///
    /// Returns [`Error::Grammar`] if no condition is found.
    ///
    /// # Arguments
    /// * `pair` – the `if_stmt` parse-tree node.
    fn parse_if_stmt(&self, pair: Pair) -> ParseResult<Box<ast::IfStmt>> {
        let pair_for_error = pair.clone();
        let mut bool_unit = None;
        let mut if_stmts = Vec::new();
        let mut else_stmts = None;
        // Track whether we have passed the `else` keyword.
        let mut in_else = false;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::bool_expr => {
                    let pos = get_pos(&inner);
                    let bool_expr = self.parse_bool_expr(inner)?;
                    // Wrap the condition expression in a BoolUnit node.
                    bool_unit = Some(Box::new(ast::BoolUnit {
                        pos,
                        inner: ast::BoolUnitInner::BoolExpr(bool_expr),
                    }));
                }
                Rule::code_block_stmt => {
                    if in_else {
                        // Append to the else branch, creating the Vec on first use.
                        let else_branch = else_stmts.get_or_insert_with(Vec::new);
                        else_branch.push(*self.parse_code_block_stmt(inner)?);
                    } else {
                        if_stmts.push(*self.parse_code_block_stmt(inner)?);
                    }
                }
                // The `else` keyword marks the start of the else branch.
                Rule::kw_else => {
                    in_else = true;
                }
                _ => {}
            }
        }

        Ok(Box::new(ast::IfStmt {
            bool_unit: bool_unit.ok_or_else(|| grammar_error("cond.bool_unit", &pair_for_error))?,
            if_stmts,
            else_stmts,
        }))
    }

    /// Parses a `while_stmt` node into a boxed [`ast::WhileStmt`].
    ///
    /// A `while` statement has the form:
    /// ```text
    /// while <bool_expr> { <body> }
    /// ```
    /// The condition is parsed as a `bool_expr` wrapped in a `BoolUnit` and
    /// all body statements are collected in order.
    ///
    /// Returns [`Error::Grammar`] if no condition is found.
    ///
    /// # Arguments
    /// * `pair` – the `while_stmt` parse-tree node.
    fn parse_while_stmt(&self, pair: Pair) -> ParseResult<Box<ast::WhileStmt>> {
        let pair_for_error = pair.clone();
        let mut bool_unit = None;
        let mut stmts = Vec::new();

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::bool_expr => {
                    let pos = get_pos(&inner);
                    let bool_expr = self.parse_bool_expr(inner)?;
                    // Wrap the condition expression in a BoolUnit node.
                    bool_unit = Some(Box::new(ast::BoolUnit {
                        pos,
                        inner: ast::BoolUnitInner::BoolExpr(bool_expr),
                    }));
                }
                Rule::code_block_stmt => {
                    stmts.push(*self.parse_code_block_stmt(inner)?);
                }
                _ => {}
            }
        }

        Ok(Box::new(ast::WhileStmt {
            bool_unit: bool_unit
                .ok_or_else(|| grammar_error("cond.bool_unit", &pair_for_error))?,
            stmts,
        }))
    }
}
