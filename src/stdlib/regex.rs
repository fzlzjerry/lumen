//! The `regex` module: a small, dependency-free regular-expression engine.
//!
//! Patterns are parsed to an AST, compiled to a flat instruction program, and
//! run by a recursive **backtracking** matcher (Cox's "VM approach"). Supported
//! syntax: literals, `.`, character classes `[...]`/`[^...]` with ranges and
//! `\d \w \s \D \W \S`, anchors `^ $`, groups `(...)` (capturing), alternation
//! `|`, and quantifiers `* + ?` and `{n}` `{n,}` `{n,m}` (greedy, or lazy with a
//! trailing `?`). All positions are character (not byte) indices, matching the
//! core language.
//!
//! Anchors match the start/end of the *whole* string only — there is no
//! multiline mode, and `$` does **not** match before a trailing newline (like
//! Go's `regexp`, unlike Python/PCRE default mode). `\d`/`\w` are ASCII. A step
//! budget and a recursion-depth limit bound pathological backtracking; both raise
//! a catchable `ValueError` rather than hanging or crashing. Because matcher
//! recursion scales with match length, a *single* match spanning more than a few
//! thousand characters also throws (matching many short spans in a long string —
//! the common case — is unaffected).

use super::{err, string_of, Vm};
use crate::object::Arity::{self, Exact};
use crate::object::LumMap;
use crate::value::{error_kind, Value};
use std::cell::Cell;

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "test", Exact(2), test),
        f(vm, "find", Exact(2), find),
        f(vm, "find_all", Exact(2), find_all),
        f(vm, "captures", Exact(2), captures),
        f(vm, "replace", Exact(3), replace),
        f(vm, "split", Exact(2), split),
    ];
    vm.make_module("regex", exports)
}

// ---- AST ------------------------------------------------------------------

#[derive(Clone)]
enum Node {
    Char(char),
    Any,
    Class(Class),
    Start,
    End,
    Group(Box<Node>, usize),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    /// `inner{lo,hi}` (hi = None is unbounded); `greedy` distinguishes `*` from `*?`.
    Repeat(Box<Node>, usize, Option<usize>, bool),
}

#[derive(Clone)]
struct Class {
    negated: bool,
    items: Vec<ClassItem>,
}

#[derive(Clone)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Pred(Pred),
}

#[derive(Clone, Copy)]
enum Pred {
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

impl Pred {
    fn matches(self, c: char) -> bool {
        let word = c.is_alphanumeric() || c == '_';
        match self {
            Pred::Digit => c.is_ascii_digit(),
            Pred::NotDigit => !c.is_ascii_digit(),
            Pred::Word => word,
            Pred::NotWord => !word,
            Pred::Space => c.is_whitespace(),
            Pred::NotSpace => !c.is_whitespace(),
        }
    }
}

impl Class {
    fn matches(&self, c: char) -> bool {
        let hit = self.items.iter().any(|it| match it {
            ClassItem::Char(x) => c == *x,
            ClassItem::Range(a, b) => *a <= c && c <= *b,
            ClassItem::Pred(p) => p.matches(c),
        });
        hit ^ self.negated
    }
}

// ---- parser ---------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    pos: usize,
    ngroups: usize,
    depth: usize, // group-nesting depth, to bound parser recursion
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn parse(&mut self) -> Result<Node, String> {
        let node = self.parse_alt()?;
        if self.pos != self.chars.len() {
            return Err(format!("unexpected '{}'", self.chars[self.pos]));
        }
        Ok(node)
    }

    fn parse_alt(&mut self) -> Result<Node, String> {
        let mut branches = vec![self.parse_concat()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 { branches.pop().unwrap() } else { Node::Alt(branches) })
    }

