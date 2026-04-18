/*
 * Integration tests for the TeaLang compiler.
 *
 * Supported platforms:
 *   - Native AArch64 Linux           (gcc)
 *   - x86/x86_64 Linux               (aarch64-linux-gnu-gcc + QEMU)
 *   - macOS AArch64 / Apple Silicon  (cc)
 *   - macOS x86_64                   (Docker linux/arm64)
 */

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Once;

static INIT: Once = Once::new();

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

fn is_native_macos() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

fn is_docker_macos() -> bool {
    cfg!(all(target_os = "macos", not(target_arch = "aarch64")))
}

fn is_cross_linux() -> bool {
    cfg!(all(
        target_os = "linux",
        any(target_arch = "x86", target_arch = "x86_64")
    ))
}

/// Returns `true` if `cmd` is on PATH.
fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Panics with a human-readable install hint if the platform-specific
/// toolchain is missing.
fn ensure_cross_tools() {
    if is_native_macos() {
        if !command_exists("cc") {
            panic!(
                "✗ cc not found.\n\
                 Please install Xcode Command Line Tools: xcode-select --install"
            );
        }
    } else if is_docker_macos() {
        if !command_exists("docker") {
            panic!(
                "✗ Docker not found.\n\
                 Please install Docker Desktop for macOS: https://www.docker.com/products/docker-desktop"
            );
        }

        let status = Command::new("docker")
            .arg("info")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            panic!(
                "✗ Docker is not running.\n\
                 Please start Docker Desktop."
            );
        }
    } else if is_cross_linux() {
        if !command_exists("aarch64-linux-gnu-gcc") {
            panic!(
                "✗ aarch64-linux-gnu-gcc not found.\n\
                 Please install: sudo apt install gcc-aarch64-linux-gnu"
            );
        }

        if !command_exists("qemu-aarch64") {
            panic!(
                "✗ qemu-aarch64 not found.\n\
                 Please install: sudo apt install qemu-user"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// stdlib object (tests/std/std.o) build
// ---------------------------------------------------------------------------

/// Platform-specific path for the compiled stdlib object:
///   - macOS AArch64                       → `tests/std/std-macos.o`
///   - Docker macOS / cross-compile Linux  → `tests/std/std-linux.o`
///   - Native AArch64 Linux                → `tests/std/std.o`
fn get_std_o_path() -> PathBuf {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let std_dir = project_root.join("tests").join("std");
    if is_native_macos() {
        std_dir.join("std-macos.o")
    } else if is_docker_macos() || is_cross_linux() {
        std_dir.join("std-linux.o")
    } else {
        std_dir.join("std.o")
    }
}

/// Compiles `tests/std/std.c` with `compiler` into `o_path`.
fn compile_std_local(std_dir: &Path, o_path: &Path, compiler: &str) {
    let status = Command::new(compiler)
        .arg("-c")
        .arg("std.c")
        .arg("-o")
        .arg(o_path)
        .current_dir(std_dir)
        .status()
        .unwrap_or_else(|e| panic!("Failed to execute {compiler}: {e}"));

    assert!(
        status.success(),
        "✗ {compiler} failed to build {} (exit {}). Ran in {}",
        o_path.display(),
        status.code().unwrap_or(-1),
        std_dir.display()
    );
}

/// Compiles `tests/std/std.c` inside a `linux/arm64` Docker container so
/// that the resulting object is AArch64 ELF (compatible with the
/// cross-compiled test binaries).
fn compile_std_in_docker(std_dir: &Path, o_path: &Path) {
    let o_name = o_path.file_name().unwrap().to_str().unwrap();

    let status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!("{}:/work", std_dir.display()))
        .arg("-w")
        .arg("/work")
        .arg("--platform")
        .arg("linux/arm64")
        .arg("gcc:latest")
        .arg("gcc")
        .arg("-c")
        .arg("std.c")
        .arg("-o")
        .arg(o_name)
        .status()
        .expect("Failed to run docker");

    assert!(
        status.success(),
        "✗ Failed to compile std.c in Docker (exit {})",
        status.code().unwrap_or(-1)
    );
}

/// Builds `std.o` if needed.  Runs at most once per test process via a
/// `Once` guard; rebuilds only when `std.c` is newer than `std.o`.
fn ensure_std() {
    INIT.call_once(|| {
        ensure_cross_tools();

        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let std_dir = project_root.join("tests").join("std");
        let c_path = std_dir.join("std.c");
        let o_path = get_std_o_path();

        let needs_build = match (fs::metadata(&c_path), fs::metadata(&o_path)) {
            (Ok(c_meta), Ok(o_meta)) => match (c_meta.modified(), o_meta.modified()) {
                (Ok(c_m), Ok(o_m)) => c_m > o_m,
                _ => true,
            },
            (Ok(_), Err(_)) => true,
            _ => {
                panic!("✗ Missing tests/std/std.c at {}", c_path.display());
            }
        };

        if needs_build {
            if is_native_macos() {
                compile_std_local(&std_dir, &o_path, "cc");
            } else if is_docker_macos() {
                compile_std_in_docker(&std_dir, &o_path);
            } else if is_cross_linux() {
                compile_std_local(&std_dir, &o_path, "aarch64-linux-gnu-gcc");
            } else {
                compile_std_local(&std_dir, &o_path, "gcc");
            }
        }
        assert!(
            o_path.is_file(),
            "✗ std.o not found at {}",
            o_path.display()
        );
    });
}

// ---------------------------------------------------------------------------
// Compile / link / run primitives
// ---------------------------------------------------------------------------

/// Invokes `teac --emit asm` from `dir` to compile `input_file` into
/// `output_file`.  `input_file` must be a bare filename so `teac`'s
/// `source_dir` resolves to `dir` and `use std;` finds `./std.teah`.
/// On Docker-macOS hosts the `--target linux` flag is added so that
/// `teac` emits Linux AArch64 assembly instead of macOS assembly.
fn launch(dir: &PathBuf, input_file: &str, output_file: &str) -> Output {
    let tool = Path::new(env!("CARGO_BIN_EXE_teac"));
    let mut cmd = Command::new(tool);
    cmd.arg(input_file)
        .arg("--emit")
        .arg("asm")
        .arg("-o")
        .arg(output_file);

    if is_docker_macos() {
        cmd.arg("--target").arg("linux");
    }

    cmd.current_dir(dir)
        .output()
        .expect("Failed to execute teac")
}

/// Runs `cmd`, piping `input` (if provided) to its stdin.  Returns
/// `(exit_code, stdout, stderr)`.
fn run_with_optional_stdin(
    cmd: &mut Command,
    input: Option<&Path>,
) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    if let Some(input_path) = input {
        let mut data = Vec::new();
        File::open(input_path)?.read_to_end(&mut data)?;

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&data)?;
        }

        let output = child.wait_with_output()?;
        Ok((
            output.status.code().unwrap_or(-1),
            output.stdout,
            output.stderr,
        ))
    } else {
        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        Ok((
            output.status.code().unwrap_or(-1),
            output.stdout,
            output.stderr,
        ))
    }
}

