//! The `apiplant` executable.
//!
//! ```text
//! apiplant init [APP_DIR]      # write a new app directory (or clone one)
//! apiplant run [APP_DIR]       # serve the app in APP_DIR (default: current dir)
//! apiplant build [APP_DIR]     # compile functions/* into loadable libraries
//! apiplant check [APP_DIR]     # load & validate the app, then exit
//! apiplant seed [APP_DIR]      # load seed/ into the database
//! apiplant call NAME [APP_DIR] # run one function and print what it returned
//! apiplant admin [APP_DIR]     # bake a static admin panel to host elsewhere
//! apiplant cli [SERVER|DIR]    # interactive console for a running app
//! apiplant studio              # serve the visual editor from this binary
//! apiplant version             # print the version and exit
//! ```
//!
//! An *app directory* holds an optional `main.toml`, an optional `resources/`
//! directory of resource definitions, an optional `functions/` directory of
//! function sources (Rust, C, Zig, Go or TypeScript) and what `apiplant build`
//! produced from them, and — to
//! enable TLS — an
//! `https/` directory with a certificate and key. Every piece is optional; an
//! empty directory is a valid (if bare) app.
//!
//! The command line itself is declared in `cli.usage.kdl` and parsed from it at
//! start-up; everything printed goes through [`apiplant_server::term`], so a
//! one-shot command and the server's own banner look like the same program.

mod cli;
mod compile;
mod init;
mod studio;
mod watch;

use apiplant_core::App;
use apiplant_server::admin;
use apiplant_server::term;

// The command line, as data.
//
// Parsing it by hand was a hundred lines of `if flag.is_some()` deciding which
// flags belonged to which command, and the help text was a second copy of the
// same knowledge that had to be kept in step by hand. The spec is now the only
// copy: `usage` reads it, scopes each flag to the command that declared it, and
// renders `--help` from the same tree it parses with.
//
// It is read at *build* time. `build.rs` parses `cli.usage.kdl` and writes the
// finished `usage::Spec` out as Rust, which this includes — reading the KDL on
// every start cost 1.9ms to rebuild something that was already decided when the
// binary was compiled. `generated_spec` is that emitted function.
include!(concat!(env!("OUT_DIR"), "/spec.rs"));

/// The KDL the spec is generated from.
///
/// Only the tests read it: they parse it the slow way and check the generated
/// spec against the result, so the emitter cannot quietly drop a field.
#[cfg(test)]
const SPEC: &str = include_str!("cli.usage.kdl");

/// The spec, with the version filled in from the crate.
///
/// Kept out of the spec file because it would be a second place to bump on
/// every release, and this one cannot drift.
fn spec() -> usage::Spec {
    let mut spec = generated_spec();
    spec.version = Some(env!("CARGO_PKG_VERSION").to_string());
    spec
}

/// What the user asked for.
#[derive(Debug)]
enum Command {
    Init {
        from: Option<String>,
        branch: Option<String>,
        name: Option<String>,
    },
    Run {
        build_first: bool,
        seed: bool,
        watch: bool,
    },
    Seed,
    Build {
        release: bool,
    },
    Check,
    Call {
        name: String,
        input: Option<String>,
        principal: Option<String>,
        quiet: bool,
    },
    Admin {
        api: String,
        out: Option<String>,
    },
    Cli {
        target: cli::Target,
    },
    Studio {
        host: String,
        port: u16,
    },
    Version,
}

#[derive(Debug)]
struct Args {
    command: Command,
    dir: String,
}

impl Args {
    /// `version` reads no app, so its directory is a placeholder.
    fn version() -> Self {
        Args {
            command: Command::Version,
            dir: ".".into(),
        }
    }
}

/// The outcome of reading the command line: something to do, or something to
/// say and then stop.
#[derive(Debug)]
enum Parsed {
    /// Run this.
    Do(Args),
    /// Print this to stdout and exit 0 — `--help` and `--version`.
    Say(String),
}

/// The values one parse produced, looked up by the names in the spec.
struct Values(usage::parse::ParseOutput);

impl Values {
    /// The innermost command that was named, or `""` for a bare `apiplant`.
    fn command(&self) -> &str {
        match self.0.cmds.len() {
            0 | 1 => "",
            _ => &self.0.cmds[self.0.cmds.len() - 1].name,
        }
    }

    /// A positional argument's value.
    fn arg(&self, name: &str) -> Option<String> {
        self.0
            .args
            .iter()
            .find(|(arg, _)| arg.name == name)
            .and_then(|(_, value)| value.clone().try_as_string())
    }

    /// A `--flag <value>`'s value.
    fn opt(&self, name: &str) -> Option<String> {
        self.flag_value(name).and_then(|v| v.try_as_string())
    }

