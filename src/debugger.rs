//! An interactive source-level debugger (`lumen debug <file>`).
//!
//! Drives the VM and pauses at breakpoints, watchpoints, or while stepping. At a
//! pause it offers: `continue`, `step` (into calls), `next` (over calls),
//! `break <line> [if <cond>]` / `delete` (line breakpoints, optionally
//! conditional), `watch <name>` / `unwatch` (pause when a variable changes),
//! `backtrace`, `locals`, `print <name>`, and `disasm`.
//!
//! Stepping is source-line granular: `step` runs to the next line (descending
//! into callees), `next` runs to the next line in the current frame or shallower
//! (treating a call as one step).

use crate::disassembler::disassemble_instruction;
use crate::value::Value;
use crate::vm::{DebugStatus, Vm};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

/// Run the debugger on `src` (named `file`). Returns the process exit code.
pub fn run(file: &str, src: &str) -> i32 {
    let (program, errs) = crate::check_source(src);
    if !errs.is_empty() {
        for d in &errs {
            eprintln!("{}", d.render(src, Some(file)));
        }
        return 1;
    }
    let proto = match crate::compiler::compile(&program) {
        Ok(p) => p,
        Err(errs) => {
            for d in &errs {
                eprintln!("{}", d.render(src, Some(file)));
            }
            return 1;
        }
    };

    let mut vm = Vm::new();
    crate::stdlib::install(&mut vm);
    if let Some(dir) = std::path::Path::new(file).parent() {
        if !dir.as_os_str().is_empty() {
            vm.set_base_dir(dir.to_path_buf());
        }
    }
    if let Err(msg) = vm.debug_start(proto) {
        eprint!("{msg}");
        return 70;
    }

    let source_lines: Vec<&str> = src.lines().collect();
    let mut breakpoints: HashMap<u32, Option<Cond>> = HashMap::new();
    let mut watches: Vec<(String, Option<Value>)> = Vec::new();
    let mut mode = Mode::Pause; // start paused at the first instruction
    let mut prev_line = 0u32;

    println!("Lumen debugger — {file}");
    println!("Type 'help' for commands. Execution is paused at the start.\n");

    let stdin = io::stdin();
    let mut handle = stdin.lock();

    loop {
        let (fname, line, _ip) = match vm.debug_location() {
            Some(loc) => loc,
            None => {
                println!("Program finished.");
                break;
            }
        };
        let depth = vm.debug_depth();
        let arrived = line != prev_line;
        let hit_bp = arrived && breakpoint_hits(&breakpoints, &vm, line);
        if mode.should_pause(line, depth) || hit_bp {
            if hit_bp {
                println!("\x1b[1;35mbreakpoint\x1b[0m hit at line {line}");
            }
            print_position(&fname, line, &source_lines);
            print_watches(&vm, &watches);
            match prompt(&mut handle, &mut vm, &mut breakpoints, &mut watches) {
                Cmd::Continue => mode = Mode::Continue,
                Cmd::Step => mode = Mode::StepInto { line, depth },
                Cmd::Next => mode = Mode::StepOver { line, depth },
                Cmd::Quit => {
                    println!("Debugger exited.");
                    return 0;
                }
            }
        }
        prev_line = line;
        match vm.debug_step() {
            DebugStatus::Running => {
                // Watchpoints: pause if any watched variable changed value.
                if check_watches(&vm, &mut watches) {
                    mode = Mode::Pause;
                }
            }
            DebugStatus::Finished => {
                println!("Program finished.");
                break;
            }
            DebugStatus::Error(msg) => {
                eprint!("{msg}");
                return 70;
            }
        }
    }
    0
}

/// What execution should do after a pause.
enum Cmd {
    Continue,
    Step,
    Next,
    Quit,
}

/// The active stepping strategy between pauses.
enum Mode {
    /// Pause at the very next opportunity.
    Pause,
    /// Run freely until a breakpoint/watchpoint.
    Continue,
    /// Step into: pause as soon as the line or depth changes.
    StepInto { line: u32, depth: usize },
    /// Step over: pause at the next line in this frame or a shallower one.
    StepOver { line: u32, depth: usize },
}

impl Mode {
    fn should_pause(&self, line: u32, depth: usize) -> bool {
        match self {
            Mode::Pause => true,
            Mode::Continue => false,
            Mode::StepInto { line: l, depth: d } => line != *l || depth != *d,
            Mode::StepOver { line: l, depth: d } => depth < *d || (depth == *d && line != *l),
        }
    }
}

