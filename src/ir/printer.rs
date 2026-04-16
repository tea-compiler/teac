use super::error::Error;
use super::function::FunctionBody;
use super::types::{Dtype, FunctionType, StructType};
use super::value::GlobalDef;
use std::io::Write;

pub struct IrPrinter<W: Write> {
    writer: W,
}

impl<W: Write> IrPrinter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn emit_header(&mut self, target_triple: &str, datalayout: &str) -> Result<(), Error> {
        writeln!(self.writer, "target triple = \"{target_triple}\"")?;
        writeln!(self.writer, "target datalayout = \"{datalayout}\"")?;
        writeln!(self.writer)?;
        Ok(())
    }

    pub fn emit_struct_type(&mut self, name: &str, st: &StructType) -> Result<(), Error> {
        let members: Vec<String> = st
            .elements
            .iter()
            .map(|e| format!("{}", e.1.dtype))
            .collect();
        let members = members.join(", ");
        writeln!(self.writer, "%{name} = type {{ {members} }}")?;
        Ok(())
    }

    pub fn emit_global(&mut self, name: &str, def: &GlobalDef) -> Result<(), Error> {
        let init_str = match (&def.initializers, &def.dtype) {
            (None, Dtype::I32) => "0".to_string(),
            (None, _) => "zeroinitializer".to_string(),
            (Some(inits), Dtype::Array { element, .. }) => {
                let elems: Vec<String> =
                    inits.iter().map(|v| format!("{element} {v}")).collect();
                format!("[{}]", elems.join(", "))
            }
            // Non-array globals have exactly one initializer (enforced by
            // `handle_global_var_decl`).
            (Some(inits), _) => {
                assert_eq!(
                    inits.len(),
                    1,
                    "non-array global {} has {} initializers; front-end invariant violated",
                    name,
                    inits.len(),
                );
                format!("{}", inits[0])
            }
        };

        writeln!(
            self.writer,
            "@{name} = dso_local global {} {init_str}, align 4",
            def.dtype,
        )?;
        Ok(())
    }

    pub fn emit_function_def(
        &mut self,
        identifier: &str,
        return_dtype: &Dtype,
        body: &FunctionBody,
    ) -> Result<(), Error> {
        let args = body
            .arguments
            .iter()
            .map(|var| {
                if matches!(&var.dtype, Dtype::Pointer { .. }) {
                    format!("ptr %r{}", var.id.0)
                } else {
                    format!("{} %r{}", var.dtype, var.id.0)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(
            self.writer,
            "define dso_local {return_dtype} @{identifier}({args}) {{",
        )?;
        for block in &body.blocks {
            writeln!(self.writer, "{}:", block.label)?;
            for stmt in &block.stmts {
                writeln!(self.writer, "{stmt}")?;
            }
        }
        writeln!(self.writer, "}}")?;
        writeln!(self.writer)?;
        Ok(())
    }

    pub fn emit_function_decl(
        &mut self,
        identifier: &str,
        func_type: &FunctionType,
    ) -> Result<(), Error> {
        let args = func_type
            .arguments
            .iter()
            .map(|(_, dtype)| format!("{dtype}"))
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(
            self.writer,
            "declare dso_local {} @{identifier}({args})",
            func_type.return_dtype,
        )?;
        writeln!(self.writer)?;
        Ok(())
    }

    pub fn emit_newline(&mut self) -> Result<(), Error> {
        writeln!(self.writer)?;
        Ok(())
    }
}