    /// Whether a boolean `--flag` was given.
    fn on(&self, name: &str) -> bool {
        self.flag_value(name)
            .and_then(|v| v.try_as_bool())
            .unwrap_or(false)
    }

    fn flag_value(&self, name: &str) -> Option<usage::parse::ParseValue> {
        self.0
            .flags
            .iter()
            .find(|(flag, _)| flag.long.iter().any(|long| long == name))
            .map(|(_, value)| value.clone())
    }
}

/// Parse the command line.
///
/// The command is required and comes first: `apiplant ./my-app` is an error,
/// not a shorthand for `run`, because a typo'd directory should never quietly
/// start a server. The directory after it is optional and defaults to `.`.
fn parse(argv: Vec<String>) -> Result<Parsed, String> {
    let spec = spec();

    // `apiplant --check ./app` is how this was invoked before there were
    // subcommands, and it is in enough scripts to be worth a line.
    let argv: Vec<String> = argv
        .into_iter()
        .map(|arg| {
            if arg == "--check" {
                "check".to_string()
            } else {
                arg
            }
        })
        .collect();

    // A first word that is not a command is a typo, and the parser's own
    // "unexpected word" does not say what to do about it.
    if let Some(first) = argv.first() {
        if !first.starts_with('-') && !spec.cmd.subcommands.contains_key(first.as_str()) {
            return Err(format!(
                "`{first}` is not a command — did you mean `apiplant run {first}`?"
            ));
        }
    }

    // `usage` reads argv as the shell hands it over, program name and all.
    let mut input = vec!["apiplant".to_string()];
    input.extend(argv);

    // `Parser::explain` rather than `usage::parse`, because the latter renders
    // every outcome — including `--help` and `--version` — into one error
    // string, and those two are not errors: they are the whole point of the
    // invocation, and belong on stdout with a zero exit code.
    let output = match usage::Parser::new(&spec).explain(&input) {
        Ok(output) => match first_message(&output.errors)? {
            Some(message) => return Ok(message),
            None => output,
        },
        Err(error) => return Err(error.to_string()),
    };

    let values = Values(output);
    // A bare `apiplant` asks what it can do rather than doing something.
    if values.command().is_empty() {
        return Ok(Parsed::Say(help(&spec)));
    }

    let mut dir = values.arg("app_dir");
    let command = match values.command() {
        "version" => return Ok(Parsed::Do(Args::version())),

        "init" => {
            let (from, branch, name) =
                (values.opt("from"), values.opt("branch"), values.opt("name"));
            // Both `apiplant init <repo>` and `apiplant init <dir> <repo>` are
            // what people type; a git URL cannot be a directory name, so
            // spotting one is unambiguous.
            let from = match values.arg("repo") {
                Some(second) => {
                    if !looks_like_a_repository(&second) {
                        return Err(format!(
                            "`{second}` is not a repository URL — `init` takes a directory \
                             and optionally a git URL to clone"
                        ));
                    }
                    if from.is_some() {
                        return Err(
                            "give the repository once: as `--from` or as an argument".into()
                        );
                    }
                    Some(second)
                }
                None if from.is_none() && dir.as_deref().is_some_and(looks_like_a_repository) => {
                    dir.take()
                }
                None => from,
            };
            Command::Init { from, branch, name }
        }

        "run" => Command::Run {
            build_first: values.on("build"),
            seed: values.on("seed"),
            watch: values.on("watch"),
        },

        // `--force` is still accepted, but an explicit `build` is always a full
        // rebuild: someone typing it wants fresh libraries, and timestamp
        // staleness misses edits a build script or a dependency made. The
        // cached path is for the implicit builds (`run --build`).
        "build" => Command::Build {
            release: values.on("release"),
        },

        "check" => Command::Check,
        "seed" => Command::Seed,

        "call" => Command::Call {
            name: values.arg("name").ok_or_else(|| {
                "`call` needs a function name: `apiplant call <NAME>`".to_string()
            })?,
            input: values.opt("input"),
            principal: values.opt("as"),
            quiet: values.on("quiet"),
        },

        "admin" => Command::Admin {
            api: values
                .opt("api")
                .ok_or_else(|| "`admin` requires `--api <URL>`".to_string())?,
            out: values.opt("out"),
        },

        // The positional argument is a server first and a directory second, so
        // the app directory the other commands default to is not read at all.
        "cli" => Command::Cli {
            target: cli::Target::parse(values.arg("server_or_dir").as_deref().unwrap_or(".")),
        },

        "studio" => {
            let port = match values.opt("port") {
                Some(port) => port
                    .parse::<u16>()
                    .map_err(|_| format!("`--port` needs a port number, got `{port}`"))?,
                None => studio::DEFAULT_PORT,
            };
            Command::Studio {
                host: values
                    .opt("host")
                    .unwrap_or_else(|| studio::DEFAULT_HOST.to_string()),
                port,
            }
        }

        other => return Err(format!("unknown command `{other}`")),
    };

    Ok(Parsed::Do(Args {
        command,
        dir: dir.unwrap_or_else(|| ".".to_string()),
    }))
}

