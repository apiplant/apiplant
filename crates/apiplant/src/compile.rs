//! Compiling function sources into loadable libraries.
//!
//! An app keeps its functions as **single source files** next to their config:
//!
//! ```text
//! my-app/functions/
//! ├── greet.rs          # Rust: one file, written as if it were a lib.rs
//! ├── greet.toml        # that function's config
//! ├── libgreet.so       # ← produced by `apiplant build`
//! ├── hello.c           # C, Zig or Go: implements the `apiplant.h` symbols
//! └── libhello.so       # ← also produced by `apiplant build`
//! ```
//!
//! Four languages, and the difference between them is only how much scaffolding
//! the toolchain insists on:
//!
//! | Source | Built with | Scaffolding |
//! |--------|------------|-------------|
//! | `.rs`  | `cargo build` | a generated cdylib crate |
//! | `.c`   | `cc -shared -fPIC` | none |
//! | `.zig` | `zig build-lib -dynamic -lc` | none |
//! | `.go`  | `go build -buildmode=c-shared` | a generated `go.mod` |
//!
//! Scaffolding lives in `.apiplant-build/` inside the app directory. Rust gets a
//! throwaway crate around the file plus one shared `target/`, so dependencies are
//! compiled once and rebuilds stay incremental; Go gets a module, because
//! `go build` will not work without one. C and Zig are single translation units
//! that go straight from source to shared library.
//!
//! ## Directories: functions that need dependencies
//!
//! A single file is enough until a function needs a third-party crate, a second
//! source file, or a linked library. For that, an entry in `functions/` may be a
//! **directory** instead of a file — a self-contained project in the language's
//! own native form, which is exactly where its dependency machinery already
//! lives:
//!
//! ```text
//! my-app/functions/
//! ├── greet/            # a directory is one function library…
//! │   ├── Cargo.toml    #   …here a real crate, with any dependencies you like
//! │   └── src/lib.rs
//! ├── greet.toml        # config still sits beside it, keyed by the dir name
//! └── libgreet.so       # ← produced next to the directory, loaded the same way
//! ```
//!
//! The language of a directory is read from what it contains, so nothing new has
//! to be declared:
//!
//! | A directory containing… | is built as | with |
//! |-------------------------|-------------|-------|
//! | `Cargo.toml`            | Rust | `cargo build` on *your* crate — any deps, any modules |
//! | `go.mod`                | Go   | `go build -buildmode=c-shared` on your module |
//! | `.c` files              | C    | `cc` over every `.c` in the directory, `-I` the directory |
//! | `.zig` files            | Zig  | `zig build-lib` from a root `.zig` that may `@import` the rest |
//!
//! A directory named `greet/` compiles to `libgreet.so` beside it, so the host
//! loads it exactly like a single-file function — the only difference is where the
//! source came from. For Rust and Go the project is yours: apiplant runs your
//! `Cargo.toml`/`go.mod` unchanged and copies out the library it produces, so you
//! own the dependencies *and* the profiles.
//!
//! Everything except Rust targets the plain C ABI declared in `apiplant.h`, so the
//! host loads them all through the same path — see `apiplant_abi::c`.
//!
//! The only requirement is whichever toolchain the app's own sources need. Each is
//! overridable through the variable its ecosystem already uses: `CARGO`, `CC`,
//! `ZIG`, `GO`, plus `CFLAGS`, `ZIGFLAGS` and `CGO_CFLAGS` for extra flags.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Where the scaffolding and cargo's target directory live, inside the app.
const BUILD_DIR: &str = ".apiplant-build";

/// Crates every generated function crate depends on. `apiplant-function` is
/// resolved separately — see [`function_crate_path`].
const DEPENDENCIES: &str = r#"abi_stable = "0.11"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"
"#;

/// Build profiles for the generated crates.
///
/// A function library is a *deployment artifact* — it gets copied next to its
/// source and shipped — so cargo's defaults are the wrong trade-off here. Each
/// cdylib statically links its own copy of `std`, `serde_json` and `schemars`
/// (that independence is exactly what makes the ABI stable), and on top of that
/// cargo emits full DWARF in `dev` and leaves the symbol table in `release`.
/// The debug info alone accounted for ~94% of a `dev` library: a one-page
/// function came to 18 MB, of which 1.5 MB was actual code.
///
/// So:
///
/// * `dev` drops DWARF but keeps the symbol table, so function names still show
///   up when something crashes — 18 MB → 2.4 MB.
/// * `release` strips fully and adds fat LTO with a single codegen unit, which
///   lets the linker discard the parts of `std`/`serde_json`/`schemars` the
///   function never reaches — 1.2 MB → ~600 KB, and faster code as a bonus.
///
/// Deliberately *not* set: `opt-level = "z"` (a further ~100 KB, but these are
/// per-request handlers, so speed wins) and `panic = "abort"` (another ~80 KB,
/// but it would turn a panic in one function into an abort of the whole server).
const PROFILES: &str = r#"[profile.dev]
strip = "debuginfo"

