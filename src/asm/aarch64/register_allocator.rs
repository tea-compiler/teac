//! Register allocation by graph coloring: builds liveness and
//! interference over virtual registers, spills what cannot be colored,
//! and rewrites the instruction stream with physical registers plus
//! spill loads/stores around the frame.

use std::collections::{HashMap, HashSet, VecDeque};

use super::frame::FrameLayout;
use super::inst::Instruction;
use super::types::{Addr, InstBinOp, InstOperand, RegisterSize, Register, SCRATCH0, SCRATCH1};
use crate::asm::common::StackSlot;
use crate::asm::error::Error;
use crate::common::bitset::Bitset;
use crate::common::graph::{BackwardLiveness, Graph};

const NUM_COLORS: usize = 8;
const ALLOCATABLE_REGS: [u8; NUM_COLORS] = [8, 9, 10, 11, 12, 13, 14, 15];

/// Floating-point virtual registers are coloured against `s18`–`s25`,
/// the caller-saved `v16`–`v31` band of the AAPCS64 FP register file.
/// Like the integer pool `x8`–`x15`, these are caller-saved: a value
/// held in one of them across a `bl` is preserved by the call site's
/// `SaveCallerRegs` / `RestoreCallerRegs` bracket (see
/// `printer::emit_save_caller_regs`), not by the callee.  The band is
/// kept disjoint from the FP scratch pair (`s16` / `s17`) and from the
/// AAPCS64 argument / result registers (`s0`–`s7`).
#[allow(dead_code)]
const ALLOCATABLE_FPRS: [u8; NUM_COLORS] = [18, 19, 20, 21, 22, 23, 24, 25];

/// Floating-point scratch pair (s16 / s17) used during spill / reload of
/// Fpr vregs.  Caller-saved, so the surrounding `bl` does not need to
/// preserve them.  Share the architectural register numbers with the
/// integer `SCRATCH0` / `SCRATCH1` because aarch64 keeps integer and FP
/// banks separate: `s16` and `x16` are independent physical registers.
#[allow(dead_code)]
const F_SCRATCH0: u8 = SCRATCH0;
#[allow(dead_code)]
const F_SCRATCH1: u8 = SCRATCH1;

/// Placement of one virtual register decided by allocation: either a
/// physical colour or a frame spill slot.  The two outcomes are
/// mutually exclusive, so one [`Location`] answers both "what physical
/// register?" and "is it spilled?" for the rewriter.
#[derive(Debug, Clone, Copy)]
enum Location {
    /// Coloured to physical register number `n` in the allocatable band.
    Register(u8),
    /// Spilled to the given frame slot.
    Spill(StackSlot),
}

/// Where every virtual register in the instruction stream ends up.
/// Produced by [`RegisterAllocator`] as it reserves the matching spill
/// slots in the [`FrameLayout`], so placement and frame layout are
/// decided in one pass and never drift apart.
#[derive(Debug, Clone)]
struct RegisterAllocation {
    locations: HashMap<usize, Location>,
}

impl RegisterAllocation {
    fn empty() -> Self {
        Self {
            locations: HashMap::new(),
        }
    }

    fn location(&self, vreg: usize) -> Option<Location> {
        self.locations.get(&vreg).copied()
    }
}

/// Register-allocation phase for one function's instruction stream.
/// Colours the virtual registers against the allocatable physical band,
/// reserves spill slots in the borrowed [`FrameLayout`] for the vregs
/// that do not fit, and rewrites the stream into physical-register form.
/// Allocation and rewriting form one phase with no externally observable
/// intermediate, mirroring the other backend stages
/// (`AsmFunctionGenerator`, `AsmPrinter`, `InstRewriter`).
pub struct RegisterAllocator<'a> {
    insts: &'a [Instruction],
    frame: &'a mut FrameLayout,
}

impl<'a> RegisterAllocator<'a> {
    pub fn new(insts: &'a [Instruction], frame: &'a mut FrameLayout) -> Self {
        Self { insts, frame }
    }

