//! Project and package management around a `lumen.toml` manifest.
//!
//! Commands: `lumen new <name>` scaffolds a project; `lumen build` static-checks
//! every source file; `lumen run` executes the package entry point; `lumen test`
//! runs every `.lum` under `tests/` (a test passes if it runs without an
//! uncaught error — `assert` throws on failure). Local path dependencies named
//! in `[dependencies]` are added to the module search path so their modules are
//! importable by name.

use crate::vm::Vm;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A parsed `lumen.toml`.
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub entry: String,
    /// `(dependency name, local path)` pairs.
    pub dependencies: Vec<(String, String)>,
}

/// A tiny TOML subset parser: `[section]` headers and `key = value` lines, with
/// `#` line comments and quoted or bare string values. Sufficient for the
/// manifest; not a general TOML implementation.
fn parse_toml(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    sections.insert(current.clone(), HashMap::new());
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len() - 1].trim().to_string();
            sections.entry(current.clone()).or_default();
        } else if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_string();
            let mut val = line[eq + 1..].trim().to_string();
            if val.len() >= 2
                && ((val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\'')))
            {
                val = val[1..val.len() - 1].to_string();
            }
            sections.entry(current.clone()).or_default().insert(key, val);
        }
    }
    sections
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let sections = parse_toml(text);
    let pkg = sections
        .get("package")
        .ok_or_else(|| "lumen.toml: missing [package] section".to_string())?;
    let name = pkg
        .get("name")
        .cloned()
        .ok_or_else(|| "lumen.toml: [package] is missing 'name'".to_string())?;
    let version = pkg.get("version").cloned().unwrap_or_else(|| "0.1.0".to_string());
    let entry = pkg.get("entry").cloned().unwrap_or_else(|| "src/main.lum".to_string());
    let dependencies = sections
        .get("dependencies")
        .map(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    Ok(Manifest { name, version, entry, dependencies })
}

/// Walk up from `start` looking for a `lumen.toml`, returning its directory.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("lumen.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn load_project() -> Result<(PathBuf, Manifest), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = find_project_root(&cwd)
        .ok_or_else(|| "no lumen.toml found in this directory or any parent".to_string())?;
    let text = std::fs::read_to_string(root.join("lumen.toml")).map_err(|e| e.to_string())?;
    let manifest = parse_manifest(&text)?;
    Ok((root, manifest))
}

/// `lumen new <name>` — scaffold a project directory.
pub fn cmd_new(name: Option<&str>) -> i32 {
    let name = match name {
        Some(n) => n,
        None => {
            eprintln!("usage: lumen new <name>");
            return 1;
        }
    };
    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("lumen: '{name}' already exists");
        return 1;
    }
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"src/main.lum\"\n\n[dependencies]\n"
    );
    let main = "fn main() {\n    println(\"Hello from ${greeting()}!\");\n}\n\nfn greeting() {\n    return \"Lumen\";\n}\n\nmain();\n";
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("tests"))?;
        std::fs::write(root.join("lumen.toml"), manifest)?;
        std::fs::write(root.join("src/main.lum"), main)?;
        std::fs::write(
            root.join("tests/basic_test.lum"),
            "// `lumen test` runs every file under tests/.\nassert(1 + 1 == 2, \"math still works\");\nprintln(\"ok\");\n",
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!("Created project '{name}'");
            println!("  cd {name} && lumen run");
            0
        }
        Err(e) => {
            eprintln!("lumen: failed to scaffold project: {e}");
            1
        }
    }
}

/// Configure a VM for a project: base dir at the project root and dependency
/// directories on the search path.
fn configure_vm(vm: &mut Vm, root: &Path, manifest: &Manifest) {
    vm.set_base_dir(root.to_path_buf());
    for (_name, path) in &manifest.dependencies {
        let dep = root.join(path);
        vm.add_search_path(dep);
    }
}

