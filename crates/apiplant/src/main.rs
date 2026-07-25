//! The `apiplant` executable.
//!
//! ```text
//! apiplant run [APP_DIR]       # serve the app in APP_DIR (default: current dir)
//! apiplant build [APP_DIR]     # compile functions/* into loadable libraries
//! apiplant check [APP_DIR]     # load & validate the app, then exit
//! apiplant admin [APP_DIR]     # emit a static admin panel for the app
//! apiplant studio              # serve the visual editor from this binary
//! ```
//!
//! An *app directory* holds an optional `main.toml`, an optional `models/`
//! directory of resource definitions, an optional `functions/` directory of
//! function sources (Rust, C, Zig or Go) and their compiled libraries, and — to
//! enable TLS — an
//! `https/` directory with a certificate and key. Every piece is optional; an
//! empty directory is a valid (if bare) app.

mod compile;
mod studio;

use apiplant_core::App;
use apiplant_server::admin;

const USAGE: &str = "\
usage:
  apiplant run [APP_DIR]       serve the app (default dir `.`)
  apiplant build [APP_DIR]     compile functions/* into loadable libraries
  apiplant check [APP_DIR]     load and validate the app, then exit
  apiplant admin [APP_DIR]     emit a static admin panel for the app
  apiplant studio              serve the visual editor on http://127.0.0.1:5273

options:
  --build           (run) compile any out-of-date function sources first
  --release         (build) compile with optimisations
  --force           (build) rebuild even when the library is up to date
  --api <URL>       (admin) API domain or full base URL to talk to
  --out <DIR>       (admin) where to write the static admin build (default: APP_DIR/admin)
  --host <ADDR>     (studio) interface to bind (default 127.0.0.1)
  --port <PORT>     (studio) port to listen on (default 5273)
  -h, --help        show this message

Every served app also gets the admin dashboard at `/admin/` — it is built into
this binary, needs no `apiplant admin` run, and is switched off with
`[admin] enabled = false` in main.toml.

`build` shells out to a toolchain per language — cargo for .rs, cc for .c, zig
for .zig, go for .go — so whichever your functions use must be on PATH.
";

/// What the user asked for.
#[derive(Debug)]
enum Command {
    Run { build_first: bool },
    Build { release: bool, force: bool },
    Check,
    Admin { api: String, out: Option<String> },
    Studio { host: String, port: u16 },
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
    let mut release = false;
    let mut force = false;
    let mut api = None;
    let mut out = None;
    let mut host = None;
    let mut port = None;
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
            "--release" => release = true,
            "--force" => force = true,
            "--api" => expecting = Some("--api"),
            "--out" => expecting = Some("--out"),
            "--host" => expecting = Some("--host"),
            "--port" => expecting = Some("--port"),
            // Kept for compatibility with the original flag-style invocation.
            "--check" => command = Some("check"),
            "run" | "build" | "check" | "admin" | "studio"
                if command.is_none() && dir.is_none() =>
            {
                command = Some(match arg.as_str() {
                    "run" => "run",
                    "build" => "build",
                    "admin" => "admin",
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
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    if let Some(flag) = expecting {
        return Err(format!("missing value for `{flag}`"));
    }

    let Some(command) = command else {
        return Err("a command is required".into());
    };

    let command = match command {
        "build" => {
            if build_first {
                return Err("`--build` only applies to `run`".into());
            }
            if api.is_some() || out.is_some() || host.is_some() || port.is_some() {
                return Err(
                    "`--api`, `--out`, `--host` and `--port` do not apply to `build`".into(),
                );
            }
            Command::Build { release, force }
        }
        "check" => {
            if build_first
                || release
                || force
                || api.is_some()
                || out.is_some()
                || host.is_some()
                || port.is_some()
            {
                return Err("`check` does not take run/build/admin/studio flags".into());
            }
            Command::Check
        }
        "admin" => {
            if build_first || release || force {
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
        "studio" => {
            if build_first || release || force || api.is_some() || out.is_some() {
                return Err("`studio` only takes `--host` and `--port`".into());
            }
            Command::Studio {
                host: host.unwrap_or_else(|| studio::DEFAULT_HOST.to_string()),
                port: port.unwrap_or(studio::DEFAULT_PORT),
            }
        }
        _ => {
            if release
                || force
                || api.is_some()
                || out.is_some()
                || host.is_some()
                || port.is_some()
            {
                return Err("`run` only takes `--build`".into());
            }
            Command::Run { build_first }
        }
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

    // Every command but `studio` reads an app directory. A missing one is a
    // typo, not an empty app: serving it would start a server for nothing, and
    // building it would report "nothing to build" and look like success.
    if !matches!(args.command, Command::Studio { .. }) && !dir.is_dir() {
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

        Command::Studio { host, port } => studio::serve(&host, port).await,

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
            args(&["admin", "./app", "--api", "https://example.com"]).command,
            Command::Admin { .. }
        ));
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
            Command::Run { build_first: false }
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
        let err = parse(vec!["admin".to_string(), ".".to_string()]).unwrap_err();
        assert!(err.contains("--api"));
    }
}