    /// Colours the stream, reserves the needed spill slots in the frame,
    /// and returns the rewritten physical-register instructions.
    /// Consumes the allocator, releasing the frame borrow so the caller
    /// can read back the final frame size.
    pub fn run(mut self) -> Result<Vec<Instruction>, Error> {
        let alloc = self.allocate();
        InstRewriter::new(&alloc).rewrite(self.insts)
    }

    /// Colours the virtual registers and reserves a frame spill slot for
    /// each spilled vreg, yielding the unified placement.  Reservation
    /// lives here so placement and frame layout are decided in one pass
    /// and never drift apart.
    fn allocate(&mut self) -> RegisterAllocation {
        if self.insts.is_empty() {
            return RegisterAllocation::empty();
        }

        let num_vregs = max_vreg_index(self.insts).map_or(0, |m| m + 1);
        if num_vregs == 0 {
            return RegisterAllocation::empty();
        }

        let cfg = Graph::from_nodes(self.insts);
        let (gen, kill, present, vreg_sizes) = build_gen_kill(self.insts, num_vregs);

        // Floating-point vregs (the S32 width is the sole Fpr class) must be
        // coloured against ALLOCATABLE_FPRS on their own interference
        // (sub)graph; the colouring below only knows the integer pool
        // ALLOCATABLE_REGS.  See asmt-4.md §3.4.
        if vreg_sizes.values().any(|size| *size == RegisterSize::S32) {
            todo!(
                "asmt-4: split the interference graph by RegisterClass and \
                 colour Fpr vregs against ALLOCATABLE_FPRS (s18-s25); \
                 see asmt-4.md §3.4"
            );
        }

        let liveness = BackwardLiveness::compute(&gen, &kill, &cfg, Bitset::new(num_vregs));
        let mut graph = InterferenceGraph::build(self.insts, &liveness, &present, num_vregs);
        let Coloring { coloring, spilled } = graph.color();

        let mut locations = HashMap::with_capacity(coloring.len() + spilled.len());
        for (vreg, color) in coloring {
            locations.insert(vreg, Location::Register(color));
        }
        for vreg in spilled {
            let size = vreg_sizes.get(&vreg).copied().unwrap_or(RegisterSize::X64);
            locations.insert(vreg, Location::Spill(self.frame.alloc_spill(size)));
        }

        RegisterAllocation { locations }
    }
}

fn max_vreg_index(instructions: &[Instruction]) -> Option<usize> {
    instructions
        .iter()
        .flat_map(|inst| {
            inst.used_vregs()
                .into_iter()
                .chain(inst.defined_vreg_with_size().map(|(v, _)| v))
        })
        .max()
}

fn build_gen_kill(
    instructions: &[Instruction],
    num_vregs: usize,
) -> (Vec<Bitset>, Vec<Bitset>, Bitset, HashMap<usize, RegisterSize>) {
    let n = instructions.len();
    let mut gen = Vec::with_capacity(n);
    let mut kill = Vec::with_capacity(n);
    let mut present = Bitset::new(num_vregs);
    let mut vreg_sizes = HashMap::new();

    for inst in instructions {
        let mut g = Bitset::new(num_vregs);
        for v in inst.used_vregs() {
            g.insert(v);
            present.insert(v);
        }
        let mut k = Bitset::new(num_vregs);
        if let Some((v, size)) = inst.defined_vreg_with_size() {
            k.insert(v);
            present.insert(v);
            vreg_sizes.insert(v, size);
        }
        gen.push(g);
        kill.push(k);
    }

    (gen, kill, present, vreg_sizes)
}

/// Size-agnostic output of the interference-graph colouring stage: the
/// physical-register assignment plus the vregs that could not be coloured.
/// [`RegisterAllocator`] turns this into [`Location`]s, sizing and
/// reserving a frame slot for each spilled vreg, so the colouring itself
/// stays unaware of operand dtypes and frame layout.
struct Coloring {
    coloring: HashMap<usize, u8>,
    spilled: Vec<usize>,
}

impl Coloring {
    fn empty() -> Self {
        Self {
            coloring: HashMap::new(),
            spilled: Vec::new(),
        }
    }
}

struct InterferenceGraph {
    /// Bit set of vregs that appear anywhere in the instruction stream.
    present: Bitset,
    /// `adjacency[v]` is the bit set of vregs that interfere with `v`.
    adjacency: Vec<Bitset>,
}