/// Links `asm_path` + `std_o` into `exe_path` using `compiler`.  Extra
/// flags (e.g. `-static`) go through `extra_args`.  Returns
/// `(exit_code, stderr)`.
fn link_local(
    build_dir: &Path,
    compiler: &str,
    asm_path: &Path,
    std_o: &Path,
    exe_path: &Path,
    extra_args: &[&str],
) -> io::Result<(i32, Vec<u8>)> {
    let output = Command::new(compiler)
        .arg(asm_path)
        .arg(std_o)
        .arg("-o")
        .arg(exe_path)
        .args(extra_args)
        .current_dir(build_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok((output.status.code().unwrap_or(-1), output.stderr))
}

/// Two-phase Docker run: links with `gcc:latest` (full toolchain), then
/// runs the resulting executable inside `debian:bookworm-slim` (smaller
/// runtime image, executable mounted read-only).  Optional `input` is
/// piped to stdin.
fn link_and_run_in_docker(
    build_dir: &Path,
    asm_name: &str,
    std_o: &Path,
    exe_name: &str,
    input: Option<&Path>,
) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let std_dir = std_o.parent().unwrap();
    let std_o_name = std_o.file_name().unwrap().to_str().unwrap();

    let link_status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!("{}:/build", build_dir.display()))
        .arg("-v")
        .arg(format!("{}:/std:ro", std_dir.display()))
        .arg("-w")
        .arg("/build")
        .arg("--platform")
        .arg("linux/arm64")
        .arg("gcc:latest")
        .arg("gcc")
        .arg(asm_name)
        .arg(format!("/std/{std_o_name}"))
        .arg("-o")
        .arg(exe_name)
        .arg("-static")
        .status()?;

    if !link_status.success() {
        return Ok((
            link_status.code().unwrap_or(-1),
            Vec::new(),
            b"Linking failed in Docker".to_vec(),
        ));
    }

    let mut run_cmd = Command::new("docker");
    run_cmd
        .arg("run")
        .arg("--rm")
        .arg("-i")
        .arg("-v")
        .arg(format!("{}:/build:ro", build_dir.display()))
        .arg("-w")
        .arg("/build")
        .arg("--platform")
        .arg("linux/arm64")
        .arg("debian:bookworm-slim")
        .arg(format!("./{exe_name}"));

    run_with_optional_stdin(&mut run_cmd, input)
}

