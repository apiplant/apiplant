//! `apiplant run --watch` — rebuild and restart the server when the app changes.
//!
//! The server is *supervised*, not reloaded: this process starts `apiplant run
//! --build` as a child, watches the app directory, and on a change kills the
//! child and starts a new one. Reloading in place is not an option — a
//! function is a shared library the process has already `dlopen`ed, and there
//! is no safe way to take one back out — so the unit of reload is the process,
//! which also picks up an edited `main.toml`, a new model, and a new function
//! with no special cases.
//!
//! Changes are found by polling mtimes rather than by subscribing to the OS.
//! Polling costs a directory walk twice a second, and it is the only thing that
//! works over a bind mount: an editor writing on the host does not deliver
//! inotify events inside a container, which is exactly where a watch is most
//! wanted (see `examples/21-docker`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// How often the app directory is walked.
const POLL: Duration = Duration::from_millis(500);

/// How long the directory has to stop changing before a restart.
///
/// An editor's save is often several writes, and a `git checkout` is hundreds,
/// so restarting on the first one would boot a server against a half-written
/// app and then immediately do it again.
const SETTLE: Duration = Duration::from_millis(300);

/// Directories that are build output, not app source. Walking `node_modules`
/// or a cargo `target/` would dominate the poll, and both change *because* of
/// a build — watching them would restart the server in a loop.
const IGNORED_DIRS: &[&str] = &[
    ".apiplant-build",
    "target",
    "zig-out",
    "zig-cache",
    "node_modules",
    "dist",
    "admin",
];

/// Supervise a server over the app in `dir`, restarting it when `dir` changes.
///
/// Returns when the process is asked to stop; the child never outlives this
/// function — a supervisor that exits leaving a server holding the port is the
/// one failure that makes the next `run` fail too.
pub async fn supervise(dir: &Path, seed: bool) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the apiplant binary to restart")?;
    let dir = dir.to_path_buf();

    // Ctrl-C reaches the child as well (it shares this terminal's process
    // group), so this flag is only about *this* loop knowing that the child's
    // death was asked for rather than a crash to restart from.
    let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    watch_for_interrupt(std::sync::Arc::clone(&interrupted));

    println!("watching {} — edit anything to restart", dir.display());

    let mut fingerprint = fingerprint(&dir);
    loop {
        let mut child = spawn(&exe, &dir, seed)?;
        let outcome = wait_for_change(&dir, &mut fingerprint, &mut child, &interrupted).await;
        stop(child);
        match outcome {
            Outcome::Changed => println!("\nchange detected — restarting"),
            Outcome::Interrupted => {
                println!();
                return Ok(());
            }
        }
    }
}

/// Flip `interrupted` when the process is asked to stop.
///
/// SIGTERM matters as much as Ctrl-C here: `docker compose down` sends it to
/// this process only, and the server it started would otherwise be left behind.
fn watch_for_interrupt(interrupted: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for SIGTERM");
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Why [`wait_for_change`] returned.
enum Outcome {
    /// Something under the app directory was written.
    Changed,
    /// The user asked to stop.
    Interrupted,
}

/// Start one server over the app.
///
/// `--build` is implied: a watch exists to run what was just edited, and a
/// source newer than its library is precisely what the watch saw. A build that
/// fails takes the child down with it, which is fine — the loop waits for the
/// next edit rather than spinning, and the compiler's output is already on the
/// terminal.
fn spawn(exe: &Path, dir: &Path, seed: bool) -> Result<Child> {
    let mut command = Command::new(exe);
    command.arg("run").arg(dir).arg("--build");
    if seed {
        // Seeding is idempotent, so repeating it on each restart re-adds only
        // the fixture rows someone deleted.
        command.arg("--seed");
    }
    command
        .spawn()
        .with_context(|| format!("failed to start `{} run`", exe.display()))
}

/// Kill the child and reap it, so a restart never leaves the port held.
fn stop(mut child: Child) {
    // In development, in-flight requests are worth less than a prompt restart:
    // this is a kill, not a drain.
    let _ = child.kill();
    let _ = child.wait();
}

/// Block until the app changes, or the process is asked to stop.
async fn wait_for_change(
    dir: &Path,
    fingerprint: &mut Fingerprint,
    child: &mut Child,
    interrupted: &std::sync::atomic::AtomicBool,
) -> Outcome {
    let mut settling: Option<std::time::Instant> = None;
    loop {
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return Outcome::Interrupted;
        }
        tokio::time::sleep(POLL).await;

        let current = self::fingerprint(dir);
        if current != *fingerprint {
            *fingerprint = current;
            settling = Some(std::time::Instant::now());
            continue;
        }
        if let Some(since) = settling {
            if since.elapsed() >= SETTLE {
                return Outcome::Changed;
            }
            continue;
        }

        // The child exited on its own: a failed build, a port already taken, a
        // panic. Keep waiting rather than restarting into the same failure —
        // whatever broke needs an edit, and an edit is what wakes this up.
        if matches!(child.try_wait(), Ok(Some(_))) {
            continue;
        }
    }
}

