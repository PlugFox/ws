use super::middleware::AuthUser;
use super::state::AppState;
use crate::auth::contacts::{self, Contact, Invalid};
use crate::auth::{hash_password, verify_password, RefreshToken};
use crate::db::{ContactsUpdate, SessionRepo, UserRecord, UserRepo};
use crate::session::{ActiveSession, StateFlusher};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::IpAddr;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub device_id: Option<String>,
    /// Optional contacts (`auth::contacts`); empty = not given.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub telegram: Option<String>,
}

/// `PATCH /api/auth/contacts`: a field left out stays as it is, `null` or
/// `""` clears it, a value replaces it.
#[derive(Deserialize, Default)]
pub struct ContactsRequest {
    #[serde(default, deserialize_with = "present")]
    pub email: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub telegram: Option<Option<String>>,
}

/// Tells a field given as `null` (`Some(None)`) from one left out (`None`,
/// via `#[serde(default)]`).
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_id: Option<String>,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: u32,
    pub username: String,
    pub status: String,
    pub email: Option<String>,
    pub telegram: Option<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

/// 400 `email_invalid` / `telegram_invalid`, with the `field` for the form.
fn invalid(e: Invalid) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": e.reason, "code": e.contact.invalid_code(), "field": e.contact.field() })),
    )
}

/// 409 `email_taken` / `telegram_taken`. This tells whoever asks that the
/// address is in use: unavoidable while registering or setting it (the
/// alternative, accepting and mailing the owner, needs SMTP). Acceptable
/// for now because login takes the account name only; with recovery by
/// email this wants rate limiting (see ROADMAP Phase 6.27).
fn taken(c: Contact) -> ApiError {
    let what = match c {
        Contact::Email => "That email address is already used by another account.",
        Contact::Telegram => "That Telegram username is already used by another account.",
    };
    (StatusCode::CONFLICT, Json(json!({ "error": what, "code": c.taken_code(), "field": c.field() })))
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err.to_string() })))
}

/// A unique violation of a contact index (a race the pre-check lost) as
/// its 409; other errors as 500.
fn db_error(err: sqlx::Error) -> ApiError {
    let index = err.as_database_error().filter(|e| e.is_unique_violation()).and_then(|e| e.constraint().map(str::to_string));
    match index.as_deref() {
        Some(i) if i == Contact::Email.index() => taken(Contact::Email),
        Some(i) if i == Contact::Telegram.index() => taken(Contact::Telegram),
        Some("users_username_lower_key") => {
            (StatusCode::CONFLICT, Json(json!({ "error": "Username already taken", "code": "USER_EXISTS", "field": "user" })))
        }
        _ => internal(err),
    }
}

/// Fails with `*_taken` when another account (not `me`) uses the value.
async fn ensure_free(state: &AppState, c: Contact, value: Option<&str>, me: Option<i64>) -> Result<(), ApiError> {
    let Some(v) = value else { return Ok(()) };
    let other = match c {
        Contact::Email => UserRepo::find_by_email(&state.pool, v).await,
        Contact::Telegram => UserRepo::find_by_telegram(&state.pool, v).await,
    }
    .map_err(internal)?;
    match other {
        Some(u) if Some(u.id) != me => Err(taken(c)),
        _ => Ok(()),
    }
}

/// The account's contacts as `/me` and the update endpoint show them (the
/// owner's own data: not masked).
fn contacts_json(u: &UserRecord) -> serde_json::Value {
    json!({
        "email": u.email,
        "email_verified": u.email_verified_at.is_some(),
        // A consumer Google address: Google sign-in will be offered (a hint
        // only; see `contacts::is_google_mail`).
        "email_google": u.email.as_deref().is_some_and(contacts::is_google_mail),
        "telegram": u.telegram,
        "telegram_verified": u.telegram_verified_at.is_some(),
    })
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub user: UserResponse,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Serialize)]
pub struct TokenRefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

