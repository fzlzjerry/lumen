//! End-to-end tests: run every example program and assert its full output
//! against a recorded snapshot in `tests/expected/`. This is the integration
//! oracle for the whole pipeline — a regression here means some stage changed
//! observable behavior. Snapshots were generated from verified-correct runs; to
//! intentionally update one, re-run the example and replace its `.txt`.

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
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

/// Run an example file (with `examples/` as the module base dir) and return its
/// captured stdout.
fn run_example(path: &Path) -> String {
    let src = std::fs::read_to_string(path).expect("read example");
    let (program, errs) = lumen::check_source(&src);
    assert!(
        errs.is_empty(),
        "{}: front-end errors: {errs:?}",
        path.display()
    );
    let proto = lumen::compiler::compile(&program).expect("compile");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.set_base_dir(Path::new("examples").to_path_buf());
    vm.interpret(proto)
        .unwrap_or_else(|e| panic!("{}: runtime error:\n{e}", path.display()));
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out
}

#[test]
fn all_examples_match_snapshots() {
    let mut checked = 0;
    let mut entries: Vec<_> = std::fs::read_dir("examples")
        .expect("examples dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("lum")
                // helper modules (no leading digit) are imported, not run directly
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.chars().next().unwrap().is_ascii_digit())
                    .unwrap_or(false)
        })
        .collect();
    entries.sort();

    for path in entries {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let expected_path = format!("tests/expected/{stem}.txt");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing snapshot {expected_path}"));
        let actual = run_example(&path);
        assert_eq!(
            actual,
            expected,
            "\noutput drift for {}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 15,
        "expected to check >=15 examples, did {checked}"
    );
}
