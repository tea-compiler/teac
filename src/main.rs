//! `teac` – the TeaLang compiler driver.
//!
//! This binary ties together all compiler stages in order:
//! parsing → IR generation → optimisation → assembly emission.
//! The output stage can be stopped early (via `--emit`) to inspect
//! the AST, IR, or final AArch64 assembly.

mod asm;
mod ast;
mod common;
mod ir;
mod opt;
mod parser;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use common::{Generator, Target};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

/// Controls which intermediate representation the compiler writes to the output.
/// The pipeline always runs up to (and including) the chosen stage, then exits.
#[derive(Copy, Clone, Debug, PartialEq, ValueEnum)]
enum EmitTarget {
    /// Stop after parsing and emit the Abstract Syntax Tree.
    Ast,
    /// Stop after IR generation and optimisation and emit the IR.
    Ir,
    /// Run all stages and emit the final AArch64 assembly (default).
    Asm,
}

/// The OS / ABI to target when generating assembly.
/// When omitted, `Target::host()` detects the platform at runtime.
#[derive(Copy, Clone, Debug, PartialEq, ValueEnum)]
enum TargetPlatform {
    /// Generate Linux (ELF) assembly.
    Linux,
    /// Generate macOS (Mach-O) assembly.
    Macos,
}

/// Command-line interface definition parsed by `clap`.
#[derive(Parser, Debug)]
#[command(name = "teac")]
#[command(about = "A compiler written in Rust for TeaLang")]
struct Cli {
    /// Path to the TeaLang source file to compile.
    #[clap(value_name = "FILE")]
    input: String,

    /// Which IR stage to emit as output (default: `asm`).
    #[arg(long, value_enum, ignore_case = true, default_value = "asm")]
    emit: EmitTarget,

    /// Target platform for assembly generation.
    /// Defaults to the host platform when not specified.
    #[arg(long, value_enum, ignore_case = true)]
    target: Option<TargetPlatform>,

    /// Write output to FILE instead of stdout.
    #[clap(short, long, value_name = "FILE")]
    output: Option<String>,
}

/// Opens a buffered writer for the compiler output.
///
/// If `output` is `None`, writes to stdout.
/// Otherwise creates the file at the given path, creating any missing
/// parent directories along the way.
///
/// # Parameters
/// - `output`: optional path to an output file.
///
/// # Returns
/// A boxed `Write` implementation, either wrapping stdout or a newly
/// created file.
fn open_writer(output: &Option<String>) -> Result<Box<dyn Write>> {
    let Some(path) = output else {
        return Ok(Box::new(BufWriter::new(io::stdout())));
    };
    let out_path = Path::new(path);
    if let Some(parent) = out_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }
    let file = File::create(out_path)
        .with_context(|| format!("failed to create file '{}'", out_path.display()))?;
    Ok(Box::new(BufWriter::new(file)))
}

/// Runs the full compiler pipeline.
///
/// Steps performed:
/// 1. Parse CLI arguments and read the source file.
/// 2. Parse the source into an AST; exit here if `--emit ast`.
/// 3. Lower the AST to IR.
/// 4. Run the default optimisation pass pipeline over every function.
///    Exit here if `--emit ir`.
/// 5. Generate AArch64 assembly for the requested target platform
///    and write it to the output.
///
/// # Returns
/// `Ok(())` on success, or an `anyhow::Error` describing the first
/// failure encountered.
fn run() -> Result<()> {
    let cli = Cli::parse();
    let source = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read '{}'", cli.input))?;
    let mut writer = open_writer(&cli.output)?;

    let mut parser = parser::Parser::new(&source);
    parser
        .generate()
        .with_context(|| format!("failed to parse '{}'", cli.input))?;

    // Early exit: the user only wants the AST dump.
    if cli.emit == EmitTarget::Ast {
        return parser
            .output(&mut writer)
            .context("failed to write AST output");
    }

    let ast = parser
        .program
        .as_ref()
        .context("internal parser state missing AST after parse")?;
    let input_path = Path::new(&cli.input);
    // `Path::parent()` returns `Some("")` for a bare filename (e.g. "main.tea"),
    // not `None`, so we filter the empty case and fall back to the current directory.
    let source_dir = input_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut ir_gen = ir::IrGenerator::new(ast, source_dir);
    ir_gen.generate().context("failed to generate IR")?;

    // Run the default optimisation passes over every function in the module.
    let pass_manager = opt::FunctionPassManager::with_default_pipeline();
    for func in ir_gen.module.function_list.values_mut() {
        pass_manager.run(func);
    }

    // Early exit: the user only wants the (optimised) IR dump.
    if cli.emit == EmitTarget::Ir {
        return ir_gen
            .output(&mut writer)
            .context("failed to write IR output");
    }

    // Resolve the target platform: use the explicit flag, or auto-detect the host.
    let target = match cli.target {
        Some(TargetPlatform::Linux) => Target::Linux,
        Some(TargetPlatform::Macos) => Target::Macos,
        None => Target::host(),
    };

    let mut asm_gen = asm::AArch64AsmGenerator::new(&ir_gen.module, &ir_gen.registry, target);
    asm_gen.generate().context("failed to generate assembly")?;
    asm_gen
        .output(&mut writer)
        .context("failed to write assembly output")
}

/// Entry point: delegates to [`run`] and converts any error into a
/// human-readable message printed to stderr, exiting with code 1.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
