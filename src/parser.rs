use std::io::Write;
use std::rc::Rc;

use pest::Parser as PestParser;
use pest_derive::Parser as DeriveParser;

use crate::ast;
use crate::common::Generator;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Syntax(String),

    #[error("invalid integer literal")]
    InvalidNumber(#[from] std::num::ParseIntError),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("unexpected parse tree structure in {0}")]
    Grammar(&'static str),
}

#[derive(DeriveParser)]
#[grammar = "tealang.pest"]
struct TeaLangParser;

type ParseResult<T> = Result<T, Error>;
type Pair<'a> = pest::iterators::Pair<'a, Rule>;

pub struct Parser<'a> {
    input: &'a str,
    pub program: Option<Box<ast::Program>>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            program: None,
        }
    }
}

impl<'a> Generator for Parser<'a> {
    type Error = Error;

    fn generate(&mut self) -> Result<(), Error> {
        self.program = Some(parse(self.input)?);
        Ok(())
    }

    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let ast = self
            .program
            .as_ref()
            .ok_or(Error::Grammar("output before generate"))?;
        write!(w, "{ast}")?;
        Ok(())
    }
}

fn parse(input: &str) -> ParseResult<Box<ast::Program>> {
    let pairs = <TeaLangParser as PestParser<Rule>>::parse(Rule::program, input)
        .map_err(|e| Error::Syntax(e.to_string()))?;

    let mut use_stmts = Vec::new();
    let mut elements = Vec::new();

    for pair in pairs {
        if pair.as_rule() == Rule::program {
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::use_stmt => {
                        use_stmts.push(parse_use_stmt(inner)?);
                    }
                    Rule::program_element => {
                        if let Some(elem) = parse_program_element(inner)? {
                            elements.push(*elem);
                        }
                    }
                    Rule::EOI => {}
                    _ => {}
                }
            }
        }
    }

    Ok(Box::new(ast::Program {
        use_stmts,
        elements,
    }))
}

fn get_pos(pair: &Pair) -> usize {
    pair.as_span().start()
}

fn parse_use_stmt(pair: Pair) -> ParseResult<ast::UseStmt> {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::identifier {
            return Ok(ast::UseStmt {
                module_name: inner.as_str().to_string(),
            });
        }
    }
    Err(Error::Grammar("use_stmt"))
}

fn parse_program_element(pair: Pair) -> ParseResult<Option<Box<ast::ProgramElement>>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::var_decl_stmt => {
                return Ok(Some(Box::new(ast::ProgramElement {
                    inner: ast::ProgramElementInner::VarDeclStmt(parse_var_decl_stmt(inner)?),
                })));
            }
            Rule::struct_def => {
                return Ok(Some(Box::new(ast::ProgramElement {
                    inner: ast::ProgramElementInner::StructDef(parse_struct_def(inner)?),
                })));
            }
            Rule::fn_decl_stmt => {
                return Ok(Some(Box::new(ast::ProgramElement {
                    inner: ast::ProgramElementInner::FnDeclStmt(parse_fn_decl_stmt(inner)?),
                })));
            }
            Rule::fn_def => {
                return Ok(Some(Box::new(ast::ProgramElement {
                    inner: ast::ProgramElementInner::FnDef(parse_fn_def(inner)?),
                })));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn parse_struct_def(pair: Pair) -> ParseResult<Box<ast::StructDef>> {
    let mut identifier = String::new();
    let mut decls = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::identifier => identifier = inner.as_str().to_string(),
            Rule::var_decl_list => decls = parse_var_decl_list(inner)?,
            _ => {}
        }
    }

    Ok(Box::new(ast::StructDef { identifier, decls }))
}

fn parse_var_decl_list(pair: Pair) -> ParseResult<Vec<ast::VarDecl>> {
    let mut decls = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::var_decl {
            decls.push(*parse_var_decl(inner)?);
        }
    }
    Ok(decls)
}

