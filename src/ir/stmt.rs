//! IR statement (instruction) definitions for teac's LLVM-like IR.
//!
//! Each instruction is a small struct (`GepStmt`, `CallStmt`, …) wrapped
//! into a [`Stmt`] by the [`StmtInner`] enum.  Instructions implement
//! [`Display`] as *bare* instruction text — no leading indentation, no
//! trailing newline — because laying out a function body (indent column,
//! label placement) is the printer's business, not the model's: that is
//! owned by [`super::printer::IrPrinter`].  The one instruction that
//! cannot be rendered infallibly is the GEP — its text depends on the
//! base operand's layout and an ill-typed base has no rendering — so it
//! carries no `Display` impl at all; instead the fallible logic lives in
//! the explicit [`GepStmt::render`] helper, which the printer consumes
//! through its `Result`-returning emit path.

use crate::ast;

use super::error::Error;
use super::function::BlockLabel;
use super::types::Dtype;
use super::value::Operand;
use std::fmt::{self, Display, Formatter};

#[derive(Clone)]
pub enum ArithBinOp {
    Add,
    Sub,
    Mul,
    SDiv,
}

impl From<&ast::ArithBiOp> for ArithBinOp {
    fn from(value: &ast::ArithBiOp) -> Self {
        match value {
            ast::ArithBiOp::Add => ArithBinOp::Add,
            ast::ArithBiOp::Sub => ArithBinOp::Sub,
            ast::ArithBiOp::Mul => ArithBinOp::Mul,
            ast::ArithBiOp::Div => ArithBinOp::SDiv,
        }
    }
}

impl Display for ArithBinOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ArithBinOp::Add => write!(f, "add"),
            ArithBinOp::Sub => write!(f, "sub"),
            ArithBinOp::Mul => write!(f, "mul"),
            ArithBinOp::SDiv => write!(f, "sdiv"),
        }
    }
}

#[derive(Clone)]
pub enum CmpPredicate {
    Eq,
    Ne,
    Sgt,
    Sge,
    Slt,
    Sle,
}

impl From<&ast::ComOp> for CmpPredicate {
    fn from(value: &ast::ComOp) -> Self {
        match value {
            ast::ComOp::Eq => CmpPredicate::Eq,
            ast::ComOp::Ne => CmpPredicate::Ne,
            ast::ComOp::Gt => CmpPredicate::Sgt,
            ast::ComOp::Ge => CmpPredicate::Sge,
            ast::ComOp::Lt => CmpPredicate::Slt,
            ast::ComOp::Le => CmpPredicate::Sle,
        }
    }
}

impl Display for CmpPredicate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CmpPredicate::Eq => write!(f, "eq"),
            CmpPredicate::Ne => write!(f, "ne"),
            CmpPredicate::Sgt => write!(f, "sgt"),
            CmpPredicate::Sge => write!(f, "sge"),
            CmpPredicate::Slt => write!(f, "slt"),
            CmpPredicate::Sle => write!(f, "sle"),
        }
    }
}

#[derive(Clone)]
pub enum StmtInner {
    Call(CallStmt),
    Load(LoadStmt),
    Phi(PhiStmt),
    BiOp(BiOpStmt),
    Alloca(AllocaStmt),
    Cmp(CmpStmt),
    CJump(CJumpStmt),
    Label(LabelStmt),
    Store(StoreStmt),
    Jump(JumpStmt),
    Gep(GepStmt),
    Return(ReturnStmt),
}

#[derive(Clone)]
pub struct Stmt {
    pub inner: StmtInner,
}

/// Describes how an operand is used within a statement.
///
/// The pointer-operand variants (`LoadPtr`, `StorePtr`) are split out from
/// the generic `Use` so that passes like `mem2reg` can distinguish pure
/// load/store traffic through a pointer — which is promotable — from any
/// other use of the same value, which would mean the address escapes and
/// the pointer cannot be promoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandRole {
    /// Instruction defines (writes) this value.
    Def,
    /// General read of a value — not a load/store pointer.
    Use,
    /// Pointer operand of a `load` instruction.
    LoadPtr,
    /// Pointer operand of a `store` instruction.
    StorePtr,
}

/// Borrowed reference to an operand together with its role.
pub struct OperandRef<'a> {
    pub operand: &'a Operand,
    pub role: OperandRole,
}

impl Stmt {
    pub fn as_call(link_name: String, res: Option<Operand>, args: Vec<Operand>) -> Self {
        Self {
            inner: StmtInner::Call(CallStmt {
                link_name,
                res,
                args,
            }),
        }
    }

