//! The `apiplant` executable.
//!
//! ```text
//! apiplant [run] [APP_DIR]     # serve the app in APP_DIR (default: current dir)
//! apiplant build [APP_DIR]     # compile functions/* into loadable libraries
//! apiplant check [APP_DIR]     # load & validate the app, then exit
//! ```
//!
//! An *app directory* holds an optional `main.toml`, an optional `models/`
//! directory of resource definitions, an optional `functions/` directory of
//! function sources (Rust, C, Zig or Go) and their compiled libraries, and — to
//! enable TLS — an
//! `https/` directory with a certificate and key. Every piece is optional; an
//! empty directory is a valid (if bare) app.

mod compile;

use apiplant_core::App;

const USAGE: &str = "\
usage:
  apiplant [run] [APP_DIR]     serve the app (default command, default dir `.`)
  apiplant build [APP_DIR]     compile functions/* into loadable libraries
  apiplant check [APP_DIR]     load and validate the app, then exit

options:
  --build           (run) compile any out-of-date function sources first
  --release         (build) compile with optimisations
  --force           (build) rebuild even when the library is up to date
  -h, --help        show this message

`build` shells out to a toolchain per language — cargo for .rs, cc for .c, zig
for .zig, go for .go — so whichever your functions use must be on PATH.
";

/// What the user asked for.
#[derive(Debug)]
enum Command {
    Run { build_first: bool },
    Build { release: bool, force: bool },
    Check,
}

#[derive(Debug)]
struct Args {
    command: Command,
    dir: String,
}

/// Parse the command line. The first non-flag argument is the command when it
/// names one, otherwise it's the app directory — so `apiplant ./my-app` still
/// serves, as it always has.
fn parse(argv: Vec<String>) -> Result<Option<Args>, String> {
    let mut command = None;
    let mut dir = None;
    let mut build_first = false;
    let mut release = false;
    let mut force = false;

    for arg in argv {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--build" => build_first = true,
            "--release" => release = true,
            "--force" => force = true,
            // Kept for compatibility with the original flag-style invocation.
            "--check" => command = Some("check"),
            "run" | "build" | "check" if command.is_none() && dir.is_none() => {
                command = Some(match arg.as_str() {
                    "run" => "run",
                    "build" => "build",
                    _ => "check",
                });
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            other => dir = Some(other.to_string()),
        }
    }

    let command = match command.unwrap_or("run") {
        "build" => Command::Build { release, force },
        "check" => Command::Check,
        _ => Command::Run { build_first },
    };
    Ok(Some(Args {
        command,
        dir: dir.unwrap_or_else(|| ".".to_string()),
    }))
}

#[ntex::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,apiplant=debug".into()),
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

    match args.command {
        Command::Build { release, force } => {
            let built = compile::build(dir, compile::Options { release, force })?;
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

        Command::Run { build_first } => {
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
            apiplant_server::run(app).await
        }
    }
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
    fn bare_directory_still_runs_the_app() {
        let parsed = args(&["./my-app"]);
        assert!(matches!(
            parsed.command,
            Command::Run { build_first: false }
        ));
        assert_eq!(parsed.dir, "./my-app");

        // No arguments at all serves the current directory.
        assert_eq!(args(&[]).dir, ".");
    }

    #[test]
    fn subcommands_are_recognised_before_the_directory() {
        assert!(matches!(
            args(&["build", "./my-app"]).command,
            Command::Build {
                release: false,
                force: false
            }
        ));
        assert_eq!(args(&["build", "./my-app"]).dir, "./my-app");
        assert!(matches!(args(&["check", "./app"]).command, Command::Check));
        assert!(matches!(
            args(&["run", "./app"]).command,
            Command::Run { build_first: false }
        ));
    }

    #[test]
    fn flags_apply_to_their_command() {
        assert!(matches!(
            args(&["build", "--release", "--force", "."]).command,
            Command::Build {
                release: true,
                force: true
            }
        ));
        assert!(matches!(
            args(&["run", "--build", "."]).command,
            Command::Run { build_first: true }
        ));
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
            Command::Run { build_first: false }
        ));
        assert_eq!(parsed.dir, "build");
    }

    #[test]
    fn unknown_options_are_rejected() {
        let err = parse(vec!["--wat".to_string()]).unwrap_err();
        assert!(err.contains("--wat"));
    }
}
