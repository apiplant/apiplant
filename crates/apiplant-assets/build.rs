//! Bake the two front-end builds into the crate.
//!
//! `admin/dist` and `studio/dist` are walked at compile time and turned into a
//! sorted `&[(path, bytes)]` table each, so the shipped binary can serve either
//! interface without a directory of files next to it.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.join("../..");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    let mut code = String::new();
    emit(&mut code, "ADMIN", &repo_root.join("admin/dist"), "admin");
    emit(
        &mut code,
        "STUDIO",
        &repo_root.join("studio/dist"),
        "studio",
    );

    fs::write(out_dir.join("embedded.rs"), code).expect("write embedded.rs");
}

fn emit(code: &mut String, name: &str, dist: &Path, package: &str) {
    println!("cargo:rerun-if-changed={}", dist.display());

    if !dist.join("index.html").is_file() {
        panic!(
            "{}/index.html is missing — run `pnpm build` in {}/",
            dist.display(),
            package
        );
    }

    let mut files = Vec::new();
    collect(dist, dist, &mut files);
    files.sort();

    writeln!(
        code,
        "pub static {name}: &[(&str, &[u8])] = &[\n{}];",
        files
            .iter()
            .map(|(relative, absolute)| format!(
                "    ({:?}, include_bytes!({:?})),\n",
                relative, absolute
            ))
            .collect::<String>()
    )
    .expect("write asset table");
}

/// Collect `(url path, absolute path)` for every file under `dir`.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => panic!("failed to read {}: {error}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("path under root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        files.push((relative, path.to_string_lossy().into_owned()));
    }
}
