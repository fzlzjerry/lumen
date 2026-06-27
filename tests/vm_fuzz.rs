//! VM-level fuzzing. The front-end fuzzer (`tests/fuzz.rs`) throws random text at
//! the lexer/parser; this one generates *valid* programs (scope-tracked, so they
//! pass the resolver) and runs them through the whole interpreter — under an
//! execution budget (to bound loops/recursion) and GC stress — asserting the VM
//! never panics. A program may finish, throw a Lumen error, or hit the budget;
//! only a Rust panic is a bug, caught with `catch_unwind` and reported with the
//! offending source.

use lumen::vm::Vm;
use std::panic;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

/// Generates random but statically-valid Lumen programs.
struct Gen {
    rng: Rng,
    out: String,
    vars: Vec<String>,
    funcs: Vec<(String, usize)>, // (name, arity)
    counter: usize,
}

const BINOPS: &[&str] = &[
    "+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!=", "&&", "||",
];

impl Gen {
    fn new(seed: u64) -> Self {
        Gen {
            rng: Rng::new(seed),
            out: String::new(),
            vars: Vec::new(),
            funcs: Vec::new(),
            counter: 0,
        }
    }

    fn fresh(&mut self) -> String {
        self.counter += 1;
        format!("v{}", self.counter)
    }

    fn program(&mut self) -> String {
        let n = 3 + self.rng.below(20);
        for _ in 0..n {
            self.stmt(0);
        }
        std::mem::take(&mut self.out)
    }

    fn pad(&mut self, indent: usize) {
        for _ in 0..indent {
            self.out.push_str("    ");
        }
    }

    fn stmt(&mut self, indent: usize) {
        // Cap nesting to keep programs small and fast.
        let kinds = if indent > 2 { 4 } else { 8 };
        match self.rng.below(kinds) {
            0 | 1 => {
                // let v = expr;
                let name = self.fresh();
                self.pad(indent);
                let e = self.expr(0);
                self.out.push_str(&format!("let {name} = {e};\n"));
                self.vars.push(name);
            }
            2 => {
                // assignment to an existing var
                if let Some(v) = self.pick_var() {
                    self.pad(indent);
                    let e = self.expr(0);
                    self.out.push_str(&format!("{v} = {e};\n"));
                } else {
                    self.stmt(indent);
                }
            }
            3 => {
                self.pad(indent);
                let e = self.expr(0);
                self.out.push_str(&format!("println({e});\n"));
            }
            4 => {
                self.pad(indent);
                let cond = self.expr(0);
                self.out.push_str(&format!("if {cond} {{\n"));
                let scope = self.vars.len();
                let body = 1 + self.rng.below(3);
                for _ in 0..body {
                    self.stmt(indent + 1);
                }
                self.vars.truncate(scope); // block-scoped vars leave scope
                self.pad(indent);
                self.out.push_str("}\n");
            }
            5 => {
                // bounded for loop
                let i = self.fresh();
                self.pad(indent);
                let lim = self.rng.below(6);
                let scope = self.vars.len();
                self.out
                    .push_str(&format!("for let {i} = 0; {i} < {lim}; {i} = {i} + 1 {{\n"));
                self.vars.push(i.clone());
                let body = 1 + self.rng.below(3);
                for _ in 0..body {
                    self.stmt(indent + 1);
                }
                self.pad(indent);
                self.out.push_str("}\n");
                self.vars.truncate(scope); // pop the loop var and body vars
            }
            6 => {
                // function definition
                let name = self.fresh();
                let arity = self.rng.below(3);
                let params: Vec<String> = (0..arity).map(|k| format!("p{k}")).collect();
                self.pad(indent);
                self.out
                    .push_str(&format!("fn {name}({}) {{\n", params.join(", ")));
                let saved = self.vars.clone();
                self.vars.extend(params.clone());
                let body = 1 + self.rng.below(2);
                for _ in 0..body {
                    self.stmt(indent + 1);
                }
                self.pad(indent + 1);
                let r = self.expr(0);
                self.out.push_str(&format!("return {r};\n"));
                self.pad(indent);
                self.out.push_str("}\n");
                self.vars = saved;
                self.funcs.push((name, arity));
            }
            _ => {
                // call a defined function as a statement
                if let Some((name, arity)) = self.pick_func() {
                    self.pad(indent);
                    let args: Vec<String> = (0..arity).map(|_| self.expr(1)).collect();
                    self.out
                        .push_str(&format!("{name}({});\n", args.join(", ")));
                } else {
                    self.stmt(indent);
                }
            }
        }
    }

