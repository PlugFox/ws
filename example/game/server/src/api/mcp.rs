//! MCP server (Model Context Protocol, Streamable HTTP transport, protocol
//! `2025-06-18`) for coding agents and operators: player feedback, server
//! status, characters, and the admin actions of `/admin/*`.
//!
//! `POST /mcp` takes one JSON-RPC 2.0 message and answers with plain JSON
//! (no SSE stream, no sessions: every request stands alone). Requests get
//! a response; notifications (`notifications/initialized`, ...) get 202.
//! `GET /mcp` is 405 (no server-initiated stream). Methods: `initialize`,
//! `ping`, `tools/list`, `tools/call`.
//!
//! Guarded like the admin API: `Authorization: Bearer $ADMIN_TOKEN`, off
//! (503) without `ADMIN_TOKEN`. A browser `Origin` from another host is
//! refused (DNS rebinding). Character tools are read-only; the only writes
//! are feedback status, announcements, the restart countdown and save-all.
//! See the server README for connecting Claude Code.

use super::admin_handlers::AdminAuth;
use super::state::AppState;
use super::status_handlers;
use crate::db::feedback_repo::{FeedbackFilter, FeedbackRecord, FeedbackRepo};
use crate::db::UserRepo;
use crate::feedback::{self, KINDS, PRIORITIES};
use crate::world::combat::items::{self, Bag, Slot};
use crate::world::combat::Class;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};
use chrono::{DateTime, Utc};
use sqlx::types::Json as Jsonb;
use sqlx::Row;
use std::collections::BTreeMap;
use tracing::{info, warn};

/// The protocol version this server speaks (and the older ones it accepts).
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

/// Page size of the list tools: default and cap.
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;
/// `list_feedback` shows this much of each body.
const PREVIEW_CHARS: usize = 240;

// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Operator tools for the ws example game server (a survivors-like web game). \
Player feedback (bug reports, suggestions, balance notes) is the main work queue: list_feedback with \
status=open, read one with get_feedback, fix it in the codebase, then close_feedback with a resolution \
that says what changed. Character tools are read-only snapshots of the database (online characters \
save every few seconds). broadcast, restart_warning and save_all act on the live server: use them only \
when asked.";

/// `GET /mcp`: no server-to-client stream here.
pub async fn get_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")], "MCP: POST JSON-RPC messages to this endpoint").into_response()
}

/// `POST /mcp`: one JSON-RPC message.
pub async fn handle(_: AdminAuth, State(state): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if foreign_origin(&headers) {
        warn!("MCP: refused a request from a foreign Origin");
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "foreign origin", "code": "FORBIDDEN" }))).into_response();
    }
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return reply(rpc_error(Value::Null, PARSE_ERROR, &format!("parse error: {e}"))),
    };
    let Value::Object(msg) = msg else {
        return reply(rpc_error(Value::Null, INVALID_REQUEST, "expected one JSON-RPC object (batches are not supported)"));
    };
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned();
    match (method, id) {
        // A notification (or a response to us, which we never ask for).
        (_, None) | (None, Some(_)) if msg.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => {
            StatusCode::ACCEPTED.into_response()
        }
        (Some(method), Some(id)) if msg.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => {
            let params = msg.get("params").cloned().unwrap_or(Value::Object(Map::new()));
            let result = dispatch(&state, method, &params).await;
            reply(match result {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => rpc_error(id, code, &message),
            })
        }
        (_, id) => reply(rpc_error(id.unwrap_or(Value::Null), INVALID_REQUEST, "not a JSON-RPC 2.0 request")),
    }
}

/// A browser page on another host (DNS rebinding); agents send no Origin.
fn foreign_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return false };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split("://").nth(1).unwrap_or(origin);
    origin_host != host
}

fn reply(v: Value) -> Response {
    Json(v).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

type RpcResult = Result<Value, (i64, String)>;

async fn dispatch(state: &AppState, method: &str, params: &Value) -> RpcResult {
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSION };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "ws-game", "title": "ws example game server", "version": crate::version::version() },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(Tool::describe).collect::<Vec<_>>() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool name".to_string()))?;
            let tool = TOOLS.iter().find(|t| t.name == name).ok_or((INVALID_PARAMS, format!("unknown tool: {name}")))?;
            let args = params.get("arguments").cloned().unwrap_or(Value::Object(Map::new()));
            if !tool.read_only {
                info!("MCP: {name} {args}");
            }
            Ok(match call(state, name, &args).await {
                Ok(v) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
                    "structuredContent": v,
                    "isError": false,
                }),
                // Tool failures go back to the model as results (it can fix
                // the arguments and retry), not as protocol errors.
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
    }
}