[profile.release]
strip = "symbols"
lto = "fat"
codegen-units = 1
"#;

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
    fn command(self) -> String {
        let (var, default) = match self {
            Language::Rust => ("CARGO", "cargo"),
            Language::C => ("CC", "cc"),
            Language::Zig => ("ZIG", "zig"),
            Language::Go => ("GO", "go"),
        };
        std::env::var(var).unwrap_or_else(|_| default.into())
    }

    /// What to call the toolchain in an error message.
    fn tool(self) -> &'static str {
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
fn crate_name(stem: &str) -> String {
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
fn library_name(crate_name: &str) -> String {
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
fn classify_directory(dir: &Path) -> Result<Option<Language>> {
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
fn files_with_extension(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    files.sort();
    Ok(files)
}

/// The `apiplant-function` crate the generated crates depend on.
///
/// Prefers `APIPLANT_FUNCTION_CRATE` so a released binary can be pointed at a
/// checkout; otherwise falls back to the path this binary was compiled from,
/// which is what makes `cargo run -p apiplant -- build …` work in-tree.
fn function_crate_path() -> Result<PathBuf> {
    if let Some(configured) = std::env::var_os("APIPLANT_FUNCTION_CRATE") {
        let path = PathBuf::from(configured);
        if !path.join("Cargo.toml").is_file() {
            bail!(
                "APIPLANT_FUNCTION_CRATE points at {}, which has no Cargo.toml",
                path.display()
            );
        }
        return Ok(path);
    }

    let compiled_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../apiplant-function");
    if compiled_from.join("Cargo.toml").is_file() {
        return Ok(compiled_from);
    }

    bail!(
        "cannot find the `apiplant-function` crate to compile against.\n\
         Set APIPLANT_FUNCTION_CRATE to its directory in an apiplant checkout."
    )
}

/// The `Cargo.toml` for one generated crate.
///
/// The empty `[workspace]` table matters: without it cargo walks up the
/// directory tree and tries to enrol the scaffold in whatever workspace happens
/// to contain the app directory. It also means the profiles have to be spelled
/// out here — a detached crate inherits nothing from apiplant's own workspace.
fn manifest(crate_name: &str, function_crate: &Path) -> String {
    format!(
        "# Generated by `apiplant build` — edits are overwritten.\n\
         [package]\n\
         name = \"{crate_name}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [lib]\n\
         name = \"{crate_name}\"\n\
         path = \"src/lib.rs\"\n\
         crate-type = [\"cdylib\"]\n\
         \n\
         [dependencies]\n\
         apiplant-function = {{ path = {function_crate:?} }}\n\
         {DEPENDENCIES}\n\
         {PROFILES}\n\
         [workspace]\n"
    )
}

/// Whether `source` needs recompiling into `library`.
///
/// For a directory, *any* source file being newer than the library makes it
/// stale, so editing a second module or bumping a dependency triggers a rebuild.
fn is_stale(source: &Source, library: &Path) -> bool {
    if source.is_dir {
        return dir_is_stale(&source.path, library);
    }
    path_is_stale(&source.path, library)
}

/// Whether a single source file is newer than the library it produces.
fn path_is_stale(source: &Path, library: &Path) -> bool {
    match (modified(source), modified(library)) {
        (Some(src), Some(lib)) => src > lib,
        // No library yet (or unreadable timestamps): build it.
        _ => true,
    }
}

/// Whether any source under a function directory is newer than its library.
fn dir_is_stale(dir: &Path, library: &Path) -> bool {
    let Some(lib) = modified(library) else {
        // No library yet: build it.
        return true;
    };
    newest_source_mtime(dir).map_or(true, |newest| newest > lib)
}

/// The most recent modification time of any source file under `dir`.
///
/// Build outputs are skipped — a `target/` or `zig-out/` from a manual build, or
/// any dotfile — so a directory isn't judged stale against artifacts it produced,
/// only against the sources a human edits.
fn newest_source_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(name.as_ref(), "target" | "zig-out" | "zig-cache") || name.starts_with('.')
            {
                continue;
            }
            if let Some(inner) = newest_source_mtime(&path) {
                newest = Some(newest.map_or(inner, |cur| cur.max(inner)));
            }
        } else if let Some(m) = modified(&path) {
            newest = Some(newest.map_or(m, |cur| cur.max(m)));
        }
    }
    newest
}

/// The modification time of a path, if it can be read.
fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Options for [`build`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Compile with optimisations.
    pub release: bool,
    /// Rebuild even when the library is newer than its source.
    pub force: bool,
}

