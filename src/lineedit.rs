//! A small raw-mode line editor for the REPL — emacs-style editing without any
//! external crate. On a TTY it puts the terminal into raw mode via a hand-written
//! `termios` FFI binding (libc is already linked by std) and reads keystrokes one
//! at a time, supporting: left/right cursor movement, Home/End (and Ctrl-A/Ctrl-E),
//! Backspace/Delete, word/line kill (Ctrl-W/Ctrl-U/Ctrl-K), Up/Down history
//! recall, Ctrl-C (cancel line), and Ctrl-D (EOF on an empty line).
//!
//! The terminal's *output* post-processing is left untouched (`ONLCR` stays on),
//! so the rest of the REPL's `println!`s keep working normally while raw mode is
//! active. When stdin/stdout isn't a TTY (pipes, tests), `is_tty()` is false and
//! the REPL falls back to ordinary line-buffered reading.

use std::io::{self, Read, Write};

/// The editable text and cursor position. Pure logic — unit-tested without a
/// terminal.
struct LineBuffer {
    chars: Vec<char>,
    cursor: usize,
}

impl LineBuffer {
    fn new() -> Self {
        LineBuffer { chars: Vec::new(), cursor: 0 }
    }

    fn set(&mut self, text: &str) {
        // History entries may span lines; flatten so the single-line editor
        // shows something sensible.
        self.chars = text.replace('\n', " ").chars().collect();
        self.cursor = self.chars.len();
    }

    fn as_string(&self) -> String {
        self.chars.iter().collect()
    }

    fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    fn delete(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    fn left(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    fn right(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Kill from the cursor back to the start of the previous word (Ctrl-W).
    fn kill_prev_word(&mut self) {
        let mut i = self.cursor;
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.chars.drain(i..self.cursor);
        self.cursor = i;
    }

    /// Kill from the cursor to the start of the line (Ctrl-U).
    fn kill_to_start(&mut self) {
        self.chars.drain(0..self.cursor);
        self.cursor = 0;
    }

    /// Kill from the cursor to the end of the line (Ctrl-K).
    fn kill_to_end(&mut self) {
        self.chars.truncate(self.cursor);
    }
}

/// The outcome of editing one physical line.
pub enum Input {
    Line(String),
    Eof,
    Interrupt,
}

/// Read and edit one line in raw mode, with `history` available for Up/Down
/// recall. Renders against `prompt` (ANSI codes in the prompt are fine — cursor
/// positioning is measured from the end of the buffer, not the prompt width).
pub fn read_line(prompt: &str, history: &[String]) -> io::Result<Input> {
    let mut buf = LineBuffer::new();
    let mut hist_idx = history.len(); // one past the end == the live line
    let mut stash = String::new(); // the live line, saved while browsing history
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut byte = [0u8; 1];

    refresh(prompt, &buf)?;
    loop {
        if input.read(&mut byte)? == 0 {
            return Ok(Input::Eof);
        }
        match byte[0] {
            b'\r' | b'\n' => {
                print!("\r\n");
                io::stdout().flush()?;
                return Ok(Input::Line(buf.as_string()));
            }
            3 => {
                // Ctrl-C: abandon this line.
                print!("^C\r\n");
                io::stdout().flush()?;
                return Ok(Input::Interrupt);
            }
            4 => {
                // Ctrl-D: EOF only on an empty line; otherwise delete-forward.
                if buf.chars.is_empty() {
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Ok(Input::Eof);
                }
                buf.delete();
            }
            1 => buf.home(),       // Ctrl-A
            5 => buf.end(),        // Ctrl-E
            2 => {
                buf.left();
            } // Ctrl-B
            6 => {
                buf.right();
            } // Ctrl-F
            23 => buf.kill_prev_word(), // Ctrl-W
            21 => buf.kill_to_start(),  // Ctrl-U
            11 => buf.kill_to_end(),    // Ctrl-K
            8 | 127 => {
                buf.backspace();
            }
            27 => {
                // Escape sequence: read the next two bytes.
                let mut seq = [0u8; 2];
                if input.read(&mut seq[..1])? == 0 {
                    continue;
                }
                if input.read(&mut seq[1..2])? == 0 {
                    continue;
                }
                if seq[0] == b'[' {
                    match seq[1] {
                        b'C' => {
                            buf.right();
                        }
                        b'D' => {
                            buf.left();
                        }
                        b'A' => history_prev(&mut buf, history, &mut hist_idx, &mut stash),
                        b'B' => history_next(&mut buf, history, &mut hist_idx, &stash),
                        b'H' => buf.home(),
                        b'F' => buf.end(),
                        b'3' => {
                            // Delete key sends ESC [ 3 ~ ; consume the trailing '~'.
                            let _ = input.read(&mut byte);
                            buf.delete();
                        }
                        _ => {}
                    }
                }
            }
            b if b >= 0x20 => {
                // A printable byte: decode it as (the start of) a UTF-8 char.
                let c = decode_char(b, &mut input)?;
                buf.insert(c);
            }
            _ => {}
        }
        refresh(prompt, &buf)?;
    }
}

/// Decode one UTF-8 character whose first byte is `first`, reading continuation
/// bytes from `input` as needed.
fn decode_char(first: u8, input: &mut impl Read) -> io::Result<char> {
    let extra = match first {
        0x00..=0x7F => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        _ => 3,
    };
    let mut bytes = vec![first];
    for _ in 0..extra {
        let mut b = [0u8; 1];
        if input.read(&mut b)? == 0 {
            break;
        }
        bytes.push(b[0]);
    }
    Ok(std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()).unwrap_or('\u{FFFD}'))
}

fn history_prev(buf: &mut LineBuffer, history: &[String], idx: &mut usize, stash: &mut String) {
    if *idx == history.len() {
        *stash = buf.as_string(); // remember the live line
    }
    if *idx > 0 {
        *idx -= 1;
        buf.set(&history[*idx]);
    }
}

fn history_next(buf: &mut LineBuffer, history: &[String], idx: &mut usize, stash: &str) {
    if *idx < history.len() {
        *idx += 1;
        if *idx == history.len() {
            buf.set(stash);
        } else {
            buf.set(&history[*idx]);
        }
    }
}

/// Redraw the current line: return to column 0, clear, print prompt + buffer,
/// then move the cursor back to its logical position.
fn refresh(prompt: &str, buf: &LineBuffer) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\r\x1b[K{prompt}{}", buf.as_string())?;
    let back = buf.chars.len() - buf.cursor;
    if back > 0 {
        write!(out, "\x1b[{back}D")?;
    }
    out.flush()
}

// ---- terminal raw mode (termios FFI) ---------------------------------------

#[cfg(target_os = "linux")]
mod term {
    // Hand-written bindings to the C library's termios interface. std already
    // links libc on Linux, so no external crate is needed.
    type Tcflag = u32;
    type Cc = u8;
    type Speed = u32;
    const NCCS: usize = 32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        c_iflag: Tcflag,
        c_oflag: Tcflag,
        c_cflag: Tcflag,
        c_lflag: Tcflag,
        c_line: Cc,
        c_cc: [Cc; NCCS],
        c_ispeed: Speed,
        c_ospeed: Speed,
    }

    impl Termios {
        fn zeroed() -> Self {
            Termios {
                c_iflag: 0,
                c_oflag: 0,
                c_cflag: 0,
                c_lflag: 0,
                c_line: 0,
                c_cc: [0; NCCS],
                c_ispeed: 0,
                c_ospeed: 0,
            }
        }
    }

    // Input flags.
    const ICRNL: Tcflag = 0o000400;
    const IXON: Tcflag = 0o002000;
    // Local flags.
    const ISIG: Tcflag = 0o000001;
    const ICANON: Tcflag = 0o000002;
    const ECHO: Tcflag = 0o000010;
    const IEXTEN: Tcflag = 0o100000;
    // c_cc indices.
    const VTIME: usize = 5;
    const VMIN: usize = 6;
    const TCSAFLUSH: i32 = 2;
    const STDIN: i32 = 0;
    const STDOUT: i32 = 1;

    extern "C" {
        fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, optional_actions: i32, termios: *const Termios) -> i32;
        fn isatty(fd: i32) -> i32;
    }

    pub fn is_tty() -> bool {
        unsafe { isatty(STDIN) == 1 && isatty(STDOUT) == 1 }
    }