/// `lumen run` (no file argument) — run the package entry point.
pub fn cmd_run() -> i32 {
    let (root, manifest) = match load_project() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    let entry = root.join(&manifest.entry);
    let src = match std::fs::read_to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lumen: cannot read entry '{}': {e}", entry.display());
            return 1;
        }
    };
    let (program, errs) = crate::check_source(&src);
    if !errs.is_empty() {
        report(&errs, &src, &entry.to_string_lossy());
        return 1;
    }
    let proto = match crate::compiler::compile(&program) {
        Ok(p) => p,
        Err(errs) => {
            report(&errs, &src, &entry.to_string_lossy());
            return 1;
        }
    };
    let mut vm = Vm::new();
    crate::stdlib::install(&mut vm);
    if let Some(dir) = entry.parent() {
        vm.set_base_dir(dir.to_path_buf());
    }
    for (_n, path) in &manifest.dependencies {
        vm.add_search_path(root.join(path));
    }
    match vm.interpret(proto) {
        Ok(()) => 0,
        Err(msg) => {
            eprint!("{msg}");
            70
        }
    }
}

/// `lumen build` — static-check every `.lum` file under `src/`.
pub fn cmd_build() -> i32 {
    let (root, _manifest) = match load_project() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    let mut files = Vec::new();
    collect_lum_files(&root.join("src"), &mut files);
    let mut errors = 0;
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap_or_default();
        let (_program, errs) = crate::check_source(&src);
        if !errs.is_empty() {
            report(&errs, &src, &file.to_string_lossy());
            errors += errs.len();
        }
    }
    if errors == 0 {
        println!("checked {} file(s); no errors", files.len());
        0
    } else {
        eprintln!("{errors} error(s)");
        1
    }
}

/// `lumen test` — run every `.lum` under `tests/`; pass == no uncaught error.
pub fn cmd_test() -> i32 {
    let (root, manifest) = match load_project() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    let mut files = Vec::new();
    collect_lum_files(&root.join("tests"), &mut files);
    files.sort();
    if files.is_empty() {
        println!("no tests found under tests/");
        return 0;
    }
    let (mut passed, mut failed) = (0, 0);
    for file in &files {
        let name = file.strip_prefix(&root).unwrap_or(file).to_string_lossy().to_string();
        let src = std::fs::read_to_string(file).unwrap_or_default();
        let (program, errs) = crate::check_source(&src);
        if !errs.is_empty() {
            println!("\x1b[31mFAIL\x1b[0m {name} (compile error)");
            report(&errs, &src, &name);
            failed += 1;
            continue;
        }
        let proto = match crate::compiler::compile(&program) {
            Ok(p) => p,
            Err(_) => {
                println!("\x1b[31mFAIL\x1b[0m {name} (compile error)");
                failed += 1;
                continue;
            }
        };
        // Each test runs in a fresh VM so they cannot interfere.
        let mut vm = Vm::new();
        crate::stdlib::install(&mut vm);
        configure_vm(&mut vm, &root, &manifest);
        if let Some(dir) = file.parent() {
            vm.set_base_dir(dir.to_path_buf());
            vm.add_search_path(root.join("src"));
        }
        match vm.interpret(proto) {
            Ok(()) => {
                println!("\x1b[32mPASS\x1b[0m {name}");
                passed += 1;
            }
            Err(msg) => {
                println!("\x1b[31mFAIL\x1b[0m {name}");
                eprint!("{}", indent(&msg));
                failed += 1;
            }
        }
    }
    println!("\n{passed} passed, {failed} failed");
    if failed == 0 {
        0
    } else {
        1
    }
}

fn collect_lum_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_lum_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("lum") {
                out.push(path);
            }
        }
    }
}

fn report(errs: &[crate::diagnostics::Diagnostic], src: &str, file: &str) {
    for d in errs {
        eprintln!("{}\n", d.render(src, Some(file)));
    }
}

fn indent(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for l in s.lines() {
        let _ = writeln!(out, "    {l}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest() {
        let m = parse_manifest(
            "# my project\n[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[dependencies]\nutils = \"lib/utils\"\n",
        )
        .unwrap();
        assert_eq!(m.name, "demo");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.entry, "src/main.lum"); // default
        assert_eq!(m.dependencies, vec![("utils".to_string(), "lib/utils".to_string())]);
    }

    #[test]
    fn missing_package_is_an_error() {
        assert!(parse_manifest("[dependencies]\n").is_err());
    }
}
