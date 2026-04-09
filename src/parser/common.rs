use pest_derive::Parser as DeriveParser;

/// Errors that can be produced during parsing of a TeaLang source file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A syntax error reported directly by the PEG parser (pest).
    /// The inner `String` contains the human-readable pest error message.
    #[error("{0}")]
    Syntax(String),

    /// An integer literal that could not be parsed as a valid `i32`.
    /// Includes the original source text, its position, and the underlying
    /// `ParseIntError` as the error source.
    #[error("invalid integer literal `{literal}` at line {line}, column {column}")]
    InvalidNumber {
        literal: String,
        line: usize,
        column: usize,
        #[source]
        source: std::num::ParseIntError,
    },

    /// An I/O error encountered while reading source input.
    #[error("I/O error")]
    Io(#[from] std::io::Error),

    /// The parse tree had an unexpected structure at the given location.
    /// The inner `String` names the grammar rule or context where the
    /// unexpected structure was found.
    #[error("unexpected parse tree structure in {0}")]
    Grammar(String),
}

/// The pest-derived parser for the TeaLang grammar.
/// It is generated automatically from `tealang.pest` and implements the
/// [`pest::Parser`] trait for the [`Rule`] enum.
#[derive(DeriveParser)]
#[grammar = "tealang.pest"]
pub(crate) struct TeaLangParser;

/// A specialized `Result` type used throughout the parser.
/// `Ok` carries a successfully parsed value of type `T`; `Err` carries an
/// [`Error`] describing what went wrong.
pub(crate) type ParseResult<T> = Result<T, Error>;

/// A single node in the pest parse tree, parameterised by the input lifetime.
/// This is a type alias for [`pest::iterators::Pair`] bound to the [`Rule`]
/// enum produced by [`TeaLangParser`].
pub(crate) type Pair<'a> = pest::iterators::Pair<'a, Rule>;

/// Collapses a raw source snippet into a compact, single-line preview string
/// suitable for use in error messages.
///
/// All runs of whitespace are collapsed to a single space, and the result is
/// truncated to at most `MAX_CHARS` characters.  If the snippet is empty after
/// normalisation, the placeholder `"<empty>"` is returned instead.
pub(crate) fn compact_snippet(snippet: &str) -> String {
    const MAX_CHARS: usize = 48;

    // Collapse all whitespace sequences into a single space.
    let compact = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    // Fall back to trimming if the split produced nothing (e.g., all whitespace).
    let normalized = if compact.is_empty() {
        snippet.trim().to_string()
    } else {
        compact
    };

    if normalized.is_empty() {
        return "<empty>".to_string();
    }

    // Take up to MAX_CHARS characters; append "..." if the string is longer.
    let mut chars = normalized.chars();
    let preview: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

/// Creates a [`Error::Grammar`] variant that includes the source position and
/// a compact snippet taken from `pair`'s span.
///
/// `context` is a short, human-readable label that identifies the grammar rule
/// or function where the unexpected structure was encountered.
pub(crate) fn grammar_error(context: &'static str, pair: &Pair<'_>) -> Error {
    let span = pair.as_span();
    // Extract line and column numbers from the start of the span.
    let (line, column) = span.start_pos().line_col();
    let near = compact_snippet(span.as_str());

    Error::Grammar(format!(
        "{context} at line {line}, column {column}, near `{near}`"
    ))
}

/// Creates a [`Error::Grammar`] variant from a static string alone, without
/// access to a specific parse-tree node.
///
/// Use this when position information is unavailable (e.g., when validating
/// program state rather than a particular source span).
pub(crate) fn grammar_error_static(context: &'static str) -> Error {
    Error::Grammar(context.to_string())
}

/// Returns the byte offset of the start of `pair`'s span within the source
/// string.  This is used to track source positions in AST nodes.
pub(crate) fn get_pos(pair: &Pair<'_>) -> usize {
    pair.as_span().start()
}

/// Parses an integer literal from a `num` parse-tree node.
///
/// Reads the raw text of `pair`, attempts to parse it as an `i32`, and wraps
/// any failure in [`Error::InvalidNumber`] that includes the literal text and
/// its source position.
pub(crate) fn parse_num(pair: Pair) -> ParseResult<i32> {
    let literal = pair.as_str().to_string();
    let (line, column) = pair.as_span().start_pos().line_col();

    literal.parse().map_err(|source| Error::InvalidNumber {
        literal,
        line,
        column,
        source,
    })
}