fn extract_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = forwarded.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }

    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if let Ok(ip) = real_ip.trim().parse::<IpAddr>() {
            return Some(ip);
        }
    }

    // Default to loopback if no proxy headers are set
    Some(IpAddr::from([127, 0, 0, 1]))
}

/// POST /api/auth/register
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RegisterRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let username = payload.username.trim();
    if username.len() < 3 || username.len() > 32 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Username must be between 3 and 32 characters", "field": "user" })),
        ));
    }

    if !username
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Username may only contain letters, numbers, hyphens, and underscores", "field": "user" })),
        ));
    }

    if payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Password must be at least 6 characters long", "field": "password" })),
        ));
    }

    // Optional contacts: checked and normalised before anything is created.
    let email = contacts::normalize_email(payload.email.as_deref().unwrap_or("")).map_err(invalid)?;
    let telegram = contacts::normalize_telegram(payload.telegram.as_deref().unwrap_or("")).map_err(invalid)?;

    // Check if username already exists
    if let Ok(Some(_)) = UserRepo::find_by_username(&state.pool, username).await {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "Username already taken", "code": "USER_EXISTS", "field": "user" })),
        ));
    }
    ensure_free(&state, Contact::Email, email.as_deref(), None).await?;
    ensure_free(&state, Contact::Telegram, telegram.as_deref(), None).await?;

    // Hash password with Argon2id
    let password_hash = hash_password(&payload.password).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Password hashing failed: {}", err) })),
        )
    })?;

    // Create user record
    let user_record = UserRepo::create_with_contacts(&state.pool, username, &password_hash, email.as_deref(), telegram.as_deref())
        .await
        .map_err(db_error)?;

    let user_id = user_record.id as u32;

    // Resolve IP & Country
    let client_ip = extract_client_ip(&headers);
    let country = client_ip.and_then(|ip| state.geo_service.resolve_country(ip));
    let ip_str = client_ip.map(|ip| ip.to_string());
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Generate session & refresh token
    let session_id = Uuid::new_v4();
    let (_, refresh_token_str, refresh_hash) = RefreshToken::generate(session_id);

    // Save session in DB
    SessionRepo::create(
        &state.pool,
        &session_id,
        user_id,
        payload.device_id.as_deref(),
        &refresh_hash,
        ip_str.as_deref(),
        country.as_deref(),
        user_agent.as_deref(),
    )
    .await
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to create session: {}", err) })),
        )
    })?;

    // Issue JWT access token
    let (access_token, exp) = state
        .jwt_manager
        .issue_access_token(user_id, &session_id)
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to issue access token: {}", err) })),
            )
        })?;

    // Register active session in registry (enforcing single active session)
    let (active_session, _) = ActiveSession::new(session_id, user_id, ip_str, country);
    if let Some(old) = state.session_registry.register_or_displace(active_session) {
        let _ = StateFlusher::flush_session(&state.pool, &old).await;
        let _ = SessionRepo::revoke_session(&state.pool, &old.session_id, "session_displaced").await;
    }

    let response = AuthResponse {
        user: UserResponse {
            id: user_id,
            username: user_record.username,
            status: user_record.status,
            email: user_record.email,
            telegram: user_record.telegram,
        },
        access_token,
        refresh_token: refresh_token_str,
        expires_in: (exp - chrono::Utc::now().timestamp()).max(0) as u64,
    };

    Ok((StatusCode::CREATED, Json(response)).into_response())
}

