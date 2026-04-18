//! Opt-in, feature-gated experimental extensions.
//!
//! Everything under `crate::experimental` is intentionally off the
//! default compile path.  Each sub-module is gated on its own Cargo
//! feature; flipping the feature is the only thing that pulls the code
//! in.  Stable compiler machinery (ir / opt / asm) must never depend
//! on anything here — the dependency arrow always points inward.

#[cfg(feature = "return-type-inference")]
pub(crate) mod return_infer;

#[cfg(feature = "return-type-inference")]
pub(crate) use return_infer::ReturnInferPass;
