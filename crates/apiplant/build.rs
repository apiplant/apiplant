//! Turn `src/cli.usage.kdl` into Rust at build time.
//!
//! The spec is the single source of truth for the command line, but reading it
//! costs about 0.4µs per byte of KDL — 1.9ms for this one, every time the
//! binary starts, to rebuild a structure that cannot have changed since it was
//! compiled. So it is built here instead, and what ships is the finished
//! `usage::Spec` as Rust code.
//!
//! The emitter writes plain field assignments rather than builder calls,
//! because a builder normalises what it is given (an arg named `<dir>` becomes
//! `dir`, a usage string is regenerated) and the point here is to reproduce
//! *exactly* what parsing produced, field for field. It covers the subset of
//! the spec format this CLI uses; `spec.rs`'s own test compares the generated
//! spec against a freshly parsed one field by field, so anything the emitter
//! silently drops fails the build's tests rather than the command line.

use std::fmt::Write as _;

fn main() {
    println!("cargo:rerun-if-changed=src/cli.usage.kdl");

    let source = include_str!("src/cli.usage.kdl");
    let spec: usage::Spec = match source.parse() {
        Ok(spec) => spec,
        // A spec that does not parse is a compile error, which is where it
        // belongs: the alternative is a binary that panics on every run.
        Err(error) => panic!("src/cli.usage.kdl is not a valid usage spec: {error}"),
    };

    let mut out = String::new();
    out.push_str(
        "// Generated from src/cli.usage.kdl by build.rs. Do not edit.\n\
         fn generated_spec() -> usage::Spec {\n\
         \x20   let mut spec = usage::Spec::default();\n",
    );
    let _ = writeln!(out, "    spec.name = {}.to_string();", quote(&spec.name));
    let _ = writeln!(out, "    spec.bin = {}.to_string();", quote(&spec.bin));
    let _ = writeln!(out, "    spec.usage = {}.to_string();", quote(&spec.usage));
    if let Some(about) = &spec.about {
        let _ = writeln!(out, "    spec.about = Some({}.to_string());", quote(about));
    }
    if let Some(about) = &spec.about_long {
        let _ = writeln!(
            out,
            "    spec.about_long = Some({}.to_string());",
            quote(about)
        );
    }
    // Spec-level rather than command-level: the commands inherit it, and
    // without it `apiplant build --relase` files the typo away as a directory.
    if let Some(unknown) = spec.unknown_flags {
        let _ = writeln!(
            out,
            "    spec.unknown_flags = Some({});",
            unknown_flags(unknown)
        );
    }
    for (field, value) in [
        ("spec.before_help", &spec.before_help),
        ("spec.after_help", &spec.after_help),
        ("spec.before_help_long", &spec.before_help_long),
        ("spec.after_help_long", &spec.after_help_long),
        ("spec.long_version", &spec.long_version),
        ("spec.author", &spec.author),
        ("spec.repository", &spec.repository),
        ("spec.license", &spec.license),
        ("spec.default_subcommand", &spec.default_subcommand),
    ] {
        option(&mut out, "    ", field, value);
    }
    if spec.multicall {
        out.push_str("    spec.multicall = true;\n");
    }
    if spec.multicall_set {
        out.push_str("    spec.multicall_set = true;\n");
    }
    if let Some(disable) = spec.disable_help {
        let _ = writeln!(out, "    spec.disable_help = Some({disable});");
    }
    let _ = writeln!(out, "    spec.cmd = {};", command(&spec.cmd, 1));
    out.push_str("    spec\n}\n");

    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("spec.rs");
    std::fs::write(&path, out).expect("writing the generated spec");
}

/// One `SpecCommand`, as an expression.
fn command(cmd: &usage::SpecCommand, depth: usize) -> String {
    let pad = "    ".repeat(depth + 1);
    let mut out = String::from("{\n");
    let _ = writeln!(out, "{pad}let mut c = usage::SpecCommand::default();");
    let _ = writeln!(out, "{pad}c.name = {}.to_string();", quote(&cmd.name));
    let _ = writeln!(out, "{pad}c.usage = {}.to_string();", quote(&cmd.usage));
    let _ = writeln!(out, "{pad}c.full_cmd = {};", strings(&cmd.full_cmd));
    option(&mut out, &pad, "c.help", &cmd.help);
    option(&mut out, &pad, "c.help_long", &cmd.help_long);
    option(&mut out, &pad, "c.help_md", &cmd.help_md);
    option(&mut out, &pad, "c.before_help", &cmd.before_help);
    option(&mut out, &pad, "c.before_help_long", &cmd.before_help_long);
    option(&mut out, &pad, "c.after_help", &cmd.after_help);
    option(&mut out, &pad, "c.after_help_long", &cmd.after_help_long);
    option(&mut out, &pad, "c.help_heading", &cmd.help_heading);
    if !cmd.aliases.is_empty() {
        let _ = writeln!(out, "{pad}c.aliases = {};", strings(&cmd.aliases));
    }
    if !cmd.hidden_aliases.is_empty() {
        let _ = writeln!(
            out,
            "{pad}c.hidden_aliases = {};",
            strings(&cmd.hidden_aliases)
        );
    }
    if let Some(order) = cmd.display_order {
        let _ = writeln!(out, "{pad}c.display_order = Some({order});");
    }
    if let Some(unknown) = cmd.unknown_flags {
        let _ = writeln!(
            out,
            "{pad}c.unknown_flags = Some({});",
            unknown_flags(unknown)
        );
    }
    for (name, value) in [
        ("hide", cmd.hide),
        ("subcommand_required", cmd.subcommand_required),
        ("external_subcommand", cmd.external_subcommand),
        ("arg_required_else_help", cmd.arg_required_else_help),
        ("disable_help_flag", cmd.disable_help_flag),
        ("disable_help_subcommand", cmd.disable_help_subcommand),
        ("disable_version_flag", cmd.disable_version_flag),
    ] {
        if value {
            let _ = writeln!(out, "{pad}c.{name} = true;");
        }
    }
    for flag in &cmd.flags {
        let _ = writeln!(out, "{pad}c.flags.push({});", spec_flag(flag, depth + 1));
    }
    for arg in &cmd.args {
        let _ = writeln!(out, "{pad}c.args.push({});", spec_arg(arg, depth + 1));
    }
    for (name, sub) in &cmd.subcommands {
        let _ = writeln!(
            out,
            "{pad}c.subcommands.insert({}.to_string(), {});",
            quote(name),
            command(sub, depth + 1)
        );
    }
    let _ = writeln!(out, "{pad}c");
    let _ = write!(out, "{}}}", "    ".repeat(depth));
    out
}

