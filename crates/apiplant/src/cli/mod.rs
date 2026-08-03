//! `apiplant cli` — an interactive console for a running app.
//!
//! Point it at a server — `apiplant cli api.example.com` — and it fetches the
//! same manifest the dashboard uses and gives you the dashboard's job in a
//! terminal: browse resources from a sidebar, page through records, create and
//! edit and delete them, and run the app's functions. Point it at an app
//! directory instead and it reads that app's `main.toml` to work out where the
//! server is; with no argument at all it reads the current directory.
//!
//! Signing in offers three doors — an API key you already have, an
//! email/password sign-in that mints one for you, or a link that hands the
//! whole thing to the dashboard in your browser (see [`link`]). Whichever you
//! use, the key is saved per server so the next run starts signed in.
//!
//! The console reads the app directory for one thing only: the address of the
//! server. Everything else it knows comes from the server, so it never
//! describes an app that is different from the one actually running.

pub mod api;
mod link;
mod state;
mod store;
mod ui;

use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;

use api::Client;
use state::Cli;
use store::Store;

/// How long to wait between frames when nothing is happening. Fast enough that
/// a keystroke feels immediate, slow enough that an idle console is not a
/// busy-loop on someone's battery.
const IDLE: Duration = Duration::from_millis(20);

/// How many input events to take before drawing again.
///
/// Rendering between every keystroke is fine for typing and far too slow for a
/// paste, which arrives as a burst of dozens of events: the queue outgrows what
/// the loop drains and characters are lost — which, for a pasted API key, shows
/// up as a credential the server rejects for no visible reason. The cap only
/// exists so a stuck key cannot starve the screen entirely.
const INPUT_BURST: usize = 4096;

/// What the console was pointed at.
///
/// A server is the ordinary case: the console needs an address and nothing
/// else, since everything it displays comes from the manifest. A directory is
/// the convenience for working on an app locally — `main.toml` says which
/// interface and port the server binds and where the dashboard sits, which is
/// enough to talk to it on the same machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Server(String),
    Dir(std::path::PathBuf),
}

impl Target {
    /// Decide whether `given` names a server or an app directory.
    ///
    /// An existing path is a directory, whatever it is called; a scheme or a
    /// `//` prefix is unambiguously an address. What is left is decided by
    /// shape, and the shapes that say "server" — a dotted name, a `host:port`,
    /// `localhost` — are ones nobody types expecting a directory. Anything else
    /// is treated as a directory so the failure names the path they typed.
    pub fn parse(given: &str) -> Target {
        let trimmed = given.trim();
        if trimmed.contains("://") || trimmed.starts_with("//") {
            return Target::Server(trimmed.to_string());
        }
        if Path::new(trimmed).exists() {
            return Target::Dir(trimmed.into());
        }

        let host = trimmed.split('/').next().unwrap_or_default();
        let looks_like_a_server = host == "localhost"
            || host.starts_with("localhost:")
            // A port, or a domain — but not `./app` or `../app`, which are
            // dotted and are still paths.
            || (host.contains(':') && !host.contains('\\'))
            || (host.contains('.') && !host.starts_with('.') && !trimmed.ends_with('/'));
        if looks_like_a_server {
            Target::Server(trimmed.to_string())
        } else {
            Target::Dir(trimmed.into())
        }
    }
}

/// Work out where to connect, and under which paths the app is served.
fn endpoint(target: &Target) -> Result<(String, String, String)> {
    let dir = match target {
        // Nothing local to read: the server's own defaults are the best guess
        // for where its manifest sits, and the manifest then says where its API
        // really is (see `adopt_api_base`).
        Target::Server(given) => {
            let defaults = apiplant_core::Config::default();
            return Ok((
                normalise_origin(given),
                defaults.server.base_path.trim_end_matches('/').to_string(),
                defaults.admin.path,
            ));
        }
        Target::Dir(dir) => dir.as_path(),
    };

    let config = apiplant_core::Config::load(dir)
        .with_context(|| format!("could not read the app in {}", dir.display()))?;

    let base_path = config.server.base_path.clone();
    let admin_path = if config.admin.enabled {
        config.admin.path.clone()
    } else {
        // The manifest is served under the admin path, so a dashboard switched
        // off means there is nothing to describe the app with. Say so where the
        // operator can act on it rather than failing on a 404 later.
        anyhow::bail!(
            "the app in {} has `[admin] enabled = false`, so it publishes no manifest for the console to read",
            dir.display()
        );
    };

    // A bound wildcard is not an address you can connect to; the app is on this
    // machine, so that is where we look for it.
    let host = match config.server.host.as_str() {
        "0.0.0.0" | "::" | "" => "127.0.0.1".to_string(),
        host => host.to_string(),
    };
    let scheme = if dir.join("https").is_dir() {
        "https"
    } else {
        "http"
    };
    let origin = format!("{scheme}://{host}:{}", config.server.port);

    Ok((origin, base_path, admin_path))
}

/// Accept `example.com`, `//example.com` or a full URL as a server address.
fn normalise_origin(given: &str) -> String {
    let trimmed = given.trim().trim_end_matches('/');
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed.trim_start_matches('/'))
    }
}

