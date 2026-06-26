//! The `lumen` command-line driver.
//!
//! This is a thin shell over the library: it parses argv into a subcommand and
//! dispatches. As later phases land (lexer, parser, ..., REPL) each subcommand
//! is fleshed out. For now the binary just reports its version so the crate has
//! a real, runnable entry point from day one.

use lumen::diagnostics::Diagnostic;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | Some("--version") | Some("-V") => {
            println!("lumen {VERSION}");
            ExitCode::SUCCESS
        }
        Some("run") if args.len() > 1 => cmd_run(args.get(1).map(String::as_str)),
        Some("run") => ExitCode::from(lumen::project::cmd_run() as u8),
        Some("new") => ExitCode::from(lumen::project::cmd_new(args.get(1).map(String::as_str)) as u8),
        Some("build") => ExitCode::from(lumen::project::cmd_build() as u8),
        Some("test") => ExitCode::from(lumen::project::cmd_test() as u8),
        Some("repl") => ExitCode::from(lumen::repl::run() as u8),
        Some("debug") => cmd_debug(args.get(1).map(String::as_str)),
        Some("lsp") => ExitCode::from(lumen::lsp::run() as u8),
        Some("bench") => cmd_bench(),
        Some("lex") => cmd_lex(args.get(1).map(String::as_str)),
        Some("parse") => cmd_parse(args.get(1).map(String::as_str)),
        Some("fmt") => cmd_fmt(&args[1..]),
        Some("disasm") => cmd_disasm(args.get(1).map(String::as_str)),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("lumen: unknown command '{other}'");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

/// Read a source file or exit with a clear message.
fn read_source(path: Option<&str>) -> Result<(String, String), ExitCode> {
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("lumen: expected a file path");
            return Err(ExitCode::FAILURE);
        }
    };
    match std::fs::read_to_string(path) {
        Ok(src) => Ok((path.to_string(), src)),
        Err(e) => {
            eprintln!("lumen: cannot read '{path}': {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn print_diagnostics(diags: &[Diagnostic], src: &str, file: &str) {
    for d in diags {
        eprintln!("{}\n", d.render(src, Some(file)));
    }
}

/// `lumen run <file>` — compile and execute a program.
fn cmd_run(path: Option<&str>) -> ExitCode {
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (program, errors) = lumen::check_source(&src);
    if !errors.is_empty() {
        print_diagnostics(&errors, &src, &file);
        eprintln!("{} error(s)", errors.len());
        return ExitCode::FAILURE;
    }
    let proto = match lumen::compiler::compile(&program) {
        Ok(p) => p,
        Err(errors) => {
            print_diagnostics(&errors, &src, &file);
            return ExitCode::FAILURE;
        }
    };
    let mut vm = lumen::vm::Vm::new();
    lumen::stdlib::install(&mut vm);
    if let Some(dir) = std::path::Path::new(&file).parent() {
        if !dir.as_os_str().is_empty() {
            vm.set_base_dir(dir.to_path_buf());
        }
    }
    // Expose any trailing arguments via os.args().
    let script_args: Vec<String> = std::env::args().skip(3).collect();
    vm.set_args(script_args);
    // Diagnostic toggle: collect on every allocation so memory-checking tools
    // (valgrind, sanitizers) exercise the full mark/sweep on real programs.
    if std::env::var_os("LUMEN_STRESS_GC").is_some() {
        vm.set_stress_gc(true);
    }
    match vm.interpret(proto) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprint!("{msg}");
            // SPEC §7: uncaught throw exits with code 70.
            ExitCode::from(70)
        }
    }
}

/// `lumen bench` — run the standard micro-benchmarks and print timings.
fn cmd_bench() -> ExitCode {
    let benches: &[(&str, &str)] = &[
        ("fib(32) recursive", "fn fib(n) { if n < 2 { return n; } return fib(n-1) + fib(n-2); } fib(32);"),
        ("loop sum to 10M", "let s = 0; for let i = 0; i < 10000000; i = i + 1 { s = s + i; } s;"),
        ("array alloc x1M (GC)", "let last = nil; for let i = 0; i < 1000000; i = i + 1 { last = [i, i + 1]; } last;"),
        ("string build x100k", "let s = \"\"; for let i = 0; i < 100000; i = i + 1 { s = s + \"x\"; } len(s);"),
        ("method dispatch x1M", "class C { init() { this.n = 0; } inc() { this.n = this.n + 1; } } let c = C(); for let i = 0; i < 1000000; i = i + 1 { c.inc(); } c.n;"),
    ];
    println!("Lumen benchmarks (build: {})", if cfg!(debug_assertions) { "debug" } else { "release" });
    println!("{:<24} {:>12}", "benchmark", "time");
    println!("{}", "-".repeat(38));
    for (name, src) in benches {
        let (program, errs) = lumen::check_source(src);
        if !errs.is_empty() {
            eprintln!("bench '{name}' failed to compile: {:?}", errs[0].message);
            continue;
        }
        let proto = match lumen::compiler::compile(&program) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let mut vm = lumen::vm::Vm::with_output(Box::new(std::io::sink()));
        lumen::stdlib::install(&mut vm);
        let start = std::time::Instant::now();
        let result = vm.interpret(proto);
        let elapsed = start.elapsed();
        match result {
            Ok(()) => println!("{name:<24} {:>10.1?}", elapsed),
            Err(_) => println!("{name:<24} {:>12}", "ERROR"),
        }
    }
    ExitCode::SUCCESS
}

