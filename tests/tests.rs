/*
 * Integration tests for the TeaLang compiler.
 *
 * Supported platforms:
 *   - Native AArch64 Linux
 *   - x86/x86_64 Linux  (cross-compile with aarch64-linux-gnu-gcc + QEMU)
 *   - macOS AArch64 / Apple Silicon  (native cc toolchain)
 *   - macOS x86_64  (Docker linux/arm64 emulation)
 *
 * Manual compilation equivalent (using the `dfs` test case as an example):
 *
 * Step 1 – Compile TeaLang source to assembly.
 *   The compiler must be invoked from inside the test-case directory so that
 *   `std.teah` is resolved as `./std.teah`:
 *
 *     cd tests/dfs && mkdir -p build
 *     ../../target/debug/teac dfs.tea --emit asm -o build/dfs.s
 *
 * Step 2 – Compile the C standard library to an object file (once per platform):
 *
 *     gcc -c tests/std/std.c -o tests/std/std.o                           # Linux native
 *     cc  -c tests/std/std.c -o tests/std/std-macos.o                    # macOS AArch64
 *     aarch64-linux-gnu-gcc -c tests/std/std.c -o tests/std/std-linux.o  # cross-compile
 *
 * Step 3 – Link assembly + stdlib object into an executable and run:
 *
 *     gcc  build/dfs.s ../std/std.o -o build/dfs                              # Linux native
 *     cc   build/dfs.s ../std/std-macos.o -o build/dfs                       # macOS AArch64
 *     aarch64-linux-gnu-gcc build/dfs.s ../std/std-linux.o -o build/dfs -static  # cross-compile
 *     qemu-aarch64 build/dfs < dfs.in                                        # run via QEMU
 */

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Once;

static INIT: Once = Once::new();

/// Returns `true` when running natively on macOS AArch64 (Apple Silicon).
fn is_native_macos() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// Returns `true` when running on macOS with a non-AArch64 host (e.g., Intel Mac).
/// In this configuration Docker is used to emulate an AArch64 Linux environment.
fn is_docker_macos() -> bool {
    cfg!(all(target_os = "macos", not(target_arch = "aarch64")))
}

/// Returns `true` when running on x86 or x86_64 Linux.
/// In this configuration the AArch64 cross-compiler and QEMU are used to
/// build and run the test binaries.
fn is_cross_linux() -> bool {
    cfg!(all(
        target_os = "linux",
        any(target_arch = "x86", target_arch = "x86_64")
    ))
}

/// Checks whether `cmd` is available on PATH by running `which <cmd>`.
/// Returns `true` if the command is found and `false` otherwise.
fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Verifies that all external tools required for the current platform are
/// installed.  Panics with a human-readable install hint if a required tool
/// is missing.
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

/// Returns the platform-specific path for the compiled std object file:
///   - macOS AArch64 → `tests/std/std-macos.o`
///   - Docker macOS or cross-compile Linux → `tests/std/std-linux.o`
///   - Native AArch64 Linux → `tests/std/std.o`
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

/// Compiles `tests/std/std.c` using the system `cc` toolchain on macOS AArch64,
/// producing the object file at `o_path`.
fn compile_std_native_macos(std_dir: &Path, o_path: &Path) {
    let status = Command::new("cc")
        .arg("-c")
        .arg("std.c")
        .arg("-o")
        .arg(o_path)
        .current_dir(std_dir)
        .status()
        .expect("Failed to execute cc");

    assert!(
        status.success(),
        "✗ cc failed to build {} (exit {}). Ran in {}",
        o_path.display(),
        status.code().unwrap_or(-1),
        std_dir.display()
    );
}

/// Compiles `tests/std/std.c` inside a `linux/arm64` Docker container so that
/// the resulting object file is an AArch64 ELF object (compatible with the
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

/// Compiles `tests/std/std.c` using the AArch64 cross-compiler
/// (`aarch64-linux-gnu-gcc`) on an x86/x86_64 Linux host.
fn compile_std_cross_linux(std_dir: &Path, o_path: &Path) {
    let status = Command::new("aarch64-linux-gnu-gcc")
        .arg("-c")
        .arg("std.c")
        .arg("-o")
        .arg(o_path)
        .current_dir(std_dir)
        .status()
        .expect("Failed to execute aarch64-linux-gnu-gcc");

    assert!(
        status.success(),
        "✗ aarch64-linux-gnu-gcc failed to build {} (exit {}). Ran in {}",
        o_path.display(),
        status.code().unwrap_or(-1),
        std_dir.display()
    );
}

