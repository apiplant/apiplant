//! Finding function sources and working out what language each is written in.
//!
//! An entry in `functions/` is either a single source file (`greet.rs`) or a
//! directory holding a project in the language's own native form (`greet/`).
//! Both become a [`Source`]; the rest of the module only cares which.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// What a source file is written in, which decides how it gets compiled.
///
/// Everything except [`Rust`](Language::Rust) targets the plain C ABI declared in
/// `apiplant.h`, so the host loads them all through the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// `.rs` — wrapped in a generated cdylib crate and handed to cargo.
    Rust,
    /// `.c` — compiled straight to a shared library by the system C compiler.
    C,
    /// `.zig` — `zig build-lib -dynamic`, which needs no scaffolding either.
    Zig,
    /// `.go` — `go build -buildmode=c-shared`, which needs a module around it.
    Go,
}

impl Language {
    /// The language for a file extension, or `None` if it isn't a source file.
    fn for_extension(ext: &str) -> Option<Language> {
        match ext {
            "rs" => Some(Language::Rust),
            "c" => Some(Language::C),
            "zig" => Some(Language::Zig),
            "go" => Some(Language::Go),
            _ => None,
        }
    }

    /// The executable this language is built with, honouring the usual override.
    pub(super) fn command(self) -> String {
        let (var, default) = match self {
            Language::Rust => ("CARGO", "cargo"),
            Language::C => ("CC", "cc"),
            Language::Zig => ("ZIG", "zig"),
            Language::Go => ("GO", "go"),
        };
        std::env::var(var).unwrap_or_else(|_| default.into())
    }

    /// What to call the toolchain in an error message.
    pub(super) fn tool(self) -> &'static str {
        match self {
            Language::Rust => "cargo",
            Language::C => "a C compiler",
            Language::Zig => "zig",
            Language::Go => "go",
        }
    }
}

/// One function source found in an app's `functions/` directory.
///
/// A source is either a single file (`greet.rs`) or a whole directory
/// (`greet/`) holding a project in the language's native form — see
/// [`is_dir`](Source::is_dir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The source file, or the directory holding the project.
    pub path: PathBuf,
    /// Crate name derived from the file stem or directory name
    /// (`post-hooks.rs` → `post_hooks`, `post-hooks/` → `post_hooks`).
    pub crate_name: String,
    /// How to compile it.
    pub language: Language,
    /// Whether [`path`](Source::path) is a directory (a native project with its
    /// own dependencies) rather than a single source file.
    pub is_dir: bool,
}

impl Source {
    /// The library file this source compiles to, e.g. `libgreet.so`.
    pub fn library_name(&self) -> String {
        library_name(&self.crate_name)
    }
}

/// Turn a file stem into a valid crate identifier.
pub(super) fn crate_name(stem: &str) -> String {
    let mut name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    name.to_lowercase()
}

/// The platform's file name for a cdylib.
pub(super) fn library_name(crate_name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

/// Every function source in an app's `functions/` directory, in a stable order.
pub fn discover(functions_dir: &Path) -> Result<Vec<Source>> {
    let entries = match std::fs::read_dir(functions_dir) {
        Ok(entries) => entries,
        // No functions/ directory at all is fine — the app just has no functions.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", functions_dir.display())),
    };

    let mut sources: Vec<Source> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();

        // A directory is a native project — a crate, a module, a set of C or
        // Zig files — named for the function it builds.
        if path.is_dir() {
            let Some(language) = classify_directory(&path)? else {
                continue;
            };
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            sources.push(Source {
                crate_name: crate_name(name),
                path,
                language,
                is_dir: true,
            });
            continue;
        }

        let Some(language) = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Language::for_extension)
        else {
            continue;
        };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        sources.push(Source {
            crate_name: crate_name(stem),
            path,
            language,
            is_dir: false,
        });
    }
    sources.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

    // Two files that differ only in punctuation — or a `greet.rs` beside a
    // `greet.c`, or a `greet.rs` beside a `greet/` directory — would fight over
    // one .so.
    for pair in sources.windows(2) {
        if pair[0].crate_name == pair[1].crate_name {
            bail!(
                "`{}` and `{}` both compile to `{}`; rename one",
                pair[0].path.display(),
                pair[1].path.display(),
                pair[0].library_name()
            );
        }
    }
    Ok(sources)
}

/// Decide which language a function *directory* is written in, from what it
/// holds — or `None` if it holds nothing apiplant knows how to build (a stray
/// directory in `functions/` is ignored, exactly as a stray file is).
///
/// A native manifest is the strongest signal, so `Cargo.toml` and `go.mod` win
/// outright. Otherwise the loose source files decide, and two languages of loose
/// files (or Rust/Go files without their manifest) is an error rather than a
/// guess — the fix is unambiguous and worth stating.
pub(super) fn classify_directory(dir: &Path) -> Result<Option<Language>> {
    if dir.join("Cargo.toml").is_file() {
        return Ok(Some(Language::Rust));
    }
    if dir.join("go.mod").is_file() {
        return Ok(Some(Language::Go));
    }

    let mut has_c = false;
    let mut has_zig = false;
    let mut has_rs = false;
    let mut has_go = false;
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        match entry.path().extension().and_then(|e| e.to_str()) {
            Some("c") => has_c = true,
            Some("zig") => has_zig = true,
            Some("rs") => has_rs = true,
            Some("go") => has_go = true,
            _ => {}
        }
    }

    match (has_c, has_zig) {
        (true, true) => bail!(
            "function directory `{}` mixes .c and .zig sources; \
             split them into one directory each",
            dir.display()
        ),
        (true, false) => return Ok(Some(Language::C)),
        (false, true) => return Ok(Some(Language::Zig)),
        (false, false) => {}
    }

    // A Rust or Go directory without its manifest can't be built the native way,
    // and generating one would defeat the point (its own dependencies). Say so.
    if has_rs {
        bail!(
            "function directory `{}` has Rust sources but no Cargo.toml; \
             add one, or make it a single `.rs` file instead",
            dir.display()
        );
    }
    if has_go {
        bail!(
            "function directory `{}` has Go sources but no go.mod; \
             add one, or make it a single `.go` file instead",
            dir.display()
        );
    }

    Ok(None)
}

/// Every top-level file in `dir` with the given extension, in a stable order.
///
/// Used to gather the `.c` files a C directory compiles together, or to find the
/// `.zig` files that could be a Zig directory's root.
pub(super) fn files_with_extension(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    files.sort();
    Ok(files)
}