fn parse_var_decl(pair: Pair) -> ParseResult<Box<ast::VarDecl>> {
    let mut identifier: Option<String> = None;
    let mut type_specifier: Rc<Option<ast::TypeSpecifier>> = Rc::new(None);
    let mut array_len: Option<usize> = None;
    let mut is_slice = false;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::identifier if identifier.is_none() => {
                identifier = Some(inner.as_str().to_string());
            }
            Rule::type_spec => {
                type_specifier = parse_type_spec(inner)?;
            }
            Rule::num => {
                array_len = Some(parse_num(inner)? as usize);
            }
            Rule::ampersand => {
                is_slice = true;
            }
            _ => {}
        }
    }

    let identifier = identifier.ok_or(Error::Grammar("var_decl.identifier"))?;
    let inner = if is_slice {
        ast::VarDeclInner::Slice
    } else if let Some(len) = array_len {
        ast::VarDeclInner::Array(Box::new(ast::VarDeclArray { len }))
    } else {
        ast::VarDeclInner::Scalar
    };

    Ok(Box::new(ast::VarDecl {
        identifier,
        type_specifier,
        inner,
    }))
}

fn parse_type_spec(pair: Pair) -> ParseResult<Rc<Option<ast::TypeSpecifier>>> {
    let pos = get_pos(&pair);

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::kw_i32 => {
                return Ok(Rc::new(Some(ast::TypeSpecifier {
                    pos,
                    inner: ast::TypeSpecifierInner::BuiltIn(ast::BuiltIn::Int),
                })));
            }
            Rule::identifier => {
                return Ok(Rc::new(Some(ast::TypeSpecifier {
                    pos,
                    inner: ast::TypeSpecifierInner::Composite(inner.as_str().to_string()),
                })));
            }
            _ => {}
        }
    }

    Ok(Rc::new(None))
}

fn parse_num(pair: Pair) -> ParseResult<i32> {
    Ok(pair.as_str().parse()?)
}

fn parse_var_decl_stmt(pair: Pair) -> ParseResult<Box<ast::VarDeclStmt>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::var_def => {
                return Ok(Box::new(ast::VarDeclStmt {
                    inner: ast::VarDeclStmtInner::Def(parse_var_def(inner)?),
                }));
            }
            Rule::var_decl => {
                return Ok(Box::new(ast::VarDeclStmt {
                    inner: ast::VarDeclStmtInner::Decl(parse_var_decl(inner)?),
                }));
            }
            _ => {}
        }
    }

    Err(Error::Grammar("var_decl_stmt"))
}

fn parse_var_def(pair: Pair) -> ParseResult<Box<ast::VarDef>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    let identifier = inner_pairs[0].as_str().to_string();

    // Determine pattern based on structure
    // Look for lbracket to detect array
    let has_bracket = inner_pairs.iter().any(|p| p.as_rule() == Rule::lbracket);
    let has_colon = inner_pairs.iter().any(|p| p.as_rule() == Rule::colon);

    if has_bracket {
        // Array definition
        let len = parse_num(
            inner_pairs
                .iter()
                .find(|p| p.as_rule() == Rule::num)
                .ok_or(Error::Grammar("var_def.array_len"))?
                .clone(),
        )? as usize;

        let type_specifier = if has_colon {
            parse_type_spec(
                inner_pairs
                    .iter()
                    .find(|p| p.as_rule() == Rule::type_spec)
                    .ok_or(Error::Grammar("var_def.type_spec"))?
                    .clone(),
            )?
        } else {
            Rc::new(None)
        };

        let vals = parse_right_val_list(
            inner_pairs
                .iter()
                .find(|p| p.as_rule() == Rule::right_val_list)
                .ok_or(Error::Grammar("var_def.vals"))?
                .clone(),
        )?;

        Ok(Box::new(ast::VarDef {
            identifier,
            type_specifier,
            inner: ast::VarDefInner::Array(Box::new(ast::VarDefArray { len, vals })),
        }))
    } else {
        // Scalar definition
        let type_specifier = if has_colon {
            parse_type_spec(
                inner_pairs
                    .iter()
                    .find(|p| p.as_rule() == Rule::type_spec)
                    .ok_or(Error::Grammar("var_def.type_spec"))?
                    .clone(),
            )?
        } else {
            Rc::new(None)
        };

        let val = parse_right_val(
            inner_pairs
                .iter()
                .find(|p| p.as_rule() == Rule::right_val)
                .ok_or(Error::Grammar("var_def.val"))?
                .clone(),
        )?;

        Ok(Box::new(ast::VarDef {
            identifier,
            type_specifier,
            inner: ast::VarDefInner::Scalar(Box::new(ast::VarDefScalar { val })),
        }))
    }
}

fn parse_right_val_list(pair: Pair) -> ParseResult<Vec<ast::RightVal>> {
    let mut vals = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::right_val {
            vals.push(*parse_right_val(inner)?);
        }
    }
    Ok(vals)
}

