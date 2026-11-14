# Deploying "Mobbed!"

The game runs at **https://mob.plugfox.dev** as one container: the Rust game
server, which also serves the web client (`/`), the engine bundle and the
baked asset pack (`/assets`), the HTTP API (`/api`, `/health`), the game
WebSocket (`/api/ws`) and the operator endpoints (`/admin/*`, `/mcp`).
It sits behind the shared Traefik of the plugfox.dev host and stores
everything in the shared PostgreSQL.

| Piece | Where |
|---|---|
| Image | Docker Hub `plugfox/mobbed` (public, assets included), built on a Mac |
| Dockerfile | `example/game/deploy/Dockerfile` (context: repository root, `Dockerfile.dockerignore` beside it) |
| Build / run / push | `mise run deploy:build`, `deploy:run`, `deploy:push` (`example/game/deploy/image.sh`) |
| Stack | stacks repository, `stacks/apps/mobbed/compose.yaml` + `secrets.env.example` (Komodo) |
| Database | role and database `mobbed` on `postgres:5432` (network `shared-db`) |
| Host | Hetzner CAX31, **linux/arm64** |

## 1. Build the image

Needs Docker with buildx, mise, and a baked pack in `example/assets/dist`
(`npm --prefix example/assets run bake`; the bake needs the private archives,
so the image never bakes, it takes the local `dist`).

```bash
mise run deploy:build                         # linux/arm64 (the host)
PLATFORM=linux/amd64 mise run deploy:build    # another target (server built under emulation: slow)
```

Tags: `plugfox/mobbed:<server version>-<git sha>` (Cargo.toml version, short
commit; `-dirty` with uncommitted changes in the repository or the assets
submodule) and `plugfox/mobbed:latest`.

What the stages do:

1. **engine**: `npm ci` + `npm run build:engine` -> `engine.bundle.js`.
2. **client**: `dart pub get` + `tool/build.sh` (`dart compile js -O2`, the
   page, manifest, service worker and brand images), with
   `ASSETS_BASE=/assets/v/<build id>`; the page's script tags get the build
   id too. Source maps and `.deps` are dropped.
3. **server**: `cargo build --release --locked` (migrations embedded).
4. **pack**: the local `dist` minus what the game never requests (the
   all-bodies pack `client/bodies/` with every RO class body and mount,
   raw skill VFX sources, the colour customization atlas, type
   declarations: ~160 MB of bake intermediates), then
   `pipeline/webp_pass.py` (location GLB atlases to WebP, sprite sheets,
   grass atlases and emotes to lossless WebP) and `deploy/precompress.sh`
   (`.br` + `.gz` beside every compressible file: js, json, html, svg,
   webmanifest, wasm, glb, bin; kept when they save 10 %+).
5. **runtime**: `gcr.io/distroless/cc-debian13:nonroot` (uid 65532, no
   shell), the binary at `/app/game_server`, the files under `/srv`.
   `HEALTHCHECK` runs `game_server healthcheck` (`GET /health` on the local
   port: 200 when the database answers).

Every body the game draws ships inside the customization pack
(`client/customization/`, index atlas + palettes), not the all-bodies
pack: the class bodies of each tier and the mounted ones, the Crusader and
Paladin on a Grand Peco (`crusader_peco`, `paladin_peco`, with their dyes
and weapon layers; `pipeline/customization_baker.py` `JOB_BODIES`). A new
mount or class body goes there too, so the image needs no new exception.
The one use of the all-bodies pack is the creation screen's preview of the
classes that are not playable yet (client `classes.dart` `lockedBodies`):
those few `client/bodies/sprites/<id>_<m|f>.*` files (~3 MB) are let back
in by `Dockerfile.dockerignore`; unlocking a class there means adding it to
the customization pack and dropping its exception.

Try it locally against the dev database before pushing:

```bash
mise run deploy:run      # http://127.0.0.1:8088 (PORT=... to change), Ctrl-C stops (and saves)
```

## 2. Publish

```bash
docker login
mise run deploy:push     # pushes <version>-<sha> and latest; refuses -dirty unless FORCE=1
```

## 3. The stack (first time)

