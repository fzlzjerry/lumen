//! The interactive REPL.
//!
//! Features: **multi-line input** (keeps reading while brackets are unbalanced or
//! a string/comment is open), **persistent state** (one VM across inputs, so
//! `let`s and `fn`s carry over), **immediate evaluation** (a trailing bare
//! expression prints its value), **history** (in memory and saved to
//! `~/.lumen_history`, listable), and **syntax highlighting** of assembled
//! multi-line blocks plus a `:hl` command.
//!
//! Line editing: on a TTY the REPL uses a raw-mode line editor (see `lineedit`)
//! providing cursor movement, history recall, and word/line kills via a
//! hand-written termios binding (no external crate). When stdin isn't a TTY
//! (pipes, tests) it falls back to ordinary line-buffered reading. Assembled
//! inputs are echoed back highlighted, and results/errors are colorized.

use crate::highlight::highlight;
use crate::lineedit::{self, Input};
use crate::token::TokenKind;
use crate::value::Value;
use crate::vm::Vm;
use std::io::{self, BufRead, Write};

const PROMPT: &str = "\x1b[1;34mlumen>\x1b[0m ";
const CONT: &str = "\x1b[1;34m.....\x1b[0m ";

/// Start the REPL. Returns the process exit code.
pub fn run() -> i32 {
    let mut vm = Vm::new();
    crate::stdlib::install(&mut vm);
    let mut globals: Vec<String> = Vec::new();
    let mut history: Vec<String> = load_history();

    println!("Lumen {} — interactive REPL", env!("CARGO_PKG_VERSION"));
    println!("Type :help for commands, :quit (or Ctrl-D) to exit.\n");

    // Raw-mode editing on a TTY; the guard restores cooked mode on drop.
    let raw = lineedit::RawGuard::enable();
    while let Some(input) = read_input(raw.is_some(), &history) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with(':') {
            if handle_command(trimmed, &history) {
                break; // :quit
            }
            continue;
        }

        history.push(input.clone());
        // Echo the input back, highlighted (our stand-in for live highlighting).
        if input.contains('\n') {
            print!("{}", highlight(input.trim_end()));
            println!();
        }

        run_input(&mut vm, &input, &mut globals);
    }

    save_history(&history);
    println!("\nGoodbye.");
    0
}

/// Lex/parse/resolve/compile/run one input against the persistent VM.
fn run_input(vm: &mut Vm, raw: &str, globals: &mut Vec<String>) {
    // Accept a bare trailing expression (no `;`): if the raw input doesn't parse
    // but appending `;` does, evaluate that (so `x + y` prints its value).
    let input = prepare_input(raw);
    let (program, mut errs) = crate::parse_source(&input);
    if errs.is_empty() {
        errs = crate::resolver::resolve_with(&program, globals);
    }
    if !errs.is_empty() {
        for d in &errs {
            eprintln!("{}\n", d.render(&input, None));
        }
        return;
    }
    match crate::compiler::compile_repl(&program) {
        Ok((proto, print_result)) => match vm.eval(proto) {
            Ok(value) => {
                if print_result && !matches!(value, Value::Nil) {
                    match vm.to_display(value, true) {
                        Ok(s) => println!("\x1b[2m=>\x1b[0m {s}"),
                        Err(_) => println!("=> <unprintable>"),
                    }
                }
                *globals = vm.global_names();
            }
            Err(msg) => eprint!("{msg}"),
        },
        Err(errs) => {
            for d in &errs {
                eprintln!("{}\n", d.render(&input, None));
            }
        }
    }
}

/// Return the input to actually compile: the raw text, or — if that fails to
/// parse and a trailing `;` fixes it — the text with `;` appended.
fn prepare_input(raw: &str) -> String {
    let (_, errs) = crate::parse_source(raw);
    if errs.is_empty() {
        return raw.to_string();
    }
    let trimmed = raw.trim_end();
    if !trimmed.ends_with(';') && !trimmed.ends_with('}') {
        let with_semi = format!("{trimmed};");
        let (_, errs2) = crate::parse_source(&with_semi);
        if errs2.is_empty() {
            return with_semi;
        }
    }
    raw.to_string()
}

