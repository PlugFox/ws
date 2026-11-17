//! Optional account contacts end to end: registration with and without an
//! email / Telegram (validation, normalisation, case-insensitive
//! uniqueness), `GET /api/auth/me`, `PATCH /api/auth/contacts`, and the
//! admin MCP `get_account` that shows them.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use game_server::{config::AppConfig, create_app};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const ADMIN: &str = "test-admin-token-contacts-0123456789";

async fn call(app: &axum::Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header("X-Forwarded-For", "127.0.0.1");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(serde_json::to_vec(&b).unwrap())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn register(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    call(app, "POST", "/api/auth/register", None, Some(body)).await
}

fn code(v: &Value) -> (&str, &str) {
    (v["code"].as_str().unwrap_or(""), v["field"].as_str().unwrap_or(""))
}

#[tokio::test]
async fn contacts_at_registration_me_and_update() {
    let mut config = AppConfig::test_config().await;
    config.admin_token = Some(ADMIN.into());
    let (app, _) = create_app(config).await.unwrap();

    // Optional: none, nulls and empty strings are all "not given".
    let (status, plain) = register(&app, json!({"username": "plain", "password": "secret123"})).await;
    assert_eq!(status, StatusCode::CREATED, "{plain}");
    assert_eq!((plain["user"]["email"].clone(), plain["user"]["telegram"].clone()), (Value::Null, Value::Null));
    let (status, _) = register(&app, json!({"username": "plain2", "password": "secret123", "email": "", "telegram": null})).await;
    assert_eq!(status, StatusCode::CREATED);

    // Given: normalised (domain lowercased; @ and t.me/ stripped).
    let (status, ann) = register(
        &app,
        json!({"username": "ann", "password": "secret123", "email": "  Ann.Lee@GMail.COM ", "telegram": "https://t.me/Ann_Lee"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ann}");
    assert_eq!(ann["user"]["email"], "Ann.Lee@gmail.com");
    assert_eq!(ann["user"]["telegram"], "Ann_Lee");
    let token = ann["access_token"].as_str().unwrap().to_string();

    // Invalid values: 400 with the code and the field; nothing created.
    let (status, v) = register(&app, json!({"username": "bad1", "password": "secret123", "email": "ann@localhost"})).await;
    assert_eq!((status, code(&v)), (StatusCode::BAD_REQUEST, ("email_invalid", "email")));
    let (status, v) = register(&app, json!({"username": "bad1", "password": "secret123", "telegram": "@abc"})).await;
    assert_eq!((status, code(&v)), (StatusCode::BAD_REQUEST, ("telegram_invalid", "telegram")));
    let (status, _) = register(&app, json!({"username": "bad1", "password": "secret123"})).await;
    assert_eq!(status, StatusCode::CREATED, "the failed attempts created nothing");

    // Taken in any case: 409.
    let (status, v) = register(&app, json!({"username": "bob", "password": "secret123", "email": "ANN.LEE@gmail.com"})).await;
    assert_eq!((status, code(&v)), (StatusCode::CONFLICT, ("email_taken", "email")));
    let (status, v) = register(&app, json!({"username": "bob", "password": "secret123", "telegram": "@ann_lee"})).await;
    assert_eq!((status, code(&v)), (StatusCode::CONFLICT, ("telegram_taken", "telegram")));

    // `/me` shows them to their owner, unverified.
    let (status, me) = call(&app, "GET", "/api/auth/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        me["user"]["contacts"],
        json!({"email": "Ann.Lee@gmail.com", "email_verified": false, "email_google": true, "telegram": "Ann_Lee", "telegram_verified": false})
    );

    // Update: omitted stays, null / "" clears, a value replaces.
    let patch = |body: Value| call(&app, "PATCH", "/api/auth/contacts", Some(&token), Some(body));
    let (status, v) = patch(json!({"telegram": null})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["contacts"]["email"].as_str(), v["contacts"]["telegram"].as_str()), (Some("Ann.Lee@gmail.com"), None));
    let (_, v) = patch(json!({"email": "ann@Work.example", "telegram": "@ann_lee"})).await;
    assert_eq!((v["contacts"]["email"].as_str(), v["contacts"]["telegram"].as_str()), (Some("ann@work.example"), Some("ann_lee")));
    assert_eq!(v["contacts"]["email_google"], false);
    let (status, v) = patch(json!({"email": "not an email"})).await;
    assert_eq!((status, code(&v)), (StatusCode::BAD_REQUEST, ("email_invalid", "email")));
    // Its own value again (another case) is not "taken".
    let (status, _) = patch(json!({"email": "ANN@work.example"})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, v) = patch(json!({"email": ""})).await;
    assert_eq!((status, v["contacts"]["email"].clone()), (StatusCode::OK, Value::Null));
    let (status, v) = patch(json!({})).await;
    assert_eq!((status, v["contacts"]["telegram"].as_str()), (StatusCode::OK, Some("ann_lee")), "nothing given, nothing changed");

    // Another account cannot take Ann's; the freed email is available.
    let (_, bob) = register(&app, json!({"username": "bob", "password": "secret123", "email": "ann.lee@gmail.com"})).await;
    assert_eq!(bob["user"]["email"], "ann.lee@gmail.com", "{bob}");
    let bob_token = bob["access_token"].as_str().unwrap().to_string();
    let (status, v) = call(&app, "PATCH", "/api/auth/contacts", Some(&bob_token), Some(json!({"telegram": "ANN_LEE"}))).await;
    assert_eq!((status, code(&v)), (StatusCode::CONFLICT, ("telegram_taken", "telegram")));

    // Signed-in only.
    let (status, _) = call(&app, "PATCH", "/api/auth/contacts", None, Some(json!({"email": ""}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The admin MCP account lookup includes them (marked PII in its description).
    let rpc = |name: &str, args: Value| {
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})
    };
    let (status, v) = call(&app, "POST", "/mcp", Some(ADMIN), Some(rpc("get_account", json!({"username": "ANN"})))).await;
    assert_eq!(status, StatusCode::OK);
    let acc = &v["result"]["structuredContent"];
    assert_eq!((acc["username"].as_str(), acc["contacts"]["telegram"].as_str()), (Some("ann"), Some("ann_lee")), "{v}");
    assert_eq!(acc["contacts"]["email"], Value::Null);
    assert!(acc.get("password_hash").is_none());
    let (_, list) = call(&app, "POST", "/mcp", Some(ADMIN), Some(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))).await;
    let tool = list["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "get_account").unwrap();
    assert!(tool["description"].as_str().unwrap().contains("PII"));
    // The character tools do not carry contacts.
    let (_, v) = call(&app, "POST", "/mcp", Some(ADMIN), Some(rpc("search_characters", json!({"query": "a"})))).await;
    assert!(!v.to_string().contains("ann_lee"));
}