1. **DNS**: an `A` record `mob.plugfox.dev` -> the host (Traefik gets the
   certificate from Let's Encrypt on the first request).
2. **Secrets** (Komodo -> Stack -> Environment), each `openssl rand -hex 32`:
   - `MOBBED_DB_PASSWORD`: the database role's password (also in the
     postgres stack's environment, used by its init script on a fresh data
     directory).
   - `MOBBED_JWT_SECRET`: signs the access tokens; changing it signs everyone out.
   - `MOBBED_ADMIN_TOKEN`: `Authorization: Bearer` of `/admin/*` and `/mcp`;
     empty disables them.
3. **Database**. `infra/postgres/init/01-create-databases.sh` creates it, but
   init scripts only run on the first start of an empty data directory. On
   the running server, once:

   ```bash
   docker compose -f /opt/stacks/infra/postgres/compose.yaml exec -T postgres \
     sh -c 'psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d postgres' <<'SQL'
   CREATE USER mobbed WITH PASSWORD '<MOBBED_DB_PASSWORD>';
   CREATE DATABASE mobbed OWNER mobbed;
   SQL
   ```

   The admin role is `POSTGRES_ADMIN_USER` of the postgres stack (not
   necessarily `postgres`); inside the container it is `$POSTGRES_USER`.

   The server applies its migrations (`server/migrations/`) at boot.
4. **Komodo**: a Stack pointing at `stacks/apps/mobbed`, then Deploy.
5. **Check**: `curl -s https://mob.plugfox.dev/health` (`"status":"healthy"`,
   the version, `"hub":"ok"`), then sign up in a browser and start a mission.

## 4. Updating

Build, push, then redeploy the stack in Komodo (it pulls `latest`). Planned
restarts can be announced first:

```bash
curl -X POST https://mob.plugfox.dev/admin/restart-warning \
  -H "Authorization: Bearer $MOBBED_ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"minutes": 5}'
```

On `docker stop` (SIGTERM) the server announces the restart, saves every
player, stops accepting connections, lets requests in flight finish for 5 s,
saves again and exits; `stop_grace_period` is 30 s. Clients see the socket
drop and reconnect to the new container; a player in a mission keeps the
slot for the stage grace period (`STAGE_LINK_GRACE_SECS`) only while the
same process lives, so a restart ends running missions.

Before a deploy that adds a migration, take a dump (see Backups): rolling
back past a migration needs it.

## 5. Rollback

Set the stack's image to the previous tag (`plugfox/mobbed:<version>-<sha>`,
listed on Docker Hub) and redeploy; set it back to `latest` with the next
release. Static files are versioned by build id, so browsers switch cleanly
in both directions (pages revalidate by content `ETag`, so a page cached
from the other build is never answered 304; a pack id another server does
not own is served `no-cache`).

If the newer release added a migration, the older binary refuses to start
(`_sqlx_migrations` names a version it does not have): restore the dump taken
before the deploy, then start the older image, or roll forward with a fix.

## 6. Backups

Nothing in the container is state: the database is everything.

- The postgres stack's `pgbackup` sidecar writes an hourly `pg_dumpall`
  (all databases, `mobbed` included) to `/opt/stacks/infra/postgres/dumps/`,
  keeping the last 24.
- Backrest (restic) snapshots `/opt/stacks/` daily to Backblaze B2 (7 daily,
  4 weekly, 6 monthly), dumps included (stacks repository, `docs/backups.md`).

A dump of the game alone (before risky deploys) and its restore:

```bash
docker compose -f /opt/stacks/infra/postgres/compose.yaml exec -T postgres \
  pg_dump -U postgres -Fc mobbed > mobbed-$(date +%Y%m%d-%H%M).dump
# restore: stop the mobbed stack first
docker compose -f /opt/stacks/infra/postgres/compose.yaml exec -T postgres \
  pg_restore -U postgres -d mobbed --clean --if-exists --no-owner --role=mobbed < mobbed-<stamp>.dump
```

## 7. Configuration

Set in the stack (`compose.yaml`) or baked into the image (marked *image*).