fn parse_right_val(pair: Pair) -> ParseResult<Box<ast::RightVal>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::bool_expr => {
                return Ok(Box::new(ast::RightVal {
                    inner: ast::RightValInner::BoolExpr(parse_bool_expr(inner)?),
                }));
            }
            Rule::arith_expr => {
                return Ok(Box::new(ast::RightVal {
                    inner: ast::RightValInner::ArithExpr(parse_arith_expr(inner)?),
                }));
            }
            _ => {}
        }
    }

    Err(Error::Grammar("right_val"))
}

fn parse_bool_expr(pair: Pair) -> ParseResult<Box<ast::BoolExpr>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.is_empty() {
        return Err(Error::Grammar("bool_expr"));
    }

    let mut expr = parse_bool_and_term(inner_pairs[0].clone())?;

    let mut i = 1;
    while i < inner_pairs.len() {
        if inner_pairs[i].as_rule() == Rule::op_or {
            let right = parse_bool_and_term(inner_pairs[i + 1].clone())?;
            expr = Box::new(ast::BoolExpr {
                pos: expr.pos,
                inner: ast::BoolExprInner::BoolBiOpExpr(Box::new(ast::BoolBiOpExpr {
                    op: ast::BoolBiOp::Or,
                    left: expr,
                    right,
                })),
            });
            i += 2;
        } else {
            i += 1;
        }
    }

    Ok(expr)
}

fn parse_bool_and_term(pair: Pair) -> ParseResult<Box<ast::BoolExpr>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.is_empty() {
        return Err(Error::Grammar("bool_and_term"));
    }

    let first_unit = parse_bool_unit_atom(inner_pairs[0].clone())?;
    let mut expr = Box::new(ast::BoolExpr {
        pos: first_unit.pos,
        inner: ast::BoolExprInner::BoolUnit(first_unit),
    });

    let mut i = 1;
    while i < inner_pairs.len() {
        if inner_pairs[i].as_rule() == Rule::op_and {
            let right_unit = parse_bool_unit_atom(inner_pairs[i + 1].clone())?;
            let right_expr = Box::new(ast::BoolExpr {
                pos: right_unit.pos,
                inner: ast::BoolExprInner::BoolUnit(right_unit),
            });

            expr = Box::new(ast::BoolExpr {
                pos: expr.pos,
                inner: ast::BoolExprInner::BoolBiOpExpr(Box::new(ast::BoolBiOpExpr {
                    op: ast::BoolBiOp::And,
                    left: expr,
                    right: right_expr,
                })),
            });
            i += 2;
        } else {
            i += 1;
        }
    }

    Ok(expr)
}

fn parse_bool_unit_atom(pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
    let pos = get_pos(&pair);
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.len() == 2 && inner_pairs[0].as_rule() == Rule::op_not {
        let cond = parse_bool_unit_atom(inner_pairs[1].clone())?;
        return Ok(Box::new(ast::BoolUnit {
            pos,
            inner: ast::BoolUnitInner::BoolUOpExpr(Box::new(ast::BoolUOpExpr {
                op: ast::BoolUOp::Not,
                cond,
            })),
        }));
    }

    for inner in inner_pairs {
        match inner.as_rule() {
            Rule::bool_unit_paren => {
                return parse_bool_unit_paren(inner);
            }
            Rule::bool_comparison => {
                return parse_bool_comparison(inner);
            }
            _ => {}
        }
    }

    Err(Error::Grammar("bool_unit_atom"))
}

fn parse_bool_unit_paren(pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
    let pos = get_pos(&pair);
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    let filtered: Vec<_> = inner_pairs
        .into_iter()
        .filter(|p| p.as_rule() != Rule::lparen && p.as_rule() != Rule::rparen)
        .collect();

    if filtered.len() == 1 && filtered[0].as_rule() == Rule::bool_expr {
        return Ok(Box::new(ast::BoolUnit {
            pos,
            inner: ast::BoolUnitInner::BoolExpr(parse_bool_expr(filtered[0].clone())?),
        }));
    }

    if filtered.len() == 3 {
        return parse_comparison_to_bool_unit(
            pos,
            filtered[0].clone(),
            filtered[1].clone(),
            filtered[2].clone(),
        );
    }

    Err(Error::Grammar("bool_unit_paren"))
}

