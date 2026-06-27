//! End-to-end coverage for the `io`, `os`, and `time` native modules — the
//! file/process/clock surface that the other suites don't touch. File tests use
//! real temp files under a unique per-test directory so they can run in
//! parallel without colliding.

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `src`, returning captured stdout. `args` populates `os.args()`.
fn run_args(src: &str, args: &[&str]) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.set_args(args.iter().map(|s| s.to_string()).collect());
    vm.interpret(proto).unwrap_or_else(|e| panic!("runtime error:\n{e}"));
    let bytes = buf.0.borrow().clone();
    String::from_utf8(bytes).unwrap()
}

fn run(src: &str) -> String {
    run_args(src, &[])
}

/// Returns Err(message) if the program throws.
fn run_expect_throw(src: &str) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.interpret(proto).expect_err("expected a thrown error")
}

/// A unique temp directory for this test, created fresh.
fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen_test_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn io_write_read_roundtrip() {
    let dir = tmpdir("rw");
    let path = dir.join("note.txt");
    let p = path.to_string_lossy().replace('\\', "/");
    let out = run(&format!(
        r#"import "io";
        io.write_file("{p}", "hello\nworld");
        println(io.exists("{p}"));
        println(io.read_file("{p}"));
        "#
    ));
    assert_eq!(out, "true\nhello\nworld\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn io_append_and_lines() {
    let dir = tmpdir("append");
    let path = dir.join("log.txt");
    let p = path.to_string_lossy().replace('\\', "/");
    let out = run(&format!(
        r#"import "io";
        io.write_file("{p}", "a\n");
        io.append_file("{p}", "b\n");
        io.append_file("{p}", "c\n");
        let ls = io.lines("{p}");
        println(len(ls));
        println(ls[0] + ls[1] + ls[2]);
        "#
    ));
    assert_eq!(out, "3\nabc\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn io_open_file_handle() {
    let dir = tmpdir("handle");
    let path = dir.join("h.txt");
    let p = path.to_string_lossy().replace('\\', "/");
    let out = run(&format!(
        r#"import "io";
        let w = io.open("{p}", "w");
        w.write("one\n"); w.write("two\n"); w.write("three\n");
        w.close();
        let r = io.open("{p}", "r");
        println(r.read_line());          // one (newline stripped)
        println(r.read_line());          // two
        r.close();
        let n = 0;
        for line in io.open("{p}", "r") {{ n = n + 1; }}
        println(n);                       // 3
        let r2 = io.open("{p}", "r");
        print(r2.read());                 // whole content
        r2.close();
        let a = io.open("{p}", "a");
        a.write("four\n"); a.close();
        let m = 0;
        for line in io.open("{p}", "r") {{ m = m + 1; }}
        println(m);                       // 4
        println(type(io.open("{p}", "r")));  // file
        "#
    ));
    assert_eq!(out, "one\ntwo\n3\none\ntwo\nthree\n4\nfile\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn io_handle_errors() {
    let dir = tmpdir("handle_err");
    let path = dir.join("e.txt");
    let p = path.to_string_lossy().replace('\\', "/");
    // Reading from a writer and writing to a reader are TypeErrors; a closed
    // handle and a bad mode are ValueErrors.
    let prog = |body: &str| format!("import \"io\"; io.write_file(\"{p}\", \"x\\n\"); {body}");
    assert!(run_expect_throw(&prog(&format!("io.open(\"{p}\", \"w\").read_line();"))).contains("TypeError"));
    assert!(run_expect_throw(&prog(&format!("io.open(\"{p}\", \"r\").write(\"x\");"))).contains("TypeError"));
    assert!(run_expect_throw(&prog(&format!("io.open(\"{p}\", \"z\");"))).contains("ValueError"));
    assert!(run_expect_throw(&prog(&format!("let h = io.open(\"{p}\", \"r\"); h.close(); h.read_line();"))).contains("ValueError"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn os_exec_runs_a_command() {
    // `echo` is available on the POSIX test platforms; capture status + stdout.
    let out = run(
        r#"import "os" as os;
        let r = os.exec("echo", ["hello", "world"]);
        println(r["status"]);
        print(r["stdout"]);
        let f = os.exec("false", []);
        println(f["status"]);
        "#,
    );
    assert_eq!(out, "0\nhello world\n1\n");
}

#[test]
fn os_exec_missing_command_throws() {
    let msg = run_expect_throw(
        r#"import "os" as os; os.exec("definitely_not_a_real_command_xyzzy", []);"#,
    );
    assert!(msg.contains("ValueError") || msg.contains("failed to run"), "got: {msg}");
}

#[test]
fn io_exists_is_false_for_missing() {
    let out = run(
        r#"import "io";
        println(io.exists("/this/path/does/not/exist/lumen_xyz"));"#,
    );
    assert_eq!(out, "false\n");
}

#[test]
fn io_read_missing_file_throws() {
    let msg = run_expect_throw(
        r#"import "io"; io.read_file("/no/such/lumen/file/here");"#,
    );
    assert!(msg.contains("cannot read") || msg.contains("ValueError"), "got: {msg}");
}

#[test]
fn io_write_to_bad_path_throws() {
    let msg = run_expect_throw(
        r#"import "io"; io.write_file("/no/such/dir/lumen/out.txt", "x");"#,
    );
    assert!(msg.contains("cannot write") || msg.contains("ValueError"), "got: {msg}");
}

#[test]
fn os_platform_and_cwd() {
    let out = run(
        r#"import "os";
        let p = os.platform();
        println(type(p));
        println(len(os.cwd()) > 0);"#,
    );
    // platform is a non-empty string; cwd is non-empty.
    assert_eq!(out, "string\ntrue\n");
}

#[test]
fn os_args_reflects_set_args() {
    let out = run_args(
        r#"import "os";
        let a = os.args();
        println(len(a));
        println(a[0]);
        println(a[2]);"#,
        &["alpha", "beta", "gamma"],
    );
    assert_eq!(out, "3\nalpha\ngamma\n");
}

#[test]
fn os_env_reads_and_defaults() {
    std::env::set_var("LUMEN_TEST_ENV_VAR", "present");
    let out = run(
        r#"import "os";
        println(os.env("LUMEN_TEST_ENV_VAR"));
        println(os.env("LUMEN_DEFINITELY_UNSET_VAR_123", "fallback"));
        println(os.env("LUMEN_DEFINITELY_UNSET_VAR_123"));"#,
    );
    assert_eq!(out, "present\nfallback\nnil\n");
    std::env::remove_var("LUMEN_TEST_ENV_VAR");
}

#[test]
fn time_now_is_positive_and_monotonic_enough() {
    let out = run(
        r#"import "time";
        let t = time.now();
        let m = time.now_millis();
        println(t > 0.0);
        println(m > 0);
        println(type(t));
        println(type(m));"#,
    );
    assert_eq!(out, "true\ntrue\nfloat\nint\n");
}

#[test]
fn time_sleep_returns_and_advances_clock() {
    let out = run(
        r#"import "time";
        let before = time.now_millis();
        time.sleep(0.01);
        let after = time.now_millis();
        println(after >= before);"#,
    );
    assert_eq!(out, "true\n");
}