/// Ensures the standard-library object file is up-to-date before any test
/// runs.  Uses a [`Once`] guard so the build happens **at most once** per
/// process even when multiple tests run in parallel.  Rebuilds the object
/// only when `std.c` is newer than the existing `.o` file.
fn ensure_std() {
    // INIT is a process-wide Once flag; the closure runs exactly once no
    // matter how many tests call ensure_std() concurrently.
    INIT.call_once(|| {
        ensure_cross_tools();

        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let std_dir = project_root.join("tests").join("std");
        let c_path = std_dir.join("std.c");
        let o_path = get_std_o_path();

        // Rebuild only when std.c is newer than the existing .o (mtime comparison),
        // or when the .o does not yet exist.  Missing std.c is a fatal error.
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
                compile_std_native_macos(&std_dir, &o_path);
            } else if is_docker_macos() {
                compile_std_in_docker(&std_dir, &o_path);
            } else if is_cross_linux() {
                compile_std_cross_linux(&std_dir, &o_path);
            } else {
                let status = Command::new("gcc")
                    .arg("-c")
                    .arg("std.c")
                    .arg("-o")
                    .arg(&o_path)
                    .current_dir(&std_dir)
                    .status()
                    .expect("Failed to execute gcc");

                assert!(
                    status.success(),
                    "✗ gcc failed to build {} (exit {}). Ran in {}",
                    o_path.display(),
                    status.code().unwrap_or(-1),
                    std_dir.display()
                );
            }
        }
        assert!(
            o_path.is_file(),
            "✗ std.o not found at {}",
            o_path.display()
        );
    });
}

/// Invokes the `teac` compiler binary with `--emit asm` to produce an assembly
/// file from a TeaLang source file.
///
/// `dir` is set as the working directory for the compiler process.  The
/// `input_file` argument is intentionally passed as a **bare filename** (no
/// directory component) so that `teac` resolves `source_dir` to `.`, which
/// is the working directory — i.e. `dir`.  This ensures that the `use std`
/// statement in the source file finds `std.teah` as `./std.teah` inside the
/// test-case directory.
///
/// On Docker-macOS hosts the `--target linux` flag is added so that `teac`
/// emits Linux AArch64 assembly instead of macOS assembly.
#[inline(always)]
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

/// Normalizes a string for whitespace-insensitive comparison: collapses runs
/// of whitespace within each line into a single space, drops blank lines, and
/// appends a trailing newline.  Used to compare expected vs. actual output
/// without being sensitive to trailing spaces or blank lines.
fn normalize_for_diff_bb(s: &str) -> String {
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

/// Reads the file at `path` into a `String`.  Returns `Ok(None)` if the file
/// does not exist (instead of propagating a `NotFound` error), and `Err` for
/// any other I/O error.
fn read_to_string_if_exists(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Runs a pre-configured [`Command`] and returns `(exit_code, stdout, stderr)`.
fn run_capture(cmd: &mut Command) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let output = cmd.output()?;
    let code = output.status.code().unwrap_or(-1);
    Ok((code, output.stdout, output.stderr))
}

/// Appends `line` followed by a newline to the file at `path`, creating the
/// file if it does not exist.  Used to append the program's exit code as the
/// final line of the actual output file so it can be compared with the golden
/// `*.out` file.
fn append_line<P: AsRef<Path>>(path: P, line: &str) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .unwrap_or_else(|e| panic!("Failed to open {} for append: {e}", path.as_ref().display()));
    writeln!(f, "{line}").expect("Failed to append line");
}

/// Links assembly with `std_o` inside a `linux/arm64` Docker container
/// producing a statically-linked AArch64 executable, then runs it inside a
/// minimal Debian container.
///
/// Two-phase design:
///   - **Link phase**: uses the `gcc:latest` Docker image which contains the
///     full GNU toolchain.
///   - **Run phase**: uses `debian:bookworm-slim` (no compiler) for a lighter
///     runtime image; the executable is mounted read-only from the host.
///
/// Optional `input` is piped to the program's stdin when provided.
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

    if let Some(input_path) = input {
        let mut data = Vec::new();
        File::open(input_path)?.read_to_end(&mut data)?;

        let mut child = run_cmd
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
        run_capture(
            run_cmd
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
    }
}

