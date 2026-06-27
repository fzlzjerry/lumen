//! Project and package management around a `lumen.toml` manifest.
//!
//! Commands: `lumen new <name>` scaffolds a project; `lumen add` records a
//! dependency; `lumen build` static-checks every source file; `lumen run`
//! executes the package entry point; `lumen test` runs every `.lum` under
//! `tests/` (a test passes if it runs without an uncaught error — `assert`
//! throws on failure).
//!
//! Dependencies in `[dependencies]` are either a local path (`name = "path"` or
//! `name = { path = "..." }`) or a git source (`name = { git = "url", rev = ".." }`).
//! Git sources are cloned into `.lumen/git/<name>` and pinned to an exact commit
//! in `lumen.lock` for reproducible builds; each dependency's directory is added
//! to the module search path so its modules are importable by name.

use crate::vm::Vm;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A parsed `lumen.toml`.
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub entry: String,
    /// Declared dependencies, sorted by name for deterministic lockfiles.
    pub dependencies: Vec<Dependency>,
}

/// A declared dependency and where its source comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub source: DepSource,
}

/// Where a dependency's source is fetched from.
#[derive(Debug, Clone, PartialEq)]
pub enum DepSource {
    /// A local directory, relative to the project root (or absolute).
    Path(String),
    /// A git repository, optionally pinned to a branch/tag/commit `rev`. The
    /// concrete commit is resolved and recorded in `lumen.lock`.
    Git { url: String, rev: Option<String> },
}

/// Strip matching single/double quotes from a TOML scalar.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Whether a dependency name is safe to use as a directory component (it is
/// joined into `.lumen/git/<name>`, so `/`, `..`, etc. must be rejected).
fn is_valid_dep_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse a dependency's value: a bare string is a path; an inline table
/// `{ git = "...", rev = "..." }` or `{ path = "..." }` selects the source.
/// (Inline tables are flat and comma-separated; values must not contain commas.)
fn parse_dep(name: &str, raw: &str) -> Result<Dependency, String> {
    if !is_valid_dep_name(name) {
        return Err(format!(
            "dependency name '{name}' must be alphanumeric (with '-' or '_')"
        ));
    }
    let raw = raw.trim();
    let source = if raw.starts_with('{') && raw.ends_with('}') {
        let mut fields: HashMap<String, String> = HashMap::new();
        for part in raw[1..raw.len() - 1].split(',') {
            if part.trim().is_empty() {
                continue;
            }
            let eq = part
                .find('=')
                .ok_or_else(|| format!("dependency '{name}': malformed entry '{}'", part.trim()))?;
            fields.insert(part[..eq].trim().to_string(), unquote(&part[eq + 1..]));
        }
        if let Some(url) = fields.get("git") {
            DepSource::Git {
                url: url.clone(),
                rev: fields.get("rev").cloned(),
            }
        } else if let Some(path) = fields.get("path") {
            DepSource::Path(path.clone())
        } else {
            return Err(format!(
                "dependency '{name}': inline table needs a 'git' or 'path' key"
            ));
        }
    } else {
        DepSource::Path(unquote(raw))
    };
    Ok(Dependency {
        name: name.to_string(),
        source,
    })
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
            sections
                .entry(current.clone())
                .or_default()
                .insert(key, val);
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
    let version = pkg
        .get("version")
        .cloned()
        .unwrap_or_else(|| "0.1.0".to_string());
    let entry = pkg
        .get("entry")
        .cloned()
        .unwrap_or_else(|| "src/main.lum".to_string());
    let mut dependencies: Vec<Dependency> = match sections.get("dependencies") {
        Some(d) => d
            .iter()
            .map(|(k, v)| parse_dep(k, v))
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    dependencies.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Manifest {
        name,
        version,
        entry,
        dependencies,
    })
}

// ---- the lockfile (`lumen.lock`) ------------------------------------------

/// One resolved dependency recorded in `lumen.lock`.
#[derive(Debug, Clone, PartialEq)]
pub struct LockEntry {
    pub name: String,
    pub locked: LockedSource,
}

/// A dependency pinned to a concrete, reproducible source.
#[derive(Debug, Clone, PartialEq)]
pub enum LockedSource {
    Path(String),
    /// A git source pinned to an exact `commit` SHA (the `rev` is kept for
    /// display / re-resolution).
    Git {
        url: String,
        rev: Option<String>,
        commit: String,
    },
}