/// POST /api/auth/login
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let username = payload.username.trim();

    let user_record = match UserRepo::find_by_username(&state.pool, username).await {
        Ok(Some(user)) => user,
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid username or password", "code": "INVALID_CREDENTIALS" })),
            ));
        }
    };

    let is_valid = verify_password(&payload.password, &user_record.password_hash).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Verification error: {}", err) })),
        )
    })?;

    if !is_valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid username or password", "code": "INVALID_CREDENTIALS" })),
        ));
    }

    let user_id = user_record.id as u32;

    // Resolve IP & Country
    let client_ip = extract_client_ip(&headers);
    let country = client_ip.and_then(|ip| state.geo_service.resolve_country(ip));
    let ip_str = client_ip.map(|ip| ip.to_string());
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Single active session enforcement:
    // If an existing session was active in registry or DB, displace and flush it!
    let _ = SessionRepo::revoke_all_user_sessions(&state.pool, user_id, "session_displaced").await;

    // Generate new session & refresh token
    let session_id = Uuid::new_v4();
    let (_, refresh_token_str, refresh_hash) = RefreshToken::generate(session_id);

    SessionRepo::create(
        &state.pool,
        &session_id,
        user_id,
        payload.device_id.as_deref(),
        &refresh_hash,
        ip_str.as_deref(),
        country.as_deref(),
        user_agent.as_deref(),
    )
    .await
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to create session: {}", err) })),
        )
    })?;

    let (access_token, exp) = state
        .jwt_manager
        .issue_access_token(user_id, &session_id)
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to issue access token: {}", err) })),
            )
        })?;

    let (active_session, _) = ActiveSession::new(session_id, user_id, ip_str, country);
    if let Some(old) = state.session_registry.register_or_displace(active_session) {
        let _ = StateFlusher::flush_session(&state.pool, &old).await;
    }

    let response = AuthResponse {
        user: UserResponse {
            id: user_id,
            username: user_record.username,
            status: user_record.status,
            email: user_record.email,
            telegram: user_record.telegram,
        },
        access_token,
        refresh_token: refresh_token_str,
        expires_in: (exp - chrono::Utc::now().timestamp()).max(0) as u64,
    };

    Ok((StatusCode::OK, Json(response)).into_response())
}

/// POST /api/auth/refresh
pub async fn refresh(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let parsed_token = RefreshToken::decode(&payload.refresh_token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid refresh token format", "code": "INVALID_TOKEN" })),
        )
    })?;

    let session_id = parsed_token.session_id;
    let presented_hash = RefreshToken::compute_hash(&payload.refresh_token);

    let session_record = match SessionRepo::find_by_id(&state.pool, &session_id).await {
        Ok(Some(s)) => s,
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Session not found or expired", "code": "SESSION_NOT_FOUND" })),
            ));
        }
    };

    // If session is already revoked, deny
    if session_record.revoked_at.is_some() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "Session has been revoked",
                "code": session_record.revoked_reason.unwrap_or_else(|| "REVOKED".into())
            })),
        ));
    }

    // A session unused for longer than the refresh lifetime has ended
    // (`REFRESH_TOKEN_TTL_SECS`, sliding: every rotation renews it).
    let idle = chrono::Utc::now() - session_record.last_seen_at;
    if idle.to_std().is_ok_and(|d| d > state.config.refresh_token_ttl) {
        let _ = SessionRepo::revoke_session(&state.pool, &session_id, "expired").await;
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Your session has expired. Please sign in again.", "code": "SESSION_EXPIRED" })),
        ));
    }

    // Check if token matches the current active refresh token
    let current = RefreshToken::verify_hash(&session_record.refresh_hash, &presented_hash);
    if current {
        // Legitimate rotation: issue new generation
        let next_generation = (session_record.refresh_generation + 1) as u32;
        let (_, new_refresh_token_str, new_hash) =
            RefreshToken::generate_with_generation(session_id, next_generation);

        let rotated = SessionRepo::rotate_refresh_token(
            &state.pool,
            &session_id,
            &presented_hash,
            &new_hash,
            next_generation,
        )
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to rotate token: {}", err) })),
            )
        })?;
        if rotated {
            let user_id = session_record.user_id as u32;
            let (access_token, exp) = state
                .jwt_manager
                .issue_access_token(user_id, &session_id)
                .map_err(|err| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": format!("Failed to issue access token: {}", err) })),
                    )
                })?;

            let response = TokenRefreshResponse {
                access_token,
                refresh_token: new_refresh_token_str,
                expires_in: (exp - chrono::Utc::now().timestamp()).max(0) as u64,
            };

            return Ok((StatusCode::OK, Json(response)).into_response());
        }
        // Lost the swap to a concurrent refresh with the same token: it is
        // the previous token now, so this presentation is a replay.
    }

    // Check if token matches the previous refresh token (REUSE DETECTION)
    let previous = session_record.prev_refresh_hash.as_deref().is_some_and(|prev| RefreshToken::verify_hash(prev, &presented_hash));
    if current || previous {
        warn!(
            "CRITICAL: Refresh token reuse detected for session {} (account {})! Revoking immediately.",
            session_id, session_record.user_id
        );

        // Revoke session immediately
        let _ = SessionRepo::revoke_session(&state.pool, &session_id, "refresh_reused").await;

        // Displace and disconnect any active session socket
        let user_id = session_record.user_id as u32;
        if let Some(active) = state.session_registry.remove_if_match(user_id, &session_id) {
            active.displace("Refresh token reuse detected; session revoked for security");
            let _ = StateFlusher::flush_session(&state.pool, &active).await;
        }

        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "Refresh token reuse detected; session revoked for security",
                "code": "REFRESH_REUSED"
            })),
        ));
    }

    // Invalid hash mismatch
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "Invalid refresh token", "code": "INVALID_TOKEN" })),
    ))
}

