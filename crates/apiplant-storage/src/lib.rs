//! # apiplant-storage
//!
//! Where uploaded files go, behind one interface so that the rest of the
//! framework never learns which.
//!
//! A `file` field stores a **relative** URL — `/files/2026/08/…-logo.png` — and
//! the server answers that URL by asking this crate for the object behind it.
//! Nothing in a row, an API response or a client names a bucket, which is what
//! makes `backend = "local"` → `backend = "s3"` a four-line configuration
//! change rather than a migration.
//!
//! Two backends:
//!
//! * [`Backend::Local`] writes into a directory. In a container that directory
//!   is a mounted volume, which is the whole of "persistent uploads".
//! * [`Backend::S3`] speaks the S3 REST API, signed with
//!   [AWS Signature Version 4] in [`sign`]. That covers S3 itself, Cloudflare
//!   R2, MinIO, Backblaze B2 and anything else with an S3 front door —
//!   they differ only in `endpoint`, which is why there is one backend and not
//!   four.
//!
//! [AWS Signature Version 4]: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-signing.html

use apiplant_core::StorageConfig;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage is not configured")]
    Disabled,
    #[error("misconfigured storage: {0}")]
    Config(String),
    #[error("{0}")]
    Io(String),
    #[error("{backend} responded {status}: {body}")]
    Provider {
        backend: &'static str,
        status: u16,
        body: String,
    },
    #[error("`{0}` is not a valid storage key")]
    BadKey(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// One stored object, as it comes back out.
pub struct Object {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// The app's file store.
#[derive(Clone)]
pub struct Storage {
    backend: Backend,
    /// URL prefix the stored links carry, e.g. `/files`.
    public_base: String,
    /// Key prefix inside the bucket or directory.
    prefix: String,
    /// Absolute origin to write links against, when the app has opted out of
    /// relative links in favour of a CDN.
    base_url: String,
    max_bytes: u64,
    allowed_types: Vec<String>,
}

#[derive(Clone)]
enum Backend {
    Local {
        root: PathBuf,
    },
    S3 {
        client: reqwest::Client,
        bucket: String,
        region: String,
        /// Origin only, no trailing slash.
        endpoint: String,
        path_style: bool,
        access_key_id: String,
        secret_access_key: String,
    },
}

impl Storage {
    /// Build the store `config` describes, or `None` when the app stores no
    /// files (`backend = "none"`).
    ///
    /// `app_root` resolves a relative `dir`; an absolute one — the usual shape
    /// of a container mount — is used as written.
    pub fn connect(config: &StorageConfig, app_root: &Path) -> Result<Option<Storage>> {
        if !config.is_active() {
            return Ok(None);
        }

        let backend = match config.backend.trim().to_lowercase().as_str() {
            "local" | "dir" | "directory" | "file" | "fs" => {
                let dir = Path::new(config.dir.trim());
                let root = match dir.is_absolute() {
                    true => dir.to_path_buf(),
                    false => app_root.join(dir),
                };
                // Created now rather than on first upload, so a directory that
                // cannot be written is a boot failure and not a 500 next week.
                std::fs::create_dir_all(&root).map_err(|e| {
                    StorageError::Config(format!(
                        "cannot create the storage directory {}: {e}",
                        root.display()
                    ))
                })?;
                Backend::Local { root }
            }
            // R2, MinIO, B2 and the rest are S3 with a different `endpoint`.
            "s3" | "r2" | "minio" | "b2" | "spaces" => {
                if config.bucket.trim().is_empty() {
                    return Err(StorageError::Config(
                        "[storage] backend = \"s3\" needs a `bucket`".to_string(),
                    ));
                }
                if config.access_key_id.trim().is_empty()
                    || config.secret_access_key.trim().is_empty()
                {
                    return Err(StorageError::Config(
                        "[storage] backend = \"s3\" needs `access_key_id` and `secret_access_key`"
                            .to_string(),
                    ));
                }
                let bucket = config.bucket.trim().to_string();
                let region = match config.region.trim() {
                    "" => "auto".to_string(),
                    region => region.to_string(),
                };
                let endpoint = match config.endpoint.trim().trim_end_matches('/') {
                    "" => format!("https://s3.{region}.amazonaws.com"),
                    endpoint => endpoint.to_string(),
                };
                Backend::S3 {
                    client: reqwest::Client::new(),
                    bucket,
                    region,
                    endpoint,
                    path_style: config.uses_path_style(),
                    access_key_id: config.access_key_id.trim().to_string(),
                    secret_access_key: config.secret_access_key.trim().to_string(),
                }
            }
            other => {
                return Err(StorageError::Config(format!(
                    "[storage] backend = \"{other}\" is not one of `local`, `s3` or `none`"
                )))
            }
        };

        Ok(Some(Storage {
            backend,
            public_base: config.normalized_public_base(),
            prefix: config.prefix.trim().trim_matches('/').to_string(),
            base_url: config.base_url.trim().trim_end_matches('/').to_string(),
            max_bytes: config.max_size_bytes(),
            allowed_types: config.allowed_types.clone(),
        }))
    }

    /// `local` or `s3`, for the boot banner.
    pub fn kind(&self) -> &'static str {
        match self.backend {
            Backend::Local { .. } => "local",
            Backend::S3 { .. } => "s3",
        }
    }

    /// Where the store keeps things, for the boot banner: a path or a bucket.
    pub fn location(&self) -> String {
        match &self.backend {
            Backend::Local { root } => root.display().to_string(),
            Backend::S3 {
                bucket, endpoint, ..
            } => format!("{bucket} at {endpoint}"),
        }
    }

    pub fn public_base(&self) -> &str {
        &self.public_base
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Whether an upload declaring `content_type` is accepted.
    pub fn allows_type(&self, content_type: &str) -> bool {
        if self.allowed_types.is_empty() {
            return true;
        }
        let actual = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        self.allowed_types.iter().any(|allowed| {
            let allowed = allowed.trim().to_lowercase();
            match allowed.strip_suffix("/*") {
                Some(group) => actual.split('/').next() == Some(group),
                None => allowed == "*" || allowed == actual,
            }
        })
    }

    /// A fresh key for `filename`.
    ///
    /// Dated, so a directory backend never grows one flat folder with a hundred
    /// thousand entries in it, and prefixed with a UUID, so two people
    /// uploading `logo.png` a second apart do not overwrite each other. The
    /// original name is kept on the end because it is the one thing a person
    /// looking at a URL can use.
    pub fn key_for(&self, filename: &str) -> String {
        let name = sanitize_filename(filename);
        let date = Utc::now().format("%Y/%m");
        let id = uuid::Uuid::new_v4().simple().to_string();
        let key = format!("{date}/{}-{name}", &id[..12]);
        match self.prefix.is_empty() {
            true => key,
            false => format!("{}/{key}", self.prefix),
        }
    }

    /// The link stored in the row: relative by default, absolute when the app
    /// pointed `base_url` at a CDN.
    pub fn url_for(&self, key: &str) -> String {
        match self.base_url.is_empty() {
            true => format!("{}/{key}", self.public_base),
            false => format!(
                "{}/{}/{key}",
                self.base_url,
                self.public_base.trim_matches('/')
            ),
        }
    }

    /// The key a request path names, or `None` when the path escapes the store.
    ///
    /// The path arrives from the network, so `..` and absolute segments are
    /// rejected here rather than trusted to the filesystem.
    pub fn key_from_path(&self, path: &str) -> Option<String> {
        let relative = path
            .trim_start_matches('/')
            .strip_prefix(self.public_base.trim_matches('/'))
            .unwrap_or(path.trim_start_matches('/'))
            .trim_matches('/');
        safe_key(relative)
    }

    /// Store `bytes` under `key`.
    pub async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        let key = safe_key(key).ok_or_else(|| StorageError::BadKey(key.to_string()))?;
        match &self.backend {
            Backend::Local { root } => {
                let path = root.join(&key);
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| StorageError::Io(format!("{}: {e}", parent.display())))?;
                }
                tokio::fs::write(&path, &bytes)
                    .await
                    .map_err(|e| StorageError::Io(format!("{}: {e}", path.display())))
            }
            Backend::S3 { .. } => {
                self.s3("PUT", &key, bytes, Some(content_type)).await?;
                Ok(())
            }
        }
    }

    /// Read the object behind `key`, or `None` when there isn't one.
    pub async fn get(&self, key: &str) -> Result<Option<Object>> {
        let key = safe_key(key).ok_or_else(|| StorageError::BadKey(key.to_string()))?;
        match &self.backend {
            Backend::Local { root } => {
                let path = root.join(&key);
                match tokio::fs::read(&path).await {
                    Ok(bytes) => Ok(Some(Object {
                        content_type: content_type_for(&key).to_string(),
                        bytes,
                    })),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(StorageError::Io(format!("{}: {e}", path.display()))),
                }
            }
            Backend::S3 { .. } => match self.s3("GET", &key, Vec::new(), None).await {
                Ok(Some(bytes)) => Ok(Some(Object {
                    content_type: content_type_for(&key).to_string(),
                    bytes,
                })),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// Remove the object behind `key`. Deleting what is not there succeeds:
    /// the caller wanted it gone, and it is.
    pub async fn delete(&self, key: &str) -> Result<()> {
        let key = safe_key(key).ok_or_else(|| StorageError::BadKey(key.to_string()))?;
        match &self.backend {
            Backend::Local { root } => match tokio::fs::remove_file(root.join(&key)).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(StorageError::Io(e.to_string())),
            },
            Backend::S3 { .. } => {
                self.s3("DELETE", &key, Vec::new(), None).await?;
                Ok(())
            }
        }
    }

    /// One signed S3 request. `Ok(None)` is a 404, which every caller here
    /// treats as "not there" rather than as a failure.
    async fn s3(
        &self,
        method: &str,
        key: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<Option<Vec<u8>>> {
        let Backend::S3 {
            client,
            bucket,
            region,
            endpoint,
            path_style,
            access_key_id,
            secret_access_key,
        } = &self.backend
        else {
            return Err(StorageError::Disabled);
        };

        // Virtual-host style puts the bucket in the hostname; path style puts it
        // in the path, and then it is part of what gets signed.
        let (host, path) = match path_style {
            true => (host_of(endpoint), format!("/{bucket}/{key}")),
            false => (format!("{bucket}.{}", host_of(endpoint)), format!("/{key}")),
        };
        let scheme = match endpoint.starts_with("http://") {
            true => "http",
            false => "https",
        };
        let url = format!("{scheme}://{host}{}", encode_path(&path));

        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let payload_hash = hex::encode(Sha256::digest(&body));

        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), host.clone()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        if let Some(content_type) = content_type {
            headers.push(("content-type".to_string(), content_type.to_string()));
        }
        headers.sort_by(|a, b| a.0.cmp(&b.0));

        let authorization = sign(
            access_key_id,
            secret_access_key,
            region,
            method,
            &encode_path(&path),
            &headers,
            &payload_hash,
            &amz_date,
            &date_stamp,
        );

        let mut request = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|e| StorageError::Config(e.to_string()))?,
                &url,
            )
            .header("authorization", authorization);
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }

        let response = request
            .body(body)
            .send()
            .await
            .map_err(|e| StorageError::Io(format!("s3 ({url}): {e}")))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StorageError::Provider {
                backend: "s3",
                status: status.as_u16(),
                body: body.trim().chars().take(500).collect(),
            });
        }

        Ok(Some(
            response
                .bytes()
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?
                .to_vec(),
        ))
    }
}