    fn parse_concat(&mut self) -> Result<Node, String> {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            nodes.push(self.parse_quant()?);
        }
        Ok(match nodes.len() {
            1 => nodes.pop().unwrap(),
            _ => Node::Concat(nodes),
        })
    }

    fn parse_quant(&mut self) -> Result<Node, String> {
        let atom = self.parse_atom()?;
        let (lo, hi) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => match self.try_braces()? {
                Some(r) => r,
                None => return Ok(atom), // a literal '{' — leave it for the next atom
            },
            _ => return Ok(atom),
        };
        let greedy = self.peek() != Some('?');
        if !greedy {
            self.pos += 1; // consume the lazy '?'
        }
        Ok(Node::Repeat(Box::new(atom), lo, hi, greedy))
    }

    /// Parse a `{n}` / `{n,}` / `{n,m}` quantifier, restoring the position and
    /// returning `None` if it is not a well-formed brace expression.
    fn try_braces(&mut self) -> Result<Option<(usize, Option<usize>)>, String> {
        let save = self.pos;
        self.pos += 1; // '{'
        let lo = match self.parse_int() {
            Some(n) => n,
            None => {
                self.pos = save;
                return Ok(None);
            }
        };
        let hi = if self.peek() == Some(',') {
            self.pos += 1;
            if self.peek() == Some('}') {
                None
            } else {
                match self.parse_int() {
                    Some(n) => Some(n),
                    None => {
                        self.pos = save;
                        return Ok(None);
                    }
                }
            }
        } else {
            Some(lo)
        };
        if self.peek() != Some('}') {
            self.pos = save;
            return Ok(None);
        }
        self.pos += 1; // '}'
        if let Some(h) = hi {
            if h < lo {
                return Err(format!("invalid repeat range {{{lo},{h}}}"));
            }
        }
        Ok(Some((lo, hi)))
    }

    fn parse_int(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        self.chars[start..self.pos].iter().collect::<String>().parse().ok()
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        match self.peek() {
            None => Err("unexpected end of pattern".into()),
            Some('(') => {
                self.pos += 1;
                self.ngroups += 1;
                let idx = self.ngroups;
                self.depth += 1;
                // Bounded low enough that the parser's own recursion to reach it
                // stays well within a small (e.g. 1 MB worker) thread stack.
                if self.depth > 128 {
                    return Err("pattern nested too deeply".into());
                }
                let inner = self.parse_alt()?;
                self.depth -= 1;
                if self.peek() != Some(')') {
                    return Err("unclosed group '('".into());
                }
                self.pos += 1;
                Ok(Node::Group(Box::new(inner), idx))
            }
            Some('[') => self.parse_class(),
            Some('.') => {
                self.pos += 1;
                Ok(Node::Any)
            }
            Some('^') => {
                self.pos += 1;
                Ok(Node::Start)
            }
            Some('$') => {
                self.pos += 1;
                Ok(Node::End)
            }
            Some('*') | Some('+') | Some('?') => Err("nothing to repeat".into()),
            Some(')') => Err("unmatched ')'".into()),
            Some('\\') => {
                self.pos += 1;
                let c = self.peek().ok_or("trailing backslash")?;
                self.pos += 1;
                Ok(match c {
                    'd' => pred_node(Pred::Digit),
                    'D' => pred_node(Pred::NotDigit),
                    'w' => pred_node(Pred::Word),
                    'W' => pred_node(Pred::NotWord),
                    's' => pred_node(Pred::Space),
                    'S' => pred_node(Pred::NotSpace),
                    'n' => Node::Char('\n'),
                    't' => Node::Char('\t'),
                    'r' => Node::Char('\r'),
                    other => Node::Char(other),
                })
            }
            Some(c) => {
                self.pos += 1;
                Ok(Node::Char(c))
            }
        }
    }

    fn parse_class(&mut self) -> Result<Node, String> {
        self.pos += 1; // '['
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => return Err("unterminated character class".into()),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => match self.read_class_atom()? {
                    ClassAtom::Pred(p) => items.push(ClassItem::Pred(p)),
                    ClassAtom::Lit(c) => {
                        // A range `c-d` (escaped or not on either side), but a `-`
                        // right before `]` is a literal hyphen.
                        if self.peek() == Some('-')
                            && self.peek2().is_some()
                            && self.peek2() != Some(']')
                        {
                            self.pos += 1; // '-'
                            match self.read_class_atom()? {
                                ClassAtom::Lit(end) => {
                                    if end < c {
                                        return Err(format!("invalid class range {c}-{end}"));
                                    }
                                    items.push(ClassItem::Range(c, end));
                                }
                                // `a-\d` is not a real range — keep the parts literal.
                                ClassAtom::Pred(p) => {
                                    items.push(ClassItem::Char(c));
                                    items.push(ClassItem::Char('-'));
                                    items.push(ClassItem::Pred(p));
                                }
                            }
                        } else {
                            items.push(ClassItem::Char(c));
                        }
                    }
                },
            }
        }
        if items.is_empty() {
            return Err("empty character class".into());
        }
        Ok(Node::Class(Class { negated, items }))
    }

    /// Read one element inside a `[...]`: a predefined class (`\d` etc.) or a
    /// single literal character (handling `\n \t \r` and `\<char>` escapes).
    fn read_class_atom(&mut self) -> Result<ClassAtom, String> {
        match self.peek() {
            None => Err("unterminated character class".into()),
            Some('\\') => {
                self.pos += 1;
                let c = self.peek().ok_or("trailing backslash in class")?;
                self.pos += 1;
                Ok(match c {
                    'd' => ClassAtom::Pred(Pred::Digit),
                    'D' => ClassAtom::Pred(Pred::NotDigit),
                    'w' => ClassAtom::Pred(Pred::Word),
                    'W' => ClassAtom::Pred(Pred::NotWord),
                    's' => ClassAtom::Pred(Pred::Space),
                    'S' => ClassAtom::Pred(Pred::NotSpace),
                    'n' => ClassAtom::Lit('\n'),
                    't' => ClassAtom::Lit('\t'),
                    'r' => ClassAtom::Lit('\r'),
                    other => ClassAtom::Lit(other),
                })
            }
            Some(c) => {
                self.pos += 1;
                Ok(ClassAtom::Lit(c))
            }
        }
    }
}