/// Compile every function source in `app_dir/functions/`, returning the names
/// of the libraries that were (re)built.
pub fn build(app_dir: &Path, options: Options) -> Result<Vec<String>> {
    let functions_dir = app_dir.join("functions");
    let sources = discover(&functions_dir)?;
    if sources.is_empty() {
        tracing::info!("no function sources in {}", functions_dir.display());
        return Ok(Vec::new());
    }

    let build_dir = app_dir.join(BUILD_DIR);
    let target_dir = build_dir.join("target");

    // Only single-file Rust sources need the `apiplant-function` crate injected —
    // a Rust *directory* brings its own Cargo.toml and depends on it there. So an
    // app made entirely of C functions, or of self-contained Rust crates, must not
    // fail for want of a checkout to build against.
    let needs_function_crate = sources
        .iter()
        .any(|s| s.language == Language::Rust && !s.is_dir);
    let function_crate = if needs_function_crate {
        Some(
            function_crate_path()?
                .canonicalize()
                .ok()
                .unwrap_or_else(|| function_crate_path().expect("checked above")),
        )
    } else {
        None
    };

    let mut built = Vec::new();
    for source in &sources {
        let library = functions_dir.join(source.library_name());
        if !options.force && !is_stale(source, &library) {
            tracing::info!(function = %source.crate_name, "up to date");
            continue;
        }

        tracing::info!(function = %source.crate_name, "compiling");
        match (source.language, source.is_dir) {
            // A single-file Rust source is wrapped in a generated cdylib crate,
            // then the library cargo produced (whose name we control) is copied
            // out of the shared target directory.
            (Language::Rust, false) => {
                let function_crate = function_crate.as_deref().expect("resolved above");
                compile_rust(source, &build_dir, &target_dir, function_crate, options)?;

                let profile = if options.release { "release" } else { "debug" };
                let produced = target_dir.join(profile).join(source.library_name());
                if !produced.is_file() {
                    bail!(
                        "cargo reported success but {} was not produced",
                        produced.display()
                    );
                }
                std::fs::copy(&produced, &library).with_context(|| {
                    format!("copying {} to {}", produced.display(), library.display())
                })?;
            }
            // A Rust *directory* is the user's own crate: build it as-is and copy
            // out whichever cdylib it produced (its lib name is theirs, not ours).
            (Language::Rust, true) => compile_rust_dir(source, &target_dir, &library, options)?,

            // `cc` and `zig` write straight to the destination — a single
            // translation unit needs no scaffolding, and a directory just widens
            // that to every source it holds. `go` needs a module built around a
            // lone file, so a file goes through `build_dir`; a directory already
            // *is* a module and builds in place.
            (Language::C, _) => compile_c(source, &library, options)?,
            (Language::Zig, _) => compile_zig(source, &library, &build_dir, options)?,
            (Language::Go, _) => compile_go(source, &library, &build_dir, options)?,
        }

        if !library.is_file() {
            bail!(
                "{} reported success but {} was not produced",
                source.language.tool(),
                library.display()
            );
        }
        tracing::info!(
            function = %source.crate_name,
            library = %library.display(),
            "built"
        );
        built.push(source.library_name());
    }
    Ok(built)
}

/// The directory holding `apiplant.h`, for compiling C functions.
///
/// Mirrors [`function_crate_path`]: an explicit override first, else the checkout
/// this binary was built from.
fn abi_include_path() -> Result<PathBuf> {
    if let Some(configured) = std::env::var_os("APIPLANT_ABI_INCLUDE") {
        let path = PathBuf::from(configured);
        if !path.join("apiplant.h").is_file() {
            bail!(
                "APIPLANT_ABI_INCLUDE points at {}, which has no apiplant.h",
                path.display()
            );
        }
        return Ok(path);
    }

    let compiled_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../apiplant-abi/include");
    if compiled_from.join("apiplant.h").is_file() {
        return Ok(compiled_from);
    }

    bail!(
        "cannot find `apiplant.h` to compile C functions against.\n\
         Set APIPLANT_ABI_INCLUDE to the directory holding it."
    )
}

/// Compile a C source — one file, or every `.c` in a directory — into `library`.
///
/// No scaffolding: a C function is a set of translation units implementing the
/// four exported symbols, so the compiler goes from source to shared library in
/// one step. A directory compiles every `.c` it holds together and is added to
/// the include path, so its own headers resolve. `CC` and `CFLAGS` are honoured
/// so an author can point at a different compiler or add their own includes and
/// libraries.
fn compile_c(source: &Source, library: &Path, options: Options) -> Result<()> {
    let include = abi_include_path()?;
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()));

    cc.arg(if cfg!(target_os = "macos") {
        "-dynamiclib"
    } else {
        "-shared"
    })
    // Required for a shared library on essentially every ELF target.
    .arg("-fPIC")
    .arg("-I")
    .arg(&include);

    // A directory's own headers live beside its sources.
    if source.is_dir {
        cc.arg("-I").arg(&source.path);
    }

    if options.release {
        cc.arg("-O2");
    } else {
        // Keep symbols and line numbers so a crash in a C function is legible;
        // the result is still tiny compared with a Rust cdylib.
        cc.arg("-O0").arg("-g");
    }

    if let Ok(flags) = std::env::var("CFLAGS") {
        cc.args(flags.split_whitespace());
    }

    cc.arg("-o").arg(library);

    if source.is_dir {
        let c_files = files_with_extension(&source.path, "c")?;
        if c_files.is_empty() {
            bail!("no .c files in {}", source.path.display());
        }
        cc.args(&c_files);
    } else {
        cc.arg(&source.path);
    }

    run(cc, source)
}

