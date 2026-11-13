use super::middleware::AuthUser;
use super::state::AppState;
use crate::db::character_repo::{CharacterRecord, CharacterRepo};
use crate::world::protocol::Look;
use crate::session::CharacterState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Character slots per account.
pub const MAX_CHARACTERS: i64 = 6;

#[derive(Deserialize)]
pub struct CreateCharacterRequest {
    pub name: String,
    pub archetype: String,
    /// Appearance; omitted fields take the default look.
    #[serde(default)]
    pub gender: Option<u8>,
    #[serde(default)]
    pub hair_style: Option<u8>,
    #[serde(default)]
    pub hair_color: Option<u8>,
    #[serde(default)]
    pub body_color: Option<u8>,
}

#[derive(Serialize)]
pub struct LookDto {
    pub gender: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub body_color: u8,
}

impl From<Look> for LookDto {
    fn from(l: Look) -> Self {
        Self { gender: l.gender, hair_style: l.hair, hair_color: l.hair_color, body_color: l.body_color }
    }
}

#[derive(Serialize)]
pub struct CharacterDto {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub archetype: String,
    pub level: i64,
    pub base_exp: i64,
    pub stats: serde_json::Value,
    pub unlocked_skills: serde_json::Value,
    pub inventory: serde_json::Value,
    pub last_location_id: String,
    pub look: LookDto,
    pub created_at: String,
}

impl CharacterDto {
    fn of(r: CharacterRecord) -> Self {
        let look = r.look().into();
        Self {
            id: r.id,
            user_id: r.user_id,
            stats: serde_json::from_str(&r.stats_json).unwrap_or(json!({})),
            unlocked_skills: serde_json::from_str(&r.unlocked_skills_json).unwrap_or(json!([])),
            inventory: serde_json::from_str(&r.inventory_json).unwrap_or(json!([])),
            name: r.name,
            archetype: r.archetype,
            level: r.level,
            base_exp: r.base_exp,
            last_location_id: r.last_location_id,
            look,
            created_at: r.created_at,
        }
    }
}

fn initial_stats_for_archetype(archetype: &str) -> (serde_json::Value, serde_json::Value) {
    let (stats, skills) = match archetype.to_lowercase().as_str() {
        "warrior" => (
            json!({ "max_hp": 150, "hp": 150, "str": 15, "agi": 8, "vit": 14, "int": 5, "dex": 10, "luk": 8 }),
            json!(["slash", "iron_defense"]),
        ),
        "ranger" => (
            json!({ "max_hp": 110, "hp": 110, "str": 8, "agi": 16, "vit": 8, "int": 8, "dex": 16, "luk": 10 }),
            json!(["arrow_volley", "dash"]),
        ),
        "mage" => (
            json!({ "max_hp": 90, "hp": 90, "str": 5, "agi": 8, "vit": 7, "int": 18, "dex": 12, "luk": 10 }),
            json!(["arcane_bolt", "mana_shield"]),
        ),
        "rogue" => (
            json!({ "max_hp": 105, "hp": 105, "str": 11, "agi": 16, "vit": 8, "int": 6, "dex": 14, "luk": 15 }),
            json!(["backstab", "smoke_bomb"]),
        ),
        _ => (
            json!({ "max_hp": 100, "hp": 100, "str": 10, "agi": 10, "vit": 10, "int": 10, "dex": 10, "luk": 10 }),
            json!(["strike"]),
        ),
    };
    (stats, skills)
}

/// GET /api/characters
pub async fn list_characters(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let records = CharacterRepo::list_by_user_id(&state.pool, auth.user_id)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        })?;

    let characters: Vec<CharacterDto> = records.into_iter().map(CharacterDto::of).collect();

    Ok((StatusCode::OK, Json(characters)).into_response())
}

/// POST /api/characters
pub async fn create_character(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateCharacterRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let name = payload.name.trim();
    if name.len() < 2 || name.len() > 24 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Character name must be between 2 and 24 characters" })),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ' ')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Character name contains invalid characters" })),
        ));
    }

    let archetype = payload.archetype.trim().to_lowercase();
    let valid_archetypes = ["warrior", "ranger", "mage", "rogue", "novice"];
    if !valid_archetypes.contains(&archetype.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Invalid archetype. Available: {:?}", valid_archetypes) })),
        ));
    }

    let defaults = Look::default();
    let look = Look {
        gender: payload.gender.unwrap_or(defaults.gender),
        hair: payload.hair_style.unwrap_or(defaults.hair),
        hair_color: payload.hair_color.unwrap_or(defaults.hair_color),
        body_color: payload.body_color.unwrap_or(defaults.body_color),
    };
    if !look.is_valid() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Unknown gender, hairstyle or dye" })),
        ));
    }

    let used = CharacterRepo::count_by_user_id(&state.pool, auth.user_id).await.map_err(|err| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err.to_string() })))
    })?;
    if used >= MAX_CHARACTERS {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("All {MAX_CHARACTERS} character slots are taken") })),
        ));
    }

    let (initial_stats, initial_skills) = initial_stats_for_archetype(&archetype);
    let initial_inventory = json!([]);
    let initial_location = "hub_central";

    let record = CharacterRepo::create(
        &state.pool,
        auth.user_id,
        name,
        &archetype,
        &initial_stats.to_string(),
        &initial_skills.to_string(),
        &initial_inventory.to_string(),
        initial_location,
        look,
    )
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "A character with that name already exists" })),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    })?;

    Ok((StatusCode::CREATED, Json(CharacterDto::of(record))).into_response())
}

/// GET /api/characters/:id
pub async fn get_character(
    auth: AuthUser,
    Path(id): Path<u32>,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let record = CharacterRepo::find_by_user_and_id(&state.pool, auth.user_id, id)
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
                Json(json!({ "error": "Character not found" })),
            )
        })?;

    Ok((StatusCode::OK, Json(CharacterDto::of(record))).into_response())
}

/// POST /api/characters/:id/select
/// Selects a character and loads it into the active simulation session.
pub async fn select_character(
    auth: AuthUser,
    Path(id): Path<u32>,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let record = CharacterRepo::find_by_user_and_id(&state.pool, auth.user_id, id)
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
                Json(json!({ "error": "Character not found" })),
            )
        })?;

    let stats = serde_json::from_str(&record.stats_json).unwrap_or(json!({}));
    let skills = serde_json::from_str(&record.unlocked_skills_json).unwrap_or(json!([]));
    let inventory = serde_json::from_str(&record.inventory_json).unwrap_or(json!([]));

    let char_state = CharacterState::new(
        record.id as u32,
        record.user_id as u32,
        record.name.clone(),
        record.archetype.clone(),
        record.level as u32,
        record.base_exp as u64,
        stats.clone(),
        skills.clone(),
        inventory.clone(),
        record.last_location_id.clone(),
        record.look(),
    );

    if let Some(active_session) = state.session_registry.get(auth.user_id) {
        let mut char_lock = active_session.character.write().await;
        *char_lock = Some(char_state);
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "selected",
            "character_id": record.id,
            "name": record.name,
            "location_id": record.last_location_id
        })),
    )
        .into_response())
}