/// One element parsed from inside a `[...]` class.
enum ClassAtom {
    Lit(char),
    Pred(Pred),
}

fn pred_node(p: Pred) -> Node {
    Node::Class(Class { negated: false, items: vec![ClassItem::Pred(p)] })
}

// ---- compilation ----------------------------------------------------------

enum Inst {
    Char(char),
    Any,
    Class(Class),
    Start,
    End,
    Match,
    Jmp(usize),
    Split(usize, usize),
    Save(usize),
    /// Record the current position in a scratch slot (start of a `*`/`+` body).
    Mark(usize),
    /// Fail unless the position advanced since the matching `Mark` — stops a
    /// quantified, empty-matching body from looping forever (e.g. `(a*)*`).
    AssertProgress(usize),
}

/// Assembles instructions, tracking capture slots (`base`) and scratch `marks`.
struct Asm {
    prog: Vec<Inst>,
    base: usize,
    marks: usize,
}

impl Asm {
    fn emit(&mut self, node: &Node) {
        match node {
            Node::Char(c) => self.prog.push(Inst::Char(*c)),
            Node::Any => self.prog.push(Inst::Any),
            Node::Class(cl) => self.prog.push(Inst::Class(cl.clone())),
            Node::Start => self.prog.push(Inst::Start),
            Node::End => self.prog.push(Inst::End),
            Node::Group(inner, idx) => {
                self.prog.push(Inst::Save(2 * idx));
                self.emit(inner);
                self.prog.push(Inst::Save(2 * idx + 1));
            }
            Node::Concat(nodes) => {
                for n in nodes {
                    self.emit(n);
                }
            }
            Node::Alt(branches) => {
                let mut jmps = Vec::new();
                for (i, b) in branches.iter().enumerate() {
                    if i + 1 < branches.len() {
                        let split = self.prog.len();
                        self.prog.push(Inst::Split(0, 0));
                        let start = self.prog.len();
                        self.emit(b);
                        jmps.push(self.prog.len());
                        self.prog.push(Inst::Jmp(0));
                        let next = self.prog.len();
                        self.prog[split] = Inst::Split(start, next);
                    } else {
                        self.emit(b);
                    }
                }
                let end = self.prog.len();
                for j in jmps {
                    self.prog[j] = Inst::Jmp(end);
                }
            }
            Node::Repeat(inner, lo, hi, greedy) => {
                for _ in 0..*lo {
                    self.emit(inner);
                }
                match hi {
                    None => self.emit_star(inner, *greedy),
                    Some(h) => {
                        for _ in 0..(h - lo) {
                            self.emit_quest(inner, *greedy);
                        }
                    }
                }
            }
        }
    }