/// Runs a native executable (AArch64 ELF or Mach-O).
fn run_native(exe: &Path, input: Option<&Path>) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut cmd = Command::new(exe);
    run_with_optional_stdin(&mut cmd, input)
}

/// Runs an AArch64 ELF executable under `qemu-aarch64`.
fn run_with_qemu(exe: &Path, input: Option<&Path>) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut cmd = Command::new("qemu-aarch64");
    cmd.arg(exe);
    run_with_optional_stdin(&mut cmd, input)
}

// ---------------------------------------------------------------------------
// Output comparison helpers
// ---------------------------------------------------------------------------

/// Whitespace-insensitive normalisation: collapses runs of whitespace
/// within each line to a single space, drops blank lines, appends a
/// trailing newline.
fn normalize_for_diff(s: &str) -> String {
    let mut out = Vec::new();
    for line in s.lines() {
        let norm = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if norm.is_empty() {
            continue;
        }
        out.push(norm);
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\n") + "\n"
    }
}

/// Returns `Ok(None)` for `NotFound`; propagates any other I/O error.
fn read_to_string_if_exists(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Appends `line + '\n'` to `path`, creating the file if necessary.
fn append_line<P: AsRef<Path>>(path: P, line: &str) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .unwrap_or_else(|e| panic!("Failed to open {} for append: {e}", path.as_ref().display()));
    writeln!(f, "{line}").expect("Failed to append line");
}

// ---------------------------------------------------------------------------
// Test drivers
// ---------------------------------------------------------------------------