    fn pick_var(&mut self) -> Option<String> {
        if self.vars.is_empty() {
            None
        } else {
            let i = self.rng.below(self.vars.len());
            Some(self.vars[i].clone())
        }
    }

    fn pick_func(&mut self) -> Option<(String, usize)> {
        if self.funcs.is_empty() {
            None
        } else {
            let i = self.rng.below(self.funcs.len());
            Some(self.funcs[i].clone())
        }
    }

    fn expr(&mut self, depth: usize) -> String {
        if depth >= 3 {
            return self.atom();
        }
        match self.rng.below(10) {
            0..=2 => self.atom(),
            3 | 4 => {
                let a = self.expr(depth + 1);
                let b = self.expr(depth + 1);
                let op = BINOPS[self.rng.below(BINOPS.len())];
                format!("({a} {op} {b})")
            }
            5 => {
                let a = self.expr(depth + 1);
                let op = if self.rng.chance(2) { "-" } else { "!" };
                format!("({op}{a})")
            }
            6 => {
                let n = self.rng.below(4);
                let items: Vec<String> = (0..n).map(|_| self.expr(depth + 1)).collect();
                format!("[{}]", items.join(", "))
            }
            7 => {
                let k = self.fresh();
                let v = self.expr(depth + 1);
                format!("{{{k}: {v}}}")
            }
            8 => {
                // index / builtin call
                let e = self.expr(depth + 1);
                match self.rng.below(4) {
                    0 => format!("len({e})"),
                    1 => format!("str({e})"),
                    2 => format!("type({e})"),
                    _ => {
                        let i = self.expr(depth + 1);
                        format!("({e})[{i}]")
                    }
                }
            }
            _ => {
                if let Some((name, arity)) = self.pick_func() {
                    let args: Vec<String> = (0..arity).map(|_| self.expr(depth + 1)).collect();
                    format!("{name}({})", args.join(", "))
                } else {
                    self.atom()
                }
            }
        }
    }

    fn atom(&mut self) -> String {
        match self.rng.below(8) {
            0 => self.rng.below(100).to_string(),
            1 => format!("{}.5", self.rng.below(10)),
            2 => "true".to_string(),
            3 => "nil".to_string(),
            4 => "\"s\"".to_string(),
            _ => self
                .pick_var()
                .unwrap_or_else(|| self.rng.below(10).to_string()),
        }
    }
}

/// Compile `src` and run it under a step limit, returning the VM result.
fn run_limited(src: &str, limit: u64) -> Result<(), String> {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.set_step_limit(limit);
    vm.interpret(proto)
}

#[test]
fn step_limit_halts_infinite_loop() {
    // Without a budget this never returns; with one it must throw, not hang.
    let r = run_limited("while (true) { let x = 1; }", 50_000);
    assert!(r.is_err(), "infinite loop should hit the budget");
}

#[test]
fn step_limit_halts_infinite_recursion() {
    let r = run_limited("fn f() { return f(); } f();", 50_000);
    assert!(r.is_err(), "infinite recursion should hit the budget");
}

#[test]
fn step_limit_allows_bounded_programs() {
    // A finite loop well under the budget completes normally.
    let r = run_limited(
        "let s = 0; for let i = 0; i < 1000; i = i + 1 { s = s + i; }",
        1_000_000,
    );
    assert!(r.is_ok(), "bounded program should finish: {r:?}");
}

#[test]
fn vm_never_panics_on_valid_programs() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {})); // quiet during fuzzing
    let mut reached = 0;
    let mut seed_rng = Rng::new(0xF0_0D_CA_FE);

    for _ in 0..2500 {
        let seed = seed_rng.next();
        let src = Gen::new(seed).program();

        // Only programs that pass the front end exercise the VM.
        let (program, errs) = lumen::check_source(&src);
        if !errs.is_empty() {
            continue;
        }
        let proto = match lumen::compiler::compile(&program) {
            Ok(p) => p,
            Err(_) => continue,
        };
        reached += 1;

        let src_for_panic = src.clone();
        let result = panic::catch_unwind(move || {
            let mut vm = Vm::with_output(Box::new(std::io::sink()));
            lumen::stdlib::install(&mut vm);
            vm.set_step_limit(200_000);
            // Every other program also runs under full GC stress.
            if seed & 1 == 0 {
                vm.set_stress_gc(true);
            }
            let _ = vm.interpret(proto); // Ok, Lumen error, or budget — all fine
        });
        if result.is_err() {
            panic::set_hook(prev);
            panic!("VM panicked on a valid program:\n{src_for_panic}");
        }
    }
    panic::set_hook(prev);
    assert!(
        reached > 500,
        "fuzzer reached the VM only {reached} times; generator too invalid"
    );
}
