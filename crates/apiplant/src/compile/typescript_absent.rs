//! TypeScript, in a build that does not have it.
//!
//! This stands in for `typescript.rs` when the `typescript` feature is off —
//! the slim build, which links neither V8 nor the TypeScript front end. It
//! exists so that `compile/mod.rs` reads the same either way: the dispatch on
//! [`Language::TypeScript`](super::Language) is still there, still matches, and
//! still ends up somewhere that explains itself.
//!
//! `.ts` stays a language this binary *recognises*. It is discovered, it is
//! reported, and it fails with a sentence naming the build — which is the point
//! of keeping the arm rather than deleting it. A slim binary that quietly
//! skipped `functions/greet.ts` would produce an app whose author can see the
//! source sitting in the directory and cannot see the endpoint.

use super::{Options, Source};
use anyhow::Result;
use std::path::Path;

/// What every entry point here says.
pub(super) fn unsupported(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} is TypeScript, and this is a slim build of apiplant — it has no \
         TypeScript support. Use a full build to compile it.",
        path.display()
    )
}

pub(super) fn compile_typescript(source: &Source, _library: &Path) -> Result<()> {
    Err(unsupported(&source.path))
}

pub(super) fn compile_typescript_dir(
    source: &Source,
    _library: &Path,
    _options: Options,
) -> Result<()> {
    Err(unsupported(&source.path))
}

/// The `ctx` type declarations an editor reads.
///
/// Nothing to write: they describe an API this build cannot run. `build`
/// refuses before reaching here, so this is the second line of defence.
pub(super) fn write_declarations(functions_dir: &Path) -> Result<()> {
    Err(unsupported(functions_dir))
}