    pub fn as_load(dst: Operand, ptr: Operand) -> Self {
        Self {
            inner: StmtInner::Load(LoadStmt { dst, ptr }),
        }
    }

    pub fn as_phi(dst: Operand, incomings: Vec<(BlockLabel, Operand)>) -> Self {
        Self {
            inner: StmtInner::Phi(PhiStmt { dst, incomings }),
        }
    }

    pub fn as_biop(kind: ArithBinOp, left: Operand, right: Operand, dst: Operand) -> Self {
        Self {
            inner: StmtInner::BiOp(BiOpStmt {
                kind,
                left,
                right,
                dst,
            }),
        }
    }

    pub fn as_alloca(dst: Operand) -> Self {
        Self {
            inner: StmtInner::Alloca(AllocaStmt { dst }),
        }
    }

    pub fn as_cmp(kind: CmpPredicate, left: Operand, right: Operand, dst: Operand) -> Self {
        Self {
            inner: StmtInner::Cmp(CmpStmt {
                kind,
                left,
                right,
                dst,
            }),
        }
    }

    pub fn as_cjump(cond: Operand, true_label: BlockLabel, false_label: BlockLabel) -> Self {
        Self {
            inner: StmtInner::CJump(CJumpStmt {
                cond,
                true_label,
                false_label,
            }),
        }
    }

    pub fn as_label(label: BlockLabel) -> Self {
        Self {
            inner: StmtInner::Label(LabelStmt { label }),
        }
    }

    pub fn as_store(src: Operand, ptr: Operand) -> Self {
        Self {
            inner: StmtInner::Store(StoreStmt { src, ptr }),
        }
    }

    pub fn as_jump(target: BlockLabel) -> Self {
        Self {
            inner: StmtInner::Jump(JumpStmt { target }),
        }
    }

    pub fn as_return(val: Option<Operand>) -> Self {
        Self {
            inner: StmtInner::Return(ReturnStmt { val }),
        }
    }

    pub fn as_gep(new_ptr: Operand, base_ptr: Operand, index: Operand) -> Self {
        Self {
            inner: StmtInner::Gep(GepStmt {
                new_ptr,
                base_ptr,
                index,
            }),
        }
    }
}

/// Emits the bare instruction text — never a leading `\t`.
///
/// Indentation inside a function body is the printer's concern
/// ([`super::printer::IrPrinter`]), so that the model types carry no
/// presentation policy and other consumers (diagnostics, debugging) get
/// unadorned text.
impl Display for Stmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            StmtInner::Alloca(s) => write!(f, "{s}"),
            StmtInner::BiOp(s) => write!(f, "{s}"),
            StmtInner::CJump(s) => write!(f, "{s}"),
            StmtInner::Call(s) => write!(f, "{s}"),
            StmtInner::Cmp(s) => write!(f, "{s}"),
            // `GepStmt` has no `Display`: its rendering is fallible
            // ([`GepStmt::render`]), and the printer consumes that
            // fallibility through its `Result`-returning emit path.
            // This arm only serves hypothetical direct `Display`
            // consumers and preserves their historical behaviour:
            // text for well-typed GEPs, `fmt::Error` for ill-typed ones.
            StmtInner::Gep(s) => match s.render() {
                Ok(text) => write!(f, "{text}"),
                Err(_) => Err(fmt::Error),
            },
            StmtInner::Label(s) => write!(f, "{s}"),
            StmtInner::Load(s) => write!(f, "{s}"),
            StmtInner::Phi(s) => write!(f, "{s}"),
            StmtInner::Return(s) => write!(f, "{s}"),
            StmtInner::Store(s) => write!(f, "{s}"),
            StmtInner::Jump(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone)]
pub struct CallStmt {
    /// The linker-visible symbol of the callee, resolved once at IR
    /// generation time via [`crate::ir::compute_link_name`] and stored
    /// here so that every backend can consume it verbatim.
    pub link_name: String,
    pub res: Option<Operand>,
    pub args: Vec<Operand>,
}

#[derive(Clone)]
pub struct LoadStmt {
    pub dst: Operand,
    pub ptr: Operand,
}

#[derive(Clone)]
pub struct PhiStmt {
    pub dst: Operand,
    pub incomings: Vec<(BlockLabel, Operand)>,
}

#[derive(Clone)]
pub struct BiOpStmt {
    pub kind: ArithBinOp,
    pub left: Operand,
    pub right: Operand,
    pub dst: Operand,
}

#[derive(Clone)]
pub struct AllocaStmt {
    pub dst: Operand,
}