impl InterferenceGraph {
    fn build(
        instructions: &[Instruction],
        liveness: &BackwardLiveness<Bitset>,
        present: &Bitset,
        num_vregs: usize,
    ) -> Self {
        let mut adjacency: Vec<Bitset> = (0..num_vregs).map(|_| Bitset::new(num_vregs)).collect();

        for (i, inst) in instructions.iter().enumerate() {
            let live_out = &liveness.live_out[i];
            if let Some((d, _)) = inst.defined_vreg_with_size() {
                adjacency[d].union_with(live_out);
                adjacency[d].remove(d);
                for r in live_out.iter() {
                    if r != d {
                        adjacency[r].insert(d);
                    }
                }
            }
        }

        Self {
            present: present.clone(),
            adjacency,
        }
    }

    fn degree(&self, v: usize) -> usize {
        self.adjacency[v].len()
    }

    fn color(&mut self) -> Coloring {
        if self.present.is_empty() {
            return Coloring::empty();
        }

        let (stack, potential_spills) = self.simplify();
        self.select(stack, potential_spills)
    }

    fn simplify(&mut self) -> (Vec<usize>, HashSet<usize>) {
        let n = self.adjacency.len();
        let total_nodes = self.present.len();

        let mut degree: Vec<usize> = (0..n).map(|v| self.degree(v)).collect();
        let mut removed = Bitset::new(n);
        let mut in_low = Bitset::new(n);

        let mut low_degree: VecDeque<usize> = VecDeque::new();
        for v in self.present.iter() {
            if degree[v] < NUM_COLORS {
                low_degree.push_back(v);
                in_low.insert(v);
            }
        }

        let mut stack: Vec<usize> = Vec::with_capacity(total_nodes);
        let mut potential_spills: HashSet<usize> = HashSet::new();

        while stack.len() < total_nodes {
            let pick = self.pick_node(
                &mut low_degree,
                &mut in_low,
                &removed,
                &degree,
                &mut potential_spills,
            );

            removed.insert(pick);
            stack.push(pick);

            for u in self.adjacency[pick].iter() {
                if removed.contains(u) {
                    continue;
                }
                if degree[u] > 0 {
                    degree[u] -= 1;
                    if degree[u] < NUM_COLORS && !in_low.contains(u) {
                        low_degree.push_back(u);
                        in_low.insert(u);
                    }
                }
            }
        }

        (stack, potential_spills)
    }

    fn pick_node(
        &self,
        low_degree: &mut VecDeque<usize>,
        in_low: &mut Bitset,
        removed: &Bitset,
        degree: &[usize],
        potential_spills: &mut HashSet<usize>,
    ) -> usize {
        while let Some(v) = low_degree.pop_front() {
            in_low.remove(v);
            if !removed.contains(v) {
                return v;
            }
        }

        let v = self
            .present
            .iter()
            .filter(|v| !removed.contains(*v))
            .max_by_key(|v| degree[*v])
            .expect("graph should not be empty");

        potential_spills.insert(v);
        v
    }

    fn select(&self, mut stack: Vec<usize>, potential_spills: HashSet<usize>) -> Coloring {
        let mut coloring: HashMap<usize, u8> = HashMap::new();
        let mut spilled: Vec<usize> = Vec::new();

        while let Some(v) = stack.pop() {
            let mut used_colors: u32 = 0;
            for u in self.adjacency[v].iter() {
                if let Some(&c) = coloring.get(&u) {
                    used_colors |= 1u32 << c;
                }
            }

            if let Some(&color) = ALLOCATABLE_REGS
                .iter()
                .find(|c| used_colors & (1u32 << **c) == 0)
            {
                coloring.insert(v, color);
            } else {
                spilled.push(v);
            }
        }

        spilled.sort_by_key(|v| (!potential_spills.contains(v), *v));

        Coloring { coloring, spilled }
    }
}

struct InstRewriter<'a> {
    alloc: &'a RegisterAllocation,
    output: Vec<Instruction>,
}

impl<'a> InstRewriter<'a> {
    fn new(alloc: &'a RegisterAllocation) -> Self {
        Self {
            alloc,
            output: Vec::new(),
        }
    }