/// `lumen debug <file>` — run a program under the interactive debugger.
fn cmd_debug(path: Option<&str>) -> ExitCode {
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    ExitCode::from(lumen::debugger::run(&file, &src) as u8)
}

/// `lumen lex <file>` — tokenize a file and print the tokens (debugging aid).
fn cmd_lex(path: Option<&str>) -> ExitCode {
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (tokens, errors) = lumen::lex(&src);
    for tok in &tokens {
        println!("{:>4}:{:<3} {:?}", tok.span.line, tok.span.col, tok.kind);
    }
    if errors.is_empty() {
        eprintln!("\n{} tokens, no lexical errors", tokens.len());
        ExitCode::SUCCESS
    } else {
        eprintln!();
        print_diagnostics(&errors, &src, &file);
        eprintln!("{} lexical error(s)", errors.len());
        ExitCode::FAILURE
    }
}

/// `lumen parse <file>` — parse, resolve, and report errors, or confirm success.
fn cmd_parse(path: Option<&str>) -> ExitCode {
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (program, errors) = lumen::check_source(&src);
    if errors.is_empty() {
        println!("{}: checked {} top-level item(s), no errors", file, program.items.len());
        ExitCode::SUCCESS
    } else {
        print_diagnostics(&errors, &src, &file);
        eprintln!("{} error(s)", errors.len());
        ExitCode::FAILURE
    }
}

/// `lumen fmt [--write] <file>` — format a source file. With `--write` the file
/// is rewritten in place; otherwise the formatted source is printed to stdout.
fn cmd_fmt(args: &[String]) -> ExitCode {
    let write = args.iter().any(|a| a == "--write" || a == "-w");
    let path = args.iter().find(|a| !a.starts_with('-')).map(String::as_str);
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (program, errors) = lumen::parse_source(&src);
    if !errors.is_empty() {
        print_diagnostics(&errors, &src, &file);
        eprintln!("cannot format a file with errors");
        return ExitCode::FAILURE;
    }
    let formatted = lumen::ast_printer::print_program(&program);
    if write {
        match std::fs::write(&file, &formatted) {
            Ok(()) => {
                eprintln!("formatted {file}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("lumen: cannot write '{file}': {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        print!("{formatted}");
        ExitCode::SUCCESS
    }
}

/// `lumen disasm <file>` — compile to bytecode and print the disassembly.
fn cmd_disasm(path: Option<&str>) -> ExitCode {
    let (file, src) = match read_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (program, errors) = lumen::check_source(&src);
    if !errors.is_empty() {
        print_diagnostics(&errors, &src, &file);
        return ExitCode::FAILURE;
    }
    match lumen::compiler::compile(&program) {
        Ok(proto) => {
            print!("{}", lumen::disassembler::disassemble(&proto));
            ExitCode::SUCCESS
        }
        Err(errors) => {
            print_diagnostics(&errors, &src, &file);
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!("lumen {VERSION} — the Lumen programming language");
    println!();
    println!("usage: lumen <command> [args]");
    println!();
    println!("commands:");
    println!("  run <file>          compile and execute a .lum program");
    println!("  run                 run the current project (needs lumen.toml)");
    println!("  repl                start an interactive session");
    println!("  debug <file>        run under the interactive debugger");
    println!("  fmt [--write] <f>   format a source file (in place with --write)");
    println!("  disasm <file>       disassemble a program to bytecode");
    println!("  new <name>          scaffold a new project");
    println!("  build               static-check the current project");
    println!("  test                run the project's tests/ files");
    println!("  lsp                 run the language server (stdio)");
    println!("  lex/parse <file>    inspect tokens / run the front end");
    println!("  version             print the version");
    println!("  help                show this message");
}
