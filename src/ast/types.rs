//! Type definitions used throughout the AST.
//!
//! This module defines the source-position alias and all type-specifier
//! nodes that appear in variable declarations, function parameters, and
//! return-type annotations.

/// Byte offset (or character index) into the source text.
/// Used to track where each AST node originated for error reporting.
pub type Pos = usize;

/// Built-in primitive types supported by the language.
#[derive(Debug, Clone)]
pub enum BuiltIn {
    /// The 32-bit signed integer type (`int`).
    Int,
}

/// The inner representation of a type specifier, distinguishing between
/// built-in primitives, user-defined composite types, and reference types.
#[derive(Debug, Clone)]
pub enum TypeSpecifierInner {
    /// A primitive type such as `int`.
    BuiltIn(BuiltIn),
    /// A user-defined struct or composite type, identified by name.
    Composite(String),
    /// A reference to another type specifier (e.g., `&int`).
    Reference(Box<TypeSpecifier>),
}

/// A fully-annotated type specifier, pairing the type's inner representation
/// with the source position where it appears.
#[derive(Debug, Clone)]
pub struct TypeSpecifier {
    /// Source position of this type specifier.
    pub pos: Pos,
    /// The actual type information (built-in, composite, or reference).
    pub inner: TypeSpecifierInner,
}