    /// Rewrites the whole stream into physical-register form and returns
    /// it, asserting that no virtual register survives.  Consumes the
    /// rewriter.
    fn rewrite(mut self, insts: &[Instruction]) -> Result<Vec<Instruction>, Error> {
        for inst in insts {
            self.rewrite_inst(inst)?;
        }
        self.verify_no_vregs()?;
        Ok(self.output)
    }

    fn verify_no_vregs(&self) -> Result<(), Error> {
        for inst in &self.output {
            if !inst.used_vregs().is_empty() || inst.defined_vreg_with_size().is_some() {
                return Err(Error::Internal(format!(
                    "rewrite left virtual regs behind: {inst:?}"
                )));
            }
        }
        Ok(())
    }

    fn map_reg(&self, r: Register) -> Result<MappedReg, Error> {
        match r {
            Register::Virtual(v) => match self.alloc.location(v) {
                Some(Location::Register(p)) => Ok(MappedReg::InReg(Register::Physical(p))),
                Some(Location::Spill(slot)) => Ok(MappedReg::Spilled(slot)),
                None => Err(Error::Internal(format!(
                    "vreg {v} has no register allocation"
                ))),
            },
            other => Ok(MappedReg::InReg(other)),
        }
    }

    fn emit_spill_load(&mut self, slot: StackSlot, size: RegisterSize, into: u8) {
        self.output.push(Instruction::Ldr {
            size,
            dst: Register::Physical(into),
            addr: FrameLayout::local_addr(slot),
        });
    }

    fn emit_spill_store(&mut self, slot: StackSlot, size: RegisterSize, from: u8) {
        self.output.push(Instruction::Str {
            size,
            src: Register::Physical(from),
            addr: FrameLayout::local_addr(slot),
        });
    }

    fn load_src_reg(&mut self, r: Register, size: RegisterSize, scratch: u8) -> Result<Register, Error> {
        match self.map_reg(r)? {
            MappedReg::InReg(reg) => Ok(reg),
            MappedReg::Spilled(slot) => {
                self.emit_spill_load(slot, size, scratch);
                Ok(Register::Physical(scratch))
            }
        }
    }

    fn load_src_operand(
        &mut self,
        op: InstOperand,
        size: RegisterSize,
        scratch: u8,
    ) -> Result<InstOperand, Error> {
        match op {
            InstOperand::Immediate(i) => Ok(InstOperand::Immediate(i)),
            InstOperand::Register(r) => Ok(InstOperand::Register(self.load_src_reg(r, size, scratch)?)),
        }
    }

    fn rewrite_inst(&mut self, inst: &Instruction) -> Result<(), Error> {
        match inst {
            Instruction::Label(name) => self.output.push(Instruction::Label(name.clone())),
            Instruction::Mov { size, dst, src } => self.rewrite_mov(*size, *dst, *src)?,
            Instruction::BinOp {
                op,
                size,
                dst,
                lhs,
                rhs,
            } => self.rewrite_binop(*op, *size, *dst, *lhs, *rhs)?,
            Instruction::FBinOp { .. } => {
                todo!("asmt-4: rewrite Inst::FBinOp through the Fpr colouring + spill path")
            }
            Instruction::Cmp { size, lhs, rhs } => self.rewrite_cmp(*size, *lhs, *rhs)?,
            Instruction::FCmp { .. } => todo!("asmt-4: rewrite Inst::FCmp through the Fpr path"),
            Instruction::Scvtf { .. } => {
                todo!("asmt-4: rewrite Inst::Scvtf — dst is Fpr, src is Gpr")
            }
            Instruction::Fcvtzs { .. } => {
                todo!("asmt-4: rewrite Inst::Fcvtzs — dst is Gpr, src is Fpr")
            }
            Instruction::Fmov { .. } => {
                todo!("asmt-4: rewrite Inst::Fmov — dst is Fpr, src may be Fpr or Gpr")
            }
            Instruction::Ldr { size, dst, addr } => self.rewrite_ldr(*size, *dst, addr)?,
            Instruction::Str { size, src, addr } => self.rewrite_str(*size, *src, addr)?,
            Instruction::Lea { dst, addr } => self.rewrite_lea(*dst, addr)?,
            Instruction::Gep {
                dst,
                base,
                index,
                scale,
            } => self.rewrite_gep(*dst, *base, *index, *scale)?,
            Instruction::B { label } => self.output.push(Instruction::B {
                label: label.clone(),
            }),
            Instruction::BCond { cond, label } => self.output.push(Instruction::BCond {
                cond: *cond,
                label: label.clone(),
            }),
            Instruction::Bl { func } => self.output.push(Instruction::Bl { func: func.clone() }),
            Instruction::SaveCallerRegs => self.output.push(Instruction::SaveCallerRegs),
            Instruction::RestoreCallerRegs => self.output.push(Instruction::RestoreCallerRegs),
            Instruction::SubSp { imm } => self.output.push(Instruction::SubSp { imm: *imm }),
            Instruction::AddSp { imm } => self.output.push(Instruction::AddSp { imm: *imm }),
            Instruction::Ret => self.output.push(Instruction::Ret),
        }
        Ok(())
    }

