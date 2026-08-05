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
//! apiplant cli [SERVER|DIR]   # interactive console for a running app
//! apiplant studio              # serve the visual editor from this binary
//! ```
//!
//! An *app directory* holds an optional `main.toml`, an optional `models/`
//! directory of resource definitions, an optional `functions/` directory of
//! function sources (Rust, C, Zig, Go or TypeScript) and what `apiplant build`
//! produced from them, and — to
//! enable TLS — an
//! `https/` directory with a certificate and key. Every piece is optional; an
//! empty directory is a valid (if bare) app.

mod cli;
mod compile;
mod init;
mod studio;

use apiplant_core::App;
use apiplant_server::admin;

const USAGE: &str = "\
usage:
  apiplant init [APP_DIR]      write a new app directory (default dir `.`)
  apiplant run [APP_DIR]       serve the app (default dir `.`)
  apiplant build [APP_DIR]     compile functions/* into loadable libraries
  apiplant check [APP_DIR]     load and validate the app, then exit
  apiplant seed [APP_DIR]      migrate, then load seed/ into the database
  apiplant call NAME [APP_DIR] run one function and print what it returned
  apiplant admin [APP_DIR]     bake a static admin panel to host elsewhere
  apiplant cli [SERVER|DIR]    interactive console for a running server
                               (a URL or host; or an app directory, default `.`)
  apiplant studio              serve the visual editor on http://127.0.0.1:5273

options:
  --from <REPO>     (init) clone this git repository instead of the sample app
  --branch <REF>    (init) branch, tag or commit to clone (with --from)
  --name <NAME>     (init) the app's name in main.toml (default: the directory)
  --build           (run) compile any out-of-date function sources first
  --seed            (run) load the app's seed/ directory after migrating
  --release         (build) compile with optimisations
  --force           (build) accepted and ignored: `build` always rebuilds
  --api <URL>       (admin) API domain or full base URL the panel talks to
  --out <DIR>       (admin) where to write it (default: APP_DIR/admin)
  --host <ADDR>     (studio) interface to bind (default 127.0.0.1)
  --port <PORT>     (studio) port to listen on (default 5273)
  --input <JSON>    (call) the function's input — JSON, `@file`, or `@-` for
                    stdin (default: `{}`)
  --as <USER_ID>    (call) the user id the function sees as its caller
  --quiet           (call) drop what the function emits instead of relaying it
  -h, --help        show this message

`call` runs one of the app's functions the way an HTTP request to
`/functions/<NAME>` would — same database, same email/cache/payments/AI — but
with no server and no access check, which is what makes it the thing to put in
a Kubernetes CronJob: schedule this binary against the same image and config.
Private functions can be called too. It does not migrate. The result goes to
stdout and anything the function emits goes to stderr, so a job's output is
still parseable while its progress stays visible in the logs.

Every served app gets the admin dashboard at `/admin/` — it is built into this
binary and describes whichever app is being served, so you only need `admin` to
host a copy on another origin. Switch the built-in one off with
`[admin] enabled = false` in main.toml.

`build` shells out to a toolchain per language — cargo for .rs, cc for .c, zig
for .zig, go for .go — so whichever your functions use must be on PATH.
TypeScript is the exception: .ts is transpiled in-process, so it needs nothing.
";

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
}

#[derive(Debug)]
struct Args {
    command: Command,
    dir: String,
}

/// Parse the command line.
///
/// The command is required and comes first: `apiplant ./my-app` is an error,
/// not a shorthand for `run`, because a typo'd directory should never quietly
/// start a server. The directory after it is optional and defaults to `.`.
fn parse(argv: Vec<String>) -> Result<Option<Args>, String> {
    // A bare `apiplant` asks what it can do rather than doing something.
    if argv.is_empty() {
        return Ok(None);
    }

    let mut command = None;
    let mut dir = None;
    let mut build_first = false;
    let mut seed = false;
    let mut release = false;
    let mut force = false;
    let mut quiet = false;
    let mut input = None;
    let mut principal = None;
    let mut api = None;
    let mut out = None;
    let mut host = None;
    let mut port = None;
    let mut extra = None;
    let mut from = None;
    let mut branch = None;
    let mut name = None;
    let mut expecting: Option<&str> = None;

    for arg in argv {
        if let Some(flag) = expecting.take() {
            match flag {
                "--api" => {
                    api = Some(arg);
                    continue;
                }
                "--out" => {
                    out = Some(arg);
                    continue;
                }
                "--host" => {
                    host = Some(arg);
                    continue;
                }
                "--from" => {
                    from = Some(arg);
                    continue;
                }
                "--branch" => {
                    branch = Some(arg);
                    continue;
                }
                "--name" => {
                    name = Some(arg);
                    continue;
                }
                "--input" => {
                    input = Some(arg);
                    continue;
                }
                "--as" => {
                    principal = Some(arg);
                    continue;
                }
                "--port" => {
                    port = Some(
                        arg.parse::<u16>()
                            .map_err(|_| format!("`--port` needs a port number, got `{arg}`"))?,
                    );
                    continue;
                }
                _ => return Err(format!("unknown pending option `{flag}`")),
            }
        }

        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--build" => build_first = true,
            "--seed" => seed = true,
            "--release" => release = true,
            "--force" => force = true,
            "--quiet" => quiet = true,
            "--input" => expecting = Some("--input"),
            "--as" => expecting = Some("--as"),
            "--api" => expecting = Some("--api"),
            "--out" => expecting = Some("--out"),
            "--host" => expecting = Some("--host"),
            "--port" => expecting = Some("--port"),
            "--from" => expecting = Some("--from"),
            "--branch" => expecting = Some("--branch"),
            "--name" => expecting = Some("--name"),
            // Kept for compatibility with the original flag-style invocation.
            "--check" => command = Some("check"),
            "init" | "run" | "build" | "check" | "seed" | "call" | "admin" | "cli" | "studio"
                if command.is_none() && dir.is_none() =>
            {
                command = Some(match arg.as_str() {
                    "init" => "init",
                    "run" => "run",
                    "build" => "build",
                    "seed" => "seed",
                    "call" => "call",
                    "admin" => "admin",
                    "cli" => "cli",
                    "studio" => "studio",
                    _ => "check",
                });
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            other if command.is_none() => {
                return Err(format!(
                    "`{other}` is not a command — did you mean `apiplant run {other}`?"
                ))
            }
            other if dir.is_none() => dir = Some(other.to_string()),
            // `apiplant init my-app <repo-url>` — the second positional is the
            // template, so the common form needs no flag at all. For `call` it
            // is the app directory, the function name having taken the first.
            other if matches!(command, Some("init") | Some("call")) && extra.is_none() => {
                extra = Some(other.to_string())
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    if let Some(flag) = expecting {
        return Err(format!("missing value for `{flag}`"));
    }

    let Some(command) = command else {
        return Err("a command is required".into());
    };
    if !matches!(command, "init" | "call") && extra.is_some() {
        return Err(format!("`{command}` takes at most one directory"));
    }
    if command != "call" && (input.is_some() || principal.is_some() || quiet) {
        return Err("`--input`, `--as` and `--quiet` only apply to `call`".into());
    }

    let command = match command {
        "init" => {
            if build_first || seed || release || force || api.is_some() || out.is_some() {
                return Err("`init` does not take run/build/admin flags".into());
            }
            if host.is_some() || port.is_some() {
                return Err("`--host` and `--port` only apply to `studio`".into());
            }
            // Both `apiplant init <repo>` and `apiplant init <dir> <repo>` are
            // what people type; a git URL cannot be a directory name, so
            // spotting one is unambiguous.
            if let Some(second) = extra {
                if !looks_like_a_repository(&second) {
                    return Err(format!(
                        "`{second}` is not a repository URL — `init` takes a directory \
                         and optionally a git URL to clone"
                    ));
                }
                if from.is_some() {
                    return Err("give the repository once: as `--from` or as an argument".into());
                }
                from = Some(second);
            } else if from.is_none() && dir.as_deref().is_some_and(looks_like_a_repository) {
                from = dir.take();
            }
            Command::Init { from, branch, name }
        }
        "build" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first || seed {
                return Err("`--build` and `--seed` only apply to `run`".into());
            }
            if api.is_some() || out.is_some() {
                return Err("`--api` and `--out` only apply to `admin`".into());
            }
            if host.is_some() || port.is_some() {
                return Err("`--host` and `--port` only apply to `studio`".into());
            }
            // `--force` is still accepted, but an explicit `build` is always a
            // full rebuild: someone typing it wants fresh libraries, and
            // timestamp staleness misses edits a build script or a dependency
            // made. The cached path is for the implicit builds (`run --build`).
            Command::Build { release }
        }
        "check" | "seed" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first
                || seed
                || release
                || force
                || api.is_some()
                || out.is_some()
                || host.is_some()
                || port.is_some()
            {
                return Err(format!(
                    "`{command}` does not take run/build/admin/studio flags"
                ));
            }
            if command == "seed" {
                Command::Seed
            } else {
                Command::Check
            }
        }
        "call" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first || seed || release || force || api.is_some() || out.is_some() {
                return Err("`call` does not take run/build/admin flags".into());
            }
            if host.is_some() || port.is_some() {
                return Err("`--host` and `--port` only apply to `studio`".into());
            }
            // The first positional is the function; the app directory follows
            // it, so `dir` has to be shuffled along by one.
            let Some(function) = dir.take() else {
                return Err("`call` needs a function name: `apiplant call <NAME>`".into());
            };
            dir = extra.take();
            Command::Call {
                name: function,
                input,
                principal,
                quiet,
            }
        }
        "admin" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first || seed || release || force {
                return Err("`admin` does not take run/build flags".into());
            }
            if host.is_some() || port.is_some() {
                return Err("`--host` and `--port` only apply to `studio`".into());
            }
            Command::Admin {
                api: api.ok_or_else(|| "`admin` requires `--api <URL>`".to_string())?,
                out,
            }
        }
        "cli" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first || seed || release || force || api.is_some() || out.is_some() {
                return Err("`cli` takes a server address or an app directory".into());
            }
            if host.is_some() || port.is_some() {
                return Err("`--host` and `--port` only apply to `studio`".into());
            }
            // The positional argument is a server first and a directory
            // second, so `dir` is spoken for and the app directory the other
            // commands default to is not read at all.
            Command::Cli {
                target: cli::Target::parse(dir.as_deref().unwrap_or(".")),
            }
        }
        "studio" => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if build_first || seed || release || force || api.is_some() || out.is_some() {
                return Err("`studio` only takes `--host` and `--port`".into());
            }
            Command::Studio {
                host: host.unwrap_or_else(|| studio::DEFAULT_HOST.to_string()),
                port: port.unwrap_or(studio::DEFAULT_PORT),
            }
        }
        _ => {
            if from.is_some() || branch.is_some() || name.is_some() {
                return Err("`--from`, `--branch` and `--name` only apply to `init`".into());
            }
            if release
                || force
                || api.is_some()
                || out.is_some()
                || host.is_some()
                || port.is_some()
            {
                return Err("`run` only takes `--build` and `--seed`".into());
            }
            Command::Run { build_first, seed }
        }
    };
    Ok(Some(Args {
        command,
        dir: dir.unwrap_or_else(|| ".".to_string()),
    }))
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // ntex logs a line per worker at INFO, which drowns out our own
                // startup output on machines with many cores.
                .unwrap_or_else(|_| "info,apiplant=debug,ntex_server=warn".into()),
        )
        .init();

    let args = match parse(std::env::args().skip(1).collect()) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return Ok(());
        }
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
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
        eprintln!(
            "{}: {}",
            if dir.exists() {
                "not a directory"
            } else {
                "no such app directory"
            },
            dir.display()
        );
        std::process::exit(2);
    }

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
            match built.len() {
                0 => println!("nothing to build"),
                n => println!(
                    "built {n} function librar{}",
                    if n == 1 { "y" } else { "ies" }
                ),
            }
            Ok(())
        }

        Command::Check => {
            let app = load(dir)?;
            for name in app.resources.keys() {
                println!("resource: {name}");
            }
            println!("ok");
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
            println!("built static admin panel in {}", output.display());
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

        Command::Seed => {
            let app = load(dir)?;
            seed_app(&app).await
        }

        Command::Run { build_first, seed } => {
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
        println!(
            "nothing to seed — {} has no seed/ directory",
            app.root.display()
        );
        return Ok(());
    }
    for file in &report.files {
        println!(
            "  {:<24} {} inserted, {} already there",
            file.resource, file.inserted, file.skipped
        );
    }
    println!(
        "seeded {} row{} ({} already there)",
        report.inserted(),
        if report.inserted() == 1 { "" } else { "s" },
        report.skipped()
    );
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
        parse(argv.iter().map(|s| s.to_string()).collect())
            .unwrap()
            .expect("not a help request")
    }

    #[test]
    fn the_command_is_required_and_a_bare_directory_is_a_mistake() {
        // `apiplant ./my-app` used to serve; now it says what it should have
        // been, because guessing "run" from a stray argument is how a typo
        // becomes a running server.
        let err = parse(vec!["./my-app".to_string()]).unwrap_err();
        assert!(err.contains("apiplant run ./my-app"), "{err}");

        // No arguments at all is a request for the usage message.
        assert!(parse(Vec::new()).unwrap().is_none());

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
        let error = parse(
            ["init", "my-app", "oops"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
        .unwrap_err();
        assert!(error.contains("not a repository URL"), "{error}");

        // And init's flags belong to init alone.
        let error = parse(
            ["run", "--from", "x"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
        .unwrap_err();
        assert!(error.contains("only apply to `init`"), "{error}");
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
        let error = parse(vec!["call".to_string()]).unwrap_err();
        assert!(error.contains("needs a function name"), "{error}");

        // And call's flags belong to call alone.
        let error = parse(
            ["run", "--input", "{}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
        .unwrap_err();
        assert!(error.contains("only apply to `call`"), "{error}");
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
        let err = parse(
            ["cli", "--api", "https://api.example.com"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
        .unwrap_err();
        assert!(err.contains("server address"), "{err}");
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

        let err = parse(
            ["studio", "--port", "nope"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
        .unwrap_err();
        assert!(err.contains("port number"));
    }

    #[test]
    fn legacy_check_flag_and_help_still_work() {
        assert!(matches!(
            args(&["--check", "./app"]).command,
            Command::Check
        ));
        assert_eq!(args(&["--check", "./app"]).dir, "./app");

        let argv = vec!["--help".to_string()];
        assert!(parse(argv).unwrap().is_none());
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
        let err = parse(vec!["--wat".to_string()]).unwrap_err();
        assert!(err.contains("--wat"));
    }

    #[test]
    fn admin_requires_api_flag() {
        // The static panel is hosted away from the API, so it has to be told
        // which origin to talk to — there is no sensible default.
        let err = parse(vec!["admin".to_string(), ".".to_string()]).unwrap_err();
        assert!(err.contains("--api"));
    }
}
