//! Getting an API key out of the browser and into the console.
//!
//! Typing a key into a terminal means finding it first — signing into the
//! dashboard, minting one, then copying a secret between two windows. Instead
//! the console opens a one-request web server on the loopback interface and
//! sends the operator to the dashboard with its address attached; the dashboard
//! mints a key for whoever is already signed in there and posts it straight
//! back.
//!
//! The listener is deliberately small: it binds to `127.0.0.1` on a port the
//! kernel picks, answers exactly one handoff, and closes. It is not reachable
//! from another machine, and it is gone as soon as the key arrives.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::api::encode;

/// A listener waiting for the dashboard to hand a key back.
pub struct Handoff {
    listener: TcpListener,
    /// The dashboard address to open, callback included.
    pub url: String,
}

impl Handoff {
    /// Bind the callback port and work out where to send the browser.
    ///
    /// `admin_url` is the dashboard root; the route lives in the hash, so this
    /// works for a dashboard served from a static directory as well as one
    /// served by the app.
    pub async fn start(admin_url: &str, label: &str) -> Result<Handoff> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("could not open a port for the browser to answer on")?;
        let port = listener.local_addr()?.port();
        let url = format!(
            "{}#/cli?callback={}&name={}",
            admin_url,
            encode(&format!("http://127.0.0.1:{port}/")),
            encode(label),
        );
        Ok(Handoff { listener, url })
    }

    /// Wait for the key. Resolves when the dashboard posts one.
    pub async fn wait(self) -> Result<String> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .context("the browser handoff was interrupted")?;
            // A browser opens speculative connections and sends preflights; only
            // a request that actually carries a key ends the wait.
            if let Some(key) = serve(stream).await? {
                return Ok(key);
            }
        }
    }
}

/// Answer one HTTP request, returning the key it carried if it had one.
async fn serve(mut stream: TcpStream) -> Result<Option<String>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];

    // Read to the end of the headers first — the body length is in them.
    let head_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = find(&buffer, b"\r\n\r\n") {
            break at + 4;
        }
        // Nothing legitimate is this big; a request that keeps growing without
        // finishing its headers is not a browser handing us a key.
        if buffer.len() > 64 * 1024 {
            return Ok(None);
        }
    };

    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    // A cross-origin POST of plain text needs no preflight, but a dashboard on
    // https will send one anyway if it sets a JSON content type.
    if method == "OPTIONS" {
        respond(&mut stream, "204 No Content", "text/plain", "").await?;
        return Ok(None);
    }

    let length: usize = header(&head, "content-length")
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();

    let key = key_from(&target, &body);
    match &key {
        Some(_) => {
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", DONE_PAGE).await?;
        }
        None => {
            respond(
                &mut stream,
                "404 Not Found",
                "text/plain",
                "no key in that request",
            )
            .await?;
        }
    }
    Ok(key)
}

/// Pull the key out of a request body or a `?key=` query, whichever came.
fn key_from(target: &str, body: &str) -> Option<String> {
    let body = body.trim();
    if !body.is_empty() {
        // The dashboard posts JSON when it can and bare text when a preflight
        // would be in the way; accept both.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(key) = value.get("api_key").and_then(|v| v.as_str()) {
                return non_empty(key);
            }
        }
        if !body.starts_with('{') {
            return non_empty(body);
        }
    }
    let query = target.split_once('?').map(|(_, q)| q)?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "key" || *key == "api_key")
        .and_then(|(_, value)| non_empty(&decode(value)))
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Access-Control-Allow-Methods: POST, GET, OPTIONS\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Connected</title>\
<body style=\"font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
<div style=\"text-align:center\"><h1 style=\"font-size:18px\">The console is connected.</h1>\
<p style=\"color:#666\">You can close this tab and go back to your terminal.</p></div>";

/// Ask the desktop to open a URL. Best effort — the console always prints the
/// address too, because plenty of terminals are on the far side of an SSH
/// connection with no browser to open.
pub fn open_browser(url: &str) -> bool {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"]), ("wslview", &[])]
    };

    for (program, leading) in candidates {
        let opened = std::process::Command::new(program)
            .args(*leading)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if opened {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_read_from_json_text_or_query() {
        assert_eq!(
            key_from("/", r#"{"api_key":"ap_live_1"}"#).as_deref(),
            Some("ap_live_1")
        );
        assert_eq!(key_from("/", "ap_live_2\n").as_deref(), Some("ap_live_2"));
        assert_eq!(
            key_from("/?key=ap%5Flive%5F3", "").as_deref(),
            Some("ap_live_3")
        );
        assert_eq!(key_from("/", "").as_deref(), None);
        // An object without the key is not a key, even though it is a body.
        assert_eq!(key_from("/", r#"{"hello":"world"}"#).as_deref(), None);
    }

    #[tokio::test]
    async fn the_callback_url_names_the_port_we_are_listening_on() {
        let handoff = Handoff::start("http://localhost:8080/admin/", "laptop")
            .await
            .unwrap();
        let port = handoff.listener.local_addr().unwrap().port();
        assert!(handoff
            .url
            .starts_with("http://localhost:8080/admin/#/cli?callback="));
        assert!(handoff
            .url
            .contains(&encode(&format!("http://127.0.0.1:{port}/"))));
        assert!(handoff.url.ends_with("&name=laptop"));
    }
}
