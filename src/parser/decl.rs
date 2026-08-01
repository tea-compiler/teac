//! Parsing of TeaLang declaration and definition rules.
//!
//! This submodule implements the [`ParseContext`] methods
//! that lower declaration-oriented parse-tree nodes into the corresponding AST
//! types: `use` statements, program elements, struct definitions, typed
//! variable declarations and definitions (including array initialisers), and
//! function declarations and definitions.

use crate::ast;

use super::common::{get_pos, grammar_error, parse_num, Pair, ParseResult, Rule};
use super::ParseContext;

impl<'a> ParseContext<'a> {
    /// Parses a `use_stmt` parse-tree node (`use module::path;`) into an
    /// [`ast::UseStmt`].
    pub(crate) fn parse_use_stmt(&self, pair: Pair) -> ParseResult<ast::UseStmt> {
        let parts: Vec<&str> = pair
            .into_inner()
            .filter(|p| p.as_rule() == Rule::identifier)
            .map(|p| p.as_str())
            .collect();
        Ok(ast::UseStmt {
            module_name: parts.join("::"),
        })
    }

    /// Parses a `program_element` node into an optional boxed [`ast::ProgramElement`].
    ///
    /// A program element is one of: a variable declaration statement, a struct
    /// definition, a function declaration statement, or a function definition.
    /// Returns `None` if the node contains no recognised inner rule; a
    /// well-formed parse tree never triggers this.
    pub(crate) fn parse_program_element(
        &self,
        pair: Pair,
    ) -> ParseResult<Option<Box<ast::ProgramElement>>> {
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::var_decl_stmt => {
                    return Ok(Some(Box::new(ast::ProgramElement {
                        inner: ast::ProgramElementInner::VarDeclStmt(
                            self.parse_var_decl_stmt(inner)?,
                        ),
                    })));
                }
                Rule::struct_def => {
                    return Ok(Some(Box::new(ast::ProgramElement {
                        inner: ast::ProgramElementInner::StructDef(self.parse_struct_def(inner)?),
                    })));
                }
                Rule::fn_decl_stmt => {
                    return Ok(Some(Box::new(ast::ProgramElement {
                        inner: ast::ProgramElementInner::FnDeclStmt(
                            self.parse_fn_decl_stmt(inner)?,
                        ),
                    })));
                }
                Rule::fn_def => {
                    return Ok(Some(Box::new(ast::ProgramElement {
                        inner: ast::ProgramElementInner::FnDef(self.parse_fn_def(inner)?),
                    })));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// Parses a `struct_def` node (`struct Name { field_list }`) into a boxed
    /// [`ast::StructDef`]; field parsing delegates to
    /// [`Self::parse_typed_var_decl_list`].
    pub(crate) fn parse_struct_def(&self, pair: Pair) -> ParseResult<Box<ast::StructDef>> {
        let mut identifier = String::new();
        let mut decls = Vec::new();

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => identifier = inner.as_str().to_string(),
                Rule::typed_var_decl_list => decls = self.parse_typed_var_decl_list(inner)?,
                _ => {}
            }
        }

        Ok(Box::new(ast::StructDef { identifier, decls }))
    }

