//! Parser module for the TeaLang compiler front-end.
//!
//! Transforms a raw TeaLang source string into a typed Abstract Syntax Tree
//! (AST).  The [pest] PEG parser generator tokenises and structurally parses
//! the source according to the grammar defined in `tealang.pest`; the
//! resulting parse tree is then lowered into the AST types defined in
//! [`crate::ast`].
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
/// [`Generator::generate`] must be called after construction before
/// accessing the [`Parser::program`] field.
pub struct Parser<'a> {
    input: &'a str,
    /// The parsed AST program, populated by [`Generator::generate`].
    /// `None` until `generate` completes successfully.
    pub program: Option<Box<ast::Program>>,
}

impl<'a> Parser<'a> {
    /// Creates a new `Parser` for the given source string.
    ///
    /// Parsing is not run yet; [`Generator::generate`] performs parsing and
    /// populates [`Parser::program`].
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
            .ok_or_else(|| grammar_error_static("output before generate"))?;
        write!(w, "{ast}")?;
        Ok(())
    }
}

/// Internal context that owns a single parse pass over one source string.
pub(crate) struct ParseContext<'a> {
    #[allow(dead_code)]
    input: &'a str,
}

impl<'a> ParseContext<'a> {
    fn new(input: &'a str) -> Self {
        Self { input }
    }

    fn parse(&self) -> ParseResult<Box<ast::Program>> {
        let pairs = <TeaLangParser as PestParser<Rule>>::parse(Rule::program, self.input)
            .map_err(|e| {
                // Pest reports either a single position or a span; the span's
                // start position populates the structured fields.
                let (line, column) = match e.line_col {
                    pest::error::LineColLocation::Pos(pos) => pos,
                    pest::error::LineColLocation::Span(start, _) => start,
                };
                Error::Syntax {
                    line,
                    column,
                    message: e.to_string(),
                }
            })?;

        let mut use_stmts = Vec::new();
        let mut elements = Vec::new();

        for pair in pairs {
            if pair.as_rule() == Rule::program {
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
                        // End-of-input marker.
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