    fn emit_star(&mut self, inner: &Node, greedy: bool) {
        let mark = self.base + self.marks;
        self.marks += 1;
        let split = self.prog.len();
        self.prog.push(Inst::Split(0, 0));
        let body = self.prog.len();
        self.prog.push(Inst::Mark(mark));
        self.emit(inner);
        self.prog.push(Inst::AssertProgress(mark));
        self.prog.push(Inst::Jmp(split));
        let exit = self.prog.len();
        self.prog[split] =
            if greedy { Inst::Split(body, exit) } else { Inst::Split(exit, body) };
    }

    fn emit_quest(&mut self, inner: &Node, greedy: bool) {
        let split = self.prog.len();
        self.prog.push(Inst::Split(0, 0));
        let body = self.prog.len();
        self.emit(inner);
        let exit = self.prog.len();
        self.prog[split] =
            if greedy { Inst::Split(body, exit) } else { Inst::Split(exit, body) };
    }
}

struct Compiled {
    prog: Vec<Inst>,
    ngroups: usize,
    slots: usize, // capture slots 2*(ngroups+1) plus one scratch slot per `*`/`+`
}

fn compile_regex(pattern: &str) -> Result<Compiled, String> {
    let mut p = Parser { chars: pattern.chars().collect(), pos: 0, ngroups: 0, depth: 0 };
    let node = p.parse()?;
    let base = 2 * (p.ngroups + 1);
    let mut asm = Asm { prog: vec![Inst::Save(0)], base, marks: 0 };
    asm.emit(&node);
    asm.prog.push(Inst::Save(1));
    asm.prog.push(Inst::Match);
    Ok(Compiled { prog: asm.prog, ngroups: p.ngroups, slots: base + asm.marks })
}

// ---- matcher --------------------------------------------------------------

/// Maximum matcher recursion depth. Each greedy-quantifier iteration on a
/// successful path adds a frame, so this also bounds the length of a single
/// repeated match; exceeding it raises a catchable error rather than overflowing
/// the host stack. Kept well under the OS stack limit, with room for the VM.
const MAX_DEPTH: u32 = 10_000;

struct Matcher<'a> {
    chars: &'a [char],
    prog: &'a [Inst],
    budget: Cell<u64>,
    overflow: Cell<bool>,
}

impl<'a> Matcher<'a> {
    fn new(chars: &'a [char], prog: &'a [Inst]) -> Self {
        Matcher { chars, prog, budget: Cell::new(2_000_000), overflow: Cell::new(false) }
    }