/// POST /api/auth/logout
pub async fn logout(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    info!(
        "User {} logging out of session {}",
        auth.user_id, auth.session_id
    );

    let _ = SessionRepo::revoke_session(&state.pool, &auth.session_id, "sign_out").await;

    if let Some(active) = state.session_registry.remove_if_match(auth.user_id, &auth.session_id) {
        let _ = StateFlusher::flush_session(&state.pool, &active).await;
    }

    Ok((StatusCode::OK, Json(json!({ "status": "ok" }))).into_response())
}

/// GET /api/auth/me
pub async fn me(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let user = UserRepo::find_by_id(&state.pool, auth.user_id)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "User not found" })),
            )
        })?;

    let session = SessionRepo::find_by_id(&state.pool, &auth.session_id)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Session not found" })),
            )
        })?;

    let is_connected = state.session_registry.get(auth.user_id).is_some();

    Ok((
        StatusCode::OK,
        Json(json!({
            "user": {
                "id": user.id,
                "username": user.username,
                "status": user.status,
                "created_at": user.created_at,
                "contacts": contacts_json(&user),
            },
            "session": {
                "id": session.id,
                "country": session.country,
                "ip_address": session.ip_address,
                "is_connected": is_connected,
                "last_seen_at": session.last_seen_at
            }
        })),
    )
        .into_response())
}

/// PATCH /api/auth/contacts: sets or clears the signed-in account's
/// optional email and Telegram (same rules as at registration). Answers
/// `{contacts}` as `/me` shows them.
pub async fn update_contacts(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<ContactsRequest>,
) -> Result<Response, ApiError> {
    let email = match &payload.email {
        None => None,
        Some(v) => Some(contacts::normalize_email(v.as_deref().unwrap_or("")).map_err(invalid)?),
    };
    let telegram = match &payload.telegram {
        None => None,
        Some(v) => Some(contacts::normalize_telegram(v.as_deref().unwrap_or("")).map_err(invalid)?),
    };
    let me = Some(i64::from(auth.user_id));
    ensure_free(&state, Contact::Email, email.as_ref().and_then(|e| e.as_deref()), me).await?;
    ensure_free(&state, Contact::Telegram, telegram.as_ref().and_then(|t| t.as_deref()), me).await?;
    let update = ContactsUpdate {
        email: email.as_ref().map(|e| e.as_deref()),
        telegram: telegram.as_ref().map(|t| t.as_deref()),
    };
    let user = UserRepo::set_contacts(&state.pool, auth.user_id, &update)
        .await
        .map_err(db_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "User not found" }))))?;
    info!("Account {} updated its contacts", auth.user_id);
    Ok((StatusCode::OK, Json(json!({ "contacts": contacts_json(&user) }))).into_response())
}