/// Render a `lumen.lock`: one `[name]` section per package, sorted by name.
fn render_lockfile(entries: &[LockEntry]) -> String {
    use std::fmt::Write;
    let mut sorted: Vec<&LockEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("# lumen.lock — generated by `lumen`. Do not edit by hand.\n");
    for e in sorted {
        let _ = write!(out, "\n[{}]\n", e.name);
        match &e.locked {
            LockedSource::Path(p) => {
                let _ = writeln!(out, "path = \"{p}\"");
            }
            LockedSource::Git { url, rev, commit } => {
                let _ = writeln!(out, "git = \"{url}\"");
                if let Some(r) = rev {
                    let _ = writeln!(out, "rev = \"{r}\"");
                }
                let _ = writeln!(out, "commit = \"{commit}\"");
            }
        }
    }
    out
}

/// Parse a `lumen.lock` into its entries (sorted by name; unknown sections are
/// skipped). A lock that doesn't parse cleanly yields an empty list, which
/// forces re-resolution rather than a hard failure.
fn parse_lockfile(text: &str) -> Vec<LockEntry> {
    let sections = parse_toml(text);
    let mut entries = Vec::new();
    for (name, fields) in &sections {
        if name.is_empty() {
            continue;
        }
        let locked = if let Some(url) = fields.get("git") {
            match fields.get("commit") {
                Some(commit) => LockedSource::Git {
                    url: url.clone(),
                    rev: fields.get("rev").cloned(),
                    commit: commit.clone(),
                },
                None => continue, // a git lock entry without a commit is unusable
            }
        } else if let Some(path) = fields.get("path") {
            LockedSource::Path(path.clone())
        } else {
            continue;
        };
        entries.push(LockEntry {
            name: name.clone(),
            locked,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

// ---- `lumen add` ----------------------------------------------------------

/// Parse `add` arguments into `(name, source)`. Forms:
/// `<name> <path>`, `<name> --path <path>`, `<name> --git <url> [--rev <rev>]`.
fn parse_add_args(args: &[String]) -> Result<(String, DepSource), String> {
    const USAGE: &str = "usage: lumen add <name> <path> | <name> --git <url> [--rev <rev>]";
    let (mut name, mut path, mut git, mut rev) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--git" => {
                i += 1;
                git = Some(args.get(i).ok_or("--git needs a URL")?.clone());
            }
            "--rev" => {
                i += 1;
                rev = Some(args.get(i).ok_or("--rev needs a value")?.clone());
            }
            "--path" => {
                i += 1;
                path = Some(args.get(i).ok_or("--path needs a path")?.clone());
            }
            s if s.starts_with("--") => return Err(format!("unknown flag '{s}'")),
            s if name.is_none() => name = Some(s.to_string()),
            s if path.is_none() && git.is_none() => path = Some(s.to_string()),
            s => return Err(format!("unexpected argument '{s}'")),
        }
        i += 1;
    }
    let name = name.ok_or(USAGE)?;
    if !is_valid_dep_name(&name) {
        return Err(format!(
            "dependency name '{name}' must be alphanumeric (with '-' or '_')"
        ));
    }
    let source = match (git, path) {
        (Some(url), _) => DepSource::Git { url, rev },
        (None, Some(p)) => DepSource::Path(p),
        (None, None) => return Err(USAGE.to_string()),
    };
    Ok((name, source))
}

/// The `lumen.toml` line for a dependency.
fn dep_toml_line(name: &str, source: &DepSource) -> String {
    match source {
        DepSource::Path(p) => format!("{name} = \"{p}\""),
        DepSource::Git { url, rev: None } => format!("{name} = {{ git = \"{url}\" }}"),
        DepSource::Git { url, rev: Some(r) } => {
            format!("{name} = {{ git = \"{url}\", rev = \"{r}\" }}")
        }
    }
}

/// Add (or replace) a dependency in a `lumen.toml`, returning the new text. The
/// `[dependencies]` section is created if absent; an existing entry for the same
/// name is replaced in place.
fn add_dependency_to_manifest(text: &str, name: &str, source: &DepSource) -> String {
    let new_line = dep_toml_line(name, source);
    let header = |l: &str| {
        let t = l.split('#').next().unwrap_or("").trim();
        t.starts_with('[') && t.ends_with(']')
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let dep_header = lines
        .iter()
        .position(|l| l.split('#').next().unwrap_or("").trim() == "[dependencies]");
    match dep_header {
        Some(hi) => {
            // The section runs until the next header (or EOF).
            let end = (hi + 1..lines.len())
                .find(|&i| header(&lines[i]))
                .unwrap_or(lines.len());
            let existing = (hi + 1..end).find(|&i| {
                let t = lines[i].split('#').next().unwrap_or("").trim();
                t.find('=').is_some_and(|eq| t[..eq].trim() == name)
            });
            match existing {
                Some(i) => lines[i] = new_line,
                None => {
                    // Insert after the last non-blank line of the section.
                    let mut at = end;
                    while at > hi + 1 && lines[at - 1].trim().is_empty() {
                        at -= 1;
                    }
                    lines.insert(at, new_line);
                }
            }
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[dependencies]".to_string());
            lines.push(new_line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `lumen add <name> ...` — record a dependency in `lumen.toml`. Resolution
/// (cloning git sources, writing `lumen.lock`) happens on the next `lumen run`
/// or `lumen build`.
pub fn cmd_add(args: &[String]) -> i32 {
    let (name, source) = match parse_add_args(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    let root = match load_project() {
        Ok((root, _)) => root,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    let manifest_path = root.join("lumen.toml");
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("lumen: cannot read lumen.toml: {e}");
            return 1;
        }
    };
    let updated = add_dependency_to_manifest(&text, &name, &source);
    if let Err(e) = std::fs::write(&manifest_path, &updated) {
        eprintln!("lumen: cannot write lumen.toml: {e}");
        return 1;
    }
    println!("Added dependency '{name}' to lumen.toml");
    println!("  run `lumen run` or `lumen build` to resolve it");
    0
}

// ---- git dependency resolution --------------------------------------------

/// The directory a git dependency is checked out into (under the project root).
fn git_checkout_dir(root: &Path, name: &str) -> PathBuf {
    root.join(".lumen").join("git").join(name)
}

/// Run a `git` subcommand, returning trimmed stdout or a descriptive error.
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Clone (if needed) and check out a git dependency into `dir`, returning the
/// exact commit SHA. A locked commit is honoured for reproducibility; otherwise
/// `rev` (a branch/tag/commit) is used, falling back to the default branch.
///
/// Security note: this fetches and later executes third-party code. Only depend
/// on sources you trust.
fn resolve_git(
    url: &str,
    rev: Option<&str>,
    locked_commit: Option<&str>,
    dir: &Path,
) -> Result<String, String> {
    if !dir.join(".git").is_dir() {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_dir_all(dir); // clear any stale partial checkout
        run_git(&["clone", "--quiet", url, &dir.to_string_lossy()], None)?;
    } else {
        // Best-effort refresh so a named rev can advance; ignore offline failures.
        let _ = run_git(&["fetch", "--quiet", "--tags", "origin"], Some(dir));
    }
    // The locked commit wins (reproducible); otherwise honour the requested rev.
    if let Some(target) = locked_commit.or(rev) {
        run_git(&["checkout", "--quiet", target], Some(dir))?;
    }
    run_git(&["rev-parse", "HEAD"], Some(dir))
}

/// Resolve all dependencies: clone/check out git sources into `.lumen/git/<name>`
/// and (re)write `lumen.lock` pinning each to a commit. Path-only projects need
/// neither fetching nor a lockfile, so they are left untouched.
fn resolve_dependencies(root: &Path, manifest: &Manifest) -> Result<(), String> {
    if !manifest
        .dependencies
        .iter()
        .any(|d| matches!(d.source, DepSource::Git { .. }))
    {
        return Ok(());
    }
    let lock_path = root.join("lumen.lock");
    let existing = std::fs::read_to_string(&lock_path)
        .map(|t| parse_lockfile(&t))
        .unwrap_or_default();
    let mut entries = Vec::new();
    for dep in &manifest.dependencies {
        let locked = match &dep.source {
            DepSource::Path(p) => LockedSource::Path(p.clone()),
            DepSource::Git { url, rev } => {
                // Reuse the locked commit only when both URL and rev still match;
                // changing the rev in the manifest forces re-resolution.
                let locked_commit =
                    existing
                        .iter()
                        .find(|e| e.name == dep.name)
                        .and_then(|e| match &e.locked {
                            LockedSource::Git {
                                url: lu,
                                rev: lr,
                                commit,
                            } if lu == url && lr == rev => Some(commit.clone()),
                            _ => None,
                        });
                let dir = git_checkout_dir(root, &dep.name);
                let commit = resolve_git(url, rev.as_deref(), locked_commit.as_deref(), &dir)
                    .map_err(|e| format!("dependency '{}': {e}", dep.name))?;
                LockedSource::Git {
                    url: url.clone(),
                    rev: rev.clone(),
                    commit,
                }
            }
        };
        entries.push(LockEntry {
            name: dep.name.clone(),
            locked,
        });
    }
    let rendered = render_lockfile(&entries);
    if std::fs::read_to_string(&lock_path).ok().as_deref() != Some(rendered.as_str()) {
        std::fs::write(&lock_path, &rendered)
            .map_err(|e| format!("cannot write lumen.lock: {e}"))?;
    }
    Ok(())
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
        // `.lumen/` caches fetched git dependencies; `lumen.lock` is committed.
        std::fs::write(root.join(".gitignore"), "/.lumen/\n")?;
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
    add_dependency_paths(vm, root, manifest);
}

/// Add each dependency's module directories to the VM's import search path.
fn add_dependency_paths(vm: &mut Vm, root: &Path, manifest: &Manifest) {
    for dep in &manifest.dependencies {
        for dir in dep_search_dirs(root, dep) {
            vm.add_search_path(dir);
        }
    }
}

/// The directories to search for a dependency's modules: its base directory
/// (a path dep resolves relative to the project root; a git dep to its checkout
/// under `.lumen/git/<name>`), plus a `src/` subdirectory if present — so a
/// dependency that is a normal Lumen project (modules under `src/`) just works.
fn dep_search_dirs(root: &Path, dep: &Dependency) -> Vec<PathBuf> {
    let base = match &dep.source {
        DepSource::Path(p) => root.join(p),
        DepSource::Git { .. } => git_checkout_dir(root, &dep.name),
    };
    let src = base.join("src");
    if src.is_dir() {
        vec![base, src]
    } else {
        vec![base]
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
    if let Err(e) = resolve_dependencies(&root, &manifest) {
        eprintln!("lumen: {e}");
        return 1;
    }
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
    add_dependency_paths(&mut vm, &root, &manifest);
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
    let (root, manifest) = match load_project() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lumen: {e}");
            return 1;
        }
    };
    if let Err(e) = resolve_dependencies(&root, &manifest) {
        eprintln!("lumen: {e}");
        return 1;
    }
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
    if let Err(e) = resolve_dependencies(&root, &manifest) {
        eprintln!("lumen: {e}");
        return 1;
    }
    let mut files = Vec::new();
    collect_lum_files(&root.join("tests"), &mut files);
    files.sort();
    if files.is_empty() {
        println!("no tests found under tests/");
        return 0;
    }
    let (mut passed, mut failed) = (0, 0);
    for file in &files {
        let name = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
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
        assert_eq!(
            m.dependencies,
            vec![Dependency {
                name: "utils".into(),
                source: DepSource::Path("lib/utils".into())
            }]
        );
    }

    #[test]
    fn missing_package_is_an_error() {
        assert!(parse_manifest("[dependencies]\n").is_err());
    }

    #[test]
    fn parses_path_and_git_dependencies() {
        let m = parse_manifest(
            "[package]\nname = \"demo\"\n\n[dependencies]\n\
             utils = \"lib/utils\"\n\
             helpers = { path = \"./helpers\" }\n\
             mathlib = { git = \"https://github.com/u/r\", rev = \"v1.0\" }\n\
             latest = { git = \"https://github.com/u/x\" }\n",
        )
        .unwrap();
        // Dependencies are sorted by name for deterministic lockfiles.
        let names: Vec<&str> = m.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["helpers", "latest", "mathlib", "utils"]);
        let dep = |name: &str| {
            &m.dependencies
                .iter()
                .find(|d| d.name == name)
                .unwrap()
                .source
        };
        assert_eq!(*dep("utils"), DepSource::Path("lib/utils".into()));
        assert_eq!(*dep("helpers"), DepSource::Path("./helpers".into()));
        assert_eq!(
            *dep("mathlib"),
            DepSource::Git {
                url: "https://github.com/u/r".into(),
                rev: Some("v1.0".into())
            }
        );
        assert_eq!(
            *dep("latest"),
            DepSource::Git {
                url: "https://github.com/u/x".into(),
                rev: None
            }
        );
    }

    #[test]
    fn rejects_inline_table_without_git_or_path() {
        assert!(
            parse_manifest("[package]\nname = \"d\"\n[dependencies]\nbad = { foo = \"x\" }\n")
                .is_err()
        );
    }

    #[test]
    fn lockfile_round_trips() {
        let entries = vec![
            LockEntry {
                name: "mathlib".into(),
                locked: LockedSource::Git {
                    url: "https://github.com/u/r".into(),
                    rev: Some("v1.0".into()),
                    commit: "abc123".into(),
                },
            },
            LockEntry {
                name: "utils".into(),
                locked: LockedSource::Path("lib/utils".into()),
            },
        ];
        let parsed = parse_lockfile(&render_lockfile(&entries));
        assert_eq!(parsed.len(), 2);
        let get = |n: &str| &parsed.iter().find(|e| e.name == n).unwrap().locked;
        assert_eq!(*get("utils"), LockedSource::Path("lib/utils".into()));
        assert_eq!(
            *get("mathlib"),
            LockedSource::Git {
                url: "https://github.com/u/r".into(),
                rev: Some("v1.0".into()),
                commit: "abc123".into(),
            }
        );
    }

    #[test]
    fn lockfile_git_without_rev_omits_it() {
        let entries = vec![LockEntry {
            name: "x".into(),
            locked: LockedSource::Git {
                url: "u".into(),
                rev: None,
                commit: "sha".into(),
            },
        }];
        let text = render_lockfile(&entries);
        assert!(!text.contains("rev ="), "no rev line when unpinned");
        let parsed = parse_lockfile(&text);
        assert_eq!(
            parsed[0].locked,
            LockedSource::Git {
                url: "u".into(),
                rev: None,
                commit: "sha".into()
            }
        );
    }

    fn args(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_args_parse_path_and_git() {
        assert_eq!(
            parse_add_args(&args(&["utils", "../utils"])).unwrap(),
            ("utils".into(), DepSource::Path("../utils".into()))
        );
        assert_eq!(
            parse_add_args(&args(&["lib", "--git", "https://x", "--rev", "v1"])).unwrap(),
            (
                "lib".into(),
                DepSource::Git {
                    url: "https://x".into(),
                    rev: Some("v1".into())
                }
            )
        );
        assert!(parse_add_args(&args(&[])).is_err(), "needs a name");
        assert!(
            parse_add_args(&args(&["onlyname"])).is_err(),
            "needs a source"
        );
    }

    #[test]
    fn add_path_dependency_into_existing_section() {
        let toml = "[package]\nname = \"d\"\n\n[dependencies]\n";
        let out = add_dependency_to_manifest(toml, "utils", &DepSource::Path("../utils".into()));
        let m = parse_manifest(&out).unwrap();
        assert_eq!(
            m.dependencies
                .iter()
                .find(|d| d.name == "utils")
                .unwrap()
                .source,
            DepSource::Path("../utils".into())
        );
    }

    #[test]
    fn add_git_dependency_creates_section() {
        let toml = "[package]\nname = \"d\"\n"; // no [dependencies] yet
        let out = add_dependency_to_manifest(
            toml,
            "lib",
            &DepSource::Git {
                url: "https://x/r".into(),
                rev: Some("v1".into()),
            },
        );
        assert!(out.contains("[dependencies]"));
        let m = parse_manifest(&out).unwrap();
        assert_eq!(
            m.dependencies[0].source,
            DepSource::Git {
                url: "https://x/r".into(),
                rev: Some("v1".into())
            }
        );
    }

    #[test]
    fn add_dependency_replaces_existing() {
        let toml = "[package]\nname = \"d\"\n\n[dependencies]\nutils = \"old\"\n";
        let out = add_dependency_to_manifest(toml, "utils", &DepSource::Path("new".into()));
        let m = parse_manifest(&out).unwrap();
        let utils: Vec<_> = m
            .dependencies
            .iter()
            .filter(|d| d.name == "utils")
            .collect();
        assert_eq!(utils.len(), 1, "replaced, not duplicated");
        assert_eq!(utils[0].source, DepSource::Path("new".into()));
    }

    #[test]
    fn resolves_a_local_git_dependency() {
        // Network-free: build a throwaway local git repo and use its path as the
        // git source, then resolve it into a separate project directory.
        let base = std::env::temp_dir().join("lumen_test_gitdep");
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("dep_repo");
        let proj = base.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("git runs")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(
            repo.join("greet.lum"),
            "export fn hi() { return \"hi from dep\"; }\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        let sha = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();

        let manifest = Manifest {
            name: "proj".into(),
            version: "0.1.0".into(),
            entry: "src/main.lum".into(),
            dependencies: vec![Dependency {
                name: "greet".into(),
                source: DepSource::Git {
                    url: repo.to_string_lossy().into_owned(),
                    rev: None,
                },
            }],
        };
        resolve_dependencies(&proj, &manifest).expect("resolve");
        // The module was checked out, and the lock pins the exact commit.
        assert!(git_checkout_dir(&proj, "greet").join("greet.lum").is_file());
        let lock = std::fs::read_to_string(proj.join("lumen.lock")).unwrap();
        assert!(lock.contains(&sha), "lock pins the resolved commit");
        // A second resolve is a no-op reuse (lock already has the commit).
        resolve_dependencies(&proj, &manifest).expect("re-resolve");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_unsafe_dependency_name() {
        // A name with path separators must never reach `.lumen/git/<name>`.
        assert!(
            parse_manifest("[package]\nname = \"d\"\n[dependencies]\n../evil = \"x\"\n").is_err()
        );
        assert!(parse_add_args(&args(&["../evil", "p"])).is_err());
        assert!(parse_add_args(&args(&["ok_name-1", "p"])).is_ok());
    }

    #[test]
    fn dep_search_dirs_include_src_when_present() {
        let base = std::env::temp_dir().join("lumen_test_depdirs");
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("proj");
        // A git checkout that is laid out as a normal project (modules under src/).
        std::fs::create_dir_all(git_checkout_dir(&root, "x").join("src")).unwrap();
        let with_src = Dependency {
            name: "x".into(),
            source: DepSource::Git {
                url: "u".into(),
                rev: None,
            },
        };
        let dirs = dep_search_dirs(&root, &with_src);
        assert!(
            dirs.contains(&git_checkout_dir(&root, "x")),
            "the checkout root"
        );
        assert!(
            dirs.contains(&git_checkout_dir(&root, "x").join("src")),
            "and its src/"
        );
        // A checkout with modules at the root: only the root is searched.
        let no_src = Dependency {
            name: "y".into(),
            source: DepSource::Git {
                url: "u".into(),
                rev: None,
            },
        };
        assert_eq!(
            dep_search_dirs(&root, &no_src),
            vec![git_checkout_dir(&root, "y")]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn changing_rev_reresolves_to_new_commit() {
        let base = std::env::temp_dir().join("lumen_test_gitdep_rev");
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let proj = base.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("git runs")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("m.lum"), "export fn v() { return 1; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "a"]);
        let sha_a = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(repo.join("m.lum"), "export fn v() { return 2; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "b"]);
        let sha_b = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(sha_a, sha_b);

        let mk = |rev: &str| Manifest {
            name: "p".into(),
            version: "0".into(),
            entry: "src/main.lum".into(),
            dependencies: vec![Dependency {
                name: "m".into(),
                source: DepSource::Git {
                    url: repo.to_string_lossy().into_owned(),
                    rev: Some(rev.to_string()),
                },
            }],
        };
        resolve_dependencies(&proj, &mk(&sha_a)).expect("resolve a");
        assert!(std::fs::read_to_string(proj.join("lumen.lock"))
            .unwrap()
            .contains(&sha_a));
        // Changing the rev must re-resolve, not reuse the stale locked commit.
        resolve_dependencies(&proj, &mk(&sha_b)).expect("resolve b");
        let lock = std::fs::read_to_string(proj.join("lumen.lock")).unwrap();
        assert!(
            lock.contains(&sha_b),
            "rev change re-resolved to the new commit"
        );
        assert!(!lock.contains(&sha_a), "the old commit is no longer pinned");
        let _ = std::fs::remove_dir_all(&base);
    }
}