/// Parse-only test: invokes `teac --emit ast` on the fixture, asserts
/// success and that every identifier in `must_contain` appears in the
/// AST.  `teac` receives the absolute source path, so `source_dir`
/// resolves to the test-case directory without a `current_dir` override.
//
// `#[allow(dead_code)]` because every in-tree caller is under a
// not-enabled-by-default `#[cfg(feature = ...)]` for a future language
// feature (float / for-loop / struct-method / multi-dim-array).
#[allow(dead_code)]
fn test_ast_parse(test_name: &str, must_contain: &[&str]) {
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let case_dir = base_dir.join(test_name);
    let tea = case_dir.join(format!("{test_name}.tea"));
    assert!(
        tea.is_file(),
        "✗ {test_name}: Test file not found at {}",
        tea.display()
    );

    let tool = Path::new(env!("CARGO_BIN_EXE_teac"));
    let output = Command::new(tool)
        .arg(&tea)
        .arg("--emit")
        .arg("ast")
        .output()
        .expect("Failed to execute teac");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "✗ Parse failed for {test_name} (exit {}). stderr:\n{stderr}",
        output.status.code().unwrap_or(-1)
    );
    assert!(
        stderr.is_empty(),
        "✗ Parse produced warnings/errors for {test_name}:\n{stderr}"
    );

    let ast_output = String::from_utf8_lossy(&output.stdout);
    assert!(
        !ast_output.trim().is_empty(),
        "✗ AST output is empty for {test_name}"
    );

    for expected in must_contain {
        assert!(
            ast_output.contains(expected),
            "✗ AST for {test_name} must contain \"{expected}\".\n\
             Hint: this identifier should appear in any correct AST for this program.\n\
             AST output ({} lines):\n{}",
            ast_output.lines().count(),
            if ast_output.len() > 2000 {
                format!("{}...(truncated)", &ast_output[..2000])
            } else {
                ast_output.to_string()
            }
        );
    }
}

/// Negative compilation test: asserts that `teac` rejects the fixture
/// and prints a non-empty diagnostic.  The exact message is not checked
/// so students have freedom in how they phrase their diagnostics.
//
// `#[allow(dead_code)]` because the only in-tree caller is `type_infer_5`,
// which is itself gated on `return-type-inference`.
#[allow(dead_code)]
fn test_compile_error(test_name: &str) {
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let case_dir = base_dir.join(test_name);
    let tea = case_dir.join(format!("{test_name}.tea"));
    assert!(
        tea.is_file(),
        "✗ {test_name}: Test file not found at {}",
        tea.display()
    );

    let tool = Path::new(env!("CARGO_BIN_EXE_teac"));
    let output = Command::new(tool)
        .arg(&tea)
        .arg("--emit")
        .arg("ir")
        .output()
        .expect("Failed to execute teac");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "✗ Expected compilation to fail for {test_name}, but it succeeded.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.trim().is_empty(),
        "✗ Compilation failed for {test_name} as expected, but no error message \
         was produced on stderr. The student's implementation should print a \
         diagnostic explaining what is wrong."
    );
    assert!(
        !stderr.contains("not yet implemented")
            && !stderr.contains("not implemented"),
        "✗ Compilation failed for {test_name}, but the failure came from an \
         unimplemented `todo!()` in the student's code, not from a real \
         diagnostic.  Finish implementing the pass before this negative \
         test can be considered passing.\nstderr:\n{stderr}"
    );
}

