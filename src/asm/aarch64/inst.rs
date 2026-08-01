//! The AArch64 instruction enum shared by lowering and printing:
//! labels, data movement, arithmetic, memory access, and control flow,
//! with virtual registers still in place until register allocation.

use std::collections::{HashMap, HashSet};

use super::types::{Addr, Cond, FBinOp, InstBinOp, InstOperand, RegisterSize, Register};
use crate::common::graph::CfgNode;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Instruction {
    Label(String),

    Mov {
        size: RegisterSize,
        dst: Register,
        src: InstOperand,
    },

    BinOp {
        op: InstBinOp,
        size: RegisterSize,
        dst: Register,
        lhs: Register,
        rhs: InstOperand,
    },

    /// Single-precision floating-point binary operation, e.g.
    /// `fadd s_d, s_n, s_m`.  Both operands and the destination live in
    /// the Fpr bank; the result is always 32-bit
    /// (`RegisterSize::S32`).
    FBinOp {
        op: FBinOp,
        dst: Register,
        lhs: Register,
        rhs: Register,
    },

    Ldr {
        size: RegisterSize,
        dst: Register,
        addr: Addr,
    },

    Str {
        size: RegisterSize,
        src: Register,
        addr: Addr,
    },

    Lea {
        dst: Register,
        addr: Addr,
    },

    Gep {
        dst: Register,
        base: Register,
        index: InstOperand,
        scale: i64,
    },

    Cmp {
        size: RegisterSize,
        lhs: Register,
        rhs: InstOperand,
    },

    /// Single-precision floating-point comparison `fcmp s_n, s_m`.
    /// Sets NZCV, which is later consumed by a `BCond { cond, label }`
    /// arm.
    FCmp {
        lhs: Register,
        rhs: Register,
    },

    /// `scvtf s_d, w_n` — convert a signed 32-bit integer to a
    /// single-precision float.
    Scvtf {
        dst: Register,
        src: Register,
    },

    /// `fcvtzs w_d, s_n` — convert a single-precision float to a signed
    /// 32-bit integer, rounding toward zero.
    Fcvtzs {
        dst: Register,
        src: Register,
    },

    /// `fmov s_d, s_n` (Fpr-to-Fpr) or `fmov s_d, w_n` (Gpr-to-Fpr).
    /// Used in phi lowering, in the AAPCS64 entry/exit shims for `f32`
    /// arguments and return values, and to materialise a float constant
    /// from a literal `InstOperand::Immediate`.
    Fmov {
        dst: Register,
        src: InstOperand,
    },

    B {
        label: String,
    },
    BCond {
        cond: Cond,
        label: String,
    },
    Bl {
        func: String,
    },

    SaveCallerRegs,
    RestoreCallerRegs,

    SubSp {
        imm: i64,
    },
    AddSp {
        imm: i64,
    },

    Ret,
}

impl CfgNode for Instruction {
    fn label(&self) -> Option<String> {
        if let Instruction::Label(name) = self {
            Some(name.clone())
        } else {
            None
        }
    }

    fn successors(
        &self,
        idx: usize,
        num_nodes: usize,
        label_map: &HashMap<String, usize>,
    ) -> Vec<usize> {
        match self {
            Instruction::Ret => vec![],
            Instruction::B { label } => label_map.get(label.as_str()).copied().into_iter().collect(),
            Instruction::BCond { label, .. } => {
                let mut succs = Vec::with_capacity(2);
                if let Some(&target) = label_map.get(label.as_str()) {
                    succs.push(target);
                }
                if idx + 1 < num_nodes {
                    succs.push(idx + 1);
                }
                succs
            }
            _ => {
                if idx + 1 < num_nodes {
                    vec![idx + 1]
                } else {
                    vec![]
                }
            }
        }
    }
}

