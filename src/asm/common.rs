//! Target-independent support utilities for the assembly backend:
//! data-type layout computation and virtual stack-frame management.

mod layout;
mod stack;

pub use layout::{align_up, StructLayouts};
pub use stack::{StackFrame, StackSlot};