/// Compile / link / run test.  Expected fixture layout:
///
/// ```text
/// tests/<test_name>/
///   <test_name>.tea    — TeaLang source (required)
///   std.teah           — stdlib header (required by `use std;`)
///   <test_name>.in     — stdin for the program (optional)
///   <test_name>.out    — golden stdout + exit code (required)
///   build/             — created on demand; holds .s and the executable
/// ```
fn test_single(test_name: &str) {
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let case_dir = base_dir.join(test_name);

    let out_dir = case_dir.join("build");
    fs::create_dir_all(&out_dir).expect("Failed to create output dir");

    let tea = case_dir.join(format!("{test_name}.tea"));
    assert!(
        tea.is_file(),
        "✗ {test_name}: Test file not found at {}",
        tea.display()
    );

    // -----------------------------------------------------------------------
    // Step 1: Compile TeaLang source to assembly
    // -----------------------------------------------------------------------
    let output_name = format!("{test_name}.s");
    let output_path = out_dir.join(&output_name);
    let output = launch(
        &case_dir,
        &format!("{test_name}.tea"),
        output_path.to_str().unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "✗ Compilation failed (exit {}). teac stderr:\n{stderr}",
        output.status.code().unwrap_or(-1)
    );
    assert!(
        stderr.is_empty(),
        "✗ Compilation produced stderr:\n{stderr}"
    );

    assert!(
        output_path.is_file(),
        "Expected compiler to produce {}",
        output_path.display()
    );

    // -----------------------------------------------------------------------
    // Step 2: Locate the pre-built stdlib object file
    // -----------------------------------------------------------------------
    let stdlib = get_std_o_path();
    assert!(
        stdlib.is_file(),
        "✗ std.o not found at {}",
        stdlib.display()
    );

    let input = case_dir.join(format!("{test_name}.in"));
    let expected_out = case_dir.join(format!("{test_name}.out"));
    let actual_out = out_dir.join(format!("{test_name}.out"));

    let input_path = if input.is_file() {
        Some(input.as_path())
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Step 3: Link assembly + stdlib → executable and run (platform-specific)
    // -----------------------------------------------------------------------
    let (run_code, run_stdout, run_stderr) = if is_native_macos() {
        let exe = out_dir.join(test_name);
        let (link_code, link_err) = link_local(&out_dir, "cc", &output_path, &stdlib, &exe, &[])
            .expect("Failed to link");
        assert!(
            link_code == 0,
            "✗ Linking failed (exit {link_code}). Stderr:\n{}",
            String::from_utf8_lossy(&link_err)
        );
        run_native(&exe, input_path).expect("Failed to run executable")
    } else if is_docker_macos() {
        link_and_run_in_docker(&out_dir, &output_name, &stdlib, test_name, input_path)
            .expect("Failed to run in Docker")
    } else if is_cross_linux() {
        let exe = out_dir.join(test_name);
        let (link_code, link_err) = link_local(
            &out_dir,
            "aarch64-linux-gnu-gcc",
            &output_path,
            &stdlib,
            &exe,
            &["-static"],
        )
        .expect("Failed to link");
        assert!(
            link_code == 0,
            "✗ Linking failed (exit {link_code}). Stderr:\n{}",
            String::from_utf8_lossy(&link_err)
        );
        run_with_qemu(&exe, input_path).expect("Failed to run with QEMU")
    } else {
        let exe = out_dir.join(test_name);
        let (link_code, link_err) = link_local(&out_dir, "gcc", &output_path, &stdlib, &exe, &[])
            .expect("Failed to link");
        assert!(
            link_code == 0,
            "✗ Linking failed (exit {link_code}). Stderr:\n{}",
            String::from_utf8_lossy(&link_err)
        );
        run_native(&exe, input_path).expect("Failed to run executable")
    };

    // On Docker macOS, linking errors surface on the run phase's stderr
    // rather than as a non-zero link exit code; propagate them here.
    if !run_stderr.is_empty() {
        let stderr_str = String::from_utf8_lossy(&run_stderr);
        if stderr_str.contains("Linking failed") {
            panic!("✗ Linking failed. Stderr:\n{stderr_str}");
        }
    }

    // -----------------------------------------------------------------------
    // Step 4: Write actual output (stdout + exit code) to file
    // -----------------------------------------------------------------------
    fs::write(&actual_out, &run_stdout)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", actual_out.display()));
    append_line(&actual_out, &run_code.to_string());

    // -----------------------------------------------------------------------
    // Step 5: Compare actual output against the golden .out file
    // -----------------------------------------------------------------------
    match read_to_string_if_exists(&expected_out).expect("Failed to read expected output file") {
        Some(exp) => {
            let got = fs::read_to_string(&actual_out)
                .unwrap_or_else(|e| panic!("Failed to read {}: {e}", actual_out.display()));
            let exp_norm = normalize_for_diff(&exp);
            let got_norm = normalize_for_diff(&got);
            if exp_norm != got_norm {
                if std::env::var_os("VERBOSE").is_some() {
                    eprintln!("✗ Output mismatch for {test_name}");
                    eprintln!("--- Expected:\n{exp}");
                    eprintln!("--- Got:\n{got}");
                }
                panic!("Output mismatch for {test_name}");
            }
        }
        None => {
            panic!(
                "✗ No expected output file for {test_name} at {}",
                expected_out.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test declaration macros
// ---------------------------------------------------------------------------

/// Declares a batch of `test_single` tests from test-case names.
macro_rules! full_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                ensure_std();
                test_single(stringify!($name));
            }
        )*
    };
}

/// Declares a batch of `test_ast_parse` tests from `name => [idents]` pairs.
#[allow(unused_macros)]
macro_rules! ast_tests {
    ($($name:ident => [$($ident:literal),* $(,)?]),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                test_ast_parse(stringify!($name), &[$($ident),*]);
            }
        )*
    };
}

