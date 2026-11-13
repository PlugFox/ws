use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use game_server::{
    config::AppConfig,
    create_app,
    db::{CharacterRepo, UserRepo},
    session::StateFlusher,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn setup_test_app() -> (axum::Router, game_server::api::AppState) {
    let config = AppConfig::test_config();
    create_app(config).await.expect("Failed to create test app")
}

#[tokio::test]
async fn test_registration_and_login_flow() {
    let (app, state) = setup_test_app().await;

    // 1. Register a new user
    let register_body = json!({
        "username": "survivor_hero",
        "password": "CorrectPassword123!"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Forwarded-For", "127.0.0.1")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let reg_json: Value = serde_json::from_slice(&body_bytes).unwrap();

    let user_id = reg_json["user"]["id"].as_u64().unwrap() as u32;
    let access_token = reg_json["access_token"].as_str().unwrap().to_string();
    let refresh_token = reg_json["refresh_token"].as_str().unwrap().to_string();

    assert_eq!(reg_json["user"]["username"], "survivor_hero");
    assert!(!access_token.is_empty());
    assert!(!refresh_token.is_empty());

    // Verify password in DB is Argon2id hashed
    let user_in_db = UserRepo::find_by_id(&state.pool, user_id).await.unwrap().unwrap();
    assert!(user_in_db.password_hash.starts_with("$argon2id$"));

    // 2. Test duplicate registration fails with 409 Conflict
    let dup_req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let dup_resp = app.clone().oneshot(dup_req).await.unwrap();
    assert_eq!(dup_resp.status(), StatusCode::CONFLICT);

    // 3. Test login with wrong password fails with 401 Unauthorized
    let wrong_login = json!({
        "username": "survivor_hero",
        "password": "WrongPassword!"
    });
    let wrong_req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&wrong_login).unwrap()))
        .unwrap();

    let wrong_resp = app.clone().oneshot(wrong_req).await.unwrap();
    assert_eq!(wrong_resp.status(), StatusCode::UNAUTHORIZED);

    // 4. Test login with correct password succeeds
    let valid_login = json!({
        "username": "survivor_hero",
        "password": "CorrectPassword123!"
    });
    let login_req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&valid_login).unwrap()))
        .unwrap();

    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);

    let login_bytes = login_resp.into_body().collect().await.unwrap().to_bytes();
    let login_json: Value = serde_json::from_slice(&login_bytes).unwrap();
    assert_eq!(login_json["user"]["id"], user_id);

    // 5. Test /api/auth/me endpoint with access token
    let me_req = Request::builder()
        .method("GET")
        .uri("/api/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .body(Body::empty())
        .unwrap();

    let me_resp = app.clone().oneshot(me_req).await.unwrap();
    assert_eq!(me_resp.status(), StatusCode::OK);

    let me_bytes = me_resp.into_body().collect().await.unwrap().to_bytes();
    let me_json: Value = serde_json::from_slice(&me_bytes).unwrap();
    assert_eq!(me_json["user"]["username"], "survivor_hero");
    assert_eq!(me_json["session"]["country"], "ZZ"); // 127.0.0.1 private loopback
}