/// What `--version` prints.
///
/// The slim build says so. Two binaries with the same name and version that
/// disagree about whether `functions/greet.ts` works is exactly the thing a bug
/// report has to be able to state, so it is in the first line of output rather
/// than something you deduce from a failure.
fn version() -> String {
    format!("apiplant {}{BUILD}", env!("CARGO_PKG_VERSION"))
}

/// The suffix naming this build — empty for the full one.
const BUILD: &str = if cfg!(feature = "typescript") {
    ""
} else {
    " (slim)"
};

/// The first thing a finished parse has to say, if it is not the parse itself.
///
/// `--help` and `--version` arrive here as "errors" because that is how the
/// parser stops early; the rest genuinely are, and the first one is the one
/// worth showing.
fn first_message(errors: &[usage::error::UsageErr]) -> Result<Option<Parsed>, String> {
    match errors.first() {
        None => Ok(None),
        Some(usage::error::UsageErr::Help(text)) => Ok(Some(Parsed::Say(text.clone()))),
        Some(usage::error::UsageErr::Version(_)) => Ok(Some(Parsed::Say(version()))),
        Some(first) => Err(first.to_string()),
    }
}

/// What a bare `apiplant` prints — the same text `--help` renders.
fn help(spec: &usage::Spec) -> String {
    let input = ["apiplant".to_string(), "--help".to_string()];
    match usage::Parser::new(spec).explain(&input) {
        Ok(output) => match output.errors.first() {
            Some(usage::error::UsageErr::Help(text)) => text.clone(),
            _ => spec.usage.clone(),
        },
        Err(_) => spec.usage.clone(),
    }
}

/// Whether an argument is a git repository rather than a directory.
///
/// Only the unmistakable forms: a URL with a scheme, an `scp`-style ssh
/// address, or a path ending in `.git`. Anything else is a directory, because
/// mistaking one for the other would clone where the user meant to scaffold.
fn looks_like_a_repository(argument: &str) -> bool {
    argument.contains("://") || argument.starts_with("git@") || argument.ends_with(".git")
}