fn parse_bool_comparison(pair: Pair) -> ParseResult<Box<ast::BoolUnit>> {
    let pos = get_pos(&pair);
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.len() == 3 {
        return parse_comparison_to_bool_unit(
            pos,
            inner_pairs[0].clone(),
            inner_pairs[1].clone(),
            inner_pairs[2].clone(),
        );
    }

    Err(Error::Grammar("bool_comparison"))
}

fn parse_comparison_to_bool_unit(
    pos: usize,
    left_pair: Pair,
    op_pair: Pair,
    right_pair: Pair,
) -> ParseResult<Box<ast::BoolUnit>> {
    let left = parse_expr_unit(left_pair)?;
    let op = parse_comp_op(op_pair)?;
    let right = parse_expr_unit(right_pair)?;

    Ok(Box::new(ast::BoolUnit {
        pos,
        inner: ast::BoolUnitInner::ComExpr(Box::new(ast::ComExpr { op, left, right })),
    }))
}

fn parse_comp_op(pair: Pair) -> ParseResult<ast::ComOp> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::op_lt => return Ok(ast::ComOp::Lt),
            Rule::op_gt => return Ok(ast::ComOp::Gt),
            Rule::op_le => return Ok(ast::ComOp::Le),
            Rule::op_ge => return Ok(ast::ComOp::Ge),
            Rule::op_eq => return Ok(ast::ComOp::Eq),
            Rule::op_ne => return Ok(ast::ComOp::Ne),
            _ => {}
        }
    }
    Err(Error::Grammar("comp_op"))
}

fn parse_arith_expr(pair: Pair) -> ParseResult<Box<ast::ArithExpr>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.is_empty() {
        return Err(Error::Grammar("arith_expr"));
    }

    let mut expr = parse_arith_term(inner_pairs[0].clone())?;

    let mut i = 1;
    while i < inner_pairs.len() {
        if inner_pairs[i].as_rule() == Rule::arith_add_op {
            let op = parse_arith_add_op(inner_pairs[i].clone())?;
            let right = parse_arith_term(inner_pairs[i + 1].clone())?;

            expr = Box::new(ast::ArithExpr {
                pos: expr.pos,
                inner: ast::ArithExprInner::ArithBiOpExpr(Box::new(ast::ArithBiOpExpr {
                    op,
                    left: expr,
                    right,
                })),
            });
            i += 2;
        } else {
            i += 1;
        }
    }

    Ok(expr)
}

fn parse_arith_term(pair: Pair) -> ParseResult<Box<ast::ArithExpr>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.is_empty() {
        return Err(Error::Grammar("arith_term"));
    }

    let first_unit = parse_expr_unit(inner_pairs[0].clone())?;
    let mut expr = Box::new(ast::ArithExpr {
        pos: first_unit.pos,
        inner: ast::ArithExprInner::ExprUnit(first_unit),
    });

    let mut i = 1;
    while i < inner_pairs.len() {
        if inner_pairs[i].as_rule() == Rule::arith_mul_op {
            let op = parse_arith_mul_op(inner_pairs[i].clone())?;
            let right_unit = parse_expr_unit(inner_pairs[i + 1].clone())?;
            let right = Box::new(ast::ArithExpr {
                pos: right_unit.pos,
                inner: ast::ArithExprInner::ExprUnit(right_unit),
            });

            expr = Box::new(ast::ArithExpr {
                pos: expr.pos,
                inner: ast::ArithExprInner::ArithBiOpExpr(Box::new(ast::ArithBiOpExpr {
                    op,
                    left: expr,
                    right,
                })),
            });
            i += 2;
        } else {
            i += 1;
        }
    }

    Ok(expr)
}

fn parse_arith_add_op(pair: Pair) -> ParseResult<ast::ArithBiOp> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::op_add => return Ok(ast::ArithBiOp::Add),
            Rule::op_sub => return Ok(ast::ArithBiOp::Sub),
            _ => {}
        }
    }
    Err(Error::Grammar("arith_add_op"))
}

fn parse_arith_mul_op(pair: Pair) -> ParseResult<ast::ArithBiOp> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::op_mul => return Ok(ast::ArithBiOp::Mul),
            Rule::op_div => return Ok(ast::ArithBiOp::Div),
            _ => {}
        }
    }
    Err(Error::Grammar("arith_mul_op"))
}