/// Compile a Zig source into `library`.
///
/// Like C, a Zig function exports the four ABI symbols, so no scaffolding is
/// needed. `@cImport`ing `apiplant.h` is what `-lc` and the include path are for.
/// A single file *is* the root; a directory has a [root file](zig_root) that may
/// `@import` the rest, so extra Zig modules just live beside it.
///
/// Both profiles keep Zig's **safety checks on**. `ReleaseFast`/`ReleaseSmall`
/// would shave a few KB, but a bounds or overflow check that silently becomes
/// undefined behaviour inside the host process is a far worse trade than the
/// difference between 8 KB and 4 KB. Note that a Zig safety failure *panics*,
/// which aborts the host — see the module docs on faults in C-ABI functions.
fn compile_zig(source: &Source, library: &Path, build_dir: &Path, options: Options) -> Result<()> {
    let include = abi_include_path()?;
    let root = if source.is_dir {
        zig_root(source)?
    } else {
        source.path.clone()
    };
    let mut zig = Command::new(Language::Zig.command());

    zig.arg("build-lib")
        .arg("-dynamic")
        // Zig functions reach the ABI through `@cImport`, which needs libc.
        .arg("-lc")
        .arg("-I")
        .arg(&include)
        // Keep zig's build cache inside .apiplant-build instead of dropping a
        // .zig-cache into the app's functions/ directory.
        .arg("--cache-dir")
        .arg(build_dir.join("zig-cache"))
        .arg("--name")
        .arg(&source.crate_name);

    if options.release {
        zig.arg("-O").arg("ReleaseSafe").arg("-fstrip");
    } else {
        // Debug keeps the safety checks *and* the stack traces that make a Zig
        // panic legible. It is bulky (~8 MB) because that machinery is compiled
        // in, not because of DWARF, so stripping would cost the traces and save
        // comparatively little. Release is what you ship, and that is ~8 KB.
        zig.arg("-O").arg("Debug");
    }

    if let Ok(flags) = std::env::var("ZIGFLAGS") {
        zig.args(flags.split_whitespace());
    }

    // `-femit-bin` names the output exactly, so nothing needs copying after.
    let mut emit = std::ffi::OsString::from("-femit-bin=");
    emit.push(library);
    zig.arg(emit).arg(&root);

    run(zig, source)
}

/// The root `.zig` file of a Zig directory — the one handed to `zig build-lib`,
/// from which any others are reached by `@import`.
///
/// The rule is small on purpose: a file named for the directory (`greet/greet.zig`)
/// is the root, and a lone `.zig` file is the root by default. Anything else is
/// ambiguous, and guessing an entry point is worse than asking.
fn zig_root(source: &Source) -> Result<PathBuf> {
    let zig_files = files_with_extension(&source.path, "zig")?;

    // `greet/greet.zig`, matched on the directory's real name (before it was
    // sanitised into a crate identifier).
    if let Some(dir_name) = source.path.file_name().and_then(|s| s.to_str()) {
        let named = source.path.join(format!("{dir_name}.zig"));
        if named.is_file() {
            return Ok(named);
        }
    }

    match zig_files.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("no .zig files in {}", source.path.display()),
        _ => bail!(
            "several .zig files in {} and none named `{}.zig`; \
             name the entry point after the directory",
            source.path.display(),
            source
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        ),
    }
}

/// Build a `.go` source into `library`.
///
/// Unlike C and Zig, `go build` insists on a module. A single `.go` file has
/// none, so this scaffolds one under `.apiplant-build/<name>/` — the same shape as
/// the generated cdylib crate for Rust. A directory already *is* a module (it has
/// the author's own `go.mod` and dependencies), so it builds in place.
///
/// Either way `-buildmode=c-shared` also emits a header next to its output, so the
/// build lands in `build_dir` and only the library is copied into `functions/` —
/// keeping the stray `.h` out of the app.
fn compile_go(source: &Source, library: &Path, build_dir: &Path, options: Options) -> Result<()> {
    let include = abi_include_path()?;

    // Where `go build` runs, and where its output (library + header) lands.
    let (module_dir, output_dir) = if source.is_dir {
        // The author's module builds in place; artifacts go to build_dir so the
        // .h never litters functions/.
        let out = build_dir.join(&source.crate_name);
        std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
        (source.path.clone(), out)
    } else {
        // A lone file is copied into a generated module and built there.
        let module_dir = build_dir.join(&source.crate_name);
        std::fs::create_dir_all(&module_dir)
            .with_context(|| format!("creating {}", module_dir.display()))?;

        let file_name = format!("{}.go", source.crate_name);
        std::fs::copy(&source.path, module_dir.join(&file_name))
            .with_context(|| format!("copying {}", source.path.display()))?;
        std::fs::write(module_dir.join("go.mod"), go_mod(&source.crate_name))
            .with_context(|| format!("writing {}/go.mod", module_dir.display()))?;
        (module_dir.clone(), module_dir)
    };

    // `go build` runs in `module_dir`, so `-o` must be absolute — a path relative
    // to the app directory would otherwise be resolved against the wrong cwd. The
    // output directory exists (just created), so canonicalising it can't fail.
    let produced = std::fs::canonicalize(&output_dir)
        .with_context(|| format!("resolving {}", output_dir.display()))?
        .join(source.library_name());
    let mut go = Command::new(Language::Go.command());
    go.current_dir(&module_dir)
        .arg("build")
        .arg("-buildmode=c-shared")
        .arg("-o")
        .arg(&produced)
        // cgo needs to find apiplant.h. Appending keeps any flags the author set.
        .env(
            "CGO_CFLAGS",
            match std::env::var("CGO_CFLAGS") {
                Ok(existing) => format!("-I{} {existing}", include.display()),
                Err(_) => format!("-I{}", include.display()),
            },
        )
        // The whole approach depends on cgo, so a cross-compiling environment
        // with it switched off should fail loudly here rather than mysteriously.
        .env("CGO_ENABLED", "1");

    if options.release {
        // Drop the symbol table and DWARF; a Go library is ~2 MB either way,
        // most of it runtime, but this takes a few hundred KB off. `-s -w` is one
        // argument to `-ldflags`, not two arguments to `go`.
        go.args(["-ldflags", "-s -w"]);
    }

    // The package to build goes last: `go` reads flags only until the first
    // non-flag argument, so anything after `.` is taken as another import path.
    go.arg(".");

    run(go, source)?;

    std::fs::copy(&produced, library)
        .with_context(|| format!("copying {} to {}", produced.display(), library.display()))?;
    Ok(())
}