/// Run the console against `target`.
pub async fn run(target: &Target) -> Result<()> {
    let (origin, base_path, admin_path) = endpoint(target)?;
    let mut client = Client::new(origin, base_path, admin_path)?;

    // Fetch the manifest *before* taking over the screen: a server that is not
    // running is by far the most likely first failure, and "connection refused"
    // is far more useful as a plain line on the terminal than as a box inside
    // an interface that then has nothing to show.
    eprintln!("connecting to {}…", client.manifest_url());
    let manifest = client.manifest().await?;

    // The directory told us where to knock; the server tells us where its API
    // actually is. Trusting the directory over the answer is how a console
    // pointed at the wrong checkout 404s on everything it tries.
    let relocated = client.adopt_api_base(&manifest.api_base_url)?;
    if let Some(note) = &relocated {
        eprintln!("{note}");
    }

    let store = Store::load();
    if let Some(saved) = store.server(&client.origin) {
        client.credentials.api_key = saved.api_key.clone();
        client.organization = saved.organization.clone();
    }

    let mut terminal = Screen::enter()?;
    let mut cli = Cli::new(client, manifest, store, target.clone());
    if let Some(note) = relocated {
        cli.status = note;
    }
    cli.start().await;

    while !cli.quit {
        terminal.0.draw(|frame| ui::draw(frame, &cli))?;

        let mut taken = 0;
        while taken < INPUT_BURST && !cli.quit && event::poll(Duration::ZERO)? {
            match event::read()? {
                // Windows reports both press and release; acting on both would
                // move every selection twice.
                Event::Key(key) if key.kind == KeyEventKind::Press => cli.on_key(key).await,
                // The terminal wrapped the paste for us, so it arrives whole
                // rather than as text indistinguishable from someone typing
                // very fast — and a key with an `i` in it cannot put a form
                // into edit mode halfway through.
                Event::Paste(text) => cli.on_paste(&text),
                Event::Resize(_, _) => {}
                _ => {}
            }
            taken += 1;
        }
        if taken == 0 {
            cli.tick().await;
            tokio::time::sleep(IDLE).await;
        }
    }

    drop(terminal);
    if let Some(message) = cli.farewell {
        println!("{message}");
    }
    Ok(())
}

/// The alternate screen, restored whatever happens.
///
/// A console that leaves the terminal in raw mode after a panic is a console
/// that makes people close the window, so the panic hook restores it too.
struct Screen(ratatui::Terminal<CrosstermBackend<Stdout>>);

impl Screen {
    fn enter() -> Result<Screen> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));

        enable_raw_mode().context("this terminal does not support raw mode")?;
        let mut stdout = std::io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        // Ask the terminal to mark pastes. Not every terminal obliges, which is
        // why the input loop also has to survive a burst of plain keystrokes.
        let _ = stdout.execute(EnableBracketedPaste);
        let terminal = ratatui::Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Screen(terminal))
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = std::io::stdout().execute(DisableBracketedPaste);
    let _ = std::io::stdout().execute(LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_dir(contents: &str) -> tempdir::Dir {
        tempdir::Dir::with("main.toml", contents)
    }

    #[test]
    fn the_address_comes_from_main_toml() {
        let dir = app_dir("[server]\nport = 9000\nbase_path = \"/api\"\n");
        let (origin, base, admin) = endpoint(&Target::Dir(dir.path().into())).unwrap();
        // `0.0.0.0` is the default bind, and it is not somewhere you can connect.
        assert_eq!(origin, "http://127.0.0.1:9000");
        assert_eq!(base, "/api");
        assert_eq!(admin, "/admin");
    }

    #[test]
    fn an_app_with_certificates_is_reached_over_https() {
        let dir = app_dir("[server]\nport = 8443\n");
        std::fs::create_dir_all(dir.path().join("https")).unwrap();
        let (origin, _, _) = endpoint(&Target::Dir(dir.path().into())).unwrap();
        assert_eq!(origin, "https://127.0.0.1:8443");
    }

    #[test]
    fn a_server_needs_nothing_local_and_may_omit_the_scheme() {
        let (origin, base, admin) = endpoint(&Target::Server("api.example.com/".into())).unwrap();
        assert_eq!(origin, "https://api.example.com");
        // No main.toml to read, so the server's own defaults say where to knock.
        assert_eq!(base, "");
        assert_eq!(admin, "/admin");

        let (origin, _, _) = endpoint(&Target::Server("http://box:1234".into())).unwrap();
        assert_eq!(origin, "http://box:1234");
    }

    #[test]
    fn a_target_is_a_server_or_a_directory() {
        for given in [
            "https://api.example.com",
            "api.example.com",
            "//box",
            "box:8080",
            "localhost:3000",
            "localhost",
        ] {
            assert_eq!(
                Target::parse(given),
                Target::Server(given.into()),
                "{given}"
            );
        }
        for given in ["./app", "../app", "my-app", "app/", "."] {
            assert_eq!(Target::parse(given), Target::Dir(given.into()), "{given}");
        }

        // An existing path wins over any shape it happens to have.
        let dir = app_dir("");
        assert_eq!(
            Target::parse(&dir.path().display().to_string()),
            Target::Dir(dir.path().into())
        );
    }

    #[test]
    fn a_dashboard_that_is_switched_off_is_reported_up_front() {
        let dir = app_dir("[admin]\nenabled = false\n");
        let error = endpoint(&Target::Dir(dir.path().into()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("no manifest"), "{error}");
    }

    /// A throwaway directory, removed when the test ends.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn with(name: &str, contents: &str) -> Dir {
                let path = std::env::temp_dir().join(format!(
                    "apiplant-cli-test-{}-{:?}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join(name), contents).unwrap();
                Dir(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
