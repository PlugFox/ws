//! `GET /api/classes`: the class lines for the character creation screen,
//! public (the screen shows them before any character exists).
//!
//! ```json
//! {"classes": [{"id": "mage", "name": "Mage", "locked": false,
//!   "roles": [{"id": "caster", "label": "Area caster"}],
//!   "ratings": [{"id": "damage", "value": 5}, ...], "difficulty": 4, "pitch": "...",
//!   "titles": ["Mage", "Wizard", "High Wizard"], "hit_die": ["d6", "d6", "d8"],
//!   "hp": [6, 438], "sp": [14, 516], "ac": 10, "save": "mind", "attacks": [1, 1, 1],
//!   "start_weapon": "rod",
//!   "weapons": [{"kind": "staff", "label": "...", "two_handed": false, "reach": "melee",
//!                "items": [{"id": "rod", "name": "Rod", "die": "1d6", "level": 1}]}],
//!   "armour": {"weight": "light", "defence": 0, "body": [{"id", "name"}], "shields": [...],
//!              "start": ["cotton_shirt"]},
//!   "skills": [{"id", "name", "icon", "text", "tier", "passive", "max_level", "signature"?: rank}]},
//!  {"id": "ninja", "name": "Ninja", "locked": true, "roles", "ratings", "difficulty",
//!   "pitch", "titles": [planned]}]}
//! ```
//!
//! Built from the combat tables and the skill book (`combat::profile`); it
//! only changes with a deploy, so clients may keep it a minute.

use super::state::AppState;
use crate::world::combat::profile;
use axum::{extract::State, http::header, response::IntoResponse, Json};

/// How long clients may reuse the answer.
const MAX_AGE_SECS: u32 = 60;

pub async fn classes(State(state): State<AppState>) -> impl IntoResponse {
    let cache = header::HeaderValue::from_str(&format!("public, max-age={MAX_AGE_SECS}")).expect("ascii");
    ([(header::CACHE_CONTROL, cache)], Json(profile::catalogue(&state.skills)))
}