// ---- conditional breakpoints ----------------------------------------------

#[derive(Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// The right-hand side of a comparison: a literal value, a string literal, or
/// another variable resolved at check time.
#[derive(Clone)]
enum Operand {
    Val(Value),
    Str(String),
    Var(String),
}

/// A breakpoint condition: a bare variable (truthy) or a comparison.
#[derive(Clone)]
struct Cond {
    lhs: String,
    cmp: Option<(CmpOp, Operand)>,
}

/// Does any breakpoint registered on `line` fire (condition absent or true)?
fn breakpoint_hits(bps: &HashMap<u32, Option<Cond>>, vm: &Vm, line: u32) -> bool {
    match bps.get(&line) {
        None => false,
        Some(None) => true,
        Some(Some(cond)) => eval_cond(vm, cond),
    }
}

fn apply_ord(op: CmpOp, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        CmpOp::Lt => o == Less,
        CmpOp::Le => o != Greater,
        CmpOp::Gt => o == Greater,
        CmpOp::Ge => o != Less,
        CmpOp::Eq => o == Equal,
        CmpOp::Ne => o != Equal,
    }
}

fn eval_cond(vm: &Vm, cond: &Cond) -> bool {
    let lv = match vm.debug_lookup(&cond.lhs) {
        Some(v) => v,
        None => return false,
    };
    let (op, operand) = match &cond.cmp {
        None => return lv.is_truthy(),
        Some(pair) => pair,
    };
    match operand {
        Operand::Str(s) => {
            let ls = vm.debug_as_str(lv);
            match op {
                CmpOp::Eq => ls.as_deref() == Some(s.as_str()),
                CmpOp::Ne => ls.as_deref() != Some(s.as_str()),
                _ => match ls {
                    Some(ls) => apply_ord(*op, ls.as_str().cmp(s.as_str())),
                    None => false,
                },
            }
        }
        Operand::Val(rv) => compare_values(vm, lv, *rv, *op),
        Operand::Var(name) => match vm.debug_lookup(name) {
            Some(rv) => compare_values(vm, lv, rv, *op),
            None => false,
        },
    }
}

fn compare_values(vm: &Vm, lv: Value, rv: Value, op: CmpOp) -> bool {
    match op {
        CmpOp::Eq => vm.values_equal(lv, rv),
        CmpOp::Ne => !vm.values_equal(lv, rv),
        _ => match vm.debug_compare(lv, rv) {
            Some(o) => apply_ord(op, o),
            None => false,
        },
    }
}

/// Parse `<line> [if <cond>]` for the `break` command.
fn parse_breakpoint(arg: &str) -> Option<(u32, Option<Cond>)> {
    let arg = arg.trim();
    let (line_str, rest) = match arg.split_once(char::is_whitespace) {
        Some((a, b)) => (a, b.trim()),
        None => (arg, ""),
    };
    let line: u32 = line_str.parse().ok()?;
    if rest.is_empty() {
        return Some((line, None));
    }
    let cond_str = rest.strip_prefix("if").map(str::trim).unwrap_or(rest);
    Some((line, Some(parse_cond(cond_str)?)))
}

/// Parse `lhs`, `lhs op rhs` — a bare variable or a single comparison.
fn parse_cond(s: &str) -> Option<Cond> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Two-character operators must be checked before single-character ones.
    for (sym, op) in [("==", CmpOp::Eq), ("!=", CmpOp::Ne), ("<=", CmpOp::Le), (">=", CmpOp::Ge)] {
        if let Some(idx) = s.find(sym) {
            let lhs = s[..idx].trim().to_string();
            let rhs = s[idx + sym.len()..].trim();
            return Some(Cond { lhs, cmp: Some((op, parse_operand(rhs))) });
        }
    }
    for (sym, op) in [("<", CmpOp::Lt), (">", CmpOp::Gt)] {
        if let Some(idx) = s.find(sym) {
            let lhs = s[..idx].trim().to_string();
            let rhs = s[idx + sym.len()..].trim();
            return Some(Cond { lhs, cmp: Some((op, parse_operand(rhs))) });
        }
    }
    Some(Cond { lhs: s.to_string(), cmp: None })
}

