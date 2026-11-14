//! Static files: the web client at `/` and the baked asset pack at
//! `/assets`, with precompressed variants, validators and cache headers.
//!
//! Precompressed variants are `<file>.br` / `<file>.gz` beside a file (the
//! image build writes them, `example/game/deploy/precompress.sh`): a client
//! that accepts one gets it, any other the plain file. Every answer carries
//! `Vary: Accept-Encoding`.
//!
//! Validators: every file answer has a strong `ETag` (a SHA-256 of the bytes
//! sent, so `br`, `gzip` and identity each have their own) and
//! `Last-Modified`. `If-None-Match` wins over `If-Modified-Since` (RFC 9110
//! 13.2.2); either answers `304` with the same cache headers. `If-Range` is
//! honoured for ranges (audio), which `ServeDir` serves itself. Hashes are
//! computed on first use and kept while the file's size and mtime hold (a
//! rebuilt file in development is rehashed, never answered with a stale
//! 304); with a build id (the image) every file is hashed once in the
//! background at startup.
//!
//! Nothing in the pack or the client is content-hashed, so the build is the
//! version (`GAME_ASSETS_VERSION`, set by the image):
//! - `/assets/v/<build>/<path>` is the pack under a build id; a client
//!   compiled with `ASSETS_BASE=/assets/v/<build>` asks for these. When
//!   `<build>` is this server's they are fresh for [`VERSIONED`] (a week),
//!   then revalidated. Any other id (a tab of an older or newer build) gets
//!   the same files `no-cache`, so a rollback never pins a wrong pack.
//! - `?v=<build>` on a client file (the page's script tags, stamped by the
//!   image build): the same rule.
//! - brand images (icons, logo, `og.jpg`): a day.
//! - anything else: `no-cache`, always revalidated (a cheap 304): pages
//!   (`/`, `*.html`), `sw.js`, the manifest, unversioned pack files. A page
//!   cached from another build is told apart by its `ETag`, so a rollback
//!   cannot answer it 304.

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tower::{Service, ServiceExt};
use tower_http::services::ServeDir;

/// This build's versioned files: fresh for a week, then revalidated.
pub const VERSIONED: &str = "public, max-age=604800";
/// Always revalidated (`ETag` / `Last-Modified` make that a 304).
pub const REVALIDATE: &str = "no-cache";
/// Brand images (icons, logo): a day.
pub const BRAND: &str = "public, max-age=86400";

/// A file's `ETag` and the size and mtime it was computed for.
struct Tag {
    len: u64,
    modified: Option<SystemTime>,
    etag: HeaderValue,
}

/// Which tree a [`Files`] serves: the routing of its cache policy.
#[derive(Clone, Copy)]
enum Tree {
    Pack,
    Client,
}

/// A static tree: `ServeDir` plus the validators and the cache policy.
#[derive(Clone)]
struct Files {
    tree: Tree,
    root: Arc<Path>,
    serve: ServeDir,
    /// This server's build id (`GAME_ASSETS_VERSION`), `None` in development.
    build: Option<Arc<str>>,
    tags: Arc<DashMap<PathBuf, Tag>>,
}

/// The service type `pack` and `client` return.
pub trait StaticService:
    Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone + Send + Sync + 'static
{
}
impl<S> StaticService for S where
    S: Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone + Send + Sync + 'static
{
}

/// The baked pack, for `nest_service("/assets", ..)`: `/v/<build>/` is
/// stripped before the lookup.
pub fn pack(dir: &Path, build: Option<&str>) -> impl StaticService {
    Files::new(Tree::Pack, dir, build).into_service()
}

/// The built web client, for the router's fallback.
pub fn client(dir: &Path, build: Option<&str>) -> impl StaticService {
    Files::new(Tree::Client, dir, build).into_service()
}

impl Files {
    fn new(tree: Tree, dir: &Path, build: Option<&str>) -> Self {
        let files = Self {
            tree,
            root: Arc::from(dir),
            serve: ServeDir::new(dir).precompressed_br().precompressed_gzip(),
            build: build.map(Arc::from),
            tags: Arc::default(),
        };
        if files.build.is_some() {
            files.warm_up();
        }
        files
    }

    fn into_service(self) -> impl StaticService {
        tower::service_fn(move |req: Request| {
            let files = self.clone();
            async move { Ok::<_, Infallible>(files.call(req).await) }
        })
    }

