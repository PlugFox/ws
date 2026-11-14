//! HTTP caching end to end: strong per-encoding `ETag`s, `Last-Modified`,
//! 304 answers to `If-None-Match` / `If-Modified-Since` (the former wins),
//! `If-Range`, and the `Cache-Control` of every route family.

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode},
};
use game_server::api::static_files::{self, BRAND, REVALIDATE, VERSIONED};
use game_server::{config::AppConfig, create_app};
use http_body_util::BodyExt;
use std::path::PathBuf;
use tower::ServiceExt;

/// A temp tree: a pack (`client/a.json` with `.br` and `.gz` twins, an
/// `.ogg`) and a client (`index.html`, `main.dart.js`, an icon).
fn tree() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("game-cache-{}", uuid::Uuid::new_v4()));
    let pack = root.join("pack");
    let web = root.join("web");
    std::fs::create_dir_all(pack.join("client")).unwrap();
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(pack.join("client/a.json"), b"{\"plain\":true}").unwrap();
    std::fs::write(pack.join("client/a.json.br"), b"fake-brotli").unwrap();
    std::fs::write(pack.join("client/a.json.gz"), b"fake-gzip").unwrap();
    std::fs::write(pack.join("client/song.ogg"), (0u8..=255).collect::<Vec<_>>()).unwrap();
    std::fs::write(web.join("index.html"), b"<!doctype html><title>t</title>").unwrap();
    std::fs::write(web.join("main.dart.js"), b"main()").unwrap();
    std::fs::write(web.join("icon-192.png"), b"png").unwrap();
    (pack, web)
}

/// A GET with the given headers.
async fn get<S>(svc: &S, uri: &str, headers: &[(header::HeaderName, &str)]) -> Response<Body>
where
    S: tower::Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible> + Clone,
{
    let mut req = Request::builder().uri(uri);
    for (k, v) in headers {
        req = req.header(k, *v);
    }
    svc.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
}

fn h<'a>(res: &'a Response<Body>, name: header::HeaderName) -> &'a str {
    res.headers().get(name).map(|v| v.to_str().unwrap()).unwrap_or("")
}