#[ntex::main]
async fn main() -> anyhow::Result<()> {
    let args = match parse(std::env::args().skip(1).collect()) {
        Ok(Parsed::Do(args)) => args,
        Ok(Parsed::Say(text)) => {
            println!("{}", text.trim_end());
            return Ok(());
        }
        Err(message) => {
            // The spec renders the help; repeating it under every mistake
            // buries the one line that says what was wrong.
            term::fail(&message);
            eprintln!("    try `apiplant --help`");
            eprintln!();
            std::process::exit(2);
        }
    };
    // Answered before anything touches the filesystem: a broken app directory
    // is often exactly why someone is asking which version they are running.
    if matches!(args.command, Command::Version) {
        println!("{}", version());
        return Ok(());
    }

    let dir = std::path::Path::new(&args.dir);

    // Every command but `studio` reads an app directory. A missing one is a
    // typo, not an empty app: serving it would start a server for nothing, and
    // building it would report "nothing to build" and look like success.
    // `init` is the one command whose directory is allowed not to exist yet —
    // creating it is the job.
    if !matches!(
        args.command,
        Command::Studio { .. } | Command::Cli { .. } | Command::Init { .. }
    ) && !dir.is_dir()
    {
        term::fail(&format!(
            "{}: {}",
            if dir.exists() {
                "not a directory"
            } else {
                "no such app directory"
            },
            dir.display()
        ));
        eprintln!();
        std::process::exit(2);
    }

    // Logging is configured from the app being run, so `main.toml` is read
    // before anything else happens — twice, in effect, since `App::load` reads
    // it again later. It is one small TOML file, and the alternative is either
    // a startup that cannot be logged or a subscriber that has to be swapped
    // out underneath a running process.
    //
    // A config that cannot be parsed does not stop this: the load that
    // matters happens below and reports the error properly. Here it just
    // means the defaults decide what that report looks like.
    let config = apiplant_core::Config::load(dir).unwrap_or_default();
    let service_name = config.app.name.clone().unwrap_or_else(|| {
        dir.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "apiplant".to_string())
    });
    // Held to the end of `main`: dropping it flushes whatever the exporters
    // are still holding, and the last batch before an exit is the one someone
    // is going to want.
    let _telemetry = apiplant_server::telemetry::init(&config.observability, &service_name);

    match args.command {
        Command::Init { from, branch, name } => {
            init::init(dir, init::Options { from, branch, name })
        }

        Command::Build { release } => {
            let built = compile::build(
                dir,
                compile::Options {
                    release,
                    force: true,
                },
            )?;
            term::heading("build", Some(&args.dir));
            for library in &built {
                term::item(library);
            }
            match built.len() {
                0 => term::done("nothing to build"),
                n => term::done(&format!(
                    "built {n} function librar{}",
                    if n == 1 { "y" } else { "ies" }
                )),
            }
            Ok(())
        }

        Command::Check => {
            let app = load(dir)?;
            term::heading("check", Some(&args.dir));
            for name in app.resources.keys() {
                term::item(name);
            }
            term::done(&match app.resources.len() {
                0 => "the app is valid — it defines no resources".to_string(),
                1 => "the app is valid — 1 resource".to_string(),
                n => format!("the app is valid — {n} resources"),
            });
            Ok(())
        }

        Command::Call {
            name,
            input,
            principal,
            quiet,
        } => {
            let input = read_input(input.as_deref())?;
            let app = if quiet { App::load(dir)? } else { load(dir)? };
            let result = apiplant_server::call::call(
                &app,
                &name,
                apiplant_server::call::Options {
                    input,
                    principal,
                    emit_to_stderr: !quiet,
                },
            )
            .await?;
            // The result alone on stdout, so `apiplant call ... | jq` works and
            // a CronJob can pipe it somewhere.
            println!("{result}");
            Ok(())
        }

        Command::Admin { api, out } => {
            term::heading("admin", Some(&args.dir));
            let stale = compile::stale(dir);
            if !stale.is_empty() {
                tracing::warn!(
                    functions = %stale.join(", "),
                    "function sources are newer than their compiled libraries — the generated admin panel only includes currently built function endpoints"
                );
            }
            let output = admin::build(
                dir,
                admin::Options {
                    api,
                    out: out.map(Into::into),
                },
            )?;
            term::done(&format!(
                "built a static admin panel in {}",
                output.display()
            ));
            Ok(())
        }

        Command::Cli { target } => {
            // The console owns the terminal and talks HTTP on its own schedule;
            // giving it a runtime of its own keeps it clear of the one ntex
            // builds for serving, which is single-threaded and shaped for a
            // different job entirely.
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(cli::run(&target))
            })
            .join()
            .map_err(|_| anyhow::anyhow!("the console stopped unexpectedly"))?
        }

        Command::Studio { host, port } => studio::serve(&host, port).await,

        // Handled above, before the app directory is looked at.
        Command::Version => Ok(()),

        Command::Seed => {
            let app = load(dir)?;
            term::heading("seed", Some(&args.dir));
            seed_app(&app).await
        }

        Command::Run {
            // `--watch` builds on every restart, so an explicit `--build`
            // alongside it is redundant rather than wrong.
            build_first: _,
            seed,
            watch: true,
        } => {
            // The supervisor spends its life waiting and then killing a child,
            // which is nothing like the single-threaded runtime ntex builds for
            // serving — and the server it starts is a separate process anyway.
            let dir = dir.to_path_buf();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(watch::supervise(&dir, seed))
            })
            .join()
            .map_err(|_| anyhow::anyhow!("the watcher stopped unexpectedly"))?
        }

        Command::Run {
            build_first, seed, ..
        } => {
            if build_first {
                compile::build(dir, compile::Options::default())?;
            } else {
                // Serving a library older than its source is a confusing way to
                // lose an afternoon; say so rather than quietly doing it.
                let stale = compile::stale(dir);
                if !stale.is_empty() {
                    tracing::warn!(
                        functions = %stale.join(", "),
                        "function sources are newer than their compiled libraries — \
                         run `apiplant build` (or add --build) to recompile"
                    );
                }
            }
            let app = load(dir)?;
            apiplant_server::run_with(app, apiplant_server::Options { seed }).await
        }
    }
}

/// `apiplant seed` — migrate (unless the app has that switched off), then load
/// `seed/`.
///
/// A command of its own rather than only a flag on `run`, because loading a
/// fixture is a thing you do to a database — into a container someone else
/// will serve from, in a CI step before the tests, against a staging box — and
/// none of those want a server started afterwards.
async fn seed_app(app: &App) -> anyhow::Result<()> {
    let db = apiplant_db::Db::connect(
        &app.config.database.resolved_url(),
        app.config.database.max_connections,
    )
    .await?;
    if app.config.database.auto_migrate {
        apiplant_db::migrate(db.connection(), app).await?;
    }
    let report = apiplant_db::seed::seed(db.connection(), app).await?;
    if report.is_empty() {
        apiplant_server::term::done(&format!(
            "nothing to seed — {} has no seed/ directory",
            app.root.display()
        ));
        return Ok(());
    }
    for file in &report.files {
        apiplant_server::term::detail(
            &file.resource,
            &format!("{} inserted, {} already there", file.inserted, file.skipped),
        );
    }
    apiplant_server::term::done(&format!(
        "seeded {} row{} ({} already there)",
        report.inserted(),
        if report.inserted() == 1 { "" } else { "s" },
        report.skipped()
    ));
    Ok(())
}