    fn rewrite_mov(&mut self, size: RegisterSize, dst: Register, src: InstOperand) -> Result<(), Error> {
        let src_op = self.load_src_operand(src, size, SCRATCH1)?;

        match self.map_reg(dst)? {
            MappedReg::InReg(reg) => {
                self.output.push(Instruction::Mov {
                    size,
                    dst: reg,
                    src: src_op,
                });
            }
            MappedReg::Spilled(slot) => {
                let from = self.operand_to_phys_reg(src_op, size, SCRATCH0)?;
                self.emit_spill_store(slot, size, from);
            }
        }
        Ok(())
    }

    fn rewrite_binop(
        &mut self,
        op: InstBinOp,
        size: RegisterSize,
        dst: Register,
        lhs: Register,
        rhs: InstOperand,
    ) -> Result<(), Error> {
        let lhs_reg = self.load_src_reg(lhs, size, SCRATCH0)?;
        let rhs_op = self.load_src_operand(rhs, size, SCRATCH1)?;

        self.write_to_dst(dst, size, SCRATCH0, |final_dst| Instruction::BinOp {
            op,
            size,
            dst: final_dst,
            lhs: lhs_reg,
            rhs: rhs_op,
        })
    }

    fn rewrite_cmp(&mut self, size: RegisterSize, lhs: Register, rhs: InstOperand) -> Result<(), Error> {
        let lhs_reg = self.load_src_reg(lhs, size, SCRATCH0)?;
        let rhs_op = self.load_src_operand(rhs, size, SCRATCH1)?;

        self.output.push(Instruction::Cmp {
            size,
            lhs: lhs_reg,
            rhs: rhs_op,
        });
        Ok(())
    }

    fn rewrite_ldr(&mut self, size: RegisterSize, dst: Register, addr: &Addr) -> Result<(), Error> {
        let (addr_rewritten, base_used_scratch) = self.rewrite_addr(addr, SCRATCH0)?;
        let dst_scratch = scratch_after_base(base_used_scratch);

        self.write_to_dst(dst, size, dst_scratch, |final_dst| Instruction::Ldr {
            size,
            dst: final_dst,
            addr: addr_rewritten,
        })
    }

    fn rewrite_str(&mut self, size: RegisterSize, src: Register, addr: &Addr) -> Result<(), Error> {
        let (addr_rewritten, base_used_scratch) = self.rewrite_addr(addr, SCRATCH0)?;
        let src_scratch = scratch_after_base(base_used_scratch);
        let src_reg = self.load_src_reg(src, size, src_scratch)?;

        self.output.push(Instruction::Str {
            size,
            src: src_reg,
            addr: addr_rewritten,
        });
        Ok(())
    }

    fn rewrite_lea(&mut self, dst: Register, addr: &Addr) -> Result<(), Error> {
        let (addr_rewritten, base_used_scratch) = self.rewrite_addr(addr, SCRATCH0)?;
        let dst_scratch = scratch_after_base(base_used_scratch);

        self.write_to_dst(dst, RegisterSize::X64, dst_scratch, |final_dst| Instruction::Lea {
            dst: final_dst,
            addr: addr_rewritten,
        })
    }