/// AWS Signature Version 4, in full: a keyed hash over the canonical form of
/// the request, so the signature covers the method, path, headers and body and
/// nothing can be altered in flight.
///
/// The same forty lines as [`apiplant_email`'s SES signer], for the same
/// reason: this is cheaper to carry than the AWS SDK.
///
/// [`apiplant_email`'s SES signer]: https://docs.rs/apiplant-email
#[allow(clippy::too_many_arguments)]
fn sign(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    canonical_path: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    amz_date: &str,
    date_stamp: &str,
) -> String {
    const SERVICE: &str = "s3";

    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();

    let canonical_request = format!(
        "{method}\n{canonical_path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let scope = format!("{date_stamp}/{region}/{SERVICE}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let hmac = |key: &[u8], data: &str| -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
        mac.update(data.as_bytes());
        mac.finalize().into_bytes().to_vec()
    };
    let date_key = hmac(format!("AWS4{secret_key}").as_bytes(), date_stamp);
    let region_key = hmac(&date_key, region);
    let service_key = hmac(&region_key, SERVICE);
    let signing_key = hmac(&service_key, "aws4_request");
    let signature = hex::encode(hmac(&signing_key, &string_to_sign));

    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

/// The host part of an origin, which is what SigV4 signs and what the `Host`
/// header carries — never the scheme.
fn host_of(endpoint: &str) -> String {
    endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Percent-encode a path for S3, per segment.
///
/// S3 signs the *encoded* path, and it is stricter than a URL parser: `/` stays
/// a separator and everything outside the unreserved set is escaped, including
/// characters a browser would have left alone.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A key that cannot escape the store: relative, no `..`, no empty segments.
fn safe_key(key: &str) -> Option<String> {
    let key = key.trim_matches('/');
    if key.is_empty() || key.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for component in Path::new(key).components() {
        match component {
            Component::Normal(segment) => parts.push(segment.to_str()?.to_string()),
            Component::CurDir => {}
            // `..`, `/`, and Windows prefixes all mean "somewhere else".
            _ => return None,
        }
    }
    match parts.is_empty() {
        true => None,
        false => Some(parts.join("/")),
    }
}

/// Reduce an uploaded filename to something safe to put in a URL and a path.
///
/// The name is decoration — the key is already unique — so this is deliberately
/// brutal: anything that is not a letter, digit, dot or dash becomes a dash.
fn sanitize_filename(filename: &str) -> String {
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .trim()
        .to_lowercase();
    let mut out = String::with_capacity(base.len());
    let mut last_dash = false;
    for ch in base.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '.' => {
                out.push(ch);
                last_dash = false;
            }
            _ if !last_dash && !out.is_empty() => {
                out.push('-');
                last_dash = true;
            }
            _ => {}
        }
    }
    // A dash that only exists because punctuation preceded the extension —
    // `logo (2).png` — reads as noise in every URL it appears in.
    let out = out.replace("-.", ".").trim_matches(['-', '.']).to_string();
    // Long names are somebody's screenshot title, not information.
    let out: String = out.chars().take(80).collect();
    match out.is_empty() {
        true => "file".to_string(),
        false => out,
    }
}