/// A tool as `tools/list` shows it.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// `properties` of the input schema and the required ones.
    params: fn() -> (Value, &'static [&'static str]),
    read_only: bool,
    /// Can it lose something (announcements and restarts cannot be undone).
    destructive: bool,
}

impl Tool {
    fn describe(&self) -> Value {
        let (properties, required) = (self.params)();
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
            "annotations": {
                "title": self.title,
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.read_only,
                "openWorldHint": false,
            },
        })
    }
}

fn no_params() -> (Value, &'static [&'static str]) {
    (json!({}), &[])
}

fn id_param(what: &str) -> Value {
    json!({ "type": "integer", "minimum": 1, "description": what })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "list_feedback",
        title: "List player feedback",
        description: "Player feedback (bug reports, suggestions, balance notes, other), newest first, with a \
            preview of each body. Filter by status (open | closed), kind and priority. Page with `cursor`: \
            pass the `next_cursor` of the previous page (null = no more).",
        params: || {
            (
                json!({
                    "status": { "type": "string", "enum": [feedback::OPEN, feedback::CLOSED], "description": "Only reports in this state." },
                    "kind": { "type": "string", "enum": KINDS, "description": "Only this kind of report." },
                    "priority": { "type": "string", "enum": PRIORITIES, "description": "Only this priority (as the player set it)." },
                    "cursor": { "type": "string", "description": "`next_cursor` from the previous page." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT },
                }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_feedback",
        title: "Read a feedback report",
        description: "One feedback report in full: the text, who sent it (account, character, class, level), \
            where they stood (map and cell), client and server versions, the browser, status and resolution.",
        params: || (json!({ "id": id_param("Report id.") }), &["id"]),
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "close_feedback",
        title: "Close a feedback report",
        description: "Marks a report closed with a resolution note (what was fixed or decided, e.g. \"Fixed in \
            1a2b3c: porings no longer stick to walls\" or \"Won't do: ...\"). Returns the updated report.",
        params: || {
            (
                json!({
                    "id": id_param("Report id."),
                    "resolution": { "type": "string", "minLength": 1, "maxLength": feedback::RESOLUTION_MAX, "description": "What was done about it." },
                }),
                &["id", "resolution"],
            )
        },
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "reopen_feedback",
        title: "Reopen a feedback report",
        description: "Sets a closed report back to open (the old resolution stays unless `note` replaces it). \
            Returns the updated report.",
        params: || {
            (
                json!({
                    "id": id_param("Report id."),
                    "note": { "type": "string", "maxLength": feedback::RESOLUTION_MAX, "description": "Why it is open again (replaces the resolution)." },
                }),
                &["id"],
            )
        },
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "server_status",
        title: "Server status",
        description: "Version, uptime, database health, players online (per map and hub channel), hub channels, \
            running mission stages, open lobbies, open feedback, the restart countdown, and resident memory \
            where the OS reports it.",
        params: no_params,
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_online_players",
        title: "Players online",
        description: "Every account in game now: account id, character id and name, class, level, map, and hub \
            channel (0 = on a mission stage).",
        params: no_params,
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_character",
        title: "Read a character",
        description: "One character by id or exact name (read-only, as last saved): account, class, level, \
            experience, zeny, equipment, bag contents, skills and hotbar, lifetime stats, where it was last, \
            and whether it is online now.",
        params: || {
            (
                json!({
                    "id": id_param("Character id."),
                    "name": { "type": "string", "description": "Exact character name (case-insensitive)." },
                }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_account",
        title: "Read an account",
        description: "One account by id or exact username (case-insensitive): status, created, its characters \
            (id, name, class, level), and its optional contacts. CONTAINS PERSONAL DATA (PII): the email \
            address and Telegram username the player gave (unverified unless marked). Use only to answer an \
            operator's question about that account; never repeat them in public channels or announcements.",
        params: || {
            (
                json!({
                    "id": id_param("Account id (the `account` of a character)."),
                    "username": { "type": "string", "description": "Exact account name (case-insensitive)." },
                }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "search_characters",
        title: "Find characters",
        description: "Characters whose name contains `query` (case-insensitive), highest level first: id, name, \
            class, level, account, last update, online now.",
        params: || {
            (
                json!({
                    "query": { "type": "string", "minLength": 1, "maxLength": 24, "description": "Part of the name." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT },
                }),
                &["query"],
            )
        },
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "db_summary",
        title: "Database summary",
        description: "Counts only, no personal data: accounts, characters (per class, level spread), live login \
            sessions, friendships, feedback per status, kind and priority.",
        params: no_params,
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "broadcast",
        title: "Announce to everyone",
        description: "Sends a server announcement to every connected player (banner and chat log). Cannot be \
            taken back: only when the operator asks for it.",
        params: || {
            (
                json!({ "text": { "type": "string", "minLength": 1, "maxLength": crate::admin::ANNOUNCE_MAX_CHARS, "description": "The announcement." } }),
                &["text"],
            )
        },
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "restart_warning",
        title: "Schedule a restart countdown",
        description: "Starts (or replaces) the restart countdown: announcements at the usual marks (30, 15, 10, \
            5..1 minutes) and a save of every session at 0. The process is not restarted by this; the \
            operator does that. `cancel: true` stops a running countdown instead.",
        params: || {
            (
                json!({
                    "minutes": { "type": "integer", "minimum": 1, "maximum": crate::admin::MAX_COUNTDOWN_MINUTES, "description": "Minutes until the restart." },
                    "message": { "type": "string", "maxLength": crate::admin::ANNOUNCE_MAX_CHARS / 2, "description": "Appended to each announcement (e.g. \"New patch: bosses.\")." },
                    "cancel": { "type": "boolean", "description": "Cancel the running countdown (minutes is then ignored)." },
                }),
                &[],
            )
        },
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "save_all",
        title: "Save every session",
        description: "Writes every online character's unsaved progress to the database now. Safe; reports how \
            many sessions were looked at, saved and failed.",
        params: no_params,
        read_only: false,
        destructive: false,
    },
];

type ToolResult = Result<Value, String>;

async fn call(state: &AppState, name: &str, args: &Value) -> ToolResult {
    let db = |e: sqlx::Error| format!("database error: {e}");
    match name {
        "list_feedback" => {
            let limit = limit(args)?;
            let before = match args.get("cursor") {
                None | Some(Value::Null) => None,
                Some(c) => Some(c.as_str().and_then(|s| s.parse::<i64>().ok()).or(c.as_i64()).ok_or("cursor: pass next_cursor as given")?),
            };
            let filter = FeedbackFilter {
                status: one_of(args, "status", &[feedback::OPEN, feedback::CLOSED])?,
                kind: one_of(args, "kind", KINDS)?,
                priority: one_of(args, "priority", PRIORITIES)?,
                before,
                limit: limit + 1,
            };
            let mut rows = FeedbackRepo::list(&state.pool, &filter).await.map_err(db)?;
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let next = more.then(|| rows.last().map(|r| r.id.to_string())).flatten();
            Ok(json!({ "items": rows.iter().map(preview).collect::<Vec<_>>(), "next_cursor": next }))
        }
        "get_feedback" => {
            let id = int(args, "id")?.ok_or("id is required")?;
            match FeedbackRepo::get(&state.pool, id).await.map_err(db)? {
                Some(r) => Ok(full(&r)),
                None => Err(format!("no feedback #{id}")),
            }
        }
        "close_feedback" | "reopen_feedback" => {
            let id = int(args, "id")?.ok_or("id is required")?;
            let closing = name == "close_feedback";
            let note = string(args, if closing { "resolution" } else { "note" })?.map(|s| feedback::clean_text(&s, feedback::RESOLUTION_MAX));
            let note = note.filter(|s| !s.is_empty());
            if closing && note.is_none() {
                return Err("resolution is required: say what was done".into());
            }
            match FeedbackRepo::set_status(&state.pool, id, closing, note.as_deref()).await.map_err(db)? {
                Some(r) => Ok(full(&r)),
                None => Err(format!("no feedback #{id}")),
            }
        }
        "server_status" => Ok(server_status(state).await),
        "list_online_players" => {
            let mut players: Vec<Value> = state
                .friends
                .online_players()
                .into_iter()
                .map(|(account, p)| {
                    json!({
                        "account": account, "character_id": p.character_id, "name": p.name, "class": p.class,
                        "level": p.level, "map": p.map, "channel": p.channel,
                    })
                })
                .collect();
            players.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Ok(json!({ "count": players.len(), "players": players }))
        }
        "get_character" => get_character(state, args).await,
        "get_account" => get_account(state, args).await,
        "search_characters" => {
            let query = string(args, "query")?.map(|q| q.trim().to_string()).filter(|q| !q.is_empty()).ok_or("query is required")?;
            let pattern = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let rows = sqlx::query(
                "SELECT c.id, c.name, c.archetype, c.level, c.user_id, u.username, c.updated_at \
                 FROM characters c JOIN users u ON u.id = c.user_id \
                 WHERE c.name ILIKE $1 ESCAPE '\\' ORDER BY c.level DESC, c.id LIMIT $2",
            )
            .bind(pattern)
            .bind(limit(args)?)
            .fetch_all(&state.pool)
            .await
            .map_err(db)?;
            let online = online_characters(state);
            let found: Vec<Value> = rows
                .iter()
                .map(|r| {
                    let id: i64 = r.get("id");
                    json!({
                        "id": id, "name": r.get::<String, _>("name"), "class": r.get::<String, _>("archetype"),
                        "level": r.get::<i32, _>("level"), "account": r.get::<i64, _>("user_id"),
                        "username": r.get::<String, _>("username"), "updated_at": r.get::<DateTime<Utc>, _>("updated_at"),
                        "online": online.contains_key(&(id as u32)),
                    })
                })
                .collect();
            Ok(json!({ "count": found.len(), "characters": found }))
        }
        "db_summary" => db_summary(state).await.map_err(db),
        "broadcast" => {
            let text = string(args, "text")?.ok_or("text is required")?;
            match state.admin.broadcast(&text) {
                Some(delivered) => Ok(json!({ "delivered": delivered })),
                None => Err("text is empty".into()),
            }
        }
        "restart_warning" => {
            if args.get("cancel").and_then(Value::as_bool) == Some(true) {
                return Ok(json!({ "cancelled": state.admin.cancel_restart() }));
            }
            let minutes = int(args, "minutes")?.ok_or("minutes is required (or cancel: true)")?;
            let minutes = u32::try_from(minutes).map_err(|_| "minutes must be between 1 and 1440")?;
            let message = string(args, "message")?;
            state.admin.schedule_restart(minutes, message.as_deref()).map(|s| json!({ "scheduled": s })).map_err(String::from)
        }
        "save_all" => Ok(json!(state.admin.save_all().await)),
        _ => Err(format!("unknown tool: {name}")),
    }
}

// --- arguments ---

fn int(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| format!("{key} must be an integer")),
    }
}

fn string(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn one_of(args: &Value, key: &str, allowed: &[&str]) -> Result<Option<String>, String> {
    match string(args, key)? {
        Some(v) if !allowed.contains(&v.as_str()) => Err(format!("{key} must be one of {}", allowed.join(", "))),
        v => Ok(v),
    }
}

fn limit(args: &Value) -> Result<i64, String> {
    Ok(int(args, "limit")?.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
}

// --- views ---

/// A list row: everything but the full body and the browser string.
fn preview(r: &FeedbackRecord) -> Value {
    let mut body: String = r.body.chars().take(PREVIEW_CHARS).collect();
    if body.len() < r.body.len() {
        body.push('…');
    }
    json!({
        "id": r.id, "created_at": r.created_at, "status": r.status, "kind": r.kind, "priority": r.priority,
        "title": r.title, "preview": body, "character": r.character_name, "class": r.class, "level": r.level,
        "map": r.map, "client_version": r.client_version, "closed_at": r.closed_at,
    })
}

fn full(r: &FeedbackRecord) -> Value {
    let mut v = json!(r);
    // Cells, rounded: enough to find the spot.
    if let (Some(x), Some(y)) = (r.pos_x, r.pos_y) {
        v["position"] = json!({ "x": (x * 10.0).round() / 10.0, "y": (y * 10.0).round() / 10.0 });
    }
    v
}

/// Online characters: character id -> (map, channel).
fn online_characters(state: &AppState) -> BTreeMap<u32, (String, u16)> {
    state.friends.online_players().into_iter().map(|(_, p)| (p.character_id, (p.map, p.channel))).collect()
}

async fn server_status(state: &AppState) -> Value {
    let players = state.friends.online_players();
    let mut per_map: BTreeMap<String, usize> = BTreeMap::new();
    for (_, p) in &players {
        let place = if p.channel > 0 { format!("{} (Hub-{})", p.map, p.channel) } else { format!("{} (mission)", p.map) };
        *per_map.entry(place).or_default() += 1;
    }
    let feedback_counts: BTreeMap<String, i64> = FeedbackRepo::count_by_status(&state.pool).await.unwrap_or_default().into_iter().collect();
    json!({
        "version": crate::version::version(),
        "uptime_secs": state.started.elapsed().as_secs(),
        "server_time": chrono::Utc::now().to_rfc3339(),
        "database": if status_handlers::db_ok(state).await { "ok" } else { "error" },
        "online": players.len(),
        "players_by_place": per_map,
        "hub": state.hub.as_ref().map(|h| h.channels().iter().map(|c| json!({
            "id": c.id, "name": c.name, "population": c.population, "capacity": c.capacity,
        })).collect::<Vec<_>>()),
        "stages": state.lobbies.as_ref().map(|l| l.stages().summary()),
        "open_lobbies": state.lobbies.as_ref().map(|l| l.open_lobbies()),
        "login_sessions": state.session_registry.active_count(),
        "feedback": feedback_counts,
        "restart": state.admin.restart_status(),
        "memory_rss_bytes": rss_bytes(),
    })
}

/// Resident memory of the process (Linux `/proc`; `None` elsewhere).
fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// `get_account`: the account row (without the password hash), its
/// characters and its contacts (PII, see the tool description).
async fn get_account(state: &AppState, args: &Value) -> ToolResult {
    let db = |e: sqlx::Error| format!("database error: {e}");
    let user = match (int(args, "id")?, string(args, "username")?) {
        (Some(id), _) => match u32::try_from(id) {
            Ok(id) => UserRepo::find_by_id(&state.pool, id).await.map_err(db)?,
            Err(_) => None,
        },
        (None, Some(name)) => UserRepo::find_by_username(&state.pool, name.trim()).await.map_err(db)?,
        (None, None) => return Err("pass id or username".into()),
    };
    let Some(u) = user else { return Err("no such account".into()) };
    let rows = sqlx::query("SELECT id, name, archetype, level FROM characters WHERE user_id = $1 ORDER BY id")
        .bind(u.id)
        .fetch_all(&state.pool)
        .await
        .map_err(db)?;
    let characters: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "id": r.get::<i64, _>("id"), "name": r.get::<String, _>("name"), "class": r.get::<String, _>("archetype"), "level": r.get::<i32, _>("level") }))
        .collect();
    Ok(json!({
        "id": u.id,
        "username": u.username,
        "status": u.status,
        "created_at": u.created_at,
        "online": state.session_registry.get(u.id as u32).is_some(),
        "characters": characters,
        "contacts": {
            "email": u.email,
            "email_verified_at": u.email_verified_at,
            "telegram": u.telegram,
            "telegram_verified_at": u.telegram_verified_at,
        },
    }))
}

async fn get_character(state: &AppState, args: &Value) -> ToolResult {
    let id = int(args, "id")?;
    let name = string(args, "name")?;
    let sql = "SELECT c.id, c.user_id, u.username, c.name, c.archetype, c.level, c.base_exp, c.zeny, c.weapon, \
               c.last_location_id, c.inventory, c.equipment, c.loadout, c.stats, c.created_at, c.updated_at \
               FROM characters c JOIN users u ON u.id = c.user_id WHERE ";
    let row = match (id, name) {
        (Some(id), _) => sqlx::query(&format!("{sql}c.id = $1")).bind(id).fetch_optional(&state.pool).await,
        (None, Some(n)) => sqlx::query(&format!("{sql}lower(c.name) = lower($1)")).bind(n.trim().to_string()).fetch_optional(&state.pool).await,
        (None, None) => return Err("pass id or name".into()),
    }
    .map_err(|e| format!("database error: {e}"))?;
    let Some(r) = row else { return Err("no such character".into()) };
    let id: i64 = r.get("id");
    let class_id: String = r.get("archetype");
    let class = Class::from_id(&class_id).unwrap_or(Class::Swordsman);
    let items: Jsonb<Vec<items::Stack>> = r.get("inventory");
    let equip: Option<Jsonb<Vec<String>>> = r.get("equipment");
    let bag = Bag::load(class, items.0, equip.map(|e| e.0), r.get("zeny"), &r.get::<String, _>("weapon"));
    let item_name = |key: &str| items::find(items::base_id(key)).map_or_else(|| key.to_string(), |e| e.name.to_string());
    let equipment: Vec<Value> = Slot::ALL
        .iter()
        .zip(bag.equip.iter())
        .filter(|(_, id)| !id.is_empty())
        .map(|(slot, id)| json!({ "slot": slot.label(), "item": id, "name": item_name(id) }))
        .collect();
    let stacks: Vec<Value> = bag.items.iter().map(|s| json!({ "item": s.key(), "name": item_name(&s.id), "count": s.count })).collect();
    let doc = |col: &str| r.get::<Jsonb<Value>, _>(col).0;
    let online = online_characters(state).get(&(id as u32)).map(|(map, channel)| json!({ "map": map, "channel": channel }));
    Ok(json!({
        "id": id,
        "account": r.get::<i64, _>("user_id"),
        "username": r.get::<String, _>("username"),
        "name": r.get::<String, _>("name"),
        "class": class_id,
        "level": r.get::<i32, _>("level"),
        "base_exp": r.get::<i64, _>("base_exp"),
        "zeny": bag.zeny,
        "last_location": r.get::<String, _>("last_location_id"),
        "equipment": equipment,
        "bag": { "stacks": stacks.len(), "items": bag.items.iter().map(|s| u64::from(s.count)).sum::<u64>(), "contents": stacks },
        "loadout": doc("loadout"),
        "stats": doc("stats"),
        "created_at": r.get::<DateTime<Utc>, _>("created_at"),
        "updated_at": r.get::<DateTime<Utc>, _>("updated_at"),
        "online": online,
    }))
}

async fn db_summary(state: &AppState) -> Result<Value, sqlx::Error> {
    let pool = &state.pool;
    let count = |sql: &'static str| async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await };
    let grouped = |sql: &'static str| async move {
        sqlx::query_as::<_, (String, i64)>(sql).fetch_all(pool).await.map(|v| v.into_iter().collect::<BTreeMap<_, _>>())
    };
    Ok(json!({
        "users": count("SELECT COUNT(*) FROM users").await?,
        "characters": count("SELECT COUNT(*) FROM characters").await?,
        "characters_by_class": grouped("SELECT archetype, COUNT(*) FROM characters GROUP BY archetype").await?,
        "characters_by_level": grouped(
            "SELECT CASE WHEN level < 10 THEN '1-9' WHEN level < 30 THEN '10-29' WHEN level < 60 THEN '30-59' \
             WHEN level < 90 THEN '60-89' ELSE '90+' END AS band, COUNT(*) FROM characters GROUP BY band"
        ).await?,
        "max_level": count("SELECT COALESCE(MAX(level), 0)::BIGINT FROM characters").await?,
        "live_login_sessions": count("SELECT COUNT(*) FROM sessions WHERE revoked_at IS NULL").await?,
        "friendships": count("SELECT COUNT(*) / 2 FROM friends WHERE state = 0").await?,
        "friend_requests": count("SELECT COUNT(*) FROM friends WHERE state = 1").await?,
        "feedback_by_status": grouped("SELECT status, COUNT(*) FROM feedback GROUP BY status").await?,
        "feedback_open_by_kind": grouped("SELECT kind, COUNT(*) FROM feedback WHERE status = 'open' GROUP BY kind").await?,
        "feedback_open_by_priority": grouped("SELECT priority, COUNT(*) FROM feedback WHERE status = 'open' GROUP BY priority").await?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_schema_and_a_unique_name() {
        let mut names = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "duplicate {}", t.name);
            let d = t.describe();
            assert_eq!(d["inputSchema"]["type"], "object");
            for req in d["inputSchema"]["required"].as_array().unwrap() {
                assert!(d["inputSchema"]["properties"].get(req.as_str().unwrap()).is_some(), "{}: required {req} undefined", t.name);
            }
            assert!(t.description.len() > 40, "{} needs a real description", t.name);
        }
    }

    #[test]
    fn foreign_origins_are_refused() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "game.example:8080".parse().unwrap());
        assert!(!foreign_origin(&h), "no origin: an agent");
        h.insert(header::ORIGIN, "http://game.example:8080".parse().unwrap());
        assert!(!foreign_origin(&h));
        h.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(foreign_origin(&h));
    }
}
