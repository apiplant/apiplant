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
//! Four of the five compile to a shared library, and the difference between them
//! is only how much scaffolding the toolchain insists on. The fifth, TypeScript,
//! has nothing to link: it is transpiled here and run in a V8 isolate by the
//! server (see `apiplant_js`), so what lands beside the source is `greet.js`.
//!
//! | Source | Built with | Scaffolding |
//! |--------|------------|-------------|
//! | `.rs`  | `cargo build` | a generated cdylib crate |
//! | `.c`   | `cc -shared -fPIC` | none |
//! | `.zig` | `zig build-lib -dynamic -lc` | none |
//! | `.go`  | `go build -buildmode=c-shared` | a generated `go.mod` |
//! | `.ts`  | transpiled in-process (oxc) | none - output is `greet.js` |
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
//! | `package.json`          | TypeScript | *your* `build` script — npm deps, your own bundler |
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
//! TypeScript needs none of that: no toolchain, and nothing to override.

mod native;
mod rust;
mod source;
// The slim build swaps in a stand-in that refuses rather than compiles, so the
// dispatch further down stays one shape. See `typescript_absent.rs`.
#[cfg(feature = "typescript")]
mod typescript;
#[cfg(not(feature = "typescript"))]
#[path = "typescript_absent.rs"]
mod typescript;

use native::{compile_c, compile_go, compile_zig};
use rust::{compile_rust, compile_rust_dir, function_crate, FunctionCrate};
pub use source::{discover, Language, Source};
use typescript::{compile_typescript, compile_typescript_dir, write_declarations};

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Stop a slim build before it starts compiling anything.
///
/// A per-file error would come out halfway through a build that had already
/// produced half an app's libraries; this names every TypeScript source at once
/// and produces nothing. The full build compiles them, so this is a no-op.
#[cfg(feature = "typescript")]
fn refuse_typescript(_sources: &[Source]) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "typescript"))]
fn refuse_typescript(sources: &[Source]) -> Result<()> {
    let names: Vec<String> = sources
        .iter()
        .filter(|s| s.language == Language::TypeScript)
        .map(|s| s.path.display().to_string())
        .collect();
    bail!(
        "this is a slim build of apiplant, which has no TypeScript support, and \
         these functions are TypeScript:\n  {}\n\
         Use a full build to compile them, or take them out of functions/.",
        names.join("\n  ")
    )
}

/// Where the scaffolding and cargo's target directory live, inside the app.
const BUILD_DIR: &str = ".apiplant-build";
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
            // Build output and installed dependencies: `node_modules` is also
            // the one that matters for speed, since walking it would mean
            // thousands of files on every staleness check.
            if matches!(
                name.as_ref(),
                "target" | "zig-out" | "zig-cache" | "node_modules" | "dist"
            ) || name.starts_with('.')
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
    // app made entirely of C functions, or of self-contained Rust crates, does no
    // work here at all.
    let needs_function_crate = sources
        .iter()
        .any(|s| s.language == Language::Rust && !s.is_dir);
    let function_crate = if needs_function_crate {
        // A path dependency is resolved relative to the generated crate, which
        // lives somewhere under the app directory — so it has to be absolute.
        Some(match function_crate()? {
            FunctionCrate::Local(path) => FunctionCrate::Local(path.canonicalize().unwrap_or(path)),
            published => published,
        })
    } else {
        None
    };

    // Editors need the `ctx` types before anything is built, so this is written
    // whether or not a function turned out to be stale.
    if sources.iter().any(|s| s.language == Language::TypeScript) {
        refuse_typescript(&sources)?;
        write_declarations(&functions_dir)?;
    }

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
                let function_crate = function_crate.as_ref().expect("resolved above");
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
            (Language::C, _) => compile_c(source, &library, &build_dir, options)?,
            (Language::Zig, _) => compile_zig(source, &library, &build_dir, options)?,
            (Language::Go, _) => compile_go(source, &library, &build_dir, options)?,

            // A single TypeScript file is transpiled in-process: no toolchain,
            // no scaffolding. A directory is an npm project, so it is built by
            // its own bundler, like a Rust directory is by its own cargo. Either
            // way what lands beside the source is JavaScript rather than a
            // shared library, and the server runs it in a V8 isolate.
            (Language::TypeScript, false) => compile_typescript(source, &library)?,
            (Language::TypeScript, true) => compile_typescript_dir(source, &library, options)?,
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