fn parse_operand(s: &str) -> Operand {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Operand::Str(inner.to_string());
    }
    match s {
        "true" => return Operand::Val(Value::Bool(true)),
        "false" => return Operand::Val(Value::Bool(false)),
        "nil" => return Operand::Val(Value::Nil),
        _ => {}
    }
    if let Ok(n) = s.parse::<i64>() {
        return Operand::Val(Value::Int(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Operand::Val(Value::Float(f));
    }
    Operand::Var(s.to_string())
}

// ---- watchpoints -----------------------------------------------------------

/// Re-read each watched variable; if a `Some -> Some` value changed, report it
/// and signal a pause. Returns true if any watch fired.
fn check_watches(vm: &Vm, watches: &mut [(String, Option<Value>)]) -> bool {
    let mut fired = false;
    for (name, last) in watches.iter_mut() {
        let cur = vm.debug_lookup(name);
        if let (Some(prev), Some(now)) = (*last, cur) {
            if !vm.values_equal(prev, now) {
                println!(
                    "\x1b[1;34mwatch\x1b[0m {name}: {} => {}",
                    vm.debug_display(prev),
                    vm.debug_display(now)
                );
                fired = true;
            }
        }
        if cur.is_some() {
            *last = cur;
        }
    }
    fired
}

fn print_watches(vm: &Vm, watches: &[(String, Option<Value>)]) {
    for (name, _) in watches {
        match vm.debug_lookup(name) {
            Some(v) => println!("  \x1b[34mwatch\x1b[0m {name} = {}", vm.debug_display(v)),
            None => println!("  \x1b[34mwatch\x1b[0m {name} = (not in scope)"),
        }
    }
}

fn print_position(fname: &str, line: u32, source_lines: &[&str]) {
    let text = source_lines.get(line.saturating_sub(1) as usize).copied().unwrap_or("");
    println!("\x1b[33m{fname}\x1b[0m at line {line}:");
    println!("  {line:>4} | {text}");
}

/// Read and run debug commands until one resumes execution.
fn prompt(
    handle: &mut impl BufRead,
    vm: &mut Vm,
    breakpoints: &mut HashMap<u32, Option<Cond>>,
    watches: &mut Vec<(String, Option<Value>)>,
) -> Cmd {
    loop {
        print!("\x1b[1;36m(dbg)\x1b[0m ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if handle.read_line(&mut line).unwrap_or(0) == 0 {
            return Cmd::Quit; // EOF
        }
        let line = line.trim();
        if line.is_empty() {
            return Cmd::Step; // bare Enter == step
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match cmd {
            "c" | "continue" => return Cmd::Continue,
            "s" | "step" => return Cmd::Step,
            "n" | "next" => return Cmd::Next,
            "q" | "quit" => return Cmd::Quit,
            "bt" | "backtrace" | "where" => {
                for (i, (name, line)) in vm.debug_backtrace().into_iter().enumerate() {
                    println!("  #{i}  {name} (line {line})");
                }
            }
            "locals" | "l" => {
                let locals = vm.debug_locals();
                if locals.is_empty() {
                    println!("  (no named locals in scope)");
                } else {
                    for (name, value) in locals {
                        println!("  {name} = {value}");
                    }
                }
            }
            "p" | "print" => match vm.debug_lookup(arg) {
                Some(v) => println!("  {arg} = {}", vm.debug_display(v)),
                None => println!("  no variable '{arg}' in scope"),
            },
            "b" | "break" => match parse_breakpoint(arg) {
                Some((n, cond)) => {
                    let conditional = cond.is_some();
                    breakpoints.insert(n, cond);
                    if conditional {
                        println!("  conditional breakpoint set at line {n}");
                    } else {
                        println!("  breakpoint set at line {n}");
                    }
                }
                None => println!("  usage: break <line> [if <cond>]"),
            },
            "d" | "delete" => match arg.parse::<u32>() {
                Ok(n) => {
                    breakpoints.remove(&n);
                    println!("  breakpoint at line {n} removed");
                }
                Err(_) => println!("  usage: delete <line>"),
            },
            "breaks" | "breakpoints" => {
                let mut bps: Vec<&u32> = breakpoints.keys().collect();
                bps.sort();
                println!("  breakpoints: {bps:?}");
            }
            "w" | "watch" => {
                if arg.is_empty() {
                    println!("  usage: watch <name>");
                } else if watches.iter().any(|(n, _)| n == arg) {
                    println!("  already watching '{arg}'");
                } else {
                    let initial = vm.debug_lookup(arg);
                    watches.push((arg.to_string(), initial));
                    println!("  watching '{arg}'");
                }
            }
            "unwatch" => {
                let before = watches.len();
                watches.retain(|(n, _)| n != arg);
                if watches.len() < before {
                    println!("  no longer watching '{arg}'");
                } else {
                    println!("  not watching '{arg}'");
                }
            }
            "watches" => {
                if watches.is_empty() {
                    println!("  (no watches)");
                } else {
                    print_watches(vm, watches);
                }
            }
            "disasm" | "dis" => disasm_current(vm),
            "help" | "h" | "?" => print_help(),
            other => println!("  unknown command '{other}' (try 'help')"),
        }
    }
}

fn disasm_current(vm: &Vm) {
    if let Some((proto, ip)) = vm.debug_current_frame() {
        println!("  == {} ==", proto.display_name());
        let mut offset = 0;
        while offset < proto.chunk.code.len() {
            let mut line = String::new();
            let next = disassemble_instruction(&proto.chunk, offset, &mut line);
            let marker = if offset == ip { "\x1b[1;31m>\x1b[0m " } else { "  " };
            print!("  {marker}{line}");
            offset = next;
        }
    } else {
        println!("  (no active frame)");
    }
}

fn print_help() {
    println!("Debugger commands:");
    println!("  c, continue          run until the next breakpoint/watchpoint");
    println!("  s, step  (Enter)     run to the next line, descending into calls");
    println!("  n, next              run to the next line, stepping over calls");
    println!("  b, break <line>      set a breakpoint at a source line");
    println!("  b, break <line> if <cond>  break only when the condition holds");
    println!("                       (e.g. 'break 12 if i == 50', 'break 8 if done')");
    println!("  d, delete <line>     remove a breakpoint");
    println!("  breakpoints          list breakpoints");
    println!("  w, watch <name>      pause when a variable's value changes");
    println!("  unwatch <name>       stop watching a variable");
    println!("  watches              show watched variables");
    println!("  bt, backtrace        show the call stack");
    println!("  locals               show the current frame's named locals");
    println!("  p, print <name>      show a variable's value");
    println!("  disasm               disassemble the current function (ip marked)");
    println!("  q, quit              exit the debugger");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unconditional_breakpoint() {
        let (line, cond) = parse_breakpoint("12").unwrap();
        assert_eq!(line, 12);
        assert!(cond.is_none());
    }

    #[test]
    fn parses_comparison_breakpoint() {
        let (line, cond) = parse_breakpoint("12 if i == 50").unwrap();
        assert_eq!(line, 12);
        let cond = cond.unwrap();
        assert_eq!(cond.lhs, "i");
        match cond.cmp {
            Some((CmpOp::Eq, Operand::Val(Value::Int(50)))) => {}
            _ => panic!("expected i == 50"),
        }
    }

    #[test]
    fn parses_two_char_operator_before_single() {
        // ">=" must not be split as ">".
        let (_l, cond) = parse_breakpoint("3 if total >= 100").unwrap();
        let cond = cond.unwrap();
        assert_eq!(cond.lhs, "total");
        assert!(matches!(cond.cmp, Some((CmpOp::Ge, Operand::Val(Value::Int(100))))));
    }

    #[test]
    fn parses_bare_truthy_condition() {
        let (_l, cond) = parse_breakpoint("5 if done").unwrap();
        let cond = cond.unwrap();
        assert_eq!(cond.lhs, "done");
        assert!(cond.cmp.is_none());
    }

    #[test]
    fn parses_string_and_var_operands() {
        let (_l, cond) = parse_breakpoint(r#"7 if name != "quit""#).unwrap();
        match cond.unwrap().cmp {
            Some((CmpOp::Ne, Operand::Str(s))) => assert_eq!(s, "quit"),
            _ => panic!("expected string operand"),
        }
        let (_l, cond) = parse_breakpoint("7 if a < b").unwrap();
        match cond.unwrap().cmp {
            Some((CmpOp::Lt, Operand::Var(v))) => assert_eq!(v, "b"),
            _ => panic!("expected variable operand"),
        }
    }

    #[test]
    fn step_over_stays_in_frame() {
        // From line 5 at depth 2, a deeper frame (depth 3) must not pause...
        let over = Mode::StepOver { line: 5, depth: 2 };
        assert!(!over.should_pause(9, 3), "inside a callee: keep running");
        // ...but the next line at the same depth does...
        assert!(over.should_pause(6, 2));
        // ...and so does returning to a shallower frame.
        assert!(over.should_pause(20, 1));
    }

    #[test]
    fn step_into_pauses_on_depth_change() {
        let into = Mode::StepInto { line: 5, depth: 2 };
        assert!(into.should_pause(9, 3), "entering a callee pauses");
        assert!(!into.should_pause(5, 2), "same line, same depth: keep running");
    }
}
