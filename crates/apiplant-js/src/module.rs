//! Serving the `apiplant` module to the isolate.
//!
//! A function's first line is usually
//!
//! ```ts
//! import { defineFunctions, db, s } from "apiplant";
//! ```
//!
//! which has to resolve to something. It could have been a file on disk, but
//! then every app would need a copy of it in `functions/`, kept in step with the
//! binary that runs it. Instead the module is compiled into this crate and
//! handed to V8 on demand — an app's `functions/` directory stays a directory of
//! the author's own source, and the module can never be the wrong version.
//!
//! Nothing else resolves. There is no bundler, so a relative import would name a
//! file the server never loaded, and a bare one would name a package that was
//! never installed; both are refused here with the reason, and also at build
//! time, where the line number is still known.

use std::rc::Rc;

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleResolveResponse,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use deno_error::JsErrorBox;

/// The specifier a function writes.
pub(crate) const NAME: &str = "apiplant";

/// The URL it becomes internally. Its own scheme, so it cannot collide with a
/// file the app happens to have.
const URL: &str = "apiplant:module";

/// The module's source, from the `typescript/` package at the repository root —
/// the same file the npm package ships and the same one `apiplant.d.ts`
/// describes, so the types an editor reads and the code that runs are one
/// artifact rather than two that agree by hand.
pub(crate) const SOURCE: &str = include_str!("../../../typescript/apiplant.js");

/// Resolves `apiplant` and nothing else.
pub(crate) struct Loader;

impl Loader {
    /// The loader, ready to hand to a [`JsRuntime`](deno_core::JsRuntime).
    pub(crate) fn shared() -> Rc<dyn ModuleLoader> {
        Rc::new(Loader)
    }
}

impl ModuleLoader for Loader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        if specifier == NAME || specifier == URL {
            return ModuleSpecifier::parse(URL).map_err(JsErrorBox::from_err);
        }
        // The function's own module is resolved through here too, on its way in
        // from `load_main_es_module_from_code`. It is not an import.
        if kind == ResolutionKind::MainModule {
            return deno_core::resolve_import(specifier, referrer).map_err(JsErrorBox::from_err);
        }
        Err(JsErrorBox::generic(format!(
            "cannot import `{specifier}`: apiplant does not bundle TypeScript \
             functions, so `apiplant` is the only module a function can import"
        )))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        if module_specifier.as_str() != URL {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::generic(format!(
                "cannot load `{module_specifier}`: only `apiplant` is available"
            ))));
        }
        ModuleLoadResponse::Sync(Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(deno_core::FastString::from_static(SOURCE)),
            module_specifier,
            None,
        )))
    }
}