| Variable | Default | |
|---|---|---|
| `SERVER_HOST`, `SERVER_PORT` | *image*: `0.0.0.0`, `8080` | listen address |
| `DATABASE_URL` | dev container | `postgres://mobbed:…@postgres:5432/mobbed` |
| `DATABASE_MAX_CONNECTIONS` | 10 | pool size |
| `DATABASE_ACQUIRE_TIMEOUT_SECS` | 5 | |
| `DATABASE_STATEMENT_TIMEOUT_MS` | 15000 | session `statement_timeout` |
| `DATABASE_CONNECT_RETRY_SECS` | 60 | boot retries an unreachable database |
| `JWT_SECRET` | an insecure dev key | **required** in production |
| `ADMIN_TOKEN` | unset (off) | `/admin/*`, `/mcp` |
| `ACCESS_TOKEN_TTL_SECS` / `REFRESH_TOKEN_TTL_SECS` | 3600 / 2592000 | |
| `FLUSH_INTERVAL_SECS` | 30 | periodic save of changed characters |
| `GEOIP_DB_PATH` | unset | MaxMind country database (not in the image) |
| `GAME_ASSETS_DIR`, `GAME_CLIENT_DIR` | *image*: `/srv/assets`, `/srv/web` | |
| `GAME_ASSETS_VERSION` | *image*: the build id | week-long caching of `/assets/v/<id>/`; hashes every file at startup |
| `HUB_CHANNEL_CAPACITY`, `HUB_MAX_CHANNELS`, `HUB_RESIDENTS` | code defaults | town channels |
| `MAX_STAGES`, `STAGE_MONSTER_CAP`, `STAGE_LINK_GRACE_SECS` | code defaults | missions |
| `RUST_LOG` | *image*: `game_server=info,tower_http=warn` | |
| `GAME_DEV`, `GAME_DEV_LEVEL`, `DATABASE_CREATE` | off | test hooks: never in production |

## 8. Caching and compression

The server sends `.br` / `.gz` twins to clients that accept them (tower-http
`ServeDir`), with `Vary: Accept-Encoding`; Traefik adds no compression (it
would only re-compress or wrap the WebSocket upgrade). Nothing is
content-hashed, so the build id versions the URLs
(`server/src/api/static_files.rs`):

| Path | `Cache-Control` |
|---|---|
| `/assets/v/<this build>/…` (pack, engine bundle), `main.dart.js?v=<this build>` | `public, max-age=604800` (a week, then revalidated) |
| `/assets/v/<other build>/…`, unversioned `/assets/…` (development) | `no-cache` |
| `/`, `*.html`, `sw.js`, `manifest.webmanifest`, the rest | `no-cache` |
| brand images (`/icon-*.png`, `/og.jpg`, ...) | `public, max-age=86400` |
| `/api/online` / `/api/leaderboard` | `public, max-age=2` / `public, max-age=10` |
| other `/api/*`, `/health`, `/admin/*`, `/mcp` | `no-store` (personal) |
| errors (404, ...) | `no-cache` |

`no-cache` means "revalidate every time", not "do not keep": every static
answer carries validators, so a revalidation is a body-less 304.

- **`ETag`**: strong, the first 128 bits of the SHA-256 of the bytes sent
  (base64url). The `.br`, `.gz` and identity variants are separate files,
  so each encoding has its own tag, as `Vary: Accept-Encoding` needs.
  Tags are computed on first use and kept while the file's size and mtime
  hold, so a file rebuilt under a running development server is rehashed.
  With a build id (the image) the whole tree is hashed once in a background
  task at startup; the server answers meanwhile (a file asked for before
  its turn is hashed on the spot). Measured on an M-series laptop: the
  unfiltered local pack, 6795 files / 257 MB, in 1.5 s; the client, 19
  files / 2 MB, in 10 ms. The image's pack is smaller.
- **`Last-Modified`**: the file's mtime (`precompress.sh` gives the twins
  the source's).
- **304**: `If-None-Match` (weak comparison, lists, `*`) wins over
  `If-Modified-Since` (exact date match, as nginx does); both answer 304
  with `ETag`, `Last-Modified`, `Cache-Control` and `Vary`.
- **Ranges** (audio): `ServeDir` serves single ranges; `If-Range` with the
  current strong tag or date keeps the range, anything else gets the whole
  file.
- The service worker (`client/web/sw.js`) keeps only the shell for offline
  starts: network first through the HTTP cache (so the policy above holds),
  brand images stale-while-revalidate; it never touches `/assets` or `/api`.

A first visit to a mission (sign-in, town, Poring Island) moves about 21 MB
(was 66 MB before the WebP pass and precompression); a returning player on
the same build within a week revalidates only the page (one 304), after a
week each file once more (304s unless it changed).
