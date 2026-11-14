use super::middleware::AuthUser;
use super::state::AppState;
use crate::db::character_repo::{CharacterRecord, CharacterRepo};
use crate::world::protocol::Look;
use crate::session::CharacterState;
use crate::world::combat::{self, Bag, Class, Loadout, Weapon};
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
    /// Display name of that location (its manifest's), for the slot cards;
    /// empty when the server does not know the map.
    pub last_location_name: String,
    pub look: LookDto,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A stored character's belongings (the starting kit for rows from
/// before items).
fn bag_of(r: &CharacterRecord) -> Bag {
    r.bag(Class::from_id(&r.archetype).unwrap_or(Class::Swordsman))
}

impl CharacterDto {
    fn of(r: CharacterRecord) -> Self {
        let look = r.look().into();
        let bag = bag_of(&r);
        Self {
            id: r.id,
            user_id: r.user_id,
            stats: class_stats(&r.archetype, i64::from(r.level), &bag),
            unlocked_skills: r.unlocked_skills.0,
            inventory: json!({
                "zeny": bag.zeny,
                "items": bag.items.iter().map(|s| json!({ "id": s.key(), "count": s.count })).collect::<Vec<_>>(),
                "equip": bag.equip,
            }),
            name: r.name,
            archetype: r.archetype,
            level: i64::from(r.level),
            base_exp: r.base_exp,
            last_location_id: r.last_location_id,
            last_location_name: String::new(),
            look,
            created_at: r.created_at,
        }
    }
}

/// Derived combat numbers of a class at a level (COMBAT.md §1.3, §2.2).
/// Computed on read: a character is class + level + learned skills, so
/// nothing here is stored or can drift from the rules.
pub(crate) fn class_stats(archetype: &str, level: i64, bag: &Bag) -> serde_json::Value {
    let Some(class) = Class::from_id(archetype) else {
        return json!({});
    };
    let level = level.clamp(1, combat::MAX_LEVEL as i64) as u32;
    let held = Weapon::held(class, bag.weapon());
    let (dice, kind) = held.damage(class, level);
    let gear = bag.gear(class);
    json!({
        "title": class.title(level),
        "tier": combat::tier(level),
        "max_hp": class.max_hp(level) + gear.hp,
        "max_sp": class.max_sp(level) + gear.sp,
        "ac": gear.ac_at(level),
        "zeny": bag.zeny,
        "proficiency": combat::proficiency(level),
        "attacks": class.attacks_per_action(level),
        "skill_points": Class::skill_points(level),
        "exp_to_next": combat::exp_to_next(level),
        "weapon": {
            "id": held.id,
            "name": held.name,
            "to_hit": held.to_hit(level),
            "damage": format!("{}d{}{:+}", dice.count, dice.sides, dice.bonus),
            "damage_type": kind.ro_element(),
            "interval_ms": held.interval_ms,
            // Basic attack reach in cells (to the target's edge): how far
            // click to move walks toward a clicked monster.
            "reach": match held.reach {
                combat::Reach::Melee { range, .. } | combat::Reach::Ranged { range, .. } => range,
            },
        },
    })
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

    let characters: Vec<CharacterDto> = records
        .into_iter()
        .map(|r| {
            let mut c = CharacterDto::of(r);
            c.last_location_name = state.place_name(&c.last_location_id).unwrap_or_default();
            c
        })
        .collect();

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
    // Only the current ids: the legacy aliases `Class::from_id` accepts are
    // for characters created before the class rework.
    let Some(class) = Class::ALL.into_iter().find(|c| c.id() == archetype) else {
        let available: Vec<&str> = Class::ALL.iter().map(|c| c.id()).collect();
        let error = if let Some(p) = combat::profile::find(&archetype).filter(|p| p.class.is_none()) {
            format!("The {} class is not available yet", p.name)
        } else {
            format!("Invalid class. Available: {available:?}")
        };
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": error }))));
    };

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

    // Nothing is learned yet; combat numbers derive from class + level.
    // The starting kit: class weapon, armour, five Red Potions. Test runs
    // (`GAME_DEV_LEVEL`) start higher.
    let bag = Bag::starting(class, "");
    let level = state.config.dev_level.map_or(1, |l| l.clamp(1, combat::MAX_LEVEL));
    let record = CharacterRepo::create(&state.pool, auth.user_id, name, class.id(), "hub_central", look, &bag, level)
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
    // The character lock is held across the read: a player trade settling
    // now (`session::trade`) stores and applies its bags under the same
    // lock, so the row read here is never older than the memory it replaces.
    let active_session = state.session_registry.get(auth.user_id);
    let mut char_lock = match &active_session {
        Some(s) => Some(s.character.write().await),
        None => None,
    };
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

    let stats = record.stats.0.clone();
    let skills = record.unlocked_skills.0.clone();
    let bag = bag_of(&record);

    let mut char_state = CharacterState::new(
        record.id as u32,
        record.user_id as u32,
        record.name.clone(),
        record.archetype.clone(),
        record.level as u32,
        record.base_exp as u64,
        stats.clone(),
        skills.clone(),
        bag,
        record.last_location_id.clone(),
        record.look(),
    );
    let mut loadout: Loadout = record.loadout.0.clone();
    // Without a skill table (no assets) the stored loadout is kept as is.
    if let (Some(class), false) = (Class::from_id(&record.archetype), state.skills.is_empty()) {
        state.skills.sanitize(class, &mut loadout);
    }
    char_state.loadout = loadout;
    // The leaderboard counters (a read failure starts them from zero only
    // in memory; the stored ones are overwritten on the next save, so
    // refuse instead).
    char_state.record = crate::db::LeaderboardRepo::record(&state.pool, record.id as u32).await.map_err(|err| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err.to_string() })))
    })?;

    if let Some(char_lock) = char_lock.as_mut() {
        **char_lock = Some(char_state);
    }
    drop(char_lock);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The client walks a clicked monster into this reach (click to move).
    #[test]
    fn stats_carry_the_basic_attack_reach() {
        let reach = |class: Class| {
            let bag = Bag::starting(class, "");
            class_stats(class.id(), 1, &bag)["weapon"]["reach"].as_f64().unwrap()
        };
        assert_eq!(reach(Class::Swordsman), 2.75, "a sword");
        assert_eq!(reach(Class::Archer), 32.0, "a bow");
    }
}