/// `go.mod` for a generated Go function module.
fn go_mod(crate_name: &str) -> String {
    format!(
        "// Generated by `apiplant build` — edits are overwritten.\n\
         module {crate_name}\n\
         \n\
         go 1.21\n"
    )
}

/// Run a compiler and turn a non-zero exit into a useful error.
fn run(mut command: Command, source: &Source) -> Result<()> {
    let tool = source.language.tool();
    let status = command.status().with_context(|| {
        format!(
            "failed to run `{tool}` — apiplant compiles `{}` with it, \
             so it must be installed and on PATH",
            source
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        )
    })?;
    if !status.success() {
        bail!(
            "compiling {} failed; see {tool}'s output above",
            source.path.display()
        );
    }
    Ok(())
}

/// Scaffold and compile one Rust source file.
fn compile_rust(
    source: &Source,
    build_dir: &Path,
    target_dir: &Path,
    function_crate: &Path,
    options: Options,
) -> Result<()> {
    let crate_dir = build_dir.join(&source.crate_name);
    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir).with_context(|| format!("creating {}", src_dir.display()))?;

    // The function's own file becomes the crate's lib.rs verbatim.
    std::fs::copy(&source.path, src_dir.join("lib.rs"))
        .with_context(|| format!("copying {}", source.path.display()))?;
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&source.crate_name, function_crate),
    )
    .with_context(|| format!("writing {}/Cargo.toml", crate_dir.display()))?;

    let mut cargo = Command::new(Language::Rust.command());
    cargo
        .arg("build")
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir);
    if options.release {
        cargo.arg("--release");
    }

    let status = cargo.status().with_context(|| {
        "failed to run `cargo` — apiplant compiles function sources with it, \
         so it must be installed and on PATH"
    })?;
    if !status.success() {
        bail!(
            "compiling {} failed; see cargo's output above",
            source.path.display()
        );
    }
    Ok(())
}

/// Build a Rust *directory* — the author's own crate — into `library`.
///
/// Nothing is scaffolded here: the crate's `Cargo.toml` is run as written, so its
/// dependencies, modules and profiles are entirely the author's. The one thing
/// apiplant can't assume is what the crate *named* its cdylib, so cargo is asked
/// for structured output (`--message-format=json-render-diagnostics`, which still
/// prints human diagnostics to stderr) and the produced library is read back from
/// it and copied to `functions/lib<dir>.so`.
fn compile_rust_dir(
    source: &Source,
    target_dir: &Path,
    library: &Path,
    options: Options,
) -> Result<()> {
    let manifest_path = source.path.join("Cargo.toml");
    let mut cargo = Command::new(Language::Rust.command());
    cargo
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if options.release {
        cargo.arg("--release");
    }

    let mut child = cargo.spawn().with_context(|| {
        "failed to run `cargo` — apiplant compiles function sources with it, \
         so it must be installed and on PATH"
    })?;
    let mut json = String::new();
    child
        .stdout
        .take()
        .expect("stdout was piped")
        .read_to_string(&mut json)
        .context("reading cargo output")?;
    let status = child.wait().context("waiting for cargo")?;
    if !status.success() {
        bail!(
            "compiling {} failed; see cargo's output above",
            source.path.display()
        );
    }

    let produced = cdylib_from_cargo_output(&json).with_context(|| {
        format!(
            "cargo built {} but produced no cdylib — is `crate-type = [\"cdylib\"]` set?",
            source.path.display()
        )
    })?;
    std::fs::copy(&produced, library)
        .with_context(|| format!("copying {} to {}", produced.display(), library.display()))?;
    Ok(())
}

/// The cdylib a `cargo build --message-format=json` run produced.
///
/// Cargo emits one JSON object per line; a `compiler-artifact` message lists the
/// files a crate produced under `filenames`. The crate at the manifest path is
/// compiled last, so the last cdylib-shaped filename is the library we want.
fn cdylib_from_cargo_output(json: &str) -> Option<PathBuf> {
    let mut cdylib = None;
    for line in json.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let Some(filenames) = message.get("filenames").and_then(|f| f.as_array()) else {
            continue;
        };
        for filename in filenames {
            let Some(path) = filename.as_str() else {
                continue;
            };
            if is_dynamic_library(Path::new(path)) {
                cdylib = Some(PathBuf::from(path));
            }
        }
    }
    cdylib
}

/// Whether a filename looks like the dynamic library for this platform.
fn is_dynamic_library(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str());
    if cfg!(target_os = "windows") {
        ext == Some("dll")
    } else if cfg!(target_os = "macos") {
        ext == Some("dylib")
    } else {
        ext == Some("so")
    }
}