async fn body(res: Response<Body>) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn pack_etags_are_per_encoding_and_answer_304() {
    let (dir, _) = tree();
    let pack = static_files::pack(&dir, Some("b1"));
    let uri = "/v/b1/client/a.json";

    let br = get(&pack, uri, &[(header::ACCEPT_ENCODING, "br")]).await;
    let gz = get(&pack, uri, &[(header::ACCEPT_ENCODING, "gzip")]).await;
    let plain = get(&pack, uri, &[]).await;
    for (res, enc) in [(&br, "br"), (&gz, "gzip"), (&plain, "")] {
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(h(res, header::CONTENT_ENCODING), enc);
        assert_eq!(h(res, header::CACHE_CONTROL), VERSIONED);
        assert_eq!(h(res, header::VARY), "accept-encoding");
        assert!(h(res, header::ETAG).starts_with('"') && h(res, header::ETAG).len() > 10, "strong ETag");
        assert!(!h(res, header::LAST_MODIFIED).is_empty());
    }
    let (br_tag, gz_tag, plain_tag) = (h(&br, header::ETAG), h(&gz, header::ETAG), h(&plain, header::ETAG));
    assert!(br_tag != gz_tag && gz_tag != plain_tag && br_tag != plain_tag, "one ETag per encoding");

    // Revalidation of the same variant: 304 with the validators and policy.
    let res = get(&pack, uri, &[(header::ACCEPT_ENCODING, "br"), (header::IF_NONE_MATCH, br_tag)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(h(&res, header::ETAG), br_tag);
    assert_eq!(h(&res, header::CACHE_CONTROL), VERSIONED);
    assert_eq!(h(&res, header::VARY), "accept-encoding");
    assert!(body(res).await.is_empty());
    // A list and a weak form match too.
    let list = format!("\"nope\", W/{gz_tag}");
    let res = get(&pack, uri, &[(header::ACCEPT_ENCODING, "gzip"), (header::IF_NONE_MATCH, &list)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    // The brotli tag does not validate the identity body.
    let res = get(&pack, uri, &[(header::IF_NONE_MATCH, br_tag)]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body(res).await, b"{\"plain\":true}");

    // If-Modified-Since alone: 304 on the exact date.
    let date = h(&plain, header::LAST_MODIFIED).to_string();
    let res = get(&pack, uri, &[(header::IF_MODIFIED_SINCE, &date)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(h(&res, header::ETAG), plain_tag);
    // If-None-Match wins: a stale tag with a current date is a full answer.
    let res = get(&pack, uri, &[(header::IF_NONE_MATCH, "\"stale\""), (header::IF_MODIFIED_SINCE, &date)]).await;
    assert_eq!(res.status(), StatusCode::OK);

    // A changed file (same mtime second, other bytes) gets a new tag: the
    // old one is not answered 304.
    std::fs::write(dir.join("client/a.json"), b"{\"plain\":false}").unwrap();
    let res = get(&pack, uri, &[(header::IF_NONE_MATCH, plain_tag)]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_ne!(h(&res, header::ETAG), plain_tag);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[tokio::test]
async fn pack_policies_by_build() {
    let (dir, _) = tree();
    let pack = static_files::pack(&dir, Some("b1"));
    let res = get(&pack, "/v/old/client/a.json", &[]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(h(&res, header::CACHE_CONTROL), REVALIDATE, "another build id");
    assert!(!h(&res, header::ETAG).is_empty());

    let dev = static_files::pack(&dir, None);
    let res = get(&dev, "/client/a.json", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), REVALIDATE, "unversioned");
    let tag = h(&res, header::ETAG).to_string();
    let res = get(&dev, "/client/a.json", &[(header::IF_NONE_MATCH, &tag)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED, "no-cache still revalidates cheaply");

    let res = get(&pack, "/v/b1/client/missing.json", &[]).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(h(&res, header::CACHE_CONTROL), REVALIDATE, "errors are not kept");
    assert!(res.headers().get(header::ETAG).is_none());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[tokio::test]
async fn ranges_honour_if_range() {
    let (dir, _) = tree();
    let pack = static_files::pack(&dir, Some("b1"));
    let uri = "/v/b1/client/song.ogg";
    let full = get(&pack, uri, &[]).await;
    assert_eq!(h(&full, header::ACCEPT_RANGES), "bytes");
    let tag = h(&full, header::ETAG).to_string();

    let res = get(&pack, uri, &[(header::RANGE, "bytes=10-19")]).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(h(&res, header::ETAG), tag, "a range carries the whole file's tag");
    assert_eq!(body(res).await, (10u8..20).collect::<Vec<_>>());

    let res = get(&pack, uri, &[(header::RANGE, "bytes=10-19"), (header::IF_RANGE, &tag)]).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT, "same version: the range");

    let res = get(&pack, uri, &[(header::RANGE, "bytes=10-19"), (header::IF_RANGE, "\"other\"")]).await;
    assert_eq!(res.status(), StatusCode::OK, "another version: the whole file");
    assert_eq!(h(&res, header::ETAG), tag);
    assert_eq!(body(res).await.len(), 256);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[tokio::test]
async fn client_pages_revalidate_and_scripts_are_versioned() {
    let (_, web) = tree();
    let client = static_files::client(&web, Some("b1"));
    for page in ["/", "/index.html"] {
        let res = get(&client, page, &[]).await;
        assert_eq!(res.status(), StatusCode::OK, "{page}");
        assert_eq!(h(&res, header::CACHE_CONTROL), REVALIDATE, "{page}: no-cache, not no-store");
        let tag = h(&res, header::ETAG).to_string();
        let res = get(&client, page, &[(header::IF_NONE_MATCH, &tag)]).await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED, "{page}");
    }
    let res = get(&client, "/main.dart.js?v=b1", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), VERSIONED);
    let res = get(&client, "/main.dart.js?v=old", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), REVALIDATE);
    let res = get(&client, "/icon-192.png", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), BRAND);
    let _ = std::fs::remove_dir_all(web.parent().unwrap());
}

/// The whole router: static trees mounted, and the API's policies.
#[tokio::test]
async fn router_cache_policies() {
    let (pack, web) = tree();
    let mut config = AppConfig::test_config().await;
    config.assets_version = Some("b1".into());
    config.client_dir = Some(web.clone());
    // The hub reads its maps from `assets_dir`: mount the pack without it.
    let (app, _) = create_app(config).await.unwrap();
    let app = app.nest_service("/pack", static_files::pack(&pack, Some("b1")));

    let res = get(&app, "/pack/v/b1/client/a.json", &[(header::ACCEPT_ENCODING, "br")]).await;
    assert_eq!((res.status(), h(&res, header::CACHE_CONTROL)), (StatusCode::OK, VERSIONED));
    let res = get(&app, "/", &[]).await;
    assert_eq!((res.status(), h(&res, header::CACHE_CONTROL)), (StatusCode::OK, REVALIDATE));
    assert!(!h(&res, header::ETAG).is_empty());

    let res = get(&app, "/api/online", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), "public, max-age=2");
    let res = get(&app, "/api/auth/me", &[]).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(h(&res, header::CACHE_CONTROL), "no-store", "personal answers are never stored");
    let res = get(&app, "/api/characters", &[]).await;
    assert_eq!(h(&res, header::CACHE_CONTROL), "no-store");
    let _ = std::fs::remove_dir_all(web.parent().unwrap());
}
