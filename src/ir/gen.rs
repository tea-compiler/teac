// `conversions` stays private to `gen` apart from `pub(super)`, which
// surfaces it to the parent `ir` module so that `src/ir.rs` can
// re-export `compose_var_def_dtype` for the feature-gated
// `experimental` layer.  Items inside still control their own
// visibility — nothing else leaks.
pub(super) mod conversions;
mod function_gen;
mod module_gen;
mod static_eval;
mod type_infer;
