//! Compiling function sources into loadable libraries.
//!
//! An app keeps its functions as **plain `.rs` files** next to their config:
//!
//! ```text
//! my-app/functions/
//! ├── greet.rs          # source: one file, written as if it were a lib.rs
//! ├── greet.toml        # that function's config
//! └── libgreet.so       # ← produced by `apiplant build`
//! ```
//!
//! `apiplant build` scaffolds a throwaway cdylib crate around each source file
//! and hands it to `cargo`, then copies the resulting library back beside the
//! source. The scaffolding lives in `.apiplant-build/` inside the app directory,
//! with one shared `target/` so dependencies are compiled once and rebuilds stay
//! incremental.
//!
//! The only requirement is a working `cargo` on the machine.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// One function source found in an app's `functions/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The `.rs` file itself.
    pub path: PathBuf,
    /// Crate name derived from the file stem (`post-hooks.rs` → `post_hooks`).
    pub crate_name: String,
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
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", functions_dir.display()))
        }
    };

    let mut sources: Vec<Source> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        sources.push(Source {
            crate_name: crate_name(stem),
            path,
        });
    }
    sources.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

    // Two files that differ only in punctuation would fight over one .so.
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
/// to contain the app directory.
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
         [workspace]\n"
    )
}

/// Whether `source` needs recompiling into `library`.
fn is_stale(source: &Path, library: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(source), modified(library)) {
        (Some(src), Some(lib)) => src > lib,
        // No library yet (or unreadable timestamps): build it.
        _ => true,
    }
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

    let function_crate = function_crate_path()?.canonicalize().ok().unwrap_or_else(
        || function_crate_path().expect("checked above"),
    );
    let build_dir = app_dir.join(BUILD_DIR);
    let target_dir = build_dir.join("target");

    let mut built = Vec::new();
    for source in &sources {
        let library = functions_dir.join(source.library_name());
        if !options.force && !is_stale(&source.path, &library) {
            tracing::info!(function = %source.crate_name, "up to date");
            continue;
        }

        tracing::info!(function = %source.crate_name, "compiling");
        compile_one(source, &build_dir, &target_dir, &function_crate, options)?;

        let profile = if options.release { "release" } else { "debug" };
        let produced = target_dir.join(profile).join(source.library_name());
        if !produced.is_file() {
            bail!(
                "cargo reported success but {} was not produced",
                produced.display()
            );
        }
        std::fs::copy(&produced, &library)
            .with_context(|| format!("copying {} to {}", produced.display(), library.display()))?;
        tracing::info!(
            function = %source.crate_name,
            library = %library.display(),
            "built"
        );
        built.push(source.library_name());
    }
    Ok(built)
}

/// Scaffold and compile one source file.
fn compile_one(
    source: &Source,
    build_dir: &Path,
    target_dir: &Path,
    function_crate: &Path,
    options: Options,
) -> Result<()> {
    let crate_dir = build_dir.join(&source.crate_name);
    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir)
        .with_context(|| format!("creating {}", src_dir.display()))?;

    // The function's own file becomes the crate's lib.rs verbatim.
    std::fs::copy(&source.path, src_dir.join("lib.rs"))
        .with_context(|| format!("copying {}", source.path.display()))?;
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&source.crate_name, function_crate),
    )
    .with_context(|| format!("writing {}/Cargo.toml", crate_dir.display()))?;

    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
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

/// Function sources whose library is missing or older than the source.
///
/// Used to warn at boot rather than silently serving stale code.
pub fn stale(app_dir: &Path) -> Vec<String> {
    let functions_dir = app_dir.join("functions");
    discover(&functions_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|source| is_stale(&source.path, &functions_dir.join(source.library_name())))
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
    fn discover_finds_only_rust_sources_in_a_stable_order() {
        let dir = temp_dir("discover");
        let functions = dir.join("functions");
        std::fs::write(functions.join("greet.rs"), "// fn").unwrap();
        std::fs::write(functions.join("audit.rs"), "// fn").unwrap();
        std::fs::write(functions.join("greet.toml"), "x = 1").unwrap();
        std::fs::write(functions.join("libold.so"), "binary").unwrap();

        let found = discover(&functions).unwrap();
        let names: Vec<_> = found.iter().map(|s| s.crate_name.as_str()).collect();
        assert_eq!(names, vec!["audit", "greet"]);
        assert_eq!(found[1].library_name(), library_name("greet"));

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
        assert!(is_stale(&source, &library), "missing library is stale");

        std::fs::write(&library, "binary").unwrap();
        // Give the library a clearly later mtime than the source.
        let later = SystemTime::now() + Duration::from_secs(10);
        filetime_set(&library, later);
        assert!(!is_stale(&source, &library));
        assert_eq!(stale(&dir), Vec::<String>::new());

        // Touching the source makes it stale again.
        let even_later = SystemTime::now() + Duration::from_secs(20);
        filetime_set(&source, even_later);
        assert!(is_stale(&source, &library));
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
}