/// One `SpecFlag`, as an expression.
fn spec_flag(flag: &usage::SpecFlag, depth: usize) -> String {
    let pad = "    ".repeat(depth + 1);
    let mut out = String::from("{\n");
    let _ = writeln!(out, "{pad}let mut f = usage::SpecFlag::default();");
    let _ = writeln!(out, "{pad}f.name = {}.to_string();", quote(&flag.name));
    let _ = writeln!(out, "{pad}f.usage = {}.to_string();", quote(&flag.usage));
    if !flag.long.is_empty() {
        let _ = writeln!(out, "{pad}f.long = {};", strings(&flag.long));
    }
    if !flag.short.is_empty() {
        let shorts: Vec<String> = flag.short.iter().map(|c| format!("{c:?}")).collect();
        let _ = writeln!(out, "{pad}f.short = vec![{}];", shorts.join(", "));
    }
    option(&mut out, &pad, "f.help", &flag.help);
    option(&mut out, &pad, "f.help_first_line", &flag.help_first_line);
    option(&mut out, &pad, "f.help_long", &flag.help_long);
    option(&mut out, &pad, "f.help_md", &flag.help_md);
    option(&mut out, &pad, "f.help_heading", &flag.help_heading);
    for (name, value) in [
        ("required", flag.required),
        ("var", flag.var),
        ("hide", flag.hide),
        ("global", flag.global),
        ("count", flag.count),
        ("exclusive", flag.exclusive),
        ("require_equals", flag.require_equals),
        ("value_optional", flag.value_optional),
        ("bool_value", flag.bool_value),
        ("builtin", flag.builtin),
    ] {
        if value {
            let _ = writeln!(out, "{pad}f.{name} = true;");
        }
    }
    if let Some(arg) = &flag.arg {
        let _ = writeln!(out, "{pad}f.arg = Some({});", spec_arg(arg, depth + 1));
    }
    let _ = writeln!(out, "{pad}f");
    let _ = write!(out, "{}}}", "    ".repeat(depth));
    out
}

/// One `SpecArg`, as an expression.
fn spec_arg(arg: &usage::SpecArg, depth: usize) -> String {
    let pad = "    ".repeat(depth + 1);
    let mut out = String::from("{\n");
    let _ = writeln!(out, "{pad}let mut a = usage::SpecArg::default();");
    let _ = writeln!(out, "{pad}a.name = {}.to_string();", quote(&arg.name));
    let _ = writeln!(out, "{pad}a.usage = {}.to_string();", quote(&arg.usage));
    option(&mut out, &pad, "a.help", &arg.help);
    option(&mut out, &pad, "a.help_first_line", &arg.help_first_line);
    option(&mut out, &pad, "a.help_long", &arg.help_long);
    option(&mut out, &pad, "a.help_md", &arg.help_md);
    option(&mut out, &pad, "a.help_heading", &arg.help_heading);
    if !arg.value_names.is_empty() {
        let _ = writeln!(out, "{pad}a.value_names = {};", strings(&arg.value_names));
    }
    for (name, value) in [
        ("required", arg.required),
        ("var", arg.var),
        ("hide", arg.hide),
        ("allow_negative_numbers", arg.allow_negative_numbers),
    ] {
        if value {
            let _ = writeln!(out, "{pad}a.{name} = true;");
        }
    }
    if let Some(min) = arg.var_min {
        let _ = writeln!(out, "{pad}a.var_min = Some({min});");
    }
    if let Some(max) = arg.var_max {
        let _ = writeln!(out, "{pad}a.var_max = Some({max});");
    }
    let _ = writeln!(out, "{pad}a");
    let _ = write!(out, "{}}}", "    ".repeat(depth));
    out
}

/// The path to an `UnknownFlags` variant.
fn unknown_flags(unknown: usage::UnknownFlags) -> &'static str {
    match unknown {
        usage::UnknownFlags::Error => "usage::UnknownFlags::Error",
        usage::UnknownFlags::Value => "usage::UnknownFlags::Value",
    }
}

/// `field = Some("…".to_string());`, when there is something to say.
fn option(out: &mut String, pad: &str, field: &str, value: &Option<String>) {
    if let Some(value) = value {
        let _ = writeln!(out, "{pad}{field} = Some({}.to_string());", quote(value));
    }
}

/// A `Vec<String>` literal.
fn strings(values: &[String]) -> String {
    let items: Vec<String> = values
        .iter()
        .map(|v| format!("{}.to_string()", quote(v)))
        .collect();
    format!("vec![{}]", items.join(", "))
}

/// A Rust string literal for `value`.
///
/// `{:?}` on a `&str` is exactly this — Rust's own escaping, which is what the
/// generated file has to be read back with.
fn quote(value: &str) -> String {
    format!("{value:?}")
}