/// Run a compiler and turn a non-zero exit into a useful error.
pub(super) fn run(mut command: Command, source: &Source) -> Result<()> {
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
    use std::path::PathBuf;

    use super::native::{go_mod, zig_root};
    use super::rust::{cdylib_from_cargo_output, manifest};
    use super::source::{classify_directory, crate_name, library_name};
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

    /// Functions in every language live side by side in one `functions/`
    /// directory, and each source has to be routed to the right toolchain.
    #[test]
    fn discover_tags_each_source_with_its_language() {
        let dir = temp_dir("languages");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        std::fs::write(functions.join("hello.c"), "/* fn */").unwrap();
        std::fs::write(functions.join("speedy.zig"), "// fn").unwrap();
        std::fs::write(functions.join("gopher.go"), "// fn").unwrap();
        std::fs::write(functions.join("scripty.ts"), "// fn").unwrap();
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
                ("scripty", Language::TypeScript),
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
        // TypeScript is the exception: nothing external builds it, so there is
        // no variable to point somewhere else.
        assert_eq!(Language::TypeScript.command(), "apiplant build");

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
        let toml = manifest(
            "post_hooks",
            &FunctionCrate::Local(PathBuf::from("/src/apiplant-function")),
        );

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
        let toml = manifest(
            "greet",
            &FunctionCrate::Local(PathBuf::from("/src/apiplant-function")),
        );
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

    /// With no checkout to point at — an installed binary — the generated crate
    /// depends on the published `apiplant-function` at the binary's own version,
    /// which the workspace keeps in lockstep with every other apiplant crate.
    #[test]
    fn generated_manifest_falls_back_to_the_published_crate() {
        let toml = manifest("greet", &FunctionCrate::Published("0.3.0"));
        assert!(toml.contains("apiplant-function = \"0.3.0\""));

        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(
            parsed["dependencies"]["apiplant-function"].as_str(),
            Some("0.3.0")
        );
    }

    /// TypeScript is the one language whose artifact is not a shared library, so
    /// staleness, loading and `apiplant build`'s output all key off a `.js`.
    #[cfg(feature = "typescript")]
    #[test]
    fn a_typescript_source_builds_a_js_beside_it() {
        let dir = temp_dir("typescript");
        let functions = dir.join("functions");
        std::fs::write(
            functions.join("greet.ts"),
            "export const manifest = [{ name: \"greet\", permission: \"public\" }];\n\
             export function greet(input: { name: string }) { return { hi: input.name }; }\n",
        )
        .unwrap();

        let found = discover(&functions).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].language, Language::TypeScript);
        assert_eq!(found[0].library_name(), "greet.js");

        let built = build(&dir, Options::default()).unwrap();
        assert_eq!(built, vec!["greet.js".to_string()]);

        let js = std::fs::read_to_string(functions.join("greet.js")).unwrap();
        assert!(js.contains("export function greet(input)"), "{js}");
        // The types are gone, and so is any doubt about where the file came from.
        assert!(!js.contains(": { name: string }"), "{js}");
        assert!(js.starts_with("// Generated by `apiplant build`"), "{js}");

        // Editors need the ambient types and the libraries an isolate has, so
        // both are written too.
        assert!(functions.join("apiplant.d.ts").is_file());
        let tsconfig = functions.join("tsconfig.json");
        assert!(tsconfig.is_file());

        // The tsconfig is the author's once it exists: a second build leaves
        // whatever they changed alone.
        std::fs::write(&tsconfig, "{ \"mine\": true }").unwrap();
        build(
            &dir,
            Options {
                force: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&tsconfig).unwrap(),
            "{ \"mine\": true }"
        );

        // Nothing is stale immediately after a build, and the declarations file
        // must not be mistaken for a function of its own.
        assert!(stale(&dir).is_empty(), "{:?}", stale(&dir));

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A `.ts` that cannot be transpiled fails the build with the file named,
    /// rather than producing a `.js` that fails at boot.
    #[cfg(feature = "typescript")]
    #[test]
    fn a_broken_typescript_source_fails_the_build() {
        let dir = temp_dir("typescript-broken");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.ts"), "export function greet( {").unwrap();

        let err = build(&dir, Options::default()).unwrap_err().to_string();
        assert!(err.contains("greet.ts"), "{err}");
        assert!(!functions.join("greet.js").exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A TypeScript *directory* is an npm project, and `package.json` is what
    /// says so — the same role `Cargo.toml` and `go.mod` play for the others.
    #[test]
    fn a_typescript_directory_is_recognised_by_its_package_json() {
        let dir = temp_dir("typescript-dir");
        let functions = dir.join("functions");

        let slug = functions.join("slug");
        std::fs::create_dir_all(slug.join("src")).unwrap();
        std::fs::write(slug.join("package.json"), r#"{"name":"slug"}"#).unwrap();
        std::fs::write(slug.join("src/index.ts"), "export default {};").unwrap();

        let found = discover(&functions).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].language, Language::TypeScript);
        assert!(found[0].is_dir);
        // Still a `.js` beside the directory, loaded like any other function.
        assert_eq!(found[0].library_name(), "slug.js");

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Loose `.ts` files with nothing to build them is the one case worth an
    /// error: there is no entry point to guess and no bundler to run.
    #[test]
    fn a_typescript_directory_without_a_package_json_says_so() {
        let dir = temp_dir("typescript-dir-bare");
        let functions = dir.join("functions");
        let loose = functions.join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("index.ts"), "export default {};").unwrap();

        let err = discover(&functions).unwrap_err().to_string();
        assert!(err.contains("package.json"), "{err}");
        assert!(err.contains("single `.ts` file"), "{err}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Where the bundle is looked for. `module` wins because a package that
    /// distinguishes the two puts its ESM build there, and the isolate loads ESM.
    #[cfg(feature = "typescript")]
    #[test]
    fn the_bundle_is_found_where_the_package_says() {
        use super::typescript::bundle_path;
        let dir = PathBuf::from("/app/functions/slug");

        let declared = serde_json::json!({ "module": "dist/slug.js", "main": "dist/slug.cjs" });
        assert_eq!(
            bundle_path(&dir, &declared, "slug"),
            dir.join("dist/slug.js")
        );

        let main_only = serde_json::json!({ "main": "build/out.js" });
        assert_eq!(
            bundle_path(&dir, &main_only, "slug"),
            dir.join("build/out.js")
        );

        // Neither: the conventional path, so a minimal package.json still works.
        let silent = serde_json::json!({ "name": "slug" });
        assert_eq!(bundle_path(&dir, &silent, "slug"), dir.join("dist/slug.js"));
    }

    /// The package manager comes from the lockfile the project already has, so
    /// there is nothing to configure and nothing to get out of step.
    #[cfg(feature = "typescript")]
    #[test]
    fn the_package_manager_is_read_from_the_lockfile() {
        use super::typescript::PackageManager;
        let dir = temp_dir("package-manager");

        // No lockfile yet: pnpm, which is also what a fresh install produces.
        assert_eq!(PackageManager::detect(&dir), PackageManager::Pnpm);

        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        assert_eq!(PackageManager::detect(&dir), PackageManager::Npm);
        assert_eq!(PackageManager::detect(&dir).command(), "npm");

        // An explicit choice outranks whatever is lying in the directory.
        std::env::set_var("NODE_PACKAGE_MANAGER", "bun");
        assert_eq!(PackageManager::detect(&dir), PackageManager::Bun);
        std::env::remove_var("NODE_PACKAGE_MANAGER");

        std::fs::remove_dir_all(dir).unwrap();
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
