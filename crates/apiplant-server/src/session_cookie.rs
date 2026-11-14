//! The session token, carried in a cookie as well as in the response body.
//!
//! Switched on by `[auth] cookie = true` (see `AuthConfig::cookie`). The token
//! is the same JWT [`Authenticator::issue_session`] signs either way. This
//! module only adds a second place to carry it, and a second place to read it
//! from:
//!
//! * every response that hands out a session also sets it, `HttpOnly`, as
//!   `<cookie_name>`, beside a readable `<cookie_name>_csrf` holding a random
//!   token;
//! * a request with no `Authorization` header and no API key is authenticated
//!   from the cookie, and if it changes anything it must repeat the CSRF token
//!   in `X-CSRF-Token` (double-submit). A cookie is attached to every request
//!   the browser makes to this host, including the ones another site makes it
//!   send; a header can only be written by a script running on this origin;
//! * `POST <base>/auth/logout` clears both.
//!
//! A header always wins over the cookie. A script that sends a bearer token is
//! saying which session it means, and the cookie the browser attaches
//! regardless has no say.
//!
//! [`Authenticator::issue_session`]: apiplant_auth::Authenticator::issue_session

use apiplant_core::config::AuthConfig;
use ntex::http::Method;
use ntex::web::{HttpRequest, HttpResponse};

/// The header a cookie-authenticated, state-changing request repeats the CSRF
/// token in.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// The readable cookie the CSRF token is kept in.
pub fn csrf_cookie_name(config: &AuthConfig) -> String {
    format!("{}_csrf", config.cookie_name)
}

/// Set the session (and a fresh CSRF token) on a response that has just
/// issued `token`. Does nothing unless `[auth] cookie` is on.
pub fn attach(config: &AuthConfig, response: &mut HttpResponse, token: &str) {
    if !config.cookie {
        return;
    }
    let max_age = config.session_ttl_secs;
    let csrf = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    append(
        response,
        cookie(config, &config.cookie_name, token, max_age, true),
    );
    append(
        response,
        cookie(config, &csrf_cookie_name(config), &csrf, max_age, false),
    );
}

/// [`attach`], for a response being returned as it is built.
pub fn respond(config: &AuthConfig, mut response: HttpResponse, token: &str) -> HttpResponse {
    attach(config, &mut response, token);
    response
}

/// Expire both cookies. Does nothing unless `[auth] cookie` is on.
pub fn clear(config: &AuthConfig, response: &mut HttpResponse) {
    if !config.cookie {
        return;
    }
    append(response, cookie(config, &config.cookie_name, "", 0, true));
    append(
        response,
        cookie(config, &csrf_cookie_name(config), "", 0, false),
    );
}

/// The session token a request carries in its cookie, provided it may use it.
///
/// `None` when cookies are off, when there is no cookie, and when the request
/// changes state without a CSRF header that matches its CSRF cookie.
pub fn session_token(config: &AuthConfig, req: &HttpRequest) -> Option<String> {
    if !config.cookie {
        return None;
    }
    let token = read(req, &config.cookie_name)?;
    if token.is_empty() {
        return None;
    }
    if !is_safe(req.method()) {
        let expected = read(req, &csrf_cookie_name(config))?;
        let sent = req.headers().get(CSRF_HEADER)?.to_str().ok()?;
        if expected.is_empty() || !constant_time_eq(expected.as_bytes(), sent.trim().as_bytes()) {
            return None;
        }
    }
    Some(token)
}

/// Methods that, by HTTP's own rules, change nothing, and so need no proof of
/// where they came from.
fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// One cookie's value, read from every `Cookie` header the request carries.
fn read(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers()
        .get_all("cookie")
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim_matches('"').to_string())
}

fn cookie(config: &AuthConfig, name: &str, value: &str, max_age: u64, http_only: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; Max-Age={max_age}; SameSite=Lax");
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if config.cookie_secure {
        cookie.push_str("; Secure");
    }
    if !config.cookie_domain.is_empty() {
        cookie.push_str("; Domain=");
        cookie.push_str(&config.cookie_domain);
    }
    cookie
}

fn append(response: &mut HttpResponse, cookie: String) {
    if let Ok(value) = ntex::http::header::HeaderValue::from_str(&cookie) {
        response
            .headers_mut()
            .append(ntex::http::header::SET_COOKIE, value);
    }
}

/// Compare without stopping at the first difference, so the time a refusal
/// takes says nothing about how much of the token was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntex::web::test::TestRequest;

    fn on() -> AuthConfig {
        AuthConfig {
            cookie: true,
            ..AuthConfig::default()
        }
    }

    #[test]
    fn nothing_is_set_or_read_when_cookies_are_off() {
        let config = AuthConfig::default();
        let mut response = HttpResponse::Ok().build();
        attach(&config, &mut response, "jwt");
        assert!(response.headers().get("set-cookie").is_none());

        let req = TestRequest::default()
            .header("cookie", "apiplant_session=jwt")
            .to_http_request();
        assert_eq!(session_token(&config, &req), None);
    }

    #[test]
    fn issuing_sets_an_http_only_session_and_a_readable_csrf_token() {
        let mut response = HttpResponse::Ok().build();
        attach(&on(), &mut response, "jwt");
        let cookies: Vec<_> = response
            .headers()
            .get_all("set-cookie")
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].starts_with("apiplant_session=jwt;"));
        assert!(cookies[0].contains("HttpOnly") && cookies[0].contains("Secure"));
        assert!(cookies[1].starts_with("apiplant_session_csrf="));
        assert!(!cookies[1].contains("HttpOnly"));
    }

    #[test]
    fn a_read_needs_no_csrf_token() {
        let req = TestRequest::default()
            .header("cookie", "other=1; apiplant_session=jwt")
            .to_http_request();
        assert_eq!(session_token(&on(), &req).as_deref(), Some("jwt"));
    }

    #[test]
    fn a_write_needs_the_matching_csrf_header() {
        let cookies = "apiplant_session=jwt; apiplant_session_csrf=abc";
        let without = TestRequest::default()
            .method(Method::POST)
            .header("cookie", cookies)
            .to_http_request();
        assert_eq!(session_token(&on(), &without), None);

        let wrong = TestRequest::default()
            .method(Method::POST)
            .header("cookie", cookies)
            .header(CSRF_HEADER, "abd")
            .to_http_request();
        assert_eq!(session_token(&on(), &wrong), None);

        let right = TestRequest::default()
            .method(Method::DELETE)
            .header("cookie", cookies)
            .header(CSRF_HEADER, "abc")
            .to_http_request();
        assert_eq!(session_token(&on(), &right).as_deref(), Some("jwt"));
    }

    #[test]
    fn clearing_expires_both() {
        let mut response = HttpResponse::Ok().build();
        clear(&on(), &mut response);
        assert!(response
            .headers()
            .get_all("set-cookie")
            .all(|v| v.to_str().unwrap().contains("Max-Age=0")));
    }
}
