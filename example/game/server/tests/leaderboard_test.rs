//! The leaderboard end to end: counters credited to a selected character
//! reach the database with the periodic save, and `GET /api/leaderboard`
//! ranks them (pages, the own row, bad parameters, the cache header).

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use game_server::{config::AppConfig, create_app, session::StateFlusher, world::record::Record};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(app: &axum::Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value, Option<String>) {
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
    let cache = resp.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cache)
}

#[tokio::test]
async fn counters_are_saved_with_the_character_and_ranked() {
    let (app, state) = create_app(AppConfig::test_config().await).await.unwrap();
    let (_, reg, _) = call(&app, "POST", "/api/auth/register", None, Some(json!({"username": "ranker", "password": "CorrectPassword123!"}))).await;
    let token = reg["access_token"].as_str().unwrap().to_string();
    let user_id = reg["user"]["id"].as_u64().unwrap() as u32;
    let mut ids = Vec::new();
    for name in ["Ann", "Bob", "Cid"] {
        let (status, ch, _) = call(&app, "POST", "/api/characters", Some(&token), Some(json!({"name": name, "archetype": "mage"}))).await;
        assert_eq!(status, StatusCode::CREATED);
        ids.push(ch["id"].as_i64().unwrap());
    }

    // Bob plays: the instance reports counters twice; one save stores both.
    let (status, _, _) = call(&app, "POST", &format!("/api/characters/{}/select", ids[1]), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let session = state.session_registry.get(user_id).unwrap();
    {
        let mut ch = session.character.write().await;
        let ch = ch.as_mut().unwrap();
        ch.add_record(&Record { kills: 40, mvps: 1, deaths: 2, missions: 1, best_combo: 33 });
        ch.add_record(&Record { kills: 2, best_combo: 12, ..Record::default() });
        assert!(ch.is_dirty);
    }
    assert!(StateFlusher::flush_session(&state.pool, &session).await.unwrap());

    let (status, board, cache) = call(&app, "GET", &format!("/api/leaderboard?sort=kills&limit=2&me={}", ids[2]), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache.as_deref(), Some("public, max-age=10"));
    assert_eq!((board["sort"].as_str(), board["page"].as_u64(), board["pages"].as_u64(), board["total"].as_u64()), (Some("kills"), Some(1), Some(2), Some(3)));
    let top = &board["entries"][0];
    assert_eq!((top["rank"].as_i64(), top["name"].as_str(), top["class"].as_str(), top["title"].as_str()), (Some(1), Some("Bob"), Some("mage"), Some("Mage")));
    assert_eq!((top["kills"].as_i64(), top["mvps"].as_i64(), top["deaths"].as_i64(), top["missions"].as_i64(), top["best_combo"].as_i64()), (Some(42), Some(1), Some(2), Some(1), Some(33)));
    assert!(top.get("base_exp").is_none(), "experience stays private");
    assert_eq!((board["entries"][1]["name"].as_str(), board["entries"][1]["rank"].as_i64()), (Some("Ann"), Some(2)));
    // Cid ties Ann at zero kills: rank 2, on page 2.
    assert_eq!((board["me"]["name"].as_str(), board["me"]["rank"].as_i64()), (Some("Cid"), Some(2)));
    let (_, page2, _) = call(&app, "GET", "/api/leaderboard?sort=kills&limit=2&page=2", None, None).await;
    assert_eq!(page2["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page2["entries"][0]["rank"].as_i64(), Some(2));
    assert!(page2["me"].is_null());

    // Reselecting loads the stored counters (a relog keeps them).
    let (status, _, _) = call(&app, "POST", &format!("/api/characters/{}/select", ids[1]), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let session = state.session_registry.get(user_id).unwrap();
    assert_eq!(session.character.read().await.as_ref().unwrap().record.kills, 42);

    let (status, _, _) = call(&app, "GET", "/api/leaderboard?sort=charisma", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, capped, _) = call(&app, "GET", "/api/leaderboard?limit=5000&page=0", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((capped["sort"].as_str(), capped["limit"].as_u64(), capped["page"].as_u64()), (Some("level"), Some(50), Some(1)));
}