// ── Full compile-link-run tests ─────────────────────────────────────────────

full_tests! {
    dfs,
    bfs,
    big_int_mul,
    bin_search,
    brainfk,
    conv,
    dijkstra,
    expr_eval,
    full_conn,
    hanoi,
    insert_order,
    int_io,
    int_split,
    jump_game,
    line_search,
    long_code,
    long_code2,
    many_globals,
    many_locals2,
    matrix_mul,
    nested_calls,
    nested_loops,
    palindrome_number,
    register_alloca,
    short_circuit3,
    sort_test5,
    sort_test7,
    sort,
    unique_path,
    type_infer_basic,
}

// ── Return-type-inference tests (feature-gated) ─────────────────────────────
//
// type_infer_1..5 exercise the return-type-inference pass (every `fn`
// omits its `-> T` clause).  Without the feature the baseline treats
// omitted returns as `-> void`, so these tests would fail spuriously.

#[cfg(feature = "return-type-inference")]
full_tests! {
    type_infer_1,
    type_infer_2,
    type_infer_3,
    type_infer_4,
}

#[cfg(feature = "return-type-inference")]
#[test]
fn type_infer_5() {
    // Negative test — compilation must fail; no linking/running needed.
    test_compile_error("type_infer_5");
}

// ── AST parse-only tests (feature-gated) ────────────────────────────────────
//
// These tests cover language features whose parser support is not yet
// implemented.  Each group is gated on its own Cargo feature (all off by
// default) so `cargo test` stays green until the corresponding syntax
// lands.  Turn a feature on to opt in to its tests:
//   cargo test --features float
//   cargo test --features for-loop
//   cargo test --features struct-method
//   cargo test --features multi-dim-array

#[cfg(feature = "float")]
ast_tests! {
    float_basic => ["main"],
    float_arith => ["main", "matmul", "print_row"],
    float_cmp   => ["main"],
    float_cast  => ["main", "result"],
    float_func  => ["main", "fadd", "fmul", "compute"],
}

#[cfg(feature = "for-loop")]
ast_tests! {
    for_basic    => ["main", "sum", "prod"],
    for_continue => ["main", "sum", "count", "bsum", "total"],
    for_mixed    => ["main", "fibonacci", "factorial", "power"],
    for_nested   => ["main", "total"],
    for_range    => ["main", "get_limit"],
}

#[cfg(feature = "struct-method")]
ast_tests! {
    struct_method_basic     => ["main", "Counter", "get", "add", "value"],
    struct_method_calls     => ["main", "Pair", "sum", "fill"],
    struct_method_namespace => ["main", "calc", "mix"],
    struct_method_loop      => ["main", "Acc", "push"],
    struct_method_nested    => ["main", "Vec2", "Body", "step", "energy"],
}

#[cfg(feature = "multi-dim-array")]
ast_tests! {
    array_2d_basic  => ["main", "mat"],
    array_2d_init   => ["main", "mat", "sum"],
    array_2d_matmul => ["main"],
    array_3d        => ["main", "cube"],
    array_attention => ["main", "scores"],
}