#[derive(Clone)]
pub struct CmpStmt {
    pub kind: CmpPredicate,
    pub left: Operand,
    pub right: Operand,
    pub dst: Operand,
}

#[derive(Clone)]
pub struct CJumpStmt {
    pub cond: Operand,
    pub true_label: BlockLabel,
    pub false_label: BlockLabel,
}

#[derive(Clone)]
pub struct LabelStmt {
    pub label: BlockLabel,
}

#[derive(Clone)]
pub struct StoreStmt {
    pub src: Operand,
    pub ptr: Operand,
}

#[derive(Clone)]
pub struct JumpStmt {
    pub target: BlockLabel,
}

#[derive(Clone)]
pub struct GepStmt {
    pub new_ptr: Operand,
    pub base_ptr: Operand,
    pub index: Operand,
}

#[derive(Clone)]
pub struct ReturnStmt {
    pub val: Option<Operand>,
}

impl Display for CallStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let args = self
            .args
            .iter()
            .map(|a| {
                if matches!(a.dtype(), Dtype::Pointer { .. } | Dtype::Array { .. }) {
                    format!("ptr {a}")
                } else {
                    format!("{} {a}", a.dtype())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let func_name = &self.link_name;
        if let Some(res) = &self.res {
            write!(f, "{res} = call {} @{func_name}({args})", res.dtype())
        } else {
            write!(f, "call void @{func_name}({args})")
        }
    }
}

impl Display for LoadStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self { dst, ptr } = self;
        write!(f, "{dst} = load {}, ptr {ptr}, align 4", dst.dtype())
    }
}

impl Display for PhiStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let dst = &self.dst;
        let dtype = dst.dtype();
        let incoming_str = self
            .incomings
            .iter()
            .map(|(label, val)| format!("[ {val}, %{label} ]"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "{dst} = phi {dtype} {incoming_str}")
    }
}

impl Display for StoreStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self { src, ptr } = self;
        write!(f, "store {} {src}, ptr {ptr}, align 4", src.dtype())
    }
}

impl Display for AllocaStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let dst = &self.dst;
        let inner = match dst.dtype() {
            Dtype::Pointer { pointee } => pointee.as_ref().clone(),
            other => other.clone(),
        };
        write!(f, "{dst} = alloca {inner}, align 4")
    }
}

impl Display for BiOpStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self {
            kind,
            left,
            right,
            dst,
        } = self;
        write!(f, "{dst} = {kind} {} {left}, {right}", dst.dtype())
    }
}

impl Display for CmpStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self {
            kind,
            left,
            right,
            dst,
        } = self;
        write!(f, "{dst} = icmp {kind} {} {left}, {right}", left.dtype())
    }
}

impl Display for CJumpStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self {
            cond,
            true_label,
            false_label,
        } = self;
        write!(f, "br i1 {cond}, label %{true_label}, label %{false_label}")
    }
}

impl Display for JumpStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "br label %{}", &self.target)
    }
}

impl Display for LabelStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.label)
    }
}

impl GepStmt {
    /// Renders this GEP as teac IR text, choosing the `getelementptr`
    /// form from the base operand's layout:
    ///
    /// - pointer to a fixed-size array or struct → two-index form
    ///   (`i32 0, i32 <index>`), i.e. aggregate/field addressing;
    /// - pointer to an unsized array (a decayed `&[T]` argument) →
    ///   element-typed single-index form;
    /// - pointer to a scalar → single-index form;
    /// - first-class array operand (e.g. a global array) → two-index form.
    ///
    /// The lookup is fallible: a base that is neither a pointer nor an
    /// array is ill-formed IR with no valid rendering.  Keeping that
    /// fallibility in an explicit helper — rather than inside a `Display`
    /// impl where it could only surface as an opaque `fmt::Error` — lets
    /// the printer report it through its own `Result`-returning emit path.
    /// This is why [`GepStmt`] deliberately has no `Display` impl.
    pub fn render(&self) -> Result<String, Error> {
        let Self {
            new_ptr,
            base_ptr,
            index,
        } = self;
        match base_ptr.dtype() {
            Dtype::Pointer { pointee } => match pointee.as_ref() {
                Dtype::Array {
                    length: Some(_), ..
                }
                | Dtype::Struct { .. } => Ok(format!(
                    "{new_ptr} = getelementptr {pointee}, ptr {base_ptr}, i32 0, i32 {index}",
                )),
                Dtype::Array {
                    element,
                    length: None,
                } => Ok(format!(
                    "{new_ptr} = getelementptr {element}, ptr {base_ptr}, i32 {index}",
                )),
                _ => Ok(format!(
                    "{new_ptr} = getelementptr {pointee}, ptr {base_ptr}, i32 {index}",
                )),
            },
            dtype @ Dtype::Array { .. } => Ok(format!(
                "{new_ptr} = getelementptr {dtype}, ptr {base_ptr}, i32 0, i32 {index}",
            )),
            // Ill-formed IR that the front end never produces; surfaced as
            // an I/O error because that is exactly how the previous
            // `fmt::Error`-based failure reached the caller (via
            // `writeln!`'s `io::Write`).
            other => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("getelementptr on non-indexable base of type '{other}'"),
            ))),
        }
    }
}

