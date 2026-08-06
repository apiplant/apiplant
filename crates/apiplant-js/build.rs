//! Builds the V8 startup snapshot every isolate begins from.
//!
//! ## Why a snapshot is required, not merely nice
//!
//! `deno_web` declares its JS — `TextEncoder`, `URL`, the streams, all of it —
//! as `lazy_loaded_js`. deno_core records those sources by **absolute path** and
//! reads them from disk when an isolate wants them; the bytes are never embedded
//! in the binary. In a checkout that works, because the path points into
//! `~/.cargo/registry` and the files are right there. In a released binary it
//! points at the CI runner's home directory and every Web global fails to load.
//!
//! Snapshotting is what deno_core offers instead: the sources are evaluated
//! here, at build time, and what survives is a serialised V8 heap that
//! `include_bytes!` can carry. `CreateSnapshotOutput::consumed_lazy_specifiers`
//! is the proof — anything listed there is inside the snapshot and will not be
//! looked for on disk.
//!
//! The startup cost we save is real but incidental: the server spawns one
//! isolate per function library, and each of them would otherwise re-parse and
//! re-execute the same few hundred kilobytes of Web platform JavaScript.

use std::path::PathBuf;

use deno_core::snapshot::{create_snapshot, CreateSnapshotOptions};

// The extension is defined once, in the crate, and compiled a second time here.
// Snapshot and runtime must register byte-identical op lists or deno_core
// rejects the snapshot when the first isolate starts.
#[path = "src/ext.rs"]
mod ext;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let snapshot_path = out_dir.join("APIPLANT_JS_SNAPSHOT.bin");

    let output = create_snapshot(
        CreateSnapshotOptions {
            cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            startup_snapshot: None,
            skip_op_registration: false,
            extensions: vec![
                deno_webidl::deno_webidl::init(),
                deno_web::deno_web::init(
                    deno_web::BlobStore::default_arc(),
                    // No `location` global: a function is not a document and has
                    // no URL of its own. `URL` still resolves absolute inputs.
                    None,
                    // The CSS parser backs `CSSStyleSheet`, which nothing here
                    // exposes; leaving it off keeps that code out of the heap.
                    false,
                    deno_web::InMemoryBroadcastChannel::default(),
                ),
                ext::extension(ext::detached()),
            ],
            extension_transpiler: None,
            // The bootstrap needs nothing from the runtime beyond its own
            // extensions, so there is no isolate to reach into before the heap
            // is serialised.
            with_runtime_cb: None,
        },
        // No warmup script: the bootstrap's own top level is the warmup, and it
        // has already run by the time the heap is serialised.
        None,
    )
    .expect("cannot build the JavaScript startup snapshot");

    std::fs::write(&snapshot_path, &output.output).expect("cannot write the startup snapshot");

    // The specifiers whose source the snapshot swallowed. Anything the bootstrap
    // asks `loadExtScript` for and that is *not* on this list would be read from
    // an absolute path at runtime — fine in a checkout, fatal in a release. The
    // list is written out so a test can assert exactly that; see `src/ext.rs`.
    std::fs::write(
        out_dir.join("consumed_lazy_specifiers.txt"),
        output.consumed_lazy_specifiers.join("\n"),
    )
    .expect("cannot record the snapshot's consumed specifiers");

    // Any source still read from disk at snapshot time has to invalidate this
    // build when it changes — that is every `deno_web` JS file plus our own.
    for path in output.files_loaded_during_snapshot {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=src/ext.rs");
    println!("cargo:rerun-if-changed=assets/bootstrap.js");
    println!("cargo:rerun-if-changed=assets/apiplant.js");
}
