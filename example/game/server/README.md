# Game server

Rust (axum + tokio) server of the example game: auth, characters, hub
channels, mission stages, all over one binary WebSocket protocol
(`src/world/protocol.rs`). `mise run game` starts the dev database, builds
the client and starts it at <http://127.0.0.1:8080>.

## Database (PostgreSQL)

Everything persistent lives in PostgreSQL 18 (sqlx). The schema is the
ordered SQL files in `migrations/` (`0001_baseline.sql`, then `0002_...`),
embedded into the binary and applied at boot; sqlx records each applied
file and its checksum in `_sqlx_migrations`. Change the schema with a new
file, never by editing one that has run. Repositories are in `src/db/`.

Development uses a container (`../compose.dev.yaml`, postgres:18-alpine on
`127.0.0.1:55432`, database, user and password `mobbed`, data in a named
volume):

```sh
mise run game:db:up     # start it and wait until ready (game, game:e2e do this too)
mise run game:db:psql   # psql shell
mise run game:db:down   # stop it (data kept; `docker compose -f ../compose.dev.yaml down -v` wipes it)
```

Tests (`cargo test`, or `mise run game:server:test` which starts the
database first) need that server, or `TEST_DATABASE_URL` pointing at another
one whose role may `CREATE DATABASE`. Every test creates its own empty
database (`gs_test_<unix secs>_<uuid>`, `src/db/testing.rs`), so tests run in
parallel without sharing rows; a run drops test databases older than 15
minutes (about 8 MB each) and e2e ones older than six hours.
Without a reachable server the tests fail with how to start it. The e2e
suite runs its server on a fresh `gs_e2e_<unix secs>` database
(`DATABASE_CREATE=1`; `E2E_DATABASE_URL` picks another server).

Production runs against the stack's shared Postgres (`postgres:5432` on the
`shared-db` network, database `mobbed` owned by role `mobbed`), e.g.
`DATABASE_URL=postgres://mobbed:<password>@postgres:5432/mobbed`. The
server only needs to own its database (it creates its tables); it retries
an unreachable database at boot for `DATABASE_CONNECT_RETRY_SECS`, so it
may start before Postgres.

## Configuration

Read from the environment (`src/config.rs`); the main ones:

| Variable | Default | Meaning |
| --- | --- | --- |
| `SERVER_HOST`, `SERVER_PORT` | `127.0.0.1`, `8080` | Listen address |
| `DATABASE_URL` | `postgres://mobbed:mobbed@127.0.0.1:55432/mobbed` | PostgreSQL database (default: the dev container) |
| `DATABASE_MAX_CONNECTIONS` | `10` | Pool size |
| `DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | How long a query waits for a free pooled connection |
| `DATABASE_STATEMENT_TIMEOUT_MS` | `15000` | `statement_timeout` of every connection; `0` = none |
| `DATABASE_CONNECT_RETRY_SECS` | `60` | How long boot retries an unreachable database (backoff 0.25 s → 5 s) |
| `DATABASE_CREATE` | unset | `1`: create the database if missing (dev, e2e); leave unset in production |
| `TEST_DATABASE_URL` | `DATABASE_URL`, else the dev container | Server the tests create their databases on |
| `JWT_SECRET` | dev fallback | Signs access tokens: set it in production |
| `FLUSH_INTERVAL_SECS` | `30` | How often unsaved characters are written |
| `GAME_ASSETS_DIR`, `GAME_CLIENT_DIR` | `../../assets/dist`, `../client/build/web` | Baked pack, built client |
| `ADMIN_TOKEN` | unset | Bearer token of the admin API; unset or empty disables it |

## Account contacts

An account may carry an email address and a Telegram username, both
optional and unverified for now (for password recovery, notifications and
Google sign-in later: ROADMAP Phase 6.27). Rules in `src/auth/contacts.rs`:
email trimmed, domain lowercased, RFC 5321-lite, at most 254 characters;
Telegram `^[A-Za-z][A-Za-z0-9_]{4,31}$` after stripping `@` and `t.me/`
links. Each is unique regardless of case when set.

```sh
# Sign-up: email / telegram may be left out, null or "".
curl -s "$GAME/api/auth/register" -H 'Content-Type: application/json' \
  -d '{"username":"ann","password":"secret123","email":"Ann@Example.com","telegram":"@ann_lee"}'