impl Display for ReturnStmt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.val {
            Some(v) => write!(f, "ret {} {v}", v.dtype()),
            None => write!(f, "ret void"),
        }
    }
}

impl Stmt {
    pub fn operands(&self) -> Vec<OperandRef<'_>> {
        use OperandRole::{Def, LoadPtr, StorePtr, Use};

        /// Build an [`OperandRef`] with less visual noise at every call site.
        fn r(operand: &Operand, role: OperandRole) -> OperandRef<'_> {
            OperandRef { operand, role }
        }

        match &self.inner {
            StmtInner::Alloca(s) => vec![r(&s.dst, Def)],
            StmtInner::Load(s) => vec![r(&s.dst, Def), r(&s.ptr, LoadPtr)],
            StmtInner::Store(s) => vec![r(&s.src, Use), r(&s.ptr, StorePtr)],
            StmtInner::BiOp(s) => vec![r(&s.dst, Def), r(&s.left, Use), r(&s.right, Use)],
            StmtInner::Cmp(s) => vec![r(&s.dst, Def), r(&s.left, Use), r(&s.right, Use)],
            StmtInner::CJump(s) => vec![r(&s.cond, Use)],
            StmtInner::Call(s) => {
                let mut ops = Vec::with_capacity(s.args.len() + 1);
                if let Some(res) = &s.res {
                    ops.push(r(res, Def));
                }
                ops.extend(s.args.iter().map(|a| r(a, Use)));
                ops
            }
            StmtInner::Gep(s) => vec![
                r(&s.new_ptr, Def),
                r(&s.base_ptr, Use),
                r(&s.index, Use),
            ],
            StmtInner::Return(s) => s.val.as_ref().map_or_else(Vec::new, |v| vec![r(v, Use)]),
            StmtInner::Phi(s) => {
                let mut ops = Vec::with_capacity(s.incomings.len() + 1);
                ops.push(r(&s.dst, Def));
                ops.extend(s.incomings.iter().map(|(_, val)| r(val, Use)));
                ops
            }
            StmtInner::Label(_) | StmtInner::Jump(_) => Vec::new(),
        }
    }

    pub fn map_use_operands(&self, f: impl Fn(&Operand) -> Operand) -> Stmt {
        match &self.inner {
            StmtInner::Alloca(s) => Stmt::as_alloca(s.dst.clone()),
            StmtInner::Load(s) => Stmt::as_load(s.dst.clone(), f(&s.ptr)),
            StmtInner::Store(s) => Stmt::as_store(f(&s.src), f(&s.ptr)),
            StmtInner::BiOp(s) => {
                Stmt::as_biop(s.kind.clone(), f(&s.left), f(&s.right), s.dst.clone())
            }
            StmtInner::Cmp(s) => {
                Stmt::as_cmp(s.kind.clone(), f(&s.left), f(&s.right), s.dst.clone())
            }
            StmtInner::CJump(s) => {
                Stmt::as_cjump(f(&s.cond), s.true_label.clone(), s.false_label.clone())
            }
            StmtInner::Call(s) => {
                let args = s.args.iter().map(&f).collect();
                Stmt::as_call(s.link_name.clone(), s.res.clone(), args)
            }
            StmtInner::Gep(s) => Stmt::as_gep(s.new_ptr.clone(), f(&s.base_ptr), f(&s.index)),
            StmtInner::Return(s) => Stmt::as_return(s.val.as_ref().map(&f)),
            StmtInner::Phi(s) => {
                let incomings = s
                    .incomings
                    .iter()
                    .map(|(label, val)| (label.clone(), f(val)))
                    .collect();
                Stmt::as_phi(s.dst.clone(), incomings)
            }
            StmtInner::Label(s) => Stmt::as_label(s.label.clone()),
            StmtInner::Jump(s) => Stmt::as_jump(s.target.clone()),
        }
    }
}