fn parse_expr_unit(pair: Pair) -> ParseResult<Box<ast::ExprUnit>> {
    let pos = get_pos(&pair);
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    let filtered: Vec<_> = inner_pairs
        .iter()
        .filter(|p| !matches!(p.as_rule(), Rule::lparen | Rule::rparen))
        .cloned()
        .collect();

    if filtered.len() == 2
        && filtered[0].as_rule() == Rule::op_sub
        && filtered[1].as_rule() == Rule::num
    {
        let num = parse_num(filtered[1].clone())?;
        return Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::Num(-num),
        }));
    }

    if filtered.len() == 1 && filtered[0].as_rule() == Rule::arith_expr {
        return Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::ArithExpr(parse_arith_expr(filtered[0].clone())?),
        }));
    }

    if filtered.len() >= 1 && filtered[0].as_rule() == Rule::fn_call {
        return Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::FnCall(parse_fn_call(filtered[0].clone())?),
        }));
    }

    if filtered.len() == 1 && filtered[0].as_rule() == Rule::num {
        let num = parse_num(filtered[0].clone())?;
        return Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::Num(num),
        }));
    }

    if inner_pairs.len() >= 1 && inner_pairs[0].as_rule() == Rule::identifier {
        let id = inner_pairs[0].as_str().to_string();

        let mut base = Box::new(ast::LeftVal {
            pos,
            inner: ast::LeftValInner::Id(id),
        });

        let mut i = 1;
        while i < inner_pairs.len() {
            match inner_pairs[i].as_rule() {
                Rule::expr_suffix => {
                    base = parse_expr_suffix_to_left_val(base, inner_pairs[i].clone())?;
                    i += 1;
                }
                _ => break,
            }
        }

        return left_val_to_expr_unit(base);
    }

    Err(Error::Grammar("expr_unit"))
}

fn parse_expr_suffix_to_left_val(
    base: Box<ast::LeftVal>,
    suffix: Pair,
) -> ParseResult<Box<ast::LeftVal>> {
    let pos = base.pos;

    for inner in suffix.into_inner() {
        match inner.as_rule() {
            Rule::lbracket | Rule::rbracket | Rule::dot => continue,
            Rule::index_expr => {
                let idx = parse_index_expr(inner)?;
                return Ok(Box::new(ast::LeftVal {
                    pos,
                    inner: ast::LeftValInner::ArrayExpr(Box::new(ast::ArrayExpr {
                        arr: base,
                        idx,
                    })),
                }));
            }
            Rule::identifier => {
                let member_id = inner.as_str().to_string();
                return Ok(Box::new(ast::LeftVal {
                    pos,
                    inner: ast::LeftValInner::MemberExpr(Box::new(ast::MemberExpr {
                        struct_id: base,
                        member_id,
                    })),
                }));
            }
            _ => {}
        }
    }

    Ok(base)
}

fn left_val_to_expr_unit(lval: Box<ast::LeftVal>) -> ParseResult<Box<ast::ExprUnit>> {
    let pos = lval.pos;

    match &lval.inner {
        ast::LeftValInner::Id(id) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::Id(id.clone()),
        })),
        ast::LeftValInner::ArrayExpr(arr_expr) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::ArrayExpr(arr_expr.clone()),
        })),
        ast::LeftValInner::MemberExpr(mem_expr) => Ok(Box::new(ast::ExprUnit {
            pos,
            inner: ast::ExprUnitInner::MemberExpr(mem_expr.clone()),
        })),
    }
}

fn parse_index_expr(pair: Pair) -> ParseResult<Box<ast::IndexExpr>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::num => {
                let num = parse_num(inner)? as usize;
                return Ok(Box::new(ast::IndexExpr {
                    inner: ast::IndexExprInner::Num(num),
                }));
            }
            Rule::identifier => {
                return Ok(Box::new(ast::IndexExpr {
                    inner: ast::IndexExprInner::Id(inner.as_str().to_string()),
                }));
            }
            _ => {}
        }
    }
    Err(Error::Grammar("index_expr"))
}

fn parse_fn_call(pair: Pair) -> ParseResult<Box<ast::FnCall>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::module_prefixed_call => {
                return parse_module_prefixed_call(inner);
            }
            Rule::local_call => {
                return parse_local_call(inner);
            }
            _ => {}
        }
    }
    Err(Error::Grammar("fn_call"))
}

