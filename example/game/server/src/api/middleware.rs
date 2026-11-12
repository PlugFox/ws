use super::state::AppState;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: u32,
    pub session_id: Uuid,
}

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|val| val.to_str().ok());

        let token = match auth_header {
            Some(header_val) if header_val.starts_with("Bearer ") => {
                &header_val[7..]
            }
            _ => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "Missing or invalid Authorization header",
                        "code": "UNAUTHORIZED"
                    })),
                )
                    .into_response());
            }
        };

        match app_state.jwt_manager.verify_access_token(token) {
            Ok(claims) => {
                let session_id = Uuid::parse_str(&claims.sid).map_err(|_| {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({
                            "error": "Invalid session identifier in token",
                            "code": "INVALID_TOKEN"
                        })),
                    )
                        .into_response()
                })?;

                Ok(AuthUser {
                    user_id: claims.sub,
                    session_id,
                })
            }
            Err(err) => {
                let message = err.to_string();
                Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": message,
                        "code": "INVALID_OR_EXPIRED_TOKEN"
                    })),
                )
                    .into_response())
            }
        }
    }
}
