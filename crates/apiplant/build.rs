use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let admin_dist_dir = manifest_dir.join("../../admin/dist");

    let index_path = admin_dist_dir.join("index.html");
    let js_path = admin_dist_dir.join("app.js");
    let css_path = admin_dist_dir.join("app.css");
    if !index_path.is_file() || !js_path.is_file() || !css_path.is_file() {
        panic!("admin/dist/index.html, app.js and app.css must exist; run `pnpm build` in admin/");
    }

    println!(
        "cargo:rustc-env=APIPLANT_ADMIN_INDEX_PATH={}",
        index_path.display()
    );
    println!(
        "cargo:rustc-env=APIPLANT_ADMIN_JS_PATH={}",
        js_path.display()
    );
    println!(
        "cargo:rustc-env=APIPLANT_ADMIN_CSS_PATH={}",
        css_path.display()
    );
    println!("cargo:rerun-if-changed={}", admin_dist_dir.display());
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
}