    fn rewrite_gep(
        &mut self,
        dst: Register,
        base: Register,
        index: InstOperand,
        scale: i64,
    ) -> Result<(), Error> {
        let base_reg = self.load_src_reg(base, RegisterSize::X64, SCRATCH0)?;
        let base_used_scratch = matches!(base_reg, Register::Physical(r) if r == SCRATCH0);
        let index_scratch = scratch_after_base(base_used_scratch);
        let index_rewritten = match index {
            InstOperand::Immediate(i) => InstOperand::Immediate(i),
            InstOperand::Register(r) => {
                InstOperand::Register(self.load_src_reg(r, RegisterSize::W32, index_scratch)?)
            }
        };
        let dst_scratch = scratch_after_base(base_used_scratch);

        self.write_to_dst(dst, RegisterSize::X64, dst_scratch, |final_dst| Instruction::Gep {
            dst: final_dst,
            base: base_reg,
            index: index_rewritten,
            scale,
        })
    }

    /// Pushes a single-destination instruction onto `self.output`, dispatching
    /// the destination through colouring + spill state.
    ///
    /// `build_inst(target)` produces the instruction to emit given the
    /// final physical destination register; `scratch` is the physical
    /// register to use when `dst` is spilled (the value is stored back
    /// to its spill slot at `spill_size`).  The closure runs at most
    /// once.
    fn write_to_dst<F>(
        &mut self,
        dst: Register,
        spill_size: RegisterSize,
        scratch: u8,
        build_inst: F,
    ) -> Result<(), Error>
    where
        F: FnOnce(Register) -> Instruction,
    {
        let (target, spill_to) = match self.map_reg(dst)? {
            MappedReg::InReg(reg) => (reg, None),
            MappedReg::Spilled(slot) => (Register::Physical(scratch), Some(slot)),
        };

        self.output.push(build_inst(target));

        if let Some(slot) = spill_to {
            self.emit_spill_store(slot, spill_size, scratch);
        }
        Ok(())
    }

    fn rewrite_addr(&mut self, addr: &Addr, scratch: u8) -> Result<(Addr, bool), Error> {
        match addr {
            Addr::Global(sym) => Ok((Addr::Global(sym.clone()), false)),
            Addr::BaseOff { base, offset } => match self.map_reg(*base)? {
                MappedReg::InReg(reg) => Ok((
                    Addr::BaseOff {
                        base: reg,
                        offset: *offset,
                    },
                    false,
                )),
                MappedReg::Spilled(slot) => {
                    self.emit_spill_load(slot, RegisterSize::X64, scratch);
                    Ok((
                        Addr::BaseOff {
                            base: Register::Physical(scratch),
                            offset: *offset,
                        },
                        true,
                    ))
                }
            },
        }
    }

    fn operand_to_phys_reg(
        &mut self,
        op: InstOperand,
        size: RegisterSize,
        scratch: u8,
    ) -> Result<u8, Error> {
        match op {
            InstOperand::Immediate(imm) => {
                self.output.push(Instruction::Mov {
                    size,
                    dst: Register::Physical(scratch),
                    src: InstOperand::Immediate(imm),
                });
                Ok(scratch)
            }
            InstOperand::Register(Register::Physical(n)) => Ok(n),
            InstOperand::Register(Register::StackPointer) => {
                Err(Error::Internal("cannot use SP as source".into()))
            }
            InstOperand::Register(Register::Virtual(_)) => {
                Err(Error::Internal("unexpected vreg in operand".into()))
            }
        }
    }
}

/// A source/destination register resolved against the allocation: either
/// already in a physical register (a pre-coloured physical operand or a
/// coloured vreg), or living in a spill slot that must be reloaded /
/// stored around the use.
enum MappedReg {
    InReg(Register),
    Spilled(StackSlot),
}

/// Returns the scratch register that should be used for a dst/src that
/// is sequenced *after* an addr rewrite already claimed a scratch:
/// `SCRATCH1` if the addr consumed `SCRATCH0`, otherwise `SCRATCH0`.
fn scratch_after_base(base_used_scratch: bool) -> u8 {
    if base_used_scratch {
        SCRATCH1
    } else {
        SCRATCH0
    }
}
