//! Conversions between AST types and IR types.
//!
//! This module provides trait implementations and helper functions to convert
//! AST-level type representations (`ast::TypeSpecifier`, `ast::VarDecl`,
//! `ast::VarDef`, `ast::VarDeclStmt`) into their corresponding IR-level
//! data types (`Dtype`), as well as implementations of the `Named` trait
//! for extracting identifiers from AST declaration nodes.

use crate::ast;
use crate::ir::types::Dtype;
use crate::ir::value::Named;

/// Converts an optional AST type specifier into the corresponding base IR data type (`Dtype`).
///
/// - `Composite` type specifiers (e.g., user-defined structs) map to `Dtype::Struct`.
/// - `Reference` type specifiers (e.g., `&[T]`) map to a pointer to an unsized array,
///   where the element type is resolved recursively.
/// - `BuiltIn` type specifiers (e.g., `i32`) and `None` (absent specifier) both default
///   to `Dtype::I32`.
fn base_dtype(type_specifier: &Option<ast::TypeSpecifier>) -> Dtype {
    match type_specifier.as_ref().map(|t| &t.inner) {
        Some(ast::TypeSpecifierInner::Composite(name)) => Dtype::Struct {
            type_name: name.to_string(),
        },
        Some(ast::TypeSpecifierInner::Reference(inner)) => Dtype::ptr_to(Dtype::Array {
            element: Box::new(base_dtype(&Some(inner.as_ref().clone()))),
            length: None,
        }),
        Some(ast::TypeSpecifierInner::BuiltIn(_)) | None => Dtype::I32,
    }
}

// --- `Named` trait implementations ---
// These allow AST declaration nodes to expose their identifier strings
// in a uniform way, which is used during IR generation to name variables.

/// Implements `Named` for a variable declaration (without initializer),
/// returning the declared identifier.
impl Named for ast::VarDecl {
    fn identifier(&self) -> Option<String> {
        Some(self.identifier.clone())
    }
}

/// Implements `Named` for a variable definition (declaration with initializer),
/// returning the defined identifier.
impl Named for ast::VarDef {
    fn identifier(&self) -> Option<String> {
        Some(self.identifier.clone())
    }
}

/// Implements `Named` for a variable declaration statement, which may be
/// either a pure declaration or a definition. Delegates to the inner variant
/// to retrieve the identifier.
impl Named for ast::VarDeclStmt {
    fn identifier(&self) -> Option<String> {
        match &self.inner {
            ast::VarDeclStmtInner::Decl(d) => Some(d.identifier.clone()),
            ast::VarDeclStmtInner::Def(d) => Some(d.identifier.clone()),
        }
    }
}

// --- `From` trait implementations: AST TypeSpecifier -> IR Dtype ---
// These provide infallible conversions from AST type specifiers to IR types.

/// Converts an owned `ast::TypeSpecifier` into a `Dtype` by delegating to the
/// by-reference implementation.
impl From<ast::TypeSpecifier> for Dtype {
    fn from(a: ast::TypeSpecifier) -> Self {
        Self::from(&a)
    }
}

/// Converts a reference to an `ast::TypeSpecifier` into the corresponding `Dtype`.
///
/// - `BuiltIn` maps to `Dtype::I32` (the only built-in type is `i32`).
/// - `Composite` maps to `Dtype::Struct` with the user-defined type name.
/// - `Reference` maps to a pointer to an unsized array whose element type
///   is recursively converted from the inner type specifier.
impl From<&ast::TypeSpecifier> for Dtype {
    fn from(a: &ast::TypeSpecifier) -> Self {
        match &a.inner {
            ast::TypeSpecifierInner::BuiltIn(_) => Self::I32,
            ast::TypeSpecifierInner::Composite(name) => Self::Struct {
                type_name: name.to_string(),
            },
            ast::TypeSpecifierInner::Reference(inner) => Self::ptr_to(Dtype::Array {
                element: Box::new(Self::from(inner.as_ref())),
                length: None,
            }),
        }
    }
}

// --- `TryFrom` trait implementations: AST declarations -> IR Dtype ---
// These are fallible conversions because certain combinations (e.g., struct
// definitions with initializers) are not supported and produce an error.

/// Converts a variable declaration (`VarDecl`) to its IR data type.
///
/// First resolves the base type from the optional type specifier, then wraps it
/// in an array type if the declaration is for an array (with a known length),
/// or returns the base type directly for scalar declarations.
impl TryFrom<&ast::VarDecl> for Dtype {
    type Error = crate::ir::Error;

    fn try_from(decl: &ast::VarDecl) -> Result<Self, Self::Error> {
        let base_dtype = base_dtype(&decl.type_specifier);
        match &decl.inner {
            ast::VarDeclInner::Array(decl) => Ok(Dtype::array_of(base_dtype, decl.len)),
            ast::VarDeclInner::Scalar => Ok(base_dtype),
        }
    }
}

/// Converts a variable definition (`VarDef`) to its IR data type.
///
/// Similar to the `VarDecl` conversion, but additionally rejects struct types
/// with initializers—struct variables cannot be initialized inline, so
/// attempting to do so returns `Error::StructInitialization`.
impl TryFrom<&ast::VarDef> for Dtype {
    type Error = crate::ir::Error;

    fn try_from(def: &ast::VarDef) -> Result<Self, Self::Error> {
        // Struct types cannot have inline initializers; reject early.
        if let Dtype::Struct { .. } = &base_dtype(&def.type_specifier) {
            return Err(crate::ir::Error::StructInitialization);
        }
        let base_dtype = base_dtype(&def.type_specifier);
        match &def.inner {
            ast::VarDefInner::Array(def) => Ok(Dtype::array_of(base_dtype, def.len)),
            ast::VarDefInner::Scalar(_) => Ok(base_dtype),
        }
    }
}

/// Converts a variable declaration statement (`VarDeclStmt`) to its IR data type.
///
/// Delegates to the `TryFrom<&VarDecl>` or `TryFrom<&VarDef>` implementation
/// depending on whether the statement is a pure declaration or a definition.
impl TryFrom<&ast::VarDeclStmt> for Dtype {
    type Error = crate::ir::Error;

    fn try_from(value: &ast::VarDeclStmt) -> Result<Self, Self::Error> {
        match &value.inner {
            ast::VarDeclStmtInner::Decl(d) => Dtype::try_from(d.as_ref()),
            ast::VarDeclStmtInner::Def(d) => Dtype::try_from(d.as_ref()),
        }
    }
}
