use super::{align_up, StructLayouts};
use crate::asm::error::Error;
use crate::ir;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct StackSlot {
    pub offset_from_fp: i64,
}

#[derive(Debug, Default)]
pub struct StackFrame {
    alloca_slots: HashMap<usize, StackSlot>,
    spill_slots: HashMap<usize, StackSlot>,
    size: i64,
}

impl StackFrame {
    pub fn from_blocks(blocks: &[ir::BasicBlock], layouts: &StructLayouts) -> Result<Self, Error> {
        let mut frame = Self::default();
        let alloca_ptrs = collect_alloca_ptrs(blocks)?;
        for (vreg, dtype) in alloca_ptrs.iter() {
            let (size, align) = size_align_of_alloca(dtype, layouts)?;
            frame.alloc_alloca(*vreg, align, size);
        }
        Ok(frame)
    }

    pub fn alloc_slot(&mut self, align: i64, size: i64) -> StackSlot {
        let align = align.max(1);
        self.size = align_up(self.size, align);
        self.size += size;
        StackSlot {
            offset_from_fp: -self.size,
        }
    }

    pub fn alloc_alloca(&mut self, vreg: usize, align: i64, size: i64) -> StackSlot {
        let slot = self.alloc_slot(align, size);
        self.alloca_slots.insert(vreg, slot);
        slot
    }

    pub fn alloc_spill(&mut self, vreg: usize, align: i64, size: i64) -> StackSlot {
        let slot = self.alloc_slot(align, size);
        self.spill_slots.insert(vreg, slot);
        slot
    }

    pub fn has_alloca(&self, vreg: usize) -> bool {
        self.alloca_slots.contains_key(&vreg)
    }

    pub fn alloca_slot(&self, vreg: usize) -> Option<StackSlot> {
        self.alloca_slots.get(&vreg).copied()
    }

    pub fn spill_slot(&self, vreg: usize) -> Option<StackSlot> {
        self.spill_slots.get(&vreg).copied()
    }

    pub fn frame_size_aligned(&self) -> i64 {
        align_up(self.size, 16)
    }
}

fn collect_alloca_ptrs(blocks: &[ir::BasicBlock]) -> Result<HashMap<usize, ir::Dtype>, Error> {
    let mut out = HashMap::new();
    for stmt in blocks.iter().flat_map(|block| block.stmts.iter()) {
        if let ir::stmt::StmtInner::Alloca(a) = &stmt.inner {
            let id = a
                .dst
                .local_id()
                .ok_or_else(|| Error::UnsupportedOperand {
                    what: format!("alloca destination is not a local variable: {}", a.dst),
                })?;
            out.insert(id.0, a.dst.dtype().clone());
        }
    }
    Ok(out)
}

fn size_align_of_alloca(dtype: &ir::Dtype, layouts: &StructLayouts) -> Result<(i64, i64), Error> {
    match dtype {
        ir::Dtype::Pointer { pointee } => layouts.size_align_of(pointee.as_ref()),
        ir::Dtype::Array { .. } => layouts.size_align_of(dtype),
        _ => Err(Error::UnsupportedDtype {
            dtype: dtype.clone(),
        }),
    }
}
