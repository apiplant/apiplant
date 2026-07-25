use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let assets_dir = manifest_dir.join("../../studio/dist/assets");

    let css_path = fs::read_dir(&assets_dir)
        .expect("studio/dist/assets must exist")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("css"))
        .expect("studio/dist/assets must contain a built CSS file");

    println!(
        "cargo:rustc-env=APIPLANT_STUDIO_CSS_PATH={}",
        css_path.display()
    );
    println!("cargo:rerun-if-changed={}", assets_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("../../studio/public/head.png").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir
            .join("../../studio/public/head-inverted.png")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/admin/index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/admin/app.js").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/admin/extra.css").display()
    );
}
