//! Claiming the listening socket, and what to say when the port is taken.
//!
//! "Address already in use" is the single most common way starting the server
//! fails, and the raw `os error 98` says neither which port was wanted nor what
//! to do about it. So the socket is opened here, up front, rather than left to
//! the HTTP server: on a collision the person gets the port named back to them
//! and an offer of the next free one, and the listener that comes out is handed
//! to `listen`/`listen_rustls`.
//!
//! Without a terminal on stdin there is nobody to answer, so the offer is
//! skipped and the error carries the same advice as text instead.

use std::io::{IsTerminal, Write};
use std::net::TcpListener;

/// How far above the wanted port to look when offering an alternative.
const SEARCH_RANGE: u16 = 64;

/// Bind `host:port`, offering another port if that one is taken.
///
/// Returns the listener and the port actually claimed, which is not
/// necessarily the one asked for.
pub fn listener(host: &str, port: u16) -> anyhow::Result<(TcpListener, u16)> {
    let mut port = port;
    loop {
        match TcpListener::bind((host, port)) {
            Ok(lst) => return Ok((lst, port)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                port = ask(host, port)?;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("cannot bind {host}:{port}")))
            }
        }
    }
}

/// Report the collision and return the port to try next.
///
/// Errors out — rather than looping forever — when the answer is "no" or when
/// there is no one to ask.
fn ask(host: &str, port: u16) -> anyhow::Result<u16> {
    let s = Style::detect();
    let free = free_port(host, port);

    eprintln!();
    eprintln!(
        "  {r}{b}Port {port} is already in use.{x}",
        r = s.red,
        b = s.bold,
        x = s.reset
    );
    eprintln!(
        "  {d}Something else is listening on {host}:{port}.{x}",
        d = s.dim,
        x = s.reset
    );
    eprintln!();

    if !std::io::stdin().is_terminal() {
        match free {
            Some(free) => anyhow::bail!("port {port} is already in use — try --port {free}"),
            None => anyhow::bail!("port {port} is already in use"),
        }
    }

    let prompt = match free {
        Some(free) => format!("  Start on port {free} instead? [Y/n/port] "),
        None => "  Start on another port? [n/port] ".to_string(),
    };

    loop {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();

        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            anyhow::bail!("port {port} is already in use");
        }
        let answer = line.trim();

        match answer.to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => match free {
                Some(free) => return Ok(free),
                // No default to accept: fall through and ask again.
                None => eprintln!(
                    "  {d}Enter a port number, or n to give up.{x}",
                    d = s.dim,
                    x = s.reset
                ),
            },
            "n" | "no" => anyhow::bail!("port {port} is already in use"),
            _ => match answer.parse::<u16>() {
                Ok(0) => eprintln!(
                    "  {d}0 would pick a random port; name one instead.{x}",
                    d = s.dim,
                    x = s.reset
                ),
                Ok(p) => return Ok(p),
                Err(_) => eprintln!("  {d}Not a port number.{x}", d = s.dim, x = s.reset),
            },
        }
    }
}

/// The first free port above `port`, if there is one nearby.
fn free_port(host: &str, port: u16) -> Option<u16> {
    (port.saturating_add(1)..=port.saturating_add(SEARCH_RANGE))
        .find(|p| TcpListener::bind((host, *p)).is_ok())
}

/// ANSI escapes, or empty strings when stderr is not a terminal.
struct Style {
    bold: &'static str,
    red: &'static str,
    dim: &'static str,
    reset: &'static str,
}

impl Style {
    fn detect() -> Self {
        if std::io::stderr().is_terminal() {
            Style {
                bold: "\x1b[1m",
                red: "\x1b[31m",
                dim: "\x1b[2m",
                reset: "\x1b[0m",
            }
        } else {
            Style {
                bold: "",
                red: "",
                dim: "",
                reset: "",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_port_is_claimed_as_asked() {
        // Port 0 is "any free port", so this exercises the happy path without
        // guessing at which fixed port a test machine has spare.
        let (lst, port) = listener("127.0.0.1", 0).unwrap();
        assert_eq!(port, 0);
        assert!(lst.local_addr().unwrap().port() != 0);
    }

    #[test]
    fn the_offered_port_is_above_the_taken_one_and_actually_free() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let free = free_port("127.0.0.1", port).expect("no free port nearby");
        assert!(free > port);
        assert!(TcpListener::bind(("127.0.0.1", free)).is_ok());
    }
}