/// Resolve `--input` to the JSON a function will be handed.
///
/// A literal is the common case, `@file` is the one that survives a shell and a
/// Kubernetes manifest (a ConfigMap mounted next to the job beats quoting JSON
/// inside YAML inside a container's args), and `@-` is stdin.
fn read_input(input: Option<&str>) -> anyhow::Result<String> {
    use std::io::Read;

    let text = match input {
        None => return Ok("{}".to_string()),
        Some("@-") => {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        }
        Some(path) if path.starts_with('@') => {
            let path = &path[1..];
            std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("cannot read `--input` file `{path}`: {e}"))?
        }
        Some(literal) => literal.to_string(),
    };

    // Checked here rather than left to the function, because "expected value at
    // line 1 column 1" from inside a plugin is a much worse error than this one.
    if !text.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| anyhow::anyhow!("`--input` is not valid JSON: {e}"))?;
    }
    Ok(text)
}

fn load(dir: &std::path::Path) -> anyhow::Result<App> {
    tracing::info!("loading app from {}", dir.display());
    let app = App::load(dir)?;
    tracing::info!(
        resources = app.resources.len(),
        tls = app.tls.is_some(),
        "app loaded"
    );
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> Args {
        match parse(argv.iter().map(|s| s.to_string()).collect()).unwrap() {
            Parsed::Do(args) => args,
            Parsed::Say(text) => panic!("expected a command, got a message: {text}"),
        }
    }

    fn error(argv: &[&str]) -> String {
        parse(argv.iter().map(|s| s.to_string()).collect())
            .expect_err("expected this line to be rejected")
    }

    /// What `parse` printed instead of doing anything — help, or the version.
    fn message(argv: &[&str]) -> String {
        match parse(argv.iter().map(|s| s.to_string()).collect()).unwrap() {
            Parsed::Say(text) => text,
            Parsed::Do(args) => panic!("expected a message, got {:?}", args.command),
        }
    }

    #[test]
    fn the_repository_forms_are_recognised_and_directories_are_not() {
        // A URL with a scheme, an scp-style ssh address, and a `.git` suffix are
        // the only forms cloned; everything else is scaffolded in place.
        assert!(looks_like_a_repository("https://github.com/org/app"));
        assert!(looks_like_a_repository("git@github.com:org/app"));
        assert!(looks_like_a_repository("git@github.com:org/app.git"));
        assert!(looks_like_a_repository("/srv/templates/app.git"));
        // A plain directory, a bare host, and a file that merely contains the
        // letters are all directories, whatever they are called.
        assert!(!looks_like_a_repository("."));
        assert!(!looks_like_a_repository("my-app"));
        assert!(!looks_like_a_repository("/home/me/my-app"));
        assert!(!looks_like_a_repository("github.com"));
    }

    #[test]
    fn the_spec_the_parser_is_generated_from_is_valid() {
        // Every other test in here would fail with the same panic, but this one
        // says why: the KDL, not the code around it.
        let spec = spec();
        assert_eq!(spec.bin, "apiplant");
        assert_eq!(spec.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        // Every documented command is declared, in the order help lists them.
        for command in [
            "init", "run", "build", "check", "seed", "call", "admin", "cli", "studio", "version",
        ] {
            assert!(
                spec.cmd.subcommands.contains_key(command),
                "`{command}` is missing from cli.usage.kdl"
            );
        }
    }

    #[test]
    fn the_generated_spec_is_the_one_the_kdl_describes() {
        // `build.rs` emits the fields this CLI uses, which is a subset of what
        // a usage spec can hold — so the emitter could silently drop something
        // and everything would still compile. This is the check that it did
        // not: parse the KDL the slow way and compare the whole tree, field for
        // field, via the representation `usage` itself serialises to.
        //
        // A failure here names the field that went missing. The fix is in
        // `build.rs`, not here.
        let parsed: usage::Spec = SPEC
            .parse()
            .expect("cli.usage.kdl is not a valid usage spec");
        let generated = generated_spec();

        // Flattened to `path = value` lines first: the whole tree printed twice
        // is thousands of characters to read for what is usually one missing
        // field, and this way the failure names it.
        let mut want = Vec::new();
        flatten(
            &serde_json::to_value(&parsed).unwrap(),
            String::new(),
            &mut want,
        );
        let mut got = Vec::new();
        flatten(
            &serde_json::to_value(&generated).unwrap(),
            String::new(),
            &mut got,
        );

        let missing: Vec<&String> = want.iter().filter(|line| !got.contains(line)).collect();
        let extra: Vec<&String> = got.iter().filter(|line| !want.contains(line)).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "the generated spec and the KDL have drifted apart — fix the emitter in build.rs\n\
             the KDL has, and the generated spec does not:\n  {}\n\
             the generated spec has, and the KDL does not:\n  {}",
            missing
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join("\n  "),
            extra
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join("\n  "),
        );
    }

    #[test]
    fn the_generated_spec_parses_command_lines_identically() {
        // The structural comparison above is the strict one; this is the one
        // that would catch a difference `Serialize` cannot see, and it reads as
        // what actually matters: the same command line means the same thing.
        let parsed: usage::Spec = SPEC.parse().unwrap();
        let generated = spec();
        for argv in [
            &["apiplant", "run", "--build", "--watch", "./app"][..],
            &[
                "apiplant", "init", "my-app", "--from", "x", "--branch", "v2",
            ],
            &[
                "apiplant", "call", "nightly", "./app", "--input", "{}", "--quiet",
            ],
            &["apiplant", "admin", ".", "--api", "https://api.example.com"],
            &["apiplant", "studio", "--host", "0.0.0.0", "--port", "9000"],
            &["apiplant", "build", "--relase"],
            &["apiplant", "seed", "--build"],
        ] {
            let input: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
            let one = describe(usage::Parser::new(&parsed).explain(&input));
            let two = describe(usage::Parser::new(&generated).explain(&input));
            assert_eq!(one, two, "{argv:?} parses differently");
        }
    }

    /// Every leaf of a JSON tree as one `path = value` line.
    fn flatten(value: &serde_json::Value, path: String, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    flatten(value, format!("{path}.{key}"), out);
                }
            }
            serde_json::Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    flatten(value, format!("{path}[{index}]"), out);
                }
            }
            leaf => out.push(format!("{path} = {leaf}")),
        }
    }

    /// A parse outcome as a comparable string: what bound, and what went wrong.
    fn describe(outcome: Result<usage::parse::ParseOutput, usage::miette::Error>) -> String {
        match outcome {
            Err(error) => format!("error: {error}"),
            Ok(output) => {
                let cmds: Vec<&str> = output.cmds.iter().map(|c| c.name.as_str()).collect();
                let args: Vec<String> = output
                    .args
                    .iter()
                    .map(|(a, v)| format!("{}={v:?}", a.name))
                    .collect();
                let flags: Vec<String> = output
                    .flags
                    .iter()
                    .map(|(f, v)| format!("{}={v:?}", f.name))
                    .collect();
                let errors: Vec<String> = output.errors.iter().map(|e| e.to_string()).collect();
                format!("{cmds:?} {args:?} {flags:?} {errors:?}")
            }
        }
    }

    #[test]
    fn the_command_is_required_and_a_bare_directory_is_a_mistake() {
        // `apiplant ./my-app` used to serve; now it says what it should have
        // been, because guessing "run" from a stray argument is how a typo
        // becomes a running server.
        let err = error(&["./my-app"]);
        assert!(err.contains("apiplant run ./my-app"), "{err}");

        // No arguments at all is a request for the usage message.
        let help = message(&[]);
        assert!(help.contains("Usage: apiplant"), "{help}");
        assert!(help.contains("studio"), "{help}");

        // The directory itself stays optional.
        assert_eq!(args(&["run"]).dir, ".");
    }

    #[test]
    fn init_takes_a_directory_a_repository_or_both() {
        // The plain form: a directory to fill with the sample app.
        let parsed = args(&["init", "my-app"]);
        assert_eq!(parsed.dir, "my-app");
        assert!(matches!(parsed.command, Command::Init { from: None, .. }));

        // A URL on its own initialises the current directory from it.
        let parsed = args(&["init", "https://example.com/template.git"]);
        assert_eq!(parsed.dir, ".");
        assert!(matches!(
            parsed.command,
            Command::Init { from: Some(ref repo), .. } if repo.ends_with("template.git")
        ));

        // Directory and repository together, positionally or by flag.
        for argv in [
            &["init", "my-app", "git@example.com:acme/template.git"][..],
            &[
                "init",
                "my-app",
                "--from",
                "git@example.com:acme/template.git",
            ][..],
        ] {
            let parsed = args(argv);
            assert_eq!(parsed.dir, "my-app");
            assert!(matches!(
                parsed.command,
                Command::Init { from: Some(ref repo), .. } if repo.starts_with("git@")
            ));
        }

        // A second positional that is not a URL is a mistake, not a template.
        let message = error(&["init", "my-app", "oops"]);
        assert!(message.contains("not a repository URL"), "{message}");

        // And init's flags belong to init alone — the spec scopes them there, so
        // `run` has never heard of `--from`.
        let message = error(&["run", "--from", "x"]);
        assert!(message.contains("--from"), "{message}");
    }

    #[test]
    fn call_takes_a_function_then_a_directory() {
        let parsed = args(&["call", "nightly_report"]);
        assert_eq!(parsed.dir, ".");
        match parsed.command {
            Command::Call {
                name,
                input,
                principal,
                quiet,
            } => {
                assert_eq!(name, "nightly_report");
                assert!(input.is_none() && principal.is_none() && !quiet);
            }
            other => panic!("expected call, got {other:?}"),
        }

        // The directory follows the function name, not the other way round.
        let parsed = args(&["call", "nightly_report", "./app"]);
        assert_eq!(parsed.dir, "./app");
        assert!(
            matches!(parsed.command, Command::Call { ref name, .. } if name == "nightly_report")
        );

        // A function may share a name with a subcommand — the command slot is
        // already spoken for by the time the name is read.
        assert!(matches!(
            args(&["call", "run"]).command,
            Command::Call { ref name, .. } if name == "run"
        ));

        let parsed = args(&[
            "call",
            "nightly_report",
            "--input",
            "{\"day\":1}",
            "--as",
            "00000000-0000-0000-0000-000000000000",
            "--quiet",
        ]);
        match parsed.command {
            Command::Call {
                input,
                principal,
                quiet,
                ..
            } => {
                assert_eq!(input.as_deref(), Some("{\"day\":1}"));
                assert!(principal.is_some());
                assert!(quiet);
            }
            other => panic!("expected call, got {other:?}"),
        }

        // A missing name is a mistake, not a call to a function named `.`.
        let message = error(&["call"]);
        assert!(message.contains("<name>"), "{message}");

        // And call's flags belong to call alone.
        let message = error(&["run", "--input", "{}"]);
        assert!(message.contains("--input"), "{message}");
    }

    #[test]
    fn input_comes_from_a_literal_a_file_or_nothing() {
        assert_eq!(read_input(None).unwrap(), "{}");
        assert_eq!(read_input(Some("{\"a\":1}")).unwrap(), "{\"a\":1}");
        assert!(read_input(Some("not json")).is_err());

        let path = std::env::temp_dir().join("apiplant-call-input.json");
        std::fs::write(&path, "{\"from\":\"file\"}").unwrap();
        assert_eq!(
            read_input(Some(&format!("@{}", path.display()))).unwrap(),
            "{\"from\":\"file\"}"
        );
        std::fs::remove_file(&path).ok();

        assert!(read_input(Some("@/no/such/input.json")).is_err());
    }

    #[test]
    fn subcommands_are_recognised_before_the_directory() {
        assert!(matches!(
            args(&["build", "./my-app"]).command,
            Command::Build { release: false }
        ));
        assert_eq!(args(&["build", "./my-app"]).dir, "./my-app");
        assert!(matches!(args(&["check", "./app"]).command, Command::Check));
        assert!(matches!(args(&["studio"]).command, Command::Studio { .. }));
        assert!(matches!(
            args(&["run", "./app"]).command,
            Command::Run {
                build_first: false,
                ..
            }
        ));
    }

    #[test]
    fn flags_apply_to_their_command() {
        assert!(matches!(
            args(&["build", "--release", "--force", "."]).command,
            Command::Build { release: true }
        ));
        assert!(matches!(
            args(&["run", "--build", "."]).command,
            Command::Run {
                build_first: true,
                ..
            }
        ));
        assert!(matches!(
            args(&["run", "--seed", "."]).command,
            Command::Run { seed: true, .. }
        ));
        // `--watch` is `run`'s alone, and carries `--seed` along when asked.
        assert!(matches!(
            args(&["run", "--watch", "."]).command,
            Command::Run {
                watch: true,
                seed: false,
                ..
            }
        ));
        assert!(matches!(
            args(&["run", "--watch", "--seed", "."]).command,
            Command::Run {
                watch: true,
                seed: true,
                ..
            }
        ));
        let err = error(&["build", "--watch"]);
        assert!(err.contains("--watch"), "{err}");
        assert!(parse(vec!["check".into(), "--watch".into()]).is_err());
        assert!(matches!(args(&["seed", "."]).command, Command::Seed));
        // `--seed` loads a fixture into a database a server is about to use;
        // `seed` loads it and stops. Neither takes the other's flags.
        assert!(parse(vec!["seed".into(), "--build".into()]).is_err());
        assert!(parse(vec!["build".into(), "--seed".into()]).is_err());
        match args(&[
            "admin",
            "--api",
            "https://api.example.com",
            "--out",
            "panel",
            ".",
        ])
        .command
        {
            Command::Admin { api, out } => {
                assert_eq!(api, "https://api.example.com");
                assert_eq!(out.as_deref(), Some("panel"));
            }
            other => panic!("expected admin command, got {other:?}"),
        }
    }

    #[test]
    fn the_console_takes_a_server_or_a_directory() {
        // A server is the ordinary case, and needs nothing local at all.
        match args(&["cli", "https://api.example.com"]).command {
            Command::Cli { target } => {
                assert_eq!(
                    target,
                    cli::Target::Server("https://api.example.com".into())
                )
            }
            other => panic!("expected cli command, got {other:?}"),
        }
        match args(&["cli", "api.example.com"]).command {
            Command::Cli { target } => {
                assert_eq!(target, cli::Target::Server("api.example.com".into()))
            }
            other => panic!("expected cli command, got {other:?}"),
        }

        // A path is the secondary behaviour; no argument means this directory.
        match args(&["cli", "./app"]).command {
            Command::Cli { target } => assert_eq!(target, cli::Target::Dir("./app".into())),
            other => panic!("expected cli command, got {other:?}"),
        }
        match args(&["cli"]).command {
            Command::Cli { target } => assert_eq!(target, cli::Target::Dir(".".into())),
            other => panic!("expected cli command, got {other:?}"),
        }

        // `--api` is gone: the address is the argument.
        let err = error(&["cli", "--api", "https://api.example.com"]);
        assert!(err.contains("--api"), "{err}");
    }

    #[test]
    fn version_is_a_word_and_a_flag() {
        assert!(matches!(args(&["version"]).command, Command::Version));
        // As flags it never reaches a command: the parser answers on the spot,
        // and it answers with the name as well as the number.
        for argv in [&["-V"][..], &["--version"]] {
            // `starts_with`, because a slim build appends "(slim)" to it.
            assert!(
                message(argv).starts_with(&format!("apiplant {}", env!("CARGO_PKG_VERSION"))),
                "{}",
                message(argv)
            );
        }
        // But a directory called `version` is still a directory.
        assert!(matches!(
            args(&["run", "version"]).command,
            Command::Run { .. }
        ));
        assert_eq!(args(&["run", "version"]).dir, "version");
    }

    #[test]
    fn studio_defaults_to_loopback_and_takes_host_and_port() {
        match args(&["studio"]).command {
            Command::Studio { host, port } => {
                assert_eq!(host, studio::DEFAULT_HOST);
                assert_eq!(port, studio::DEFAULT_PORT);
            }
            other => panic!("expected studio command, got {other:?}"),
        }
        match args(&["studio", "--host", "0.0.0.0", "--port", "9000"]).command {
            Command::Studio { host, port } => {
                assert_eq!(host, "0.0.0.0");
                assert_eq!(port, 9000);
            }
            other => panic!("expected studio command, got {other:?}"),
        }

        let err = error(&["studio", "--port", "nope"]);
        assert!(err.contains("port number"), "{err}");
    }

    #[test]
    fn legacy_check_flag_and_help_still_work() {
        assert!(matches!(
            args(&["--check", "./app"]).command,
            Command::Check
        ));
        assert_eq!(args(&["--check", "./app"]).dir, "./app");

        let help = message(&["--help"]);
        assert!(help.contains("Usage: apiplant"), "{help}");

        // Each command's own help carries the prose that belongs to it, from
        // the spec rather than from a second copy kept in the source.
        let help = message(&["call", "--help"]);
        assert!(help.contains("Kubernetes CronJob"), "{help}");
        assert!(help.contains("--quiet"), "{help}");
    }

    #[test]
    fn a_directory_named_like_a_command_is_still_a_directory() {
        // `apiplant run build` serves the directory called `build`.
        let parsed = args(&["run", "build"]);
        assert!(matches!(
            parsed.command,
            Command::Run {
                build_first: false,
                ..
            }
        ));
        assert_eq!(parsed.dir, "build");
    }

    #[test]
    fn unknown_options_are_rejected() {
        // At the top level and, because the spec says `unknown_flags "error"`,
        // inside a command too — where a lenient parser would quietly file
        // `--relase` away as the app directory.
        assert!(error(&["--wat"]).contains("--wat"));
        let err = error(&["build", "--relase"]);
        assert!(err.contains("--relase"), "{err}");
    }

    #[test]
    fn admin_requires_api_flag() {
        // The static panel is hosted away from the API, so it has to be told
        // which origin to talk to — there is no sensible default.
        let err = error(&["admin", "."]);
        assert!(err.contains("--api"), "{err}");
    }
}