/// Function sources whose library is missing or older than the source.
///
/// Used to warn at boot rather than silently serving stale code.
pub fn stale(app_dir: &Path) -> Vec<String> {
    let functions_dir = app_dir.join("functions");
    discover(&functions_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|source| is_stale(source, &functions_dir.join(source.library_name())))
        .map(|source| source.crate_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "apiplant-compile-{label}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("functions")).unwrap();
        dir
    }

    #[test]
    fn crate_names_are_valid_identifiers() {
        assert_eq!(crate_name("greet"), "greet");
        assert_eq!(crate_name("post-hooks"), "post_hooks");
        assert_eq!(crate_name("Post Hooks"), "post_hooks");
        assert_eq!(crate_name("2fa"), "_2fa");
    }

    #[test]
    fn library_names_follow_the_platform_convention() {
        let name = library_name("greet");
        if cfg!(target_os = "windows") {
            assert_eq!(name, "greet.dll");
        } else if cfg!(target_os = "macos") {
            assert_eq!(name, "libgreet.dylib");
        } else {
            assert_eq!(name, "libgreet.so");
        }
    }

    #[test]
    fn discover_finds_only_sources_in_a_stable_order() {
        let dir = temp_dir("discover");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        std::fs::write(functions.join("audit.rs"), "// fn").unwrap();
        std::fs::write(functions.join("greet.toml"), "x = 1").unwrap();
        std::fs::write(functions.join("libold.so"), "binary").unwrap();
        // Not sources: a header a C function includes, and its config.
        std::fs::write(functions.join("shared.h"), "/* h */").unwrap();

        let found = discover(&functions).unwrap();
        let names: Vec<_> = found.iter().map(|s| s.crate_name.as_str()).collect();
        assert_eq!(names, vec!["audit", "greet"]);
        assert_eq!(found[1].library_name(), library_name("greet"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Functions in all four languages live side by side in one `functions/`
    /// directory, and each source has to be routed to the right toolchain.
    #[test]
    fn discover_tags_each_source_with_its_language() {
        let dir = temp_dir("languages");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        std::fs::write(functions.join("hello.c"), "/* fn */").unwrap();
        std::fs::write(functions.join("speedy.zig"), "// fn").unwrap();
        std::fs::write(functions.join("gopher.go"), "// fn").unwrap();
        // Neither of these is a source file.
        std::fs::write(functions.join("shared.h"), "/* h */").unwrap();
        std::fs::write(functions.join("go.mod"), "module x").unwrap();

        let found = discover(&functions).unwrap();
        let languages: Vec<_> = found
            .iter()
            .map(|s| (s.crate_name.as_str(), s.language))
            .collect();
        assert_eq!(
            languages,
            vec![
                ("gopher", Language::Go),
                ("greet", Language::Rust),
                ("hello", Language::C),
                ("speedy", Language::Zig),
            ]
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Each toolchain is overridable the way its ecosystem expects, so a pinned
    /// or vendored compiler can be used without patching apiplant.
    #[test]
    fn each_language_names_its_own_toolchain() {
        assert_eq!(Language::Rust.tool(), "cargo");
        assert_eq!(Language::Zig.tool(), "zig");
        assert_eq!(Language::Go.tool(), "go");

        // Defaults when nothing is set. `command()` reads the environment, so
        // only assert on a variable this test controls.
        std::env::set_var("ZIG", "/opt/zig-0.16/zig");
        assert_eq!(Language::Zig.command(), "/opt/zig-0.16/zig");
        std::env::remove_var("ZIG");
    }

    /// `go build` needs a module, so one is scaffolded next to the copied source.
    #[test]
    fn generated_go_module_is_minimal_and_named_after_the_function() {
        let go_mod = go_mod("post_hooks");
        assert!(go_mod.contains("module post_hooks"), "{go_mod}");
        assert!(go_mod.contains("go 1.21"), "{go_mod}");
        assert!(go_mod.starts_with("// Generated by"), "{go_mod}");
    }

    /// `greet.rs` and `greet.c` would both produce `libgreet.so`.
    #[test]
    fn discover_rejects_a_rust_and_a_c_source_with_the_same_stem() {
        let dir = temp_dir("cross-collide");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        std::fs::write(functions.join("greet.c"), "/* fn */").unwrap();

        let err = discover(&functions).unwrap_err().to_string();
        assert!(err.contains("rename one"), "{err}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn discover_tolerates_a_missing_functions_directory() {
        let dir = temp_dir("missing");
        assert!(discover(&dir.join("nowhere")).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn discover_rejects_sources_that_collide_on_one_library() {
        let dir = temp_dir("collide");
        let functions = dir.join("functions");
        std::fs::write(functions.join("post-hooks.rs"), "// fn").unwrap();
        std::fs::write(functions.join("post_hooks.rs"), "// fn").unwrap();

        let err = discover(&functions).unwrap_err().to_string();
        assert!(err.contains("rename one"), "{err}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staleness_tracks_source_and_library_timestamps() {
        let dir = temp_dir("stale");
        let functions = dir.join("functions");
        let source = functions.join("greet.rs");
        let library = functions.join(library_name("greet"));

        std::fs::write(&source, "// fn").unwrap();
        assert!(path_is_stale(&source, &library), "missing library is stale");

        std::fs::write(&library, "binary").unwrap();
        // Give the library a clearly later mtime than the source.
        let later = SystemTime::now() + Duration::from_secs(10);
        filetime_set(&library, later);
        assert!(!path_is_stale(&source, &library));
        assert_eq!(stale(&dir), Vec::<String>::new());

        // Touching the source makes it stale again.
        let even_later = SystemTime::now() + Duration::from_secs(20);
        filetime_set(&source, even_later);
        assert!(path_is_stale(&source, &library));
        assert_eq!(stale(&dir), vec!["greet".to_string()]);

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Set a file's mtime without pulling in a dependency.
    fn filetime_set(path: &Path, time: SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    #[test]
    fn generated_manifest_is_a_standalone_cdylib_crate() {
        let toml = manifest("post_hooks", Path::new("/src/apiplant-function"));

        assert!(toml.contains("name = \"post_hooks\""));
        assert!(toml.contains("crate-type = [\"cdylib\"]"));
        assert!(toml.contains("path = \"src/lib.rs\""));
        assert!(toml.contains("apiplant-function = { path = \"/src/apiplant-function\" }"));
        assert!(toml.contains("abi_stable"));
        // Detaches the scaffold from any surrounding workspace.
        assert!(toml.contains("[workspace]"));

        // It must parse as TOML, or cargo will reject it.
        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(parsed["package"]["name"].as_str(), Some("post_hooks"));
        assert_eq!(parsed["lib"]["crate-type"][0].as_str(), Some("cdylib"));
    }

    /// The generated crate is detached from every workspace, so it inherits no
    /// profiles — these have to travel in the manifest or the libraries ship
    /// with cargo's defaults, which for a `dev` build is ~94% debug info.
    #[test]
    fn generated_manifest_keeps_libraries_small() {
        let toml = manifest("greet", Path::new("/src/apiplant-function"));
        let parsed: toml::Value = toml::from_str(&toml).unwrap();

        // `dev` sheds DWARF but keeps the symbol table for legible backtraces.
        let dev = &parsed["profile"]["dev"];
        assert_eq!(dev["strip"].as_str(), Some("debuginfo"));

        // `release` strips fully, and fat LTO over one codegen unit lets the
        // linker drop the unreached parts of std/serde_json/schemars.
        let release = &parsed["profile"]["release"];
        assert_eq!(release["strip"].as_str(), Some("symbols"));
        assert_eq!(release["lto"].as_str(), Some("fat"));
        assert_eq!(release["codegen-units"].as_integer(), Some(1));

        // Panics must keep unwinding: the host turns a panicking function into a
        // 500 rather than letting it abort the whole server.
        assert!(release.get("panic").is_none());
    }

    /// A directory is a function too, classified by what it holds, and named for
    /// the directory rather than a file stem.
    #[test]
    fn discover_classifies_directories_by_their_contents() {
        let dir = temp_dir("dirs");
        let functions = dir.join("functions");

        // A Rust crate.
        let rusty = functions.join("rusty");
        std::fs::create_dir_all(rusty.join("src")).unwrap();
        std::fs::write(rusty.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(rusty.join("src/lib.rs"), "// fn").unwrap();

        // A Go module.
        let gopher = functions.join("gopher");
        std::fs::create_dir_all(&gopher).unwrap();
        std::fs::write(gopher.join("go.mod"), "module gopher").unwrap();
        std::fs::write(gopher.join("main.go"), "// fn").unwrap();

        // Loose C sources, no manifest.
        let cee = functions.join("cee");
        std::fs::create_dir_all(&cee).unwrap();
        std::fs::write(cee.join("hello.c"), "/* fn */").unwrap();
        std::fs::write(cee.join("helper.c"), "/* fn */").unwrap();

        // Loose Zig sources.
        let ziggy = functions.join("ziggy");
        std::fs::create_dir_all(&ziggy).unwrap();
        std::fs::write(ziggy.join("ziggy.zig"), "// fn").unwrap();

        // A directory with nothing recognisable is ignored, like a stray file.
        std::fs::create_dir_all(functions.join("notes")).unwrap();
        std::fs::write(functions.join("notes/todo.txt"), "later").unwrap();

        let found = discover(&functions).unwrap();
        let seen: Vec<_> = found
            .iter()
            .map(|s| (s.crate_name.as_str(), s.language, s.is_dir))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("cee", Language::C, true),
                ("gopher", Language::Go, true),
                ("rusty", Language::Rust, true),
                ("ziggy", Language::Zig, true),
            ]
        );
        assert_eq!(found[2].library_name(), library_name("rusty"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A directory of Rust or Go sources without its manifest can't be built the
    /// native way, and mixing C and Zig has no single answer — each is an error
    /// with an actionable fix rather than a silent guess.
    #[test]
    fn directory_classification_rejects_the_ambiguous_cases() {
        let dir = temp_dir("dir-errors");
        let functions = dir.join("functions");

        let no_cargo = functions.join("no_cargo");
        std::fs::create_dir_all(&no_cargo).unwrap();
        std::fs::write(no_cargo.join("lib.rs"), "// fn").unwrap();
        let err = classify_directory(&no_cargo).unwrap_err().to_string();
        assert!(err.contains("Cargo.toml"), "{err}");

        let no_mod = functions.join("no_mod");
        std::fs::create_dir_all(&no_mod).unwrap();
        std::fs::write(no_mod.join("main.go"), "// fn").unwrap();
        let err = classify_directory(&no_mod).unwrap_err().to_string();
        assert!(err.contains("go.mod"), "{err}");

        let mixed = functions.join("mixed");
        std::fs::create_dir_all(&mixed).unwrap();
        std::fs::write(mixed.join("a.c"), "/* */").unwrap();
        std::fs::write(mixed.join("b.zig"), "// ").unwrap();
        let err = classify_directory(&mixed).unwrap_err().to_string();
        assert!(err.contains(".c and .zig"), "{err}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A `greet/` directory and a `greet.rs` file would both produce `libgreet.so`.
    #[test]
    fn discover_rejects_a_directory_colliding_with_a_file() {
        let dir = temp_dir("dir-collide");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        let greet = functions.join("greet");
        std::fs::create_dir_all(&greet).unwrap();
        std::fs::write(greet.join("Cargo.toml"), "[package]").unwrap();

        let err = discover(&functions).unwrap_err().to_string();
        assert!(err.contains("rename one"), "{err}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The Zig root is the file named for the directory, or the only `.zig` file;
    /// anything else is ambiguous.
    #[test]
    fn zig_root_prefers_the_file_named_for_the_directory() {
        let dir = temp_dir("zig-root");
        let functions = dir.join("functions");

        // A lone file is the root whatever it is called.
        let lone = functions.join("lone");
        std::fs::create_dir_all(&lone).unwrap();
        std::fs::write(lone.join("whatever.zig"), "// fn").unwrap();
        let src = Source {
            path: lone.clone(),
            crate_name: "lone".into(),
            language: Language::Zig,
            is_dir: true,
        };
        assert_eq!(zig_root(&src).unwrap(), lone.join("whatever.zig"));

        // With several, the one named for the directory wins.
        let many = functions.join("many");
        std::fs::create_dir_all(&many).unwrap();
        std::fs::write(many.join("many.zig"), "// root").unwrap();
        std::fs::write(many.join("helper.zig"), "// dep").unwrap();
        let src = Source {
            path: many.clone(),
            crate_name: "many".into(),
            language: Language::Zig,
            is_dir: true,
        };
        assert_eq!(zig_root(&src).unwrap(), many.join("many.zig"));

        // Several with no obvious root is an error.
        let ambiguous = functions.join("ambiguous");
        std::fs::create_dir_all(&ambiguous).unwrap();
        std::fs::write(ambiguous.join("a.zig"), "//").unwrap();
        std::fs::write(ambiguous.join("b.zig"), "//").unwrap();
        let src = Source {
            path: ambiguous.clone(),
            crate_name: "ambiguous".into(),
            language: Language::Zig,
            is_dir: true,
        };
        assert!(zig_root(&src).is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The produced library is read back from cargo's JSON output, since an
    /// author's crate names its own cdylib.
    #[test]
    fn cdylib_is_read_from_cargo_json() {
        let so = library_name("greet");
        let json = format!(
            "{{\"reason\":\"compiler-artifact\",\"target\":{{\"name\":\"dep\"}},\
               \"filenames\":[\"/t/debug/deps/libdep.rlib\"]}}\n\
             {{\"reason\":\"build-script-executed\"}}\n\
             {{\"reason\":\"compiler-artifact\",\"target\":{{\"name\":\"greet\"}},\
               \"filenames\":[\"/t/debug/{so}\"]}}\n\
             {{\"reason\":\"build-finished\",\"success\":true}}\n"
        );
        assert_eq!(
            cdylib_from_cargo_output(&json),
            Some(PathBuf::from(format!("/t/debug/{so}")))
        );

        // A crate that produced no cdylib (e.g. crate-type left as rlib).
        let none = "{\"reason\":\"compiler-artifact\",\
             \"filenames\":[\"/t/debug/libgreet.rlib\"]}\n";
        assert_eq!(cdylib_from_cargo_output(none), None);
    }

    /// A directory is stale when any source under it — not just a top-level file —
    /// is newer than the library, and build outputs never count.
    #[test]
    fn a_directory_is_stale_when_any_nested_source_changes() {
        let dir = temp_dir("dir-stale");
        let functions = dir.join("functions");
        let crate_dir = functions.join("rusty");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(crate_dir.join("src/lib.rs"), "// fn").unwrap();

        let source = Source {
            path: crate_dir.clone(),
            crate_name: "rusty".into(),
            language: Language::Rust,
            is_dir: true,
        };
        let library = functions.join(source.library_name());

        assert!(is_stale(&source, &library), "missing library is stale");

        std::fs::write(&library, "binary").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        filetime_set(&library, later);
        // A build artifact newer than the library must not count as a source.
        std::fs::create_dir_all(crate_dir.join("target")).unwrap();
        std::fs::write(crate_dir.join("target/junk"), "art").unwrap();
        filetime_set(
            &crate_dir.join("target/junk"),
            SystemTime::now() + Duration::from_secs(120),
        );
        assert!(
            !is_stale(&source, &library),
            "artifacts don't make it stale"
        );

        // Editing a nested source does.
        filetime_set(
            &crate_dir.join("src/lib.rs"),
            SystemTime::now() + Duration::from_secs(180),
        );
        assert!(is_stale(&source, &library));
        assert_eq!(stale(&dir), vec!["rusty".to_string()]);

        std::fs::remove_dir_all(dir).unwrap();
    }
}