/// Every watched file's path and modification time.
///
/// A map rather than a hash so that a file being *removed* is a change too, and
/// so the comparison says nothing about ordering.
type Fingerprint = BTreeMap<PathBuf, Option<SystemTime>>;

/// Walk the app directory, recording what a rebuild would care about.
///
/// Generated files are left out — the libraries and the JavaScript that
/// `build` produces, and the TypeScript declarations it writes — because a
/// build changes them, and a change would trigger a build.
fn fingerprint(dir: &Path) -> Fingerprint {
    let mut generated: Vec<String> = vec!["apiplant.d.ts".to_string(), "tsconfig.json".to_string()];
    for source in crate::compile::discover(&dir.join("functions")).unwrap_or_default() {
        generated.push(source.library_name());
    }

    let mut into = BTreeMap::new();
    walk(dir, &generated, &mut into);
    into
}

fn walk(dir: &Path, generated: &[String], into: &mut Fingerprint) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Dotfiles are an editor's swap files and a repository's own
        // bookkeeping; neither is the app.
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if !IGNORED_DIRS.contains(&name.as_str()) {
                walk(&path, generated, into);
            }
            continue;
        }
        if generated.contains(&name) {
            continue;
        }
        into.insert(
            path,
            std::fs::metadata(entry.path())
                .and_then(|m| m.modified())
                .ok(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apiplant-watch-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_edit_changes_the_fingerprint_and_a_build_does_not() {
        let dir = temp_dir("fingerprint");
        std::fs::create_dir_all(dir.join("functions")).unwrap();
        std::fs::write(dir.join("main.toml"), "name = \"app\"\n").unwrap();
        std::fs::write(dir.join("functions/greet.ts"), "export default () => 1\n").unwrap();

        let before = fingerprint(&dir);

        // What `build` writes is invisible: the transpiled function, the
        // declarations, and the scaffolding directory.
        std::fs::write(dir.join("functions/greet.js"), "1").unwrap();
        std::fs::write(dir.join("functions/apiplant.d.ts"), "declare…").unwrap();
        std::fs::create_dir_all(dir.join(".apiplant-build/target")).unwrap();
        std::fs::write(dir.join(".apiplant-build/target/x"), "…").unwrap();
        assert_eq!(fingerprint(&dir), before, "a build must not trigger itself");

        // A new source, a new model, and a deletion each are a change.
        std::fs::write(dir.join("functions/greet.toml"), "name = \"greet\"\n").unwrap();
        let after_config = fingerprint(&dir);
        assert_ne!(after_config, before);

        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("models/note.toml"), "name = \"note\"\n").unwrap();
        let after_model = fingerprint(&dir);
        assert_ne!(after_model, after_config);

        std::fs::remove_file(dir.join("models/note.toml")).unwrap();
        assert_eq!(fingerprint(&dir), after_config);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn editor_noise_is_not_a_change() {
        let dir = temp_dir("noise");
        std::fs::write(dir.join("main.toml"), "name = \"app\"\n").unwrap();
        let before = fingerprint(&dir);

        std::fs::write(dir.join(".main.toml.swp"), "…").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "…").unwrap();
        assert_eq!(fingerprint(&dir), before);

        std::fs::remove_dir_all(&dir).ok();
    }
}