fn parse_module_prefixed_call(pair: Pair) -> ParseResult<Box<ast::FnCall>> {
    let inner_pairs: Vec<_> = pair.into_inner().collect();
    let mut module_prefix = None;
    let mut name = String::new();
    let mut vals = Vec::new();

    for inner in inner_pairs {
        match inner.as_rule() {
            Rule::identifier => {
                if module_prefix.is_none() {
                    module_prefix = Some(inner.as_str().to_string());
                } else {
                    name = inner.as_str().to_string();
                }
            }
            Rule::right_val_list => vals = parse_right_val_list(inner)?,
            _ => {}
        }
    }

    Ok(Box::new(ast::FnCall {
        module_prefix,
        name,
        vals,
    }))
}

fn parse_local_call(pair: Pair) -> ParseResult<Box<ast::FnCall>> {
    let mut name = String::new();
    let mut vals = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::identifier => name = inner.as_str().to_string(),
            Rule::right_val_list => vals = parse_right_val_list(inner)?,
            _ => {}
        }
    }

    Ok(Box::new(ast::FnCall {
        module_prefix: None,
        name,
        vals,
    }))
}

fn parse_left_val(pair: Pair) -> ParseResult<Box<ast::LeftVal>> {
    let pos = get_pos(&pair);
    let inner_pairs: Vec<_> = pair.into_inner().collect();

    if inner_pairs.is_empty() {
        return Err(Error::Grammar("left_val"));
    }

    let id = inner_pairs[0].as_str().to_string();

    let mut base = Box::new(ast::LeftVal {
        pos,
        inner: ast::LeftValInner::Id(id),
    });

    let mut i = 1;
    while i < inner_pairs.len() {
        match inner_pairs[i].as_rule() {
            Rule::left_val_suffix => {
                base = parse_left_val_suffix(base, inner_pairs[i].clone())?;
                i += 1;
            }
            _ => break,
        }
    }

    Ok(base)
}

fn parse_left_val_suffix(base: Box<ast::LeftVal>, suffix: Pair) -> ParseResult<Box<ast::LeftVal>> {
    let pos = base.pos;

    for inner in suffix.into_inner() {
        match inner.as_rule() {
            Rule::lbracket | Rule::rbracket | Rule::dot => continue,
            Rule::index_expr => {
                let idx = parse_index_expr(inner)?;
                return Ok(Box::new(ast::LeftVal {
                    pos,
                    inner: ast::LeftValInner::ArrayExpr(Box::new(ast::ArrayExpr {
                        arr: base,
                        idx,
                    })),
                }));
            }
            Rule::identifier => {
                let member_id = inner.as_str().to_string();
                return Ok(Box::new(ast::LeftVal {
                    pos,
                    inner: ast::LeftValInner::MemberExpr(Box::new(ast::MemberExpr {
                        struct_id: base,
                        member_id,
                    })),
                }));
            }
            _ => {}
        }
    }

    Ok(base)
}

fn parse_fn_decl_stmt(pair: Pair) -> ParseResult<Box<ast::FnDeclStmt>> {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::fn_decl {
            return Ok(Box::new(ast::FnDeclStmt {
                fn_decl: parse_fn_decl(inner)?,
            }));
        }
    }

    Err(Error::Grammar("fn_decl_stmt"))
}

fn parse_fn_decl(pair: Pair) -> ParseResult<Box<ast::FnDecl>> {
    let mut identifier = String::new();
    let mut param_decl = None;
    let mut return_dtype = Rc::new(None);

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::identifier => identifier = inner.as_str().to_string(),
            Rule::param_decl => param_decl = Some(parse_param_decl(inner)?),
            Rule::type_spec => return_dtype = parse_type_spec(inner)?,
            _ => {}
        }
    }

    Ok(Box::new(ast::FnDecl {
        identifier,
        param_decl,
        return_dtype,
    }))
}

fn parse_param_decl(pair: Pair) -> ParseResult<Box<ast::ParamDecl>> {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::var_decl_list {
            return Ok(Box::new(ast::ParamDecl {
                decls: parse_var_decl_list(inner)?,
            }));
        }
    }
    Err(Error::Grammar("param_decl"))
}

fn parse_fn_def(pair: Pair) -> ParseResult<Box<ast::FnDef>> {
    let mut fn_decl = None;
    let mut stmts = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::fn_decl => fn_decl = Some(parse_fn_decl(inner)?),
            Rule::code_block_stmt => stmts.push(*parse_code_block_stmt(inner)?),
            _ => {}
        }
    }

    Ok(Box::new(ast::FnDef {
        fn_decl: fn_decl.ok_or(Error::Grammar("fn_def.fn_decl"))?,
        stmts,
    }))
}