    /// Hashes every file in the background, so the first requests after a
    /// deploy do not wait for it. Logs the count, size and time.
    fn warm_up(&self) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let files = self.clone();
        rt.spawn_blocking(move || {
            let started = Instant::now();
            let (mut count, mut bytes) = (0usize, 0u64);
            let mut stack = vec![files.root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    match entry.file_type() {
                        Ok(t) if t.is_dir() => stack.push(path),
                        Ok(t) if t.is_file() => {
                            if let Some(len) = files.tag_blocking(&path).map(|(len, _)| len) {
                                count += 1;
                                bytes += len;
                            }
                        }
                        _ => {}
                    }
                }
            }
            tracing::info!(
                "static validators: {count} files, {} MB hashed in {} ms ({})",
                bytes / 1_000_000,
                started.elapsed().as_millis(),
                files.root.display()
            );
        });
    }

    async fn call(self, mut req: Request) -> Response {
        let policy = self.route(&mut req);
        let path = req.uri().path().to_string();

        // `If-Modified-Since` and `If-Range` are evaluated here, against the
        // variant actually sent (`ServeDir` would answer 304 by date alone).
        let headers = req.headers_mut();
        let if_none_match = headers.remove(header::IF_NONE_MATCH);
        let if_modified_since = headers.remove(header::IF_MODIFIED_SINCE);
        let if_range = headers.remove(header::IF_RANGE);
        let retry = if_range.is_some().then(|| (req.method().clone(), req.uri().clone(), req.headers().clone()));

        let mut res = self.serve(req).await;
        let mut etag = self.etag_of(variant_file(&self.root, &path, &res)).await;

        if let Some(cond) = &if_none_match {
            if etag.as_ref().is_some_and(|tag| none_match_hits(cond, tag)) {
                return finish(not_modified(&res, etag), policy);
            }
        } else if let Some(since) = &if_modified_since {
            if res.status() == StatusCode::OK && res.headers().get(header::LAST_MODIFIED) == Some(since) {
                return finish(not_modified(&res, etag), policy);
            }
        }

        // A range of another version than the client holds: the whole file.
        if res.status() == StatusCode::PARTIAL_CONTENT {
            let current = if_range.as_ref().is_none_or(|cond| {
                let strong = etag.as_ref().is_some_and(|tag| cond == tag && !cond.as_bytes().starts_with(b"W/"));
                strong || res.headers().get(header::LAST_MODIFIED) == Some(cond)
            });
            if let (false, Some((method, uri, mut headers))) = (current, retry) {
                headers.remove(header::RANGE);
                let mut full = Request::new(Body::empty());
                (*full.method_mut(), *full.uri_mut(), *full.headers_mut()) = (method, uri, headers);
                res = self.serve(full).await;
                etag = self.etag_of(variant_file(&self.root, &path, &res)).await;
            }
        }
        if let Some(tag) = etag {
            res.headers_mut().insert(header::ETAG, tag);
        }
        finish(res, policy)
    }

    async fn serve(&self, req: Request) -> Response {
        match self.serve.clone().oneshot(req).await {
            Ok(res) => res.map(Body::new),
            Err(never) => match never {},
        }
    }

    /// The cache policy of a request (see the module docs); a pack request
    /// loses its `/v/<build>` prefix.
    fn route(&self, req: &mut Request) -> &'static str {
        let build = self.build.as_deref();
        match self.tree {
            Tree::Pack => {
                let Some((id, rest)) = split_build(req.uri().path()) else { return REVALIDATE };
                let fresh = build == Some(id);
                let rest = match req.uri().query() {
                    Some(q) => format!("{rest}?{q}"),
                    None => rest.to_string(),
                };
                if let Ok(uri) = rest.parse::<Uri>() {
                    *req.uri_mut() = uri;
                }
                if fresh { VERSIONED } else { REVALIDATE }
            }
            Tree::Client => client_policy(req.uri().path(), query_v(req.uri().query()), build),
        }
    }

    /// The `ETag` of a [`variant_file`].
    async fn etag_of(&self, file: Option<PathBuf>) -> Option<HeaderValue> {
        self.tag(file?).await
    }

    /// The cached tag while the file's size and mtime hold, else hashed now.
    async fn tag(&self, file: PathBuf) -> Option<HeaderValue> {
        let meta = tokio::fs::metadata(&file).await.ok()?;
        if let Some(tag) = self.tags.get(&file) {
            if tag.len == meta.len() && tag.modified == meta.modified().ok() {
                return Some(tag.etag.clone());
            }
        }
        let files = self.clone();
        tokio::task::spawn_blocking(move || files.tag_blocking(&file).map(|(_, etag)| etag)).await.ok().flatten()
    }

    /// Hashes `file` and caches the tag: `(size, etag)`.
    fn tag_blocking(&self, file: &Path) -> Option<(u64, HeaderValue)> {
        let mut f = std::fs::File::open(file).ok()?;
        let meta = f.metadata().ok()?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hash.update(&buf[..n]),
                Err(_) => return None,
            }
        }
        // 128 bits are plenty to tell versions of one file apart.
        let digest = hash.finalize();
        let etag = HeaderValue::from_str(&format!("\"{}\"", URL_SAFE_NO_PAD.encode(&digest[..16]))).ok()?;
        self.tags.insert(file.to_path_buf(), Tag { len: meta.len(), modified: meta.modified().ok(), etag: etag.clone() });
        Some((meta.len(), etag))
    }
}

/// The file behind a file answer `res` to `path`: the file, or its
/// `.br` / `.gz` twin by `Content-Encoding`. `None` for anything but a file
/// answer.
fn variant_file(root: &Path, path: &str, res: &Response) -> Option<PathBuf> {
    if !matches!(res.status(), StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
        return None;
    }
    let suffix = match res.headers().get(header::CONTENT_ENCODING).map(HeaderValue::as_bytes) {
        None => "",
        Some(b"br") => ".br",
        Some(b"gzip") => ".gz",
        Some(_) => return None,
    };
    let mut file = file_of(root, path)?;
    file.as_mut_os_string().push(suffix);
    Some(file)
}