/// The content type a stored key is served as, from its extension.
///
/// Kept here rather than taken from the extension of the *upload* because the
/// key is what survives: the same file served twice is served the same way.
pub fn content_type_for(key: &str) -> &'static str {
    let ext = key.rsplit('.').next().unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        // Not `text/html`: an uploaded file is somebody else's content, and
        // serving it as a document from our own origin is a stored XSS.
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_cannot_escape_the_store() {
        assert_eq!(safe_key("a/b.png").as_deref(), Some("a/b.png"));
        assert_eq!(safe_key("/a/b.png").as_deref(), Some("a/b.png"));
        assert_eq!(safe_key("./a/b.png").as_deref(), Some("a/b.png"));
        assert!(safe_key("../secret").is_none());
        assert!(safe_key("a/../../secret").is_none());
        assert!(safe_key("").is_none());
        assert!(safe_key("/").is_none());
    }

    #[test]
    fn filenames_are_reduced_to_something_url_safe() {
        assert_eq!(sanitize_filename("Logo Final (2).PNG"), "logo-final-2.png");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("  "), "file");
        assert_eq!(sanitize_filename("«»"), "file");
    }

    #[test]
    fn uploaded_html_is_never_served_as_a_document() {
        assert_eq!(content_type_for("x.html"), "application/octet-stream");
        assert_eq!(content_type_for("x.svg"), "image/svg+xml");
        assert_eq!(content_type_for("A/B/c.PNG"), "image/png");
    }

    #[test]
    fn paths_are_encoded_the_way_s3_signs_them() {
        assert_eq!(encode_path("/b/a file.png"), "/b/a%20file.png");
        assert_eq!(encode_path("/b/a-b_c.d~e"), "/b/a-b_c.d~e");
    }

    fn storage(config: StorageConfig) -> Storage {
        Storage::connect(&config, &std::env::temp_dir())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn links_are_relative_unless_a_cdn_is_named() {
        let relative = storage(StorageConfig::default());
        assert_eq!(relative.url_for("2026/08/a.png"), "/files/2026/08/a.png");

        let cdn = storage(StorageConfig {
            base_url: "https://cdn.example.com/".to_string(),
            ..StorageConfig::default()
        });
        assert_eq!(
            cdn.url_for("2026/08/a.png"),
            "https://cdn.example.com/files/2026/08/a.png"
        );
    }

    #[test]
    fn a_request_path_maps_back_to_its_key() {
        let store = storage(StorageConfig::default());
        assert_eq!(
            store.key_from_path("/files/2026/08/a.png").as_deref(),
            Some("2026/08/a.png")
        );
        assert!(store.key_from_path("/files/../../etc/passwd").is_none());
    }

    #[test]
    fn allowed_types_accept_exact_names_and_groups() {
        let images = storage(StorageConfig {
            allowed_types: vec!["image/*".to_string(), "application/pdf".to_string()],
            ..StorageConfig::default()
        });
        assert!(images.allows_type("image/png"));
        assert!(images.allows_type("application/pdf"));
        assert!(images.allows_type("IMAGE/PNG; charset=binary"));
        assert!(!images.allows_type("text/html"));

        assert!(storage(StorageConfig::default()).allows_type("text/html"));
    }

    #[tokio::test]
    async fn a_local_store_round_trips_and_forgets() {
        let store = storage(StorageConfig {
            dir: format!("storage-test-{}", uuid::Uuid::new_v4()),
            ..StorageConfig::default()
        });
        let key = store.key_for("Hello World.png");
        store
            .put(&key, b"bytes".to_vec(), "image/png")
            .await
            .unwrap();

        let object = store.get(&key).await.unwrap().expect("just written");
        assert_eq!(object.bytes, b"bytes");
        assert_eq!(object.content_type, "image/png");

        store.delete(&key).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_none());
        // Deleting what is already gone is not an error.
        store.delete(&key).await.unwrap();
    }
}