impl Instruction {
    pub fn used_vregs(&self) -> HashSet<usize> {
        let mut used = HashSet::new();

        let add_reg = |s: &mut HashSet<usize>, r: &Register| {
            if let Register::Virtual(v) = r {
                s.insert(*v);
            }
        };

        let add_operand = |s: &mut HashSet<usize>, op: &InstOperand| {
            if let InstOperand::Register(Register::Virtual(v)) = op {
                s.insert(*v);
            }
        };

        let add_addr = |s: &mut HashSet<usize>, addr: &Addr| {
            if let Addr::BaseOff {
                base: Register::Virtual(v),
                ..
            } = addr
            {
                s.insert(*v);
            }
        };

        match self {
            Instruction::Mov { src, .. } => add_operand(&mut used, src),
            Instruction::BinOp { lhs, rhs, .. } => {
                add_reg(&mut used, lhs);
                add_operand(&mut used, rhs);
            }
            Instruction::FBinOp { lhs, rhs, .. } => {
                add_reg(&mut used, lhs);
                add_reg(&mut used, rhs);
            }
            Instruction::Ldr { addr, .. } => add_addr(&mut used, addr),
            Instruction::Str { src, addr, .. } => {
                add_reg(&mut used, src);
                add_addr(&mut used, addr);
            }
            Instruction::Lea { addr, .. } => add_addr(&mut used, addr),
            Instruction::Gep { base, index, .. } => {
                add_reg(&mut used, base);
                if let InstOperand::Register(r) = index {
                    add_reg(&mut used, r);
                }
            }
            Instruction::Cmp { lhs, rhs, .. } => {
                add_reg(&mut used, lhs);
                add_operand(&mut used, rhs);
            }
            Instruction::FCmp { lhs, rhs } => {
                add_reg(&mut used, lhs);
                add_reg(&mut used, rhs);
            }
            Instruction::Scvtf { src, .. } | Instruction::Fcvtzs { src, .. } => add_reg(&mut used, src),
            Instruction::Fmov { src, .. } => add_operand(&mut used, src),
            Instruction::Label(_)
            | Instruction::B { .. }
            | Instruction::BCond { .. }
            | Instruction::Bl { .. }
            | Instruction::SaveCallerRegs
            | Instruction::RestoreCallerRegs
            | Instruction::SubSp { .. }
            | Instruction::AddSp { .. }
            | Instruction::Ret => {}
        }
        used
    }

    /// Returns the virtual register defined by this instruction together
    /// with the [`RegisterSize`] it carries, if exactly one virtual
    /// register is defined.  Instructions with no destination (`Str`,
    /// `Cmp`, `Jump`, ...), or whose destination is physical, return
    /// `None`.
    ///
    /// `Lea`/`Gep` always produce pointer-sized (`X64`) destinations.
    /// The integer/float scalar ops carry their dtype-derived size:
    /// `Mov`/`BinOp`/`Ldr` track it explicitly via the `size` field;
    /// `Scvtf`/`Fmov`/`FBinOp` always produce a single-precision (`S32`)
    /// destination, and `Fcvtzs` produces a 32-bit integer (`W32`).
    pub fn defined_vreg_with_size(&self) -> Option<(usize, RegisterSize)> {
        let (dst, size) = match self {
            Instruction::Mov { size, dst, .. } => (dst, *size),
            Instruction::BinOp { size, dst, .. } => (dst, *size),
            Instruction::FBinOp { dst, .. } => (dst, RegisterSize::S32),
            Instruction::Ldr { size, dst, .. } => (dst, *size),
            Instruction::Lea { dst, .. } => (dst, RegisterSize::X64),
            Instruction::Gep { dst, .. } => (dst, RegisterSize::X64),
            Instruction::Scvtf { dst, .. } => (dst, RegisterSize::S32),
            Instruction::Fcvtzs { dst, .. } => (dst, RegisterSize::W32),
            Instruction::Fmov { dst, .. } => (dst, RegisterSize::S32),
            _ => return None,
        };
        if let Register::Virtual(v) = dst {
            Some((*v, size))
        } else {
            None
        }
    }
}