// Statement parsing

fn parse_code_block_stmt(pair: Pair) -> ParseResult<Box<ast::CodeBlockStmt>> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::var_decl_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::VarDecl(parse_var_decl_stmt(inner)?),
                }));
            }
            Rule::assignment_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Assignment(parse_assignment_stmt(inner)?),
                }));
            }
            Rule::call_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Call(parse_call_stmt(inner)?),
                }));
            }
            Rule::if_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::If(parse_if_stmt(inner)?),
                }));
            }
            Rule::while_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::While(parse_while_stmt(inner)?),
                }));
            }
            Rule::return_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Return(parse_return_stmt(inner)?),
                }));
            }
            Rule::continue_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Continue(Box::new(ast::ContinueStmt {})),
                }));
            }
            Rule::break_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Break(Box::new(ast::BreakStmt {})),
                }));
            }
            Rule::null_stmt => {
                return Ok(Box::new(ast::CodeBlockStmt {
                    inner: ast::CodeBlockStmtInner::Null(Box::new(ast::NullStmt {})),
                }));
            }
            _ => {}
        }
    }

    Err(Error::Grammar("code_block_stmt"))
}

fn parse_assignment_stmt(pair: Pair) -> ParseResult<Box<ast::AssignmentStmt>> {
    let mut left_val = None;
    let mut right_val = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::left_val => left_val = Some(parse_left_val(inner)?),
            Rule::right_val => right_val = Some(parse_right_val(inner)?),
            _ => {}
        }
    }

    Ok(Box::new(ast::AssignmentStmt {
        left_val: left_val.ok_or(Error::Grammar("assignment.left_val"))?,
        right_val: right_val.ok_or(Error::Grammar("assignment.right_val"))?,
    }))
}

fn parse_call_stmt(pair: Pair) -> ParseResult<Box<ast::CallStmt>> {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::fn_call {
            return Ok(Box::new(ast::CallStmt {
                fn_call: parse_fn_call(inner)?,
            }));
        }
    }

    Err(Error::Grammar("call_stmt"))
}

fn parse_return_stmt(pair: Pair) -> ParseResult<Box<ast::ReturnStmt>> {
    let mut val = None;

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::right_val {
            val = Some(parse_right_val(inner)?);
        }
    }

    Ok(Box::new(ast::ReturnStmt { val }))
}

fn parse_if_stmt(pair: Pair) -> ParseResult<Box<ast::IfStmt>> {
    let mut bool_unit = None;
    let mut if_stmts = Vec::new();
    let mut else_stmts = None;
    let mut in_else = false;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::bool_expr => {
                let pos = get_pos(&inner);
                let bool_expr = parse_bool_expr(inner)?;
                bool_unit = Some(Box::new(ast::BoolUnit {
                    pos,
                    inner: ast::BoolUnitInner::BoolExpr(bool_expr),
                }));
            }
            Rule::code_block_stmt => {
                if in_else {
                    if else_stmts.is_none() {
                        else_stmts = Some(Vec::new());
                    }
                    else_stmts
                        .as_mut()
                        .unwrap()
                        .push(*parse_code_block_stmt(inner)?);
                } else {
                    if_stmts.push(*parse_code_block_stmt(inner)?);
                }
            }
            Rule::kw_else => {
                in_else = true;
            }
            _ => {}
        }
    }

    Ok(Box::new(ast::IfStmt {
        bool_unit: bool_unit.ok_or(Error::Grammar("cond.bool_unit"))?,
        if_stmts,
        else_stmts,
    }))
}

fn parse_while_stmt(pair: Pair) -> ParseResult<Box<ast::WhileStmt>> {
    let mut bool_unit = None;
    let mut stmts = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::bool_expr => {
                let pos = get_pos(&inner);
                let bool_expr = parse_bool_expr(inner)?;
                bool_unit = Some(Box::new(ast::BoolUnit {
                    pos,
                    inner: ast::BoolUnitInner::BoolExpr(bool_expr),
                }));
            }
            Rule::code_block_stmt => {
                stmts.push(*parse_code_block_stmt(inner)?);
            }
            _ => {}
        }
    }

    Ok(Box::new(ast::WhileStmt {
        bool_unit: bool_unit.ok_or(Error::Grammar("cond.bool_unit"))?,
        stmts,
    }))
}