# {"user":{"id":1,"username":"ann","status":"active","email":"Ann@example.com","telegram":"ann_lee"},"access_token":...}

# Read (the owner's own data, not masked): user.contacts in /me.
curl -s "$GAME/api/auth/me" -H "Authorization: Bearer $TOKEN"
# {"user":{...,"contacts":{"email":"Ann@example.com","email_verified":false,"email_google":false,
#   "telegram":"ann_lee","telegram_verified":false}},"session":{...}}

# Change: a field left out stays, null or "" clears it, a value replaces it
# (a real change clears its verification mark). Answers {"contacts":{...}}.
curl -s -X PATCH "$GAME/api/auth/contacts" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"email":"ann@work.example","telegram":null}'
```

Errors carry `code` and the form `field`: `400 email_invalid` /
`telegram_invalid`, `409 email_taken` / `telegram_taken` (registration
errors also name `field` `user` / `password`). A taken contact is
reported as such, so these endpoints tell whether an address is in use;
accepted while nothing is mailed and login takes the account name only,
to revisit (rate limits, a neutral answer) with recovery by email.

## Local chat

Players talk with `CHAT` frames (client `0x5E`, server `0x7A` op 0). A line
reaches the players within 30 cells on a hub map (about one screen) and the
whole party on a mission; the speaker gets it back too. The server strips
control and bidi-override characters, collapses whitespace, cuts lines at
120 characters and allows a burst of 4 lines, then one per 1.5 s
(`chat_too_fast` otherwise). See `src/world/chat.rs`.

## Admin API

Operator endpoints under `/admin/*`. Every request needs
`Authorization: Bearer $ADMIN_TOKEN`; without `ADMIN_TOKEN` they answer
`503`, with a wrong or missing token `401`. Use a long random token, keep it
out of the repository, and keep `/admin/*` off the public internet (a
reverse proxy rule or a firewall) where you can.

```sh
export ADMIN_TOKEN="$(openssl rand -hex 24)"   # set the same value for the server
export GAME=http://127.0.0.1:8080
```

### Broadcast

A server-wide announcement: a banner at the top of every connected client
and a line in its chat log (`CHAT` op 1). Answers how many sockets got it.

```sh
curl -X POST "$GAME/admin/broadcast" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"text": "Double EXP weekend starts now!"}'
# {"delivered":12}
```

### Restart countdown

Schedules the usual restart warnings: right away, then at 120, 60, 30, 15,
10, 5, 4, 3, 2 and 1 minutes left (those below `minutes`), then "The server
is restarting now." together with a save of every session. An optional
`message` is appended to each warning. A new countdown replaces a running
one; `minutes` is 1..1440.

```sh
# 30, 15, 10, 5, 4, 3, 2, 1, now
curl -X POST "$GAME/admin/restart-warning" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"minutes": 30, "message": "Patch 1.2: new bosses."}'
# {"scheduled":{"id":1,"minutes":30,"restart_at":"2026-11-14T18:30:00Z","pending":[30,15,10,5,4,3,2,1,0]}}

# What is still to come (null when nothing is scheduled)
curl "$GAME/admin/restart-warning" -H "Authorization: Bearer $ADMIN_TOKEN"

# Cancel: players are told "The server restart was cancelled."
curl -X DELETE "$GAME/admin/restart-warning" -H "Authorization: Bearer $ADMIN_TOKEN"
# {"cancelled":true}
```

The countdown only warns and saves; stopping the process is up to you
(e.g. stop the service once `pending` is empty).

### Save

Writes every active session's unsaved character to the database now, the
same way the periodic flusher does (each character under its own lock, so
it is safe alongside the flusher, logouts and other save calls: a character
is written once and found clean by the others).

```sh
curl -X POST "$GAME/admin/save" -H "Authorization: Bearer $ADMIN_TOKEN"
# {"sessions":12,"saved":7,"failed":0}
```

`sessions` counts active sessions, `saved` the characters written, `failed`
the writes that failed (those stay unsaved for the next pass and are
logged).

## Status endpoints

Public, no token:

```sh
curl "$GAME/health"
# {"status":"healthy","service":"game_server","version":"0.1.0+1a2b3c4d","uptime_secs":5321,
#  "checks":{"database":"ok","hub":"ok"},"online":12,"timestamp":"2026-11-14T18:00:00+00:00"}
curl "$GAME/api/online"
# {"online":12,"server_time":1763143200000}
```

`/health` pings the database and answers `503` (`"status":"unhealthy"`)
when it does not reply within 2 s. `/api/online` counts the players in game
(any map), recounted at most every 2 s; the login screen polls it.

The version is the package version plus the git commit the binary was built
from (`build.rs`; `GAME_SERVER_GIT_HASH` overrides it, e.g. in a Docker
build without `.git`). It is also sent in the `connected` frame, and the
client shows it in the title screen corner and under the settings.

## Player feedback

Players leave bug reports and suggestions at Liesel the Scribe (next to the
notice board in Izlude, NPC kind `feedback`) or with "Leave feedback" at a
Kafra employee: a kind (`bug`, `suggestion`, `balance`, `other`), a
priority (`low` … `critical`), a title (4–80 characters) and the details
(20–4000). The server stores them in the `feedback` table with the
character, map, position, level, class, client and server versions and the
browser; one report per account per minute, ten a day (`src/feedback.rs`).
Operators work through them over MCP.

## MCP server (coding agents)

`POST /mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server (Streamable HTTP, protocol `2025-06-18`, plain JSON answers) behind
the same `Authorization: Bearer $ADMIN_TOKEN` as the admin API (off without
`ADMIN_TOKEN`). Tools:

| Tool | What it does |
| --- | --- |
| `list_feedback` | Reports, newest first; filters `status`, `kind`, `priority`; `cursor` / `limit` paging |
| `get_feedback` | One report in full, with its context |
| `close_feedback` | Close with a `resolution` note |
| `reopen_feedback` | Back to open |
| `server_status` | Version, uptime, database, players per map and channel, stages, lobbies, open feedback, restart countdown, memory (Linux) |
| `list_online_players` | Who is in game and where |
| `get_character`, `search_characters` | Read-only character data (stats, equipment, bag, skills) |
| `get_account` | One account by id or username: status, characters, and its contacts (email, Telegram: **PII**, the description says so) |
| `db_summary` | Row counts only (accounts, characters per class and level, feedback per status) |
| `broadcast`, `restart_warning`, `save_all` | The admin actions above |

Connect Claude Code (the token stays in your shell, not in the repo):

```sh
claude mcp add --transport http ws-game "$GAME/mcp" --header "Authorization: Bearer $ADMIN_TOKEN"
```

Or in a project `.mcp.json` (Claude Code expands `${VAR}`; keep the real
token in the environment, never in the file):

```json
{
  "mcpServers": {
    "ws-game": {
      "type": "http",
      "url": "http://127.0.0.1:8080/mcp",
      "headers": { "Authorization": "Bearer ${WS_GAME_ADMIN_TOKEN}" }
    }
  }
}
```

Then ask the agent e.g. "list the open bug reports and fix the top one".
A quick check by hand:

```sh
curl -s "$GAME/mcp" -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_feedback","arguments":{"status":"open"}}}'
```

Requests from a browser `Origin` other than the server's host are refused
(DNS rebinding); like `/admin/*`, keep `/mcp` behind TLS and off the public
internet where you can.

## Tests

```sh
cargo test                             # unit + integration tests
cargo test --test chat_admin_test      # chat, broadcast, countdown, save
cargo test --test feedback_mcp_test    # feedback, /mcp, /health, /api/online
```
