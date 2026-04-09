//! Parser module for the TeaLang compiler front-end.
//!
//! This module is responsible for transforming a raw TeaLang source string into
//! a typed Abstract Syntax Tree (AST).  It uses the [pest] PEG parser generator
//! to tokenise and structurally parse the source according to the grammar
//! defined in `tealang.pest`, and then walks the resulting parse tree to build
//! the AST types defined in [`crate::ast`].
//!
//! # Main entry points
//! * [`Parser`] – the public façade that implements [`crate::common::Generator`].
//! * [`ParseContext`] – an internal helper that owns a single parse-and-lower
//!   pass over one source string.
//!
//! Sub-modules handle different grammatical categories:
//! * `common`  – shared utilities (error types, helper functions)
//! * `decl`    – declaration and definition rules
//! * `expr`    – expression rules
//! * `stmt`    – statement rules

mod common;
mod decl;
mod expr;
mod stmt;

use std::io::Write;

use pest::Parser as PestParser;

use crate::ast;
use crate::common::Generator;

pub use self::common::Error;
use self::common::{grammar_error_static, ParseResult, Rule, TeaLangParser};

/// Public parser that turns a TeaLang source string into an AST.
///
/// After construction with [`Parser::new`] you must call
/// [`Generator::generate`] before accessing the [`Parser::program`] field.
pub struct Parser<'a> {
    /// The raw TeaLang source text to be parsed.
    input: &'a str,
    /// The parsed AST program, populated by [`Generator::generate`].
    /// `None` until `generate` completes successfully.
    pub program: Option<Box<ast::Program>>,
}

impl<'a> Parser<'a> {
    /// Creates a new `Parser` for the given source string.
    ///
    /// The parser is not yet run; call [`Generator::generate`] to perform
    /// parsing and populate [`Parser::program`].
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            program: None,
        }
    }
}

impl<'a> Generator for Parser<'a> {
    type Error = Error;

    /// Runs the full parse pipeline: tokenisation → parse-tree → AST.
    ///
    /// On success the resulting [`ast::Program`] is stored in
    /// [`Parser::program`].  Any syntax or structural error is returned as
    /// [`Error`].
    fn generate(&mut self) -> Result<(), Error> {
        let ctx = ParseContext::new(self.input);
        self.program = Some(ctx.parse()?);
        Ok(())
    }

    /// Writes a pretty-printed representation of the parsed program to `w`.
    ///
    /// Returns [`Error::Grammar`] if called before [`Generator::generate`].
    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let ast = self
            .program
            .as_ref()
            // Guard: generate() must be called before output().
            .ok_or_else(|| grammar_error_static("output before generate"))?;
        write!(w, "{ast}")?;
        Ok(())
    }
}

/// Internal context that owns a single parse pass over one source string.
///
/// `ParseContext` is constructed by [`Parser`] and carries the source slice so
/// that all parser helper methods can reference it if needed.
pub(crate) struct ParseContext<'a> {
    #[allow(dead_code)]
    /// The original source text being parsed.
    input: &'a str,
}

impl<'a> ParseContext<'a> {
    /// Creates a new `ParseContext` for the given source string.
    fn new(input: &'a str) -> Self {
        Self { input }
    }

    /// Parses the full source string into a boxed [`ast::Program`].
    ///
    /// Uses [`TeaLangParser`] to produce a parse tree for the `program` rule,
    /// then iterates over top-level nodes to collect `use` statements and
    /// program elements (variable declarations, struct definitions, function
    /// declarations and definitions).
    fn parse(&self) -> ParseResult<Box<ast::Program>> {
        // Run the pest parser; convert any pest::Error into Error::Syntax.
        let pairs = <TeaLangParser as PestParser<Rule>>::parse(Rule::program, self.input)
            .map_err(|e| Error::Syntax(e.to_string()))?;

        let mut use_stmts = Vec::new();
        let mut elements = Vec::new();

        for pair in pairs {
            if pair.as_rule() == Rule::program {
                // Walk the top-level children of the `program` node.
                for inner in pair.into_inner() {
                    match inner.as_rule() {
                        Rule::use_stmt => {
                            use_stmts.push(self.parse_use_stmt(inner)?);
                        }
                        Rule::program_element => {
                            if let Some(elem) = self.parse_program_element(inner)? {
                                elements.push(*elem);
                            }
                        }
                        // End-of-input marker; nothing to do.
                        Rule::EOI => {}
                        _ => {}
                    }
                }
            }
        }

        Ok(Box::new(ast::Program {
            use_stmts,
            elements,
        }))
    }
}