/// Read one logical input, continuing across lines while it is incomplete.
/// Uses the raw-mode editor on a TTY (`raw`), else line-buffered stdin.
fn read_input(raw: bool, history: &[String]) -> Option<String> {
    if raw {
        read_input_raw(history)
    } else {
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        read_input_cooked(&mut handle)
    }
}

/// Raw-mode path: edit each physical line with `lineedit`, accumulating until the
/// logical input is complete.
fn read_input_raw(history: &[String]) -> Option<String> {
    let mut buf = String::new();
    loop {
        let prompt = if buf.is_empty() { PROMPT } else { CONT };
        match lineedit::read_line(prompt, history).ok()? {
            Input::Line(line) => {
                buf.push_str(&line);
                buf.push('\n');
                if !is_incomplete(&buf) {
                    return Some(buf);
                }
            }
            Input::Eof => {
                return if buf.trim().is_empty() { None } else { Some(buf) };
            }
            Input::Interrupt => {
                // Ctrl-C abandons the current (possibly multi-line) input.
                buf.clear();
            }
        }
    }
}

/// Cooked-mode path: ordinary line-buffered reading (pipes, non-TTY, tests).
fn read_input_cooked(handle: &mut impl BufRead) -> Option<String> {
    let mut buf = String::new();
    loop {
        print!("{}", if buf.is_empty() { PROMPT } else { CONT });
        let _ = io::stdout().flush();
        let mut line = String::new();
        let n = handle.read_line(&mut line).ok()?;
        if n == 0 {
            // EOF: return any buffered partial input, else signal exit.
            return if buf.trim().is_empty() { None } else { Some(buf) };
        }
        buf.push_str(&line);
        if !is_incomplete(&buf) {
            return Some(buf);
        }
    }
}

/// Is `src` an unfinished input (open brackets or an open string/comment)?
fn is_incomplete(src: &str) -> bool {
    let (tokens, errs) = crate::lexer::lex(src);
    if errs.iter().any(|e| e.message.contains("unterminated")) {
        return true;
    }
    let mut depth: i32 = 0;
    for t in &tokens {
        match t.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

/// Handle a `:command`. Returns true if the REPL should quit.
fn handle_command(cmd: &str, history: &[String]) -> bool {
    let mut parts = cmd.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match name {
        ":quit" | ":q" | ":exit" => return true,
        ":help" | ":h" => {
            println!("REPL commands:");
            println!("  :help            show this help");
            println!("  :history         list input history");
            println!("  :hl <code>       print <code> with syntax highlighting");
            println!("  :disasm <expr>   show bytecode for <expr>");
            println!("  :quit            exit (or Ctrl-D)");
            println!();
            println!("Anything else is evaluated; a trailing expression prints its value.");
        }
        ":history" => {
            for (i, h) in history.iter().enumerate() {
                println!("{:>3}  {}", i + 1, h.trim_end().replace('\n', "\n     "));
            }
        }
        ":hl" => println!("{}", highlight(rest)),
        ":disasm" => disasm_input(rest),
        other => eprintln!("unknown command '{other}' (try :help)"),
    }
    false
}

fn disasm_input(src: &str) {
    let wrapped = if src.trim_end().ends_with(';') { src.to_string() } else { format!("{src};") };
    let (program, errs) = crate::check_source(&wrapped);
    if !errs.is_empty() {
        for d in &errs {
            eprintln!("{}", d.render(&wrapped, None));
        }
        return;
    }
    match crate::compiler::compile(&program) {
        Ok(proto) => print!("{}", crate::disassembler::disassemble(&proto)),
        Err(errs) => {
            for d in &errs {
                eprintln!("{}", d.render(&wrapped, None));
            }
        }
    }
}

// ---- history persistence ---------------------------------------------------

fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".lumen_history"))
}

fn load_history() -> Vec<String> {
    history_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().map(|l| l.replace("\\n", "\n")).collect())
        .unwrap_or_default()
}

fn save_history(history: &[String]) {
    if let Some(path) = history_path() {
        // Store one entry per line, encoding embedded newlines.
        let data: String = history
            .iter()
            .rev()
            .take(1000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|h| h.trim_end().replace('\n', "\\n"))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(path, data);
    }
}