/// The file `ServeDir` reads for a URL path: percent-decoded, `index.html`
/// for a directory. `None` for anything it would not serve (`..`).
fn file_of(root: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path)?;
    let mut file = root.to_path_buf();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains('\\') => return None,
            p => file.push(p),
        }
    }
    if decoded.ends_with('/') {
        file.push("index.html");
    }
    Some(file)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether an `If-None-Match` list names `etag` (weak comparison, `*`).
fn none_match_hits(cond: &HeaderValue, etag: &HeaderValue) -> bool {
    let Ok(cond) = cond.to_str() else { return false };
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_string();
    let etag = bare(etag.to_str().unwrap_or_default());
    cond.trim() == "*" || cond.split(',').any(|t| bare(t) == etag)
}

/// A `304` for `res`: its validators only (the cache headers come after).
fn not_modified(res: &Response, etag: Option<HeaderValue>) -> Response {
    let mut out = Response::new(Body::empty());
    *out.status_mut() = StatusCode::NOT_MODIFIED;
    let headers: &mut HeaderMap = out.headers_mut();
    if let Some(tag) = etag {
        headers.insert(header::ETAG, tag);
    }
    if let Some(date) = res.headers().get(header::LAST_MODIFIED) {
        headers.insert(header::LAST_MODIFIED, date.clone());
    }
    out
}

/// Sets the cache policy (`no-cache` on errors: a 404 is not kept) and
/// `Vary: Accept-Encoding`.
fn finish(mut res: Response, policy: &'static str) -> Response {
    let ok = res.status().is_success() || res.status() == StatusCode::NOT_MODIFIED;
    let headers = res.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(if ok { policy } else { REVALIDATE }));
    if !headers.contains_key(header::VARY) {
        headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    res
}

/// `/v/<id>/<rest>` → `(id, "/<rest>")`.
fn split_build(path: &str) -> Option<(&str, &str)> {
    let tail = path.strip_prefix("/v/")?;
    let slash = tail.find('/')?;
    let id = &tail[..slash];
    (!id.is_empty()).then(|| (id, &tail[slash..]))
}

/// The `v=` parameter of a query string.
fn query_v(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|kv| kv.strip_prefix("v="))
}

/// Cache policy of a client file (see the module docs).
fn client_policy(path: &str, v: Option<&str>, build: Option<&str>) -> &'static str {
    if v.is_some() && v == build {
        return VERSIONED;
    }
    let top_level = path.rfind('/') == Some(0);
    let image = [".png", ".ico", ".svg", ".webp", ".jpg"].iter().any(|ext| path.ends_with(ext));
    if top_level && image { BRAND } else { REVALIDATE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prefix_splits_off() {
        assert_eq!(split_build("/v/0.1.0-abc/client/ui/x.png"), Some(("0.1.0-abc", "/client/ui/x.png")));
        assert_eq!(split_build("/client/ui/x.png"), None);
        assert_eq!(split_build("/v//x"), None);
        assert_eq!(split_build("/v/abc"), None);
    }

    #[test]
    fn client_policies() {
        let b = Some("b1");
        assert_eq!(client_policy("/main.dart.js", Some("b1"), b), VERSIONED);
        assert_eq!(client_policy("/main.dart.js", Some("old"), b), REVALIDATE, "another build: revalidate");
        assert_eq!(client_policy("/main.dart.js", None, None), REVALIDATE);
        assert_eq!(client_policy("/", None, b), REVALIDATE);
        assert_eq!(client_policy("/index.html", None, b), REVALIDATE);
        assert_eq!(client_policy("/sw.js", None, b), REVALIDATE);
        assert_eq!(client_policy("/manifest.webmanifest", None, b), REVALIDATE);
        assert_eq!(client_policy("/icon-192.png", None, b), BRAND);
        assert_eq!(client_policy("/og.jpg", None, b), BRAND);
        assert_eq!(query_v(Some("x=1&v=b1")), Some("b1"));
        assert_eq!(query_v(None), None);
    }

    #[test]
    fn url_paths_map_to_files() {
        let root = Path::new("/srv");
        assert_eq!(file_of(root, "/a%20b/c.json"), Some(PathBuf::from("/srv/a b/c.json")));
        assert_eq!(file_of(root, "/"), Some(PathBuf::from("/srv/index.html")));
        assert_eq!(file_of(root, "/x/../y"), None);
        assert_eq!(file_of(root, "/%zz"), None);
    }

    #[test]
    fn if_none_match_lists() {
        let tag = HeaderValue::from_static("\"abc\"");
        for (cond, hit) in [("\"abc\"", true), ("W/\"abc\"", true), ("\"x\", \"abc\"", true), ("*", true), ("\"abd\"", false)] {
            assert_eq!(none_match_hits(&HeaderValue::from_static(cond), &tag), hit, "{cond}");
        }
    }
}
