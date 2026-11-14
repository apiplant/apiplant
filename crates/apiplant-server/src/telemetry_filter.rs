//! The default log filter: the app's `[observability.logs] level`, with the
//! web stack's own chatter turned down.
//!
//! ntex 4 logs at `info` once per worker as it starts (`Starting
//! "main:worker:N" arbiter`), and again per worker for every connection
//! setting left at its default. On a 32-core machine that is 64 lines before
//! the first request, none of them about the app. Those crates are capped at
//! `warn` unless the configured level names them itself, and `RUST_LOG`
//! replaces all of this when it is set.

/// Crates whose `info` lines describe ntex's internals, not the app.
const QUIET: &[&str] = &[
    "ntex",
    "ntex_rt",
    "ntex_server",
    "ntex_service",
    "ntex_io",
    "ntex_net",
    "ntex_h2",
    "ntex_tls",
];

/// `level` plus `<crate>=warn` for every [`QUIET`] crate it does not mention.
pub fn directives(level: &str) -> String {
    let mut directives = level.trim().to_string();
    for name in QUIET {
        let named = directives
            .split(',')
            .any(|d| d.trim().split('=').next() == Some(*name));
        if !named {
            if !directives.is_empty() {
                directives.push(',');
            }
            directives.push_str(name);
            directives.push_str("=warn");
        }
    }
    directives
}

#[cfg(test)]
mod tests {
    use super::directives;

    #[test]
    fn the_web_stack_is_capped_at_warn() {
        let d = directives("info");
        assert!(d.starts_with("info,"));
        assert!(d.contains("ntex_rt=warn") && d.contains("ntex_service=warn"));
    }

    #[test]
    fn a_level_that_names_a_crate_keeps_it() {
        let d = directives("info,ntex_rt=debug");
        assert!(d.contains("ntex_rt=debug"));
        assert!(!d.contains("ntex_rt=warn"));
    }
}