    fn run(&self, mut pc: usize, mut sp: usize, saves: &mut [Option<usize>], depth: u32) -> bool {
        // A recursion-depth backstop: deep success paths (very long greedy
        // matches) and any residual pathological recursion become a catchable
        // "too complex" error rather than a host-process stack overflow.
        if depth >= MAX_DEPTH {
            self.overflow.set(true);
            return false;
        }
        loop {
            let b = self.budget.get();
            if b == 0 {
                self.overflow.set(true);
                return false;
            }
            self.budget.set(b - 1);
            match &self.prog[pc] {
                Inst::Char(c) => {
                    if sp < self.chars.len() && self.chars[sp] == *c {
                        pc += 1;
                        sp += 1;
                    } else {
                        return false;
                    }
                }
                Inst::Any => {
                    if sp < self.chars.len() && self.chars[sp] != '\n' {
                        pc += 1;
                        sp += 1;
                    } else {
                        return false;
                    }
                }
                Inst::Class(cl) => {
                    if sp < self.chars.len() && cl.matches(self.chars[sp]) {
                        pc += 1;
                        sp += 1;
                    } else {
                        return false;
                    }
                }
                Inst::Start => {
                    if sp == 0 {
                        pc += 1;
                    } else {
                        return false;
                    }
                }
                Inst::End => {
                    if sp == self.chars.len() {
                        pc += 1;
                    } else {
                        return false;
                    }
                }
                Inst::Match => return true,
                Inst::Jmp(x) => pc = *x,
                Inst::Split(x, y) => {
                    if self.run(*x, sp, saves, depth + 1) {
                        return true;
                    }
                    if self.overflow.get() {
                        return false;
                    }
                    pc = *y;
                }
                Inst::Save(n) => {
                    let old = saves[*n];
                    saves[*n] = Some(sp);
                    if self.run(pc + 1, sp, saves, depth + 1) {
                        return true;
                    }
                    saves[*n] = old;
                    return false;
                }
                Inst::Mark(n) => {
                    // Save/restore like `Save`: the slot is reused across nested
                    // iterations, so backtracking must see *this* iteration's mark
                    // when its `AssertProgress` runs (not a deeper one's).
                    let old = saves[*n];
                    saves[*n] = Some(sp);
                    if self.run(pc + 1, sp, saves, depth + 1) {
                        return true;
                    }
                    saves[*n] = old;
                    return false;
                }
                Inst::AssertProgress(n) => {
                    if saves[*n] == Some(sp) {
                        return false; // the body matched empty — stop the loop
                    }
                    pc += 1;
                }
            }
        }
    }

    /// The leftmost match at or after `from`, returning the capture slots. The
    /// step budget is refreshed per start position, so it bounds catastrophic
    /// *backtracking* (which blows up within one start) without limiting the
    /// linear scan over a long input.
    fn search(&self, from: usize, slots: usize) -> Option<Vec<Option<usize>>> {
        for start in from..=self.chars.len() {
            self.budget.set(2_000_000);
            let mut saves = vec![None; slots];
            if self.run(0, start, &mut saves, 0) {
                return Some(saves);
            }
            if self.overflow.get() {
                return None;
            }
        }
        None
    }
}

/// Run a search on a dedicated thread with a large stack, so the matcher's
/// recursion (bounded by `MAX_DEPTH`) can never overflow the *caller's* stack —
/// which may be small (e.g. a worker thread). Returns the captures and whether
/// the depth/step limit was hit. The `Matcher` is created inside the thread, so
/// its non-`Sync` `Cell`s never cross a thread boundary.
fn search_big_stack(
    chars: &[char],
    prog: &[Inst],
    from: usize,
    slots: usize,
) -> (Option<Vec<Option<usize>>>, bool) {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn_scoped(s, || {
                let m = Matcher::new(chars, prog);
                let res = m.search(from, slots);
                (res, m.overflow.get())
            })
            .expect("spawn regex matcher thread")
            .join()
            .expect("regex matcher thread panicked")
    })
}

// ---- native functions -----------------------------------------------------

/// Compile a pattern argument, mapping a parse error to a `ValueError`.
fn compiled(vm: &mut Vm, pattern: &str) -> Result<Compiled, Value> {
    compile_regex(pattern).map_err(|e| err(vm, error_kind::VALUE, format!("invalid regex: {e}")))
}

fn complexity_error(vm: &mut Vm) -> Value {
    err(vm, error_kind::VALUE, "regex too complex (backtracking limit exceeded)")
}

fn test(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let (saves, ovf) = search_big_stack(&text, &c.prog, 0, c.slots);
    if ovf {
        return Err(complexity_error(vm));
    }
    Ok(Value::Bool(saves.is_some()))
}

fn find(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let (saves, ovf) = search_big_stack(&text, &c.prog, 0, c.slots);
    if ovf {
        return Err(complexity_error(vm));
    }
    match saves {
        Some(s) => Ok(match_map(vm, &text, s[0].unwrap(), s[1].unwrap())),
        None => Ok(Value::Nil),
    }
}

fn find_all(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let mut maps = Vec::new();
    let mut pos = 0;
    while pos <= text.len() {
        let (res, ovf) = search_big_stack(&text, &c.prog, pos, c.slots);
        if ovf {
            return Err(complexity_error(vm));
        }
        match res {
            Some(s) => {
                let (start, end) = (s[0].unwrap(), s[1].unwrap());
                maps.push(match_map(vm, &text, start, end));
                pos = if end > start { end } else { start + 1 };
            }
            None => break,
        }
    }
    Ok(vm.new_array(maps))
}

