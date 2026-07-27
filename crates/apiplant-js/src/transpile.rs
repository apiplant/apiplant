//! Turning a `.ts` source into the `.js` the server loads.
//!
//! This is the build-time half of TypeScript support: [oxc] parses the module
//! and drops the type annotations, which is all "compiling" TypeScript ever is.
//! No type *checking* happens — the same choice `deno run --no-check` and `bun`
//! make — so the cost is a few milliseconds per file and the errors an author
//! sees at build time are syntax errors, not type errors. See [`DECLARATIONS`]
//! for what makes an editor (or `tsc --noEmit`) do the checking properly.
//!
//! [oxc]: https://oxc.rs

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Program, Statement};
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};

/// The type declarations `apiplant build` writes beside an app's TypeScript
/// functions: the `apiplant` module, and the globals a module that imports
/// nothing still gets.
///
/// Copied in rather than installed, because a `functions/` directory has no
/// `node_modules` and should not need one — a `.d.ts` beside the source is
/// enough for TypeScript, with no tooling and no configuration at all. It is the
/// same file the npm package ships as its `types`.
pub const DECLARATIONS: &str = include_str!("../../../typescript/apiplant.d.ts");

/// Strip the types from `source`, returning JavaScript.
///
/// `label` names the module in error messages and stack traces.
pub fn to_js(label: &str, source: &str) -> Result<String, String> {
    let allocator = Allocator::default();
    // The path is what tells oxc this is TypeScript rather than JavaScript, and
    // it is also the file name that appears in a diagnostic.
    let name = format!("{label}.ts");
    let path = Path::new(&name);
    let source_type = SourceType::from_path(path).map_err(|e| e.to_string())?;

    let parsed = Parser::new(&allocator, source, source_type).parse();
    if let Some(errors) = report(parsed.diagnostics.iter()) {
        return Err(errors);
    }
    let mut program = parsed.program;
    reject_imports(source, &program)?;

    // The TypeScript transform rewrites `enum` and parameter properties, which
    // needs to know what every name binds to — hence a semantic pass first.
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let transformed = Transformer::new(&allocator, path, &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    if let Some(errors) = report(transformed.diagnostics.iter()) {
        return Err(errors);
    }

    Ok(Codegen::new().build(&program).code)
}

/// Collect diagnostics into one message, or `None` when there are none.
fn report<'a, D: std::fmt::Display + 'a>(
    diagnostics: impl Iterator<Item = &'a D>,
) -> Option<String> {
    let message = diagnostics
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    (!message.is_empty()).then_some(message)
}

/// Refuse an import of anything but the `apiplant` module.
///
/// Nothing here bundles, so a relative import would resolve to a file the server
/// never loaded and a bare one to a package that was never installed. Both fail
/// at boot with a message about a missing module, far from the line that caused
/// it — so they are refused here instead, where the line number is known and the
/// explanation can be about functions rather than about module resolution.
///
/// Read off the parsed module rather than the text, so an `import` inside a
/// string or a comment is not one, and a declaration spread over several lines
/// still is.
fn reject_imports(source: &str, program: &Program<'_>) -> Result<(), String> {
    for statement in &program.body {
        // `export … from "x"` reaches another module exactly as an import does.
        let (specifier, span) = match statement {
            Statement::ImportDeclaration(declaration) => {
                (declaration.source.value.as_str(), declaration.span)
            }
            Statement::ExportNamedDeclaration(declaration) => match &declaration.source {
                Some(source) => (source.value.as_str(), declaration.span),
                None => continue,
            },
            Statement::ExportAllDeclaration(declaration) => {
                (declaration.source.value.as_str(), declaration.span)
            }
            _ => continue,
        };

        if specifier == crate::module::NAME {
            continue;
        }

        return Err(format!(
            "line {}: cannot import `{specifier}`.\n\
             apiplant does not bundle TypeScript functions, so a function is one \
             self-contained file and `apiplant` is the only module it can import \
             — the host, the database, the cache and the mailer all come from \
             there.",
            line_of(source, span.start)
        ));
    }
    Ok(())
}

/// The 1-based line a byte offset falls on.
fn line_of(source: &str, offset: u32) -> usize {
    source[..offset as usize]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_stripped_and_the_code_survives() {
        let js = to_js(
            "greet",
            r#"
            interface In { name: string }
            export const manifest = [{ name: "greet", permission: "public" }];
            export function greet(input: In, ctx: unknown): { hi: string } {
                return { hi: input.name };
            }
            "#,
        )
        .unwrap();

        assert!(js.contains("export function greet(input, ctx)"), "{js}");
        // An interface is types-only: it leaves no trace in the output.
        assert!(!js.contains("interface"), "{js}");
    }

    #[test]
    fn a_syntax_error_is_reported_rather_than_emitted() {
        let err = to_js("broken", "export function oops( {").unwrap_err();
        assert!(!err.is_empty());
    }

    /// The one module a function may import, and the reason it is the only one.
    #[test]
    fn the_apiplant_module_is_importable() {
        let js = to_js(
            "greet",
            "import { defineFunctions, db, s } from \"apiplant\";\n\
             export default defineFunctions({});\n",
        )
        .unwrap();
        assert!(js.contains("from \"apiplant\""), "{js}");
    }

    /// The failure mode this saves an author from is a boot-time "module not
    /// found" pointing at a file they never wrote.
    #[test]
    fn every_other_import_is_refused_with_a_reason() {
        let relative = to_js("greet", "\nimport { x } from \"./other.ts\";\n").unwrap_err();
        assert!(relative.contains("does not bundle"), "{relative}");
        assert!(relative.contains("line 2"), "{relative}");

        let package = to_js("greet", "import zod from \"zod\";\n").unwrap_err();
        assert!(package.contains("cannot import `zod`"), "{package}");

        // `export … from` reaches another module by another name.
        let reexport = to_js("greet", "export * from \"./other.ts\";\n").unwrap_err();
        assert!(reexport.contains("cannot import"), "{reexport}");
    }

    /// A declaration spread over several lines is still one import, and one
    /// written inside a string is not an import at all -- which is why this
    /// reads the parsed module rather than the text.
    #[test]
    fn imports_are_recognised_by_shape_not_by_spelling() {
        let js = to_js(
            "greet",
            "import {\n  db,\n} from \"apiplant\";\n\
             export const manifest = [{ name: \"g\" }];\n\
             export function g() { return db.query(\"SELECT 1\"); }\n",
        )
        .unwrap();
        assert!(js.contains("from \"apiplant\""), "{js}");

        let err = to_js("greet", "import {\n  x,\n} from \"./other.ts\";\n").unwrap_err();
        assert!(err.contains("line 1"), "{err}");
    }

    #[test]
    fn the_word_import_inside_code_is_not_an_import() {
        let js = to_js(
            "greet",
            "export const manifest = [{ name: \"g\" }];\nexport const note = \"import me\";\n",
        )
        .unwrap();
        assert!(js.contains("import me"));
    }
}