/// Links assembly and the stdlib object using the AArch64 cross-linker
/// (`aarch64-linux-gnu-gcc`) with `-static`, producing a statically-linked
/// AArch64 ELF executable.  Returns `(exit_code, stderr_bytes)`.
fn link_cross_linux(
    build_dir: &Path,
    asm_path: &Path,
    std_o: &Path,
    exe_path: &Path,
) -> io::Result<(i32, Vec<u8>)> {
    let output = Command::new("aarch64-linux-gnu-gcc")
        .arg(asm_path)
        .arg(std_o)
        .arg("-o")
        .arg(exe_path)
        .arg("-static")
        .current_dir(build_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok((output.status.code().unwrap_or(-1), output.stderr))
}

/// Runs an AArch64 ELF executable under `qemu-aarch64`.  Optional `input` is
/// piped to stdin.  Returns `(exit_code, stdout_bytes, stderr_bytes)`.
fn run_with_qemu(exe: &Path, input: Option<&Path>) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    if let Some(input_path) = input {
        let mut data = Vec::new();
        File::open(input_path)?.read_to_end(&mut data)?;

        let mut child = Command::new("qemu-aarch64")
            .arg(exe)
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
        run_capture(
            Command::new("qemu-aarch64")
                .arg(exe)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
    }
}

/// Links assembly and the stdlib object using `cc` on macOS AArch64, producing
/// a native AArch64 Mach-O executable.  Returns `(exit_code, stderr_bytes)`.
fn link_native_macos(
    build_dir: &Path,
    asm_path: &Path,
    std_o: &Path,
    exe_path: &Path,
) -> io::Result<(i32, Vec<u8>)> {
    let output = Command::new("cc")
        .arg(asm_path)
        .arg(std_o)
        .arg("-o")
        .arg(exe_path)
        .current_dir(build_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok((output.status.code().unwrap_or(-1), output.stderr))
}

/// Links assembly and the stdlib object using `gcc` on a native AArch64 Linux
/// host, producing a native AArch64 ELF executable.  Returns
/// `(exit_code, stderr_bytes)`.
fn link_native(
    build_dir: &Path,
    asm_path: &Path,
    std_o: &Path,
    exe_path: &Path,
) -> io::Result<(i32, Vec<u8>)> {
    let output = Command::new("gcc")
        .arg(asm_path)
        .arg(std_o)
        .arg("-o")
        .arg(exe_path)
        .current_dir(build_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok((output.status.code().unwrap_or(-1), output.stderr))
}

/// Runs a native executable (AArch64 ELF or Mach-O).  Optional `input` is
/// piped to stdin.  Returns `(exit_code, stdout_bytes, stderr_bytes)`.
fn run_native(exe: &Path, input: Option<&Path>) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    if let Some(input_path) = input {
        let mut data = Vec::new();
        File::open(input_path)?.read_to_end(&mut data)?;

        let mut child = Command::new(exe)
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
        run_capture(
            Command::new(exe)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
    }
}

/// Runs a parse-only (AST-emit) test for the given `test_name`.
///
/// Locates `tests/<test_name>/<test_name>.tea`, invokes `teac --emit ast` on
/// it, checks that the command succeeds and produces no stderr output, and
/// then verifies that every identifier in `must_contain` appears somewhere in
/// the AST output.
///
/// Note: `teac` is given the **absolute path** to the source file, so
/// `source_dir` inside the compiler is set to the test-case directory and
/// `std.teah` is found there automatically — no `current_dir` override is
/// needed.
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

/// Runs a full compile-link-execute test for `test_name`.
///
/// Expected directory layout under `tests/<test_name>/`:
/// ```text
/// tests/<test_name>/
///   <test_name>.tea    — TeaLang source file (required)
///   std.teah           — standard-library header (required by `use std`)
///   <test_name>.in     — stdin input for the program (optional)
///   <test_name>.out    — golden stdout + exit-code output (required)
///   build/             — created automatically; receives .s and executable
/// ```
///
/// Five-step pipeline:
///   1. Compile `.tea` → `.s` using `teac --emit asm`
///   2. Locate the pre-built stdlib object file
///   3. Link `.s` + stdlib → executable (and run it), platform-specific
///   4. Write actual stdout + exit code to `build/<test_name>.out`
///   5. Compare actual output against the golden `<test_name>.out` file
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

    // --- Step 1: Compile TeaLang source to assembly ---
    // case_dir is used as the working directory so that `teac` resolves
    // `std.teah` relative to `./` (i.e. the test-case directory).
    // The source file is passed as a bare filename (no directory prefix) so
    // that `source_dir` inside the compiler falls back to `.` = case_dir.
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

    // --- Step 2: Locate the pre-built stdlib object file ---
    let stdlib = get_std_o_path();
    assert!(
        stdlib.is_file(),
        "✗ std.o not found at {}",
        stdlib.display()
    );

    let input = case_dir.join(format!("{test_name}.in"));
    let expected_out = case_dir.join(format!("{test_name}.out"));
    let actual_out = out_dir.join(format!("{test_name}.out"));

    // input_path is None when no `.in` file exists; the program reads from
    // /dev/null (or equivalent) in that case.
    let input_path = if input.is_file() {
        Some(input.as_path())
    } else {
        None
    };

    // --- Step 3: Link assembly + stdlib → executable and run (platform-specific) ---
    let (run_code, run_stdout, run_stderr) = if is_native_macos() {
        let exe = out_dir.join(test_name);
        let (link_code, link_err) =
            link_native_macos(&out_dir, &output_path, &stdlib, &exe).expect("Failed to link");
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
        let (link_code, link_err) =
            link_cross_linux(&out_dir, &output_path, &stdlib, &exe).expect("Failed to link");
        assert!(
            link_code == 0,
            "✗ Linking failed (exit {link_code}). Stderr:\n{}",
            String::from_utf8_lossy(&link_err)
        );
        run_with_qemu(&exe, input_path).expect("Failed to run with QEMU")
    } else {
        let exe = out_dir.join(test_name);
        let (link_code, link_err) =
            link_native(&out_dir, &output_path, &stdlib, &exe).expect("Failed to link");
        assert!(
            link_code == 0,
            "✗ Linking failed (exit {link_code}). Stderr:\n{}",
            String::from_utf8_lossy(&link_err)
        );
        run_native(&exe, input_path).expect("Failed to run executable")
    };

    // On Docker macOS, linking errors are reported via stderr of the run
    // phase rather than as a non-zero link exit code; propagate them as a
    // test failure here.
    if !run_stderr.is_empty() {
        let stderr_str = String::from_utf8_lossy(&run_stderr);
        if stderr_str.contains("Linking failed") {
            panic!("✗ Linking failed. Stderr:\n{stderr_str}");
        }
    }

    // --- Step 4: Write actual output (stdout + exit code) to file ---
    fs::write(&actual_out, &run_stdout)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", actual_out.display()));
    append_line(&actual_out, &run_code.to_string());

    // --- Step 5: Compare actual output against the golden .out file ---
    match read_to_string_if_exists(&expected_out).expect("Failed to read expected output file") {
        Some(exp) => {
            let got = fs::read_to_string(&actual_out)
                .unwrap_or_else(|e| panic!("Failed to read {}: {e}", actual_out.display()));
            let exp_norm = normalize_for_diff_bb(&exp);
            let got_norm = normalize_for_diff_bb(&got);
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

// ── Full compile-link-run tests ──────────────────────────────────────────────
// Each test calls ensure_std() to build std.o if needed, then test_single()
// which compiles the .tea file, links it with std.o, runs the result, and
// compares stdout+exit-code against the golden *.out file.

#[test]
fn dfs() {
    ensure_std();
    test_single("dfs");
}
#[test]
fn bfs() {
    ensure_std();
    test_single("bfs");
}
#[test]
fn big_int_mul() {
    ensure_std();
    test_single("big_int_mul");
}
#[test]
fn bin_search() {
    ensure_std();
    test_single("bin_search");
}
#[test]
fn brainfk() {
    ensure_std();
    test_single("brainfk");
}
#[test]
fn conv() {
    ensure_std();
    test_single("conv");
}
#[test]
fn dijkstra() {
    ensure_std();
    test_single("dijkstra");
}
#[test]
fn expr_eval() {
    ensure_std();
    test_single("expr_eval");
}
#[test]
fn full_conn() {
    ensure_std();
    test_single("full_conn");
}
#[test]
fn hanoi() {
    ensure_std();
    test_single("hanoi");
}
#[test]
fn insert_order() {
    ensure_std();
    test_single("insert_order");
}
#[test]
fn int_io() {
    ensure_std();
    test_single("int_io");
}
#[test]
fn int_split() {
    ensure_std();
    test_single("int_split");
}
#[test]
fn jump_game() {
    ensure_std();
    test_single("jump_game");
}
#[test]
fn line_search() {
    ensure_std();
    test_single("line_search");
}
#[test]
fn long_code() {
    ensure_std();
    test_single("long_code");
}
#[test]
fn long_code2() {
    ensure_std();
    test_single("long_code2");
}
#[test]
fn many_globals() {
    ensure_std();
    test_single("many_globals");
}
#[test]
fn many_locals2() {
    ensure_std();
    test_single("many_locals2");
}
#[test]
fn matrix_mul() {
    ensure_std();
    test_single("matrix_mul");
}
#[test]
fn nested_calls() {
    ensure_std();
    test_single("nested_calls");
}
#[test]
fn nested_loops() {
    ensure_std();
    test_single("nested_loops");
}
#[test]
fn palindrome_number() {
    ensure_std();
    test_single("palindrome_number");
}
#[test]
fn register_alloca() {
    ensure_std();
    test_single("register_alloca");
}
#[test]
fn short_circuit3() {
    ensure_std();
    test_single("short_circuit3");
}
#[test]
fn sort_test5() {
    ensure_std();
    test_single("sort_test5");
}
#[test]
fn sort_test7() {
    ensure_std();
    test_single("sort_test7");
}
#[test]
fn sort() {
    ensure_std();
    test_single("sort");
}
#[test]
fn unique_path() {
    ensure_std();
    test_single("unique_path");
}
#[test]
fn type_infer() {
    ensure_std();
    test_single("type_infer");
}
// ── AST parse-only tests ─────────────────────────────────────────────────────
// These tests only verify that teac can parse the source file and produce a
// non-empty AST containing the expected identifiers.  They do NOT link or run
// the program, so no std.o is needed.

#[test]
fn float_basic() {
    test_ast_parse("float_basic", &["main"]);
}
#[test]
fn float_arith() {
    test_ast_parse("float_arith", &["main", "matmul", "print_row"]);
}
#[test]
fn float_cmp() {
    test_ast_parse("float_cmp", &["main"]);
}
#[test]
fn float_cast() {
    test_ast_parse("float_cast", &["main", "result"]);
}
#[test]
fn float_func() {
    test_ast_parse("float_func", &["main", "fadd", "fmul", "compute"]);
}
#[test]
fn for_basic() {
    test_ast_parse("for_basic", &["main", "sum", "prod"]);
}
#[test]
fn for_continue() {
    test_ast_parse("for_continue", &["main", "sum", "count", "bsum", "total"]);
}
#[test]
fn for_mixed() {
    test_ast_parse("for_mixed", &["main", "fibonacci", "factorial", "power"]);
}
#[test]
fn for_nested() {
    test_ast_parse("for_nested", &["main", "total"]);
}
#[test]
fn for_range() {
    test_ast_parse("for_range", &["main", "get_limit"]);
}
#[test]
fn struct_method_basic() {
    test_ast_parse(
        "struct_method_basic",
        &["main", "Counter", "get", "add", "value"],
    );
}
#[test]
fn struct_method_calls() {
    test_ast_parse("struct_method_calls", &["main", "Pair", "sum", "fill"]);
}
#[test]
fn struct_method_namespace() {
    test_ast_parse("struct_method_namespace", &["main", "calc", "mix"]);
}
#[test]
fn struct_method_loop() {
    test_ast_parse("struct_method_loop", &["main", "Acc", "push"]);
}
#[test]
fn struct_method_nested() {
    test_ast_parse(
        "struct_method_nested",
        &["main", "Vec2", "Body", "step", "energy"],
    );
}
#[test]
fn array_2d_basic() {
    test_ast_parse("array_2d_basic", &["main", "mat"]);
}
#[test]
fn array_2d_init() {
    test_ast_parse("array_2d_init", &["main", "mat", "sum"]);
}
#[test]
fn array_2d_matmul() {
    test_ast_parse("array_2d_matmul", &["main"]);
}
#[test]
fn array_3d() {
    test_ast_parse("array_3d", &["main", "cube"]);
}
#[test]
fn attention() {
    test_ast_parse("attention", &["main", "scores"]);
}
