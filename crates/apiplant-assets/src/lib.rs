//! # apiplant-assets
//!
//! The `admin/` and `studio/` front-end builds, embedded in the binary at
//! compile time so `apiplant` can serve either one with no files on disk.
//!
//! Both tables are sorted by path and hold every file the Vite build emitted,
//! hashed asset names included — [`find`] resolves a request path against one.

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// A bundle of embedded files, as `(path relative to the build root, bytes)`.
pub type Assets = &'static [(&'static str, &'static [u8])];

/// Look up a request path in a bundle.
///
/// Leading slashes are ignored, an empty path (or one ending in `/`) resolves
/// to that directory's `index.html`, and `..` is refused outright — a bundle is
/// a flat table, but the request path still arrives from the network.
pub fn find(assets: Assets, path: &str) -> Option<&'static [u8]> {
    let mut wanted = path.trim_matches('/').to_string();
    if wanted.is_empty() {
        wanted = "index.html".to_string();
    }
    if wanted
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return None;
    }

    if let Some(found) = lookup(assets, &wanted) {
        return Some(found);
    }
    // A directory: serve its index, which is how `/some/route/` works.
    lookup(assets, &format!("{wanted}/index.html"))
}

fn lookup(assets: Assets, path: &str) -> Option<&'static [u8]> {
    assets
        .binary_search_by(|(candidate, _)| (*candidate).cmp(path))
        .ok()
        .map(|index| assets[index].1)
}

/// The `Content-Type` to serve a path with, by extension.
pub fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or("");
    match extension {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "map" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_bundles_have_an_index() {
        assert!(find(ADMIN, "/").is_some());
        assert!(find(STUDIO, "").is_some());
    }

    #[test]
    fn traversal_is_refused_and_unknown_paths_are_none() {
        assert!(find(ADMIN, "../../etc/passwd").is_none());
        assert!(find(ADMIN, "nope.js").is_none());
    }

    #[test]
    fn content_types_cover_the_build_output() {
        assert_eq!(
            content_type("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type("assets/index-abc.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type("head.png"), "image/png");
    }
}
