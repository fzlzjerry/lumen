//! Fuzz tests: throw piles of random and mutated input at the front end and
//! assert it never panics (it must always degrade to a diagnostic, never crash).
//!
//! Deterministic: a fixed-seed xorshift PRNG drives generation so a failure is
//! reproducible. Each input is run inside `catch_unwind` so a panic is reported
//! *with the offending input* rather than as an opaque abort. Three strategies:
//! raw random text, random token soup, and byte-mutated real programs.

use std::panic;

/// Deterministic xorshift64* PRNG.
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
}

/// Run the whole front end; must never panic, regardless of input.
fn front_end(src: &str) {
    let (program, errs) = lumen::parse_source(src);
    if errs.is_empty() {
        let _ = lumen::resolver::resolve(&program);
        // A clean parse must also compile or report errors — never panic.
        let _ = lumen::compiler::compile(&program);
    }
    // Always exercise the lexer on its own too.
    let _ = lumen::lexer::lex(src);
}

/// Assert `front_end(src)` does not panic; on panic, surface the input.
fn check_no_panic(src: &str) {
    let result = panic::catch_unwind(|| front_end(src));
    if result.is_err() {
        panic!("front end panicked on input:\n{src:?}");
    }
}

const ALPHABET: &[char] = &[
    'a', 'b', 'z', '_', '0', '1', '9', ' ', '\n', '\t', '"', '\\', '$', '{', '}', '(', ')', '[',
    ']', ';', ':', ',', '.', '+', '-', '*', '/', '%', '=', '<', '>', '!', '&', '|', '#', '@', '?',
    '\'', 'é', '🚀',
];

#[test]
fn random_text_never_panics() {
    // Quiet the default panic printer during fuzzing; catch_unwind still works.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..4000 {
        let len = rng.below(60);
        let s: String = (0..len)
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();
        check_no_panic(&s);
    }
    panic::set_hook(prev);
}

const VOCAB: &[&str] = &[
    "let", "const", "fn", "class", "if", "else", "while", "for", "in", "return", "break",
    "continue", "match", "try", "catch", "finally", "throw", "import", "export", "this", "super",
    "true", "false", "nil", "and", "or", "not", "x", "y", "foo", "1", "42", "3.14", "0xFF",
    "\"str\"", "\"${x}\"", "(", ")", "{", "}", "[", "]", ";", ",", ":", ".", "..", "=>", "+", "-",
    "*", "/", "%", "=", "==", "!=", "<", ">", "&&", "||", "!",
];

#[test]
fn random_token_soup_never_panics() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut rng = Rng::new(0x1234_5678);
    for _ in 0..4000 {
        let n = rng.below(40);
        let mut s = String::new();
        for _ in 0..n {
            s.push_str(VOCAB[rng.below(VOCAB.len())]);
            s.push(' ');
        }
        check_no_panic(&s);
    }
    panic::set_hook(prev);
}

#[test]
fn mutated_real_programs_never_panic() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let sources: Vec<String> = std::fs::read_dir("examples")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("lum"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    assert!(!sources.is_empty());

    let mut rng = Rng::new(0xDEAD_BEEF);
    for _ in 0..3000 {
        let base = &sources[rng.below(sources.len())];
        let mut bytes: Vec<u8> = base.bytes().collect();
        if bytes.is_empty() {
            continue;
        }
        // Apply a few random byte mutations (insert/delete/replace).
        for _ in 0..(1 + rng.below(5)) {
            match rng.below(3) {
                0 if !bytes.is_empty() => {
                    let i = rng.below(bytes.len());
                    bytes[i] = (rng.below(95) + 32) as u8; // printable ASCII
                }
                1 => {
                    let i = rng.below(bytes.len() + 1);
                    bytes.insert(i, (rng.below(95) + 32) as u8);
                }
                _ if bytes.len() > 1 => {
                    let i = rng.below(bytes.len());
                    bytes.remove(i);
                }
                _ => {}
            }
        }
        // Mutations may break UTF-8; the lexer takes &str, so recover lossily —
        // the point is that whatever valid string results must not panic.
        let s = String::from_utf8_lossy(&bytes);
        check_no_panic(&s);
    }
    panic::set_hook(prev);
}