fn captures(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let (saves, ovf) = search_big_stack(&text, &c.prog, 0, c.slots);
    if ovf {
        return Err(complexity_error(vm));
    }
    match saves {
        Some(s) => {
            let mut groups = Vec::with_capacity(c.ngroups + 1);
            for g in 0..=c.ngroups {
                let v = match (s[2 * g], s[2 * g + 1]) {
                    (Some(start), Some(end)) => {
                        vm.new_string(&text[start..end].iter().collect::<String>())
                    }
                    _ => Value::Nil,
                };
                groups.push(v);
            }
            Ok(vm.new_array(groups))
        }
        None => Ok(Value::Nil),
    }
}

fn replace(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let repl: Vec<char> = string_of(vm, a[2])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let mut out = String::new();
    let mut pos = 0;
    while pos <= text.len() {
        let (res, ovf) = search_big_stack(&text, &c.prog, pos, c.slots);
        if ovf {
            return Err(complexity_error(vm));
        }
        match res {
            Some(s) => {
                let (start, end) = (s[0].unwrap(), s[1].unwrap());
                out.extend(&text[pos..start]);
                expand(&repl, &s, &text, &mut out);
                if end > start {
                    pos = end;
                } else {
                    if start < text.len() {
                        out.push(text[start]);
                    }
                    pos = start + 1;
                }
            }
            None => {
                out.extend(&text[pos..]);
                break;
            }
        }
    }
    Ok(vm.new_string(&out))
}

fn split(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pat = string_of(vm, a[0])?;
    let text: Vec<char> = string_of(vm, a[1])?.chars().collect();
    let c = compiled(vm, &pat)?;
    let mut parts: Vec<Value> = Vec::new();
    let mut pos = 0;
    let mut last = 0;
    while pos <= text.len() {
        let (res, ovf) = search_big_stack(&text, &c.prog, pos, c.slots);
        if ovf {
            return Err(complexity_error(vm));
        }
        match res {
            Some(s) => {
                let (start, end) = (s[0].unwrap(), s[1].unwrap());
                if end > start {
                    parts.push(vm.new_string(&text[last..start].iter().collect::<String>()));
                    last = end;
                    pos = end;
                } else {
                    pos = start + 1; // skip empty matches
                }
            }
            None => break,
        }
    }
    parts.push(vm.new_string(&text[last..].iter().collect::<String>()));
    Ok(vm.new_array(parts))
}

/// Expand a replacement template's `$0`..`$9` group references and `$$`.
fn expand(repl: &[char], saves: &[Option<usize>], text: &[char], out: &mut String) {
    let mut i = 0;
    while i < repl.len() {
        if repl[i] == '$' && i + 1 < repl.len() {
            let next = repl[i + 1];
            if next == '$' {
                out.push('$');
                i += 2;
                continue;
            }
            if let Some(d) = next.to_digit(10) {
                let g = d as usize;
                if 2 * g + 1 < saves.len() {
                    if let (Some(s), Some(e)) = (saves[2 * g], saves[2 * g + 1]) {
                        out.extend(&text[s..e]);
                    }
                }
                i += 2;
                continue;
            }
        }
        out.push(repl[i]);
        i += 1;
    }
}

/// A `{ start, end, text }` match map (built directly — this native never calls
/// back into Lumen, so no GC can run mid-construction).
fn match_map(vm: &mut Vm, text: &[char], start: usize, end: usize) -> Value {
    let matched = vm.new_string(&text[start..end].iter().collect::<String>());
    let mut map = LumMap::new();
    for (k, v) in [
        ("start", Value::Int(start as i64)),
        ("end", Value::Int(end as i64)),
        ("text", matched),
    ] {
        let key = vm.new_string(k);
        if let Ok(norm) = vm.map_key(key) {
            map.insert(norm, key, v);
        }
    }
    Value::Obj(vm.heap.alloc_map(map))
}