    /// An RAII guard: enabling raw mode on creation, restoring on drop.
    pub struct RawGuard {
        saved: Termios,
    }

    impl RawGuard {
        pub fn enable() -> Option<RawGuard> {
            if !is_tty() {
                return None;
            }
            let mut saved = Termios::zeroed();
            if unsafe { tcgetattr(STDIN, &mut saved) } != 0 {
                return None;
            }
            let mut raw = saved;
            // Disable canonical mode, echo, signal generation, extended input,
            // CR->NL translation and flow control — but leave output flags
            // (ONLCR) alone so println!'s "\n" still produces "\r\n".
            raw.c_lflag &= !(ICANON | ECHO | ISIG | IEXTEN);
            raw.c_iflag &= !(ICRNL | IXON);
            raw.c_cc[VMIN] = 1;
            raw.c_cc[VTIME] = 0;
            if unsafe { tcsetattr(STDIN, TCSAFLUSH, &raw) } != 0 {
                return None;
            }
            Some(RawGuard { saved })
        }
    }

    impl Drop for RawGuard {
        fn drop(&mut self) {
            unsafe {
                tcsetattr(STDIN, TCSAFLUSH, &self.saved);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod term {
    pub struct RawGuard;
    impl RawGuard {
        pub fn enable() -> Option<RawGuard> {
            None // raw editing only wired up for Linux; elsewhere fall back
        }
    }
    pub fn is_tty() -> bool {
        false
    }
}

pub use term::{is_tty, RawGuard};

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_from(text: &str, cursor: usize) -> LineBuffer {
        let mut b = LineBuffer::new();
        b.chars = text.chars().collect();
        b.cursor = cursor;
        b
    }

    #[test]
    fn insert_and_backspace() {
        let mut b = LineBuffer::new();
        for c in "let x".chars() {
            b.insert(c);
        }
        assert_eq!(b.as_string(), "let x");
        assert_eq!(b.cursor, 5);
        assert!(b.backspace());
        assert_eq!(b.as_string(), "let ");
        assert_eq!(b.cursor, 4);
    }

    #[test]
    fn cursor_movement_and_mid_insert() {
        let mut b = buf_from("ac", 1);
        b.insert('b'); // insert between a and c
        assert_eq!(b.as_string(), "abc");
        assert_eq!(b.cursor, 2);
        b.home();
        assert_eq!(b.cursor, 0);
        assert!(!b.left()); // already at start
        b.end();
        assert_eq!(b.cursor, 3);
        assert!(!b.right()); // already at end
    }

    #[test]
    fn delete_forward() {
        let mut b = buf_from("abc", 1);
        assert!(b.delete()); // removes 'b'
        assert_eq!(b.as_string(), "ac");
        assert_eq!(b.cursor, 1);
    }

    #[test]
    fn kill_word_and_line() {
        let mut b = buf_from("foo bar baz", 11);
        b.kill_prev_word();
        assert_eq!(b.as_string(), "foo bar ");
        b.kill_to_start();
        assert_eq!(b.as_string(), "");

        let mut b = buf_from("hello world", 5);
        b.kill_to_end();
        assert_eq!(b.as_string(), "hello");
    }

    #[test]
    fn history_browsing() {
        let history = vec!["first".to_string(), "second".to_string()];
        let mut b = LineBuffer::new();
        b.set("live");
        let mut idx = history.len();
        let mut stash = String::new();

        history_prev(&mut b, &history, &mut idx, &mut stash); // -> "second"
        assert_eq!(b.as_string(), "second");
        history_prev(&mut b, &history, &mut idx, &mut stash); // -> "first"
        assert_eq!(b.as_string(), "first");
        history_prev(&mut b, &history, &mut idx, &mut stash); // clamp at oldest
        assert_eq!(b.as_string(), "first");
        history_next(&mut b, &history, &mut idx, &stash); // -> "second"
        assert_eq!(b.as_string(), "second");
        history_next(&mut b, &history, &mut idx, &stash); // -> back to live line
        assert_eq!(b.as_string(), "live");
    }

    #[test]
    fn set_flattens_newlines() {
        let mut b = LineBuffer::new();
        b.set("fn f() {\n  return 1;\n}");
        assert!(!b.as_string().contains('\n'));
    }
}