    pub(crate) fn parse_typed_var_decl_list(&self, pair: Pair) -> ParseResult<Vec<ast::VarDecl>> {
        let mut decls = Vec::new();
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::typed_var_decl {
                decls.push(*self.parse_var_decl(inner)?);
            }
        }
        Ok(decls)
    }

    /// Parses a `typed_var_decl` (or `var_decl`) node into a boxed
    /// [`ast::VarDecl`]: variable name, optional type specifier, and — for
    /// array declarations — the array length.
    ///
    /// Returns [`Error::Grammar`](super::common::Error::Grammar) if the
    /// identifier child is missing.
    pub(crate) fn parse_var_decl(&self, pair: Pair) -> ParseResult<Box<ast::VarDecl>> {
        // Cloned because into_inner() consumes `pair`; error reporting still
        // needs the node afterwards.
        let pair_for_error = pair.clone();
        let mut identifier: Option<String> = None;
        let mut type_specifier: Option<ast::TypeSpecifier> = None;
        let mut array_len: Option<usize> = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                // Only the first identifier child is the variable name.
                Rule::identifier if identifier.is_none() => {
                    identifier = Some(inner.as_str().to_string());
                }
                Rule::type_spec => {
                    type_specifier = self.parse_type_spec(inner)?;
                }
                // A numeric literal indicates an array declaration.
                Rule::num => {
                    array_len = Some(parse_num(inner)? as usize);
                }
                _ => {}
            }
        }

        let identifier =
            identifier.ok_or_else(|| grammar_error("var_decl.identifier", &pair_for_error))?;
        let inner = if let Some(len) = array_len {
            ast::VarDeclInner::Array(Box::new(ast::VarDeclArray { len }))
        } else {
            ast::VarDeclInner::Scalar
        };

        Ok(Box::new(ast::VarDecl {
            identifier,
            type_specifier,
            inner,
        }))
    }

    /// Parses a `type_spec` node into an optional [`ast::TypeSpecifier`].
    ///
    /// Recognises reference types (`&T`), the built-in `i32` keyword, and
    /// user-defined composite (struct) types by their identifier.  Returns
    /// `Ok(None)` when the node is empty or contains no recognised type rule,
    /// and [`Error::Grammar`](super::common::Error::Grammar) if a reference
    /// type is missing or has an empty inner type specifier.
    pub(crate) fn parse_type_spec(&self, pair: Pair) -> ParseResult<Option<ast::TypeSpecifier>> {
        let pair_for_error = pair.clone();
        // Captured before `into_inner` consumes `pair`.
        let pos = get_pos(&pair);

        let children: Vec<_> = pair.into_inner().collect();

        for child in &children {
            match child.as_rule() {
                Rule::ref_type => {
                    // A reference type wraps an inner type_spec: `&<inner>`.
                    let ref_children: Vec<_> = child.clone().into_inner().collect();
                    let inner_type_spec = ref_children
                        .iter()
                        .find(|c| c.as_rule() == Rule::type_spec)
                        .ok_or_else(|| grammar_error("ref_type.type_spec", &pair_for_error))?;
                    let inner_ts = self
                        .parse_type_spec(inner_type_spec.clone())?
                        .ok_or_else(|| grammar_error("ref_type.empty", &pair_for_error))?;
                    return Ok(Some(ast::TypeSpecifier {
                        pos,
                        inner: ast::TypeSpecifierInner::Reference(Box::new(inner_ts)),
                    }));
                }
                Rule::kw_i32 => {
                    return Ok(Some(ast::TypeSpecifier {
                        pos,
                        inner: ast::TypeSpecifierInner::BuiltIn(ast::BuiltIn::Int),
                    }));
                }
                Rule::identifier => {
                    return Ok(Some(ast::TypeSpecifier {
                        pos,
                        inner: ast::TypeSpecifierInner::Composite(child.as_str().to_string()),
                    }));
                }
                _ => {}
            }
        }

        Ok(None)
    }

    /// Parses a `var_decl_stmt` node into a boxed [`ast::VarDeclStmt`].
    ///
    /// A variable declaration statement is either a variable definition
    /// (`var_def`, e.g. `let x = 1;`) or a plain declaration (`var_decl`,
    /// e.g. `let x: i32;`).  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if neither child is
    /// present.
    pub(crate) fn parse_var_decl_stmt(&self, pair: Pair) -> ParseResult<Box<ast::VarDeclStmt>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::var_def => {
                    return Ok(Box::new(ast::VarDeclStmt {
                        inner: ast::VarDeclStmtInner::Def(self.parse_var_def(inner)?),
                    }));
                }
                Rule::var_decl => {
                    return Ok(Box::new(ast::VarDeclStmt {
                        inner: ast::VarDeclStmtInner::Decl(self.parse_var_decl(inner)?),
                    }));
                }
                _ => {}
            }
        }

        Err(grammar_error("var_decl_stmt", &pair_for_error))
    }

    /// Parses a `var_def` node into a boxed [`ast::VarDef`].
    ///
    /// Handles both scalar and array variable definitions.  An array definition
    /// contains an `array_initializer` child; a scalar definition contains a
    /// `right_val` child.  The type annotation (after `:`) is optional in both
    /// forms.
    pub(crate) fn parse_var_def(&self, pair: Pair) -> ParseResult<Box<ast::VarDef>> {
        let pair_for_error = pair.clone();
        let inner_pairs: Vec<_> = pair.into_inner().collect();

        // The first child is always the variable name identifier.
        let identifier = inner_pairs
            .first()
            .ok_or_else(|| grammar_error("var_def.identifier", &pair_for_error))?
            .as_str()
            .to_string();

        let has_initializer = inner_pairs
            .iter()
            .any(|p| p.as_rule() == Rule::array_initializer);
        let has_colon = inner_pairs.iter().any(|p| p.as_rule() == Rule::colon);

        if has_initializer {
            // Array definition: `let arr[N]: T = [...]` or `let arr[N] = [v; N]`.
            let len = parse_num(
                inner_pairs
                    .iter()
                    .find(|p| p.as_rule() == Rule::num)
                    .ok_or_else(|| grammar_error("var_def.array_len", &pair_for_error))?
                    .clone(),
            )? as usize;

            let type_specifier = if has_colon {
                self.parse_type_spec(
                    inner_pairs
                        .iter()
                        .find(|p| p.as_rule() == Rule::type_spec)
                        .ok_or_else(|| grammar_error("var_def.type_spec", &pair_for_error))?
                        .clone(),
                )?
            } else {
                None
            };

            let initializer = self.parse_array_initializer(
                inner_pairs
                    .iter()
                    .find(|p| p.as_rule() == Rule::array_initializer)
                    .ok_or_else(|| grammar_error("var_def.array_init", &pair_for_error))?
                    .clone(),
            )?;

            Ok(Box::new(ast::VarDef {
                identifier,
                type_specifier,
                inner: ast::VarDefInner::Array(Box::new(ast::VarDefArray { len, initializer })),
            }))
        } else {
            // Scalar definition: `let x: T = expr` or `let x = expr`.
            let type_specifier = if has_colon {
                self.parse_type_spec(
                    inner_pairs
                        .iter()
                        .find(|p| p.as_rule() == Rule::type_spec)
                        .ok_or_else(|| grammar_error("var_def.type_spec", &pair_for_error))?
                        .clone(),
                )?
            } else {
                None
            };

            let val = self.parse_right_val(
                inner_pairs
                    .iter()
                    .find(|p| p.as_rule() == Rule::right_val)
                    .ok_or_else(|| grammar_error("var_def.val", &pair_for_error))?
                    .clone(),
            )?;

            Ok(Box::new(ast::VarDef {
                identifier,
                type_specifier,
                inner: ast::VarDefInner::Scalar(Box::new(ast::VarDefScalar { val })),
            }))
        }
    }

    /// Parses an `array_initializer` node into an [`ast::ArrayInitializer`].
    ///
    /// Two forms are supported:
    /// * **Explicit list** – `[v0, v1, v2]`: contains a `right_val_list`.
    /// * **Fill** – `[v; N]`: contains a single `right_val` and a `num`.
    fn parse_array_initializer(&self, pair: Pair) -> ParseResult<ast::ArrayInitializer> {
        let pair_for_error = pair.clone();
        let children: Vec<_> = pair.into_inner().collect();

        if let Some(list_pair) = children
            .iter()
            .find(|p| p.as_rule() == Rule::right_val_list)
        {
            let vals = self.parse_right_val_list(list_pair.clone())?;
            return Ok(ast::ArrayInitializer::ExplicitList(vals));
        }

        // Fill form `[val; count]`.
        let val_pair = children
            .iter()
            .find(|p| p.as_rule() == Rule::right_val)
            .ok_or_else(|| grammar_error("array_initializer.val", &pair_for_error))?;
        let count_pair = children
            .iter()
            .find(|p| p.as_rule() == Rule::num)
            .ok_or_else(|| grammar_error("array_initializer.count", &pair_for_error))?;

        let val = self.parse_right_val(val_pair.clone())?;
        let count = parse_num(count_pair.clone())? as usize;

        Ok(ast::ArrayInitializer::Fill { val, count })
    }

    /// Parses a `fn_decl_stmt` node into a boxed [`ast::FnDeclStmt`].
    ///
    /// A function declaration statement wraps a single `fn_decl` child.
    /// Returns [`Error::Grammar`](super::common::Error::Grammar) if the
    /// expected child is absent.
    pub(crate) fn parse_fn_decl_stmt(&self, pair: Pair) -> ParseResult<Box<ast::FnDeclStmt>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::fn_decl {
                return Ok(Box::new(ast::FnDeclStmt {
                    fn_decl: self.parse_fn_decl(inner)?,
                }));
            }
        }

        Err(grammar_error("fn_decl_stmt", &pair_for_error))
    }

    fn parse_fn_decl(&self, pair: Pair) -> ParseResult<Box<ast::FnDecl>> {
        let mut identifier = String::new();
        let mut param_decl = None;
        let mut return_dtype = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => identifier = inner.as_str().to_string(),
                Rule::param_decl => param_decl = Some(self.parse_param_decl(inner)?),
                // The optional return type follows `->`.
                Rule::type_spec => return_dtype = self.parse_type_spec(inner)?,
                _ => {}
            }
        }

        Ok(Box::new(ast::FnDecl {
            identifier,
            param_decl,
            return_dtype,
        }))
    }

    /// Parses a `param_decl` node into a boxed [`ast::ParamDecl`].
    ///
    /// A parameter declaration wraps a `typed_var_decl_list` holding the
    /// formal parameters with their types.  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if the expected child
    /// is absent.
    fn parse_param_decl(&self, pair: Pair) -> ParseResult<Box<ast::ParamDecl>> {
        let pair_for_error = pair.clone();
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::typed_var_decl_list {
                return Ok(Box::new(ast::ParamDecl {
                    decls: self.parse_typed_var_decl_list(inner)?,
                }));
            }
        }
        Err(grammar_error("param_decl", &pair_for_error))
    }

    /// Parses a `fn_def` node into a boxed [`ast::FnDef`].
    ///
    /// A function definition contains a `fn_decl` header followed by one or
    /// more `code_block_stmt` nodes that form the function body.  Returns
    /// [`Error::Grammar`](super::common::Error::Grammar) if the `fn_decl`
    /// child is absent.
    pub(crate) fn parse_fn_def(&self, pair: Pair) -> ParseResult<Box<ast::FnDef>> {
        let pair_for_error = pair.clone();
        let mut fn_decl = None;
        let mut stmts = Vec::new();

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::fn_decl => fn_decl = Some(self.parse_fn_decl(inner)?),
                Rule::code_block_stmt => stmts.push(*self.parse_code_block_stmt(inner)?),
                _ => {}
            }
        }

        Ok(Box::new(ast::FnDef {
            fn_decl: fn_decl.ok_or_else(|| grammar_error("fn_def.fn_decl", &pair_for_error))?,
            stmts,
        }))
    }
}
