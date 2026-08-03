//! Building C, Zig and Go functions — everything that targets the plain C ABI
//! declared in `apiplant.h`.
//!
//! C and Zig go straight from source to shared library with no scaffolding; Go
//! gets a generated `go.mod`, because `go build` will not work without one.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::source::{files_with_extension, Language, Source};
use super::{run, Options};

/// The directory holding `apiplant.h`, for compiling C functions.
///
/// Mirrors [`super::rust::function_crate`]: an explicit override first, then the
/// checkout this binary was built from, and finally the copy embedded in the
/// binary — written into the build directory so there is a path to hand `-I`.
pub(super) fn abi_include_path(build_dir: &Path) -> Result<PathBuf> {
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

    // No checkout to point at: materialise the embedded header. Rewritten only
    // when it differs, so a repeat build does not touch the file (and does not
    // make every C function look stale).
    let include = build_dir.join("include");
    let header = include.join("apiplant.h");
    if std::fs::read_to_string(&header).ok().as_deref() != Some(apiplant_abi::HEADER) {
        std::fs::create_dir_all(&include)
            .with_context(|| format!("creating {}", include.display()))?;
        std::fs::write(&header, apiplant_abi::HEADER)
            .with_context(|| format!("writing {}", header.display()))?;
    }
    Ok(include)
}

/// Compile a C source — one file, or every `.c` in a directory — into `library`.
///
/// No scaffolding: a C function is a set of translation units implementing the
/// four exported symbols, so the compiler goes from source to shared library in
/// one step. A directory compiles every `.c` it holds together and is added to
/// the include path, so its own headers resolve. `CC` and `CFLAGS` are honoured
/// so an author can point at a different compiler or add their own includes and
/// libraries.
pub(super) fn compile_c(
    source: &Source,
    library: &Path,
    build_dir: &Path,
    options: Options,
) -> Result<()> {
    let include = abi_include_path(build_dir)?;
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
pub(super) fn compile_zig(
    source: &Source,
    library: &Path,
    build_dir: &Path,
    options: Options,
) -> Result<()> {
    let include = abi_include_path(build_dir)?;
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
pub(super) fn zig_root(source: &Source) -> Result<PathBuf> {
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
pub(super) fn compile_go(
    source: &Source,
    library: &Path,
    build_dir: &Path,
    options: Options,
) -> Result<()> {
    let include = abi_include_path(build_dir)?;

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
pub(super) fn go_mod(crate_name: &str) -> String {
    format!(
        "// Generated by `apiplant build` — edits are overwritten.\n\
         module {crate_name}\n\
         \n\
         go 1.21\n"
    )
}