#[tokio::test]
async fn test_refresh_token_rotation_and_reuse_detection() {
    let (app, _state) = setup_test_app().await;

    // Register user
    let register_body = json!({
        "username": "token_tester",
        "password": "SecretPassword123!"
    });

    let reg_req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let reg_resp = app.clone().oneshot(reg_req).await.unwrap();
    let reg_bytes = reg_resp.into_body().collect().await.unwrap().to_bytes();
    let reg_json: Value = serde_json::from_slice(&reg_bytes).unwrap();

    let initial_refresh = reg_json["refresh_token"].as_str().unwrap().to_string();

    // 1. Normal rotation (generation 0 -> generation 1)
    let refresh_payload = json!({ "refresh_token": initial_refresh });
    let ref_req = Request::builder()
        .method("POST")
        .uri("/api/auth/refresh")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&refresh_payload).unwrap()))
        .unwrap();

    let ref_resp = app.clone().oneshot(ref_req).await.unwrap();
    assert_eq!(ref_resp.status(), StatusCode::OK);

    let ref_bytes = ref_resp.into_body().collect().await.unwrap().to_bytes();
    let ref_json: Value = serde_json::from_slice(&ref_bytes).unwrap();

    let rotated_refresh = ref_json["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(initial_refresh, rotated_refresh);

    // 2. REUSE ATTACK: Attacker replays `initial_refresh` (generation 0)!
    let reuse_req = Request::builder()
        .method("POST")
        .uri("/api/auth/refresh")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&refresh_payload).unwrap()))
        .unwrap();

    let reuse_resp = app.clone().oneshot(reuse_req).await.unwrap();
    assert_eq!(reuse_resp.status(), StatusCode::UNAUTHORIZED);

    let reuse_bytes = reuse_resp.into_body().collect().await.unwrap().to_bytes();
    let reuse_json: Value = serde_json::from_slice(&reuse_bytes).unwrap();
    assert_eq!(reuse_json["code"], "REFRESH_REUSED");

    // 3. Verify session was permanently revoked due to reuse detection
    let next_req = Request::builder()
        .method("POST")
        .uri("/api/auth/refresh")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "refresh_token": rotated_refresh })).unwrap()))
        .unwrap();

    let next_resp = app.clone().oneshot(next_req).await.unwrap();
    assert_eq!(next_resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_character_crud_and_dirty_flag_flushing() {
    let (app, state) = setup_test_app().await;

    // Register user
    let register_body = json!({
        "username": "game_champ",
        "password": "ChampPassword123!"
    });

    let reg_req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let reg_resp = app.clone().oneshot(reg_req).await.unwrap();
    let reg_bytes = reg_resp.into_body().collect().await.unwrap().to_bytes();
    let reg_json: Value = serde_json::from_slice(&reg_bytes).unwrap();

    let user_id = reg_json["user"]["id"].as_u64().unwrap() as u32;
    let access_token = reg_json["access_token"].as_str().unwrap().to_string();

    // 1. Create Character
    let char_payload = json!({
        "name": "SwiftBlade",
        "archetype": "swordsman"
    });

    let create_char_req = Request::builder()
        .method("POST")
        .uri("/api/characters")
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&char_payload).unwrap()))
        .unwrap();

    let create_resp = app.clone().oneshot(create_char_req).await.unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let create_bytes = create_resp.into_body().collect().await.unwrap().to_bytes();
    let char_json: Value = serde_json::from_slice(&create_bytes).unwrap();
    let char_id = char_json["id"].as_u64().unwrap() as u32;

    assert_eq!(char_json["name"], "SwiftBlade");
    assert_eq!(char_json["archetype"], "swordsman");
    assert_eq!(char_json["stats"]["max_hp"], 12);
    assert_eq!(char_json["stats"]["weapon"]["id"], "sword", "a new swordsman holds a sword");
    assert_eq!(char_json["stats"]["weapon"]["damage"], "1d8+3");
    assert_eq!(char_json["level"], 1);
    assert_eq!(char_json["last_location_id"], "hub_central");

    // 2. Select Character into active session
    let select_req = Request::builder()
        .method("POST")
        .uri(format!("/api/characters/{}/select", char_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .body(Body::empty())
        .unwrap();

    let select_resp = app.clone().oneshot(select_req).await.unwrap();
    assert_eq!(select_resp.status(), StatusCode::OK);

    // 3. Mutate Character in-memory (simulating gameplay XP gain and skill unlock)
    let active_session = state.session_registry.get(user_id).expect("Session must be active");
    {
        let mut char_lock = active_session.character.write().await;
        let char_state = char_lock.as_mut().expect("Character must be loaded");
        assert!(!char_state.is_dirty);

        // Award XP and unlock skill
        char_state.add_exp(500);
        char_state.level = 2;
        char_state.unlock_skill("whirlwind");
        char_state.set_location("stage_field_01".to_string());

        assert!(char_state.is_dirty);
    }

    // 4. Trigger persistence flusher
    let flushed = StateFlusher::flush_session(&state.pool, &active_session).await.unwrap();
    assert!(flushed);

    // Verify dirty flag is now cleared
    {
        let char_lock = active_session.character.read().await;
        assert!(!char_lock.as_ref().unwrap().is_dirty);
    }

    // 5. Query character from DB and verify persisted values
    let db_char = CharacterRepo::find_by_id(&state.pool, char_id).await.unwrap().unwrap();
    assert_eq!(db_char.level, 2);
    assert_eq!(db_char.base_exp, 500);
    assert_eq!(db_char.last_location_id, "stage_field_01");

    let unlocked_skills: Vec<String> = serde_json::from_str(&db_char.unlocked_skills_json).unwrap();
    assert!(unlocked_skills.contains(&"whirlwind".to_string()));
}
