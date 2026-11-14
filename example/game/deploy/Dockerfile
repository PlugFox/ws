# syntax=docker/dockerfile:1.7
#
# "Mobbed!" (https://mob.plugfox.dev): the game server with the web client,
# the engine bundle and the baked asset pack, in one image. Public on Docker
# Hub as plugfox/mobbed, assets included (example/game/DEPLOY.md).
#
# Build from the repository root (`mise run deploy:build` does it):
#   docker buildx build -f example/game/deploy/Dockerfile \
#     --build-arg BUILD_ID=<id> --build-arg GIT_HASH=<sha> -t plugfox/mobbed:dev .
#
# The pack is NOT baked here (the bake needs the private archives): the
# context takes the local example/assets/dist, minus what the game never
# loads (Dockerfile.dockerignore), and the WebP pass and precompression run
# on that copy.

ARG RUST_VERSION=1.99.0
ARG DART_VERSION=3.13.7
ARG NODE_VERSION=22

# --- engine: TypeScript / Three.js runtime -> engine.bundle.js ----------------
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-trixie-slim AS engine
WORKDIR /src/example/assets
COPY example/assets/package.json ./
COPY example/assets/engine/package.json example/assets/engine/package-lock.json engine/
RUN --mount=type=cache,target=/root/.npm npm ci --prefix engine --no-audit --no-fund
COPY example/assets/engine/tsconfig.json example/assets/engine/vite.config.ts engine/
COPY example/assets/engine/src engine/src
RUN mkdir -p dist && npm run build:engine

# --- client: Dart web client -> main.dart.js + the page and brand files -------
FROM --platform=$BUILDPLATFORM dart:${DART_VERSION} AS client
WORKDIR /src
# The client depends on package:ws at the repository root.
COPY pubspec.yaml ./
COPY lib lib
COPY example/game/client/pubspec.yaml example/game/client/pubspec.lock example/game/client/
RUN cd example/game/client && dart pub get
COPY example/game/client example/game/client
# The build id versions every static URL (server/src/api/static_files.rs):
# the pack at /assets/v/<id>/, the page's scripts with ?v=<id>.
ARG BUILD_ID=dev
RUN cd example/game/client \
    && ASSETS_BASE="/assets/v/${BUILD_ID}" ./tool/build.sh \
    && rm -f build/web/*.deps build/web/*.map \
    && sed -i \
        -e "s#src=\"/assets/engine.bundle.js\"#src=\"/assets/v/${BUILD_ID}/engine.bundle.js\"#" \
        -e "s#src=\"main.dart.js\"#src=\"main.dart.js?v=${BUILD_ID}\"#" \
        build/web/index.html \
    && grep -q "/assets/v/${BUILD_ID}/engine.bundle.js" build/web/index.html \
    && grep -q "main.dart.js?v=${BUILD_ID}" build/web/index.html

# --- server: Rust game server (release) ---------------------------------------
FROM rust:${RUST_VERSION}-trixie AS server
WORKDIR /src
COPY example/game/server/Cargo.toml example/game/server/Cargo.lock example/game/server/build.rs ./
COPY example/game/server/src src
# Embedded by sqlx::migrate! at compile time.
COPY example/game/server/migrations migrations
# The context has no .git: the commit comes as an argument (`/health` version).
ARG GIT_HASH=""
ENV GAME_SERVER_GIT_HASH=${GIT_HASH}
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked --bin game_server \
    && cp target/release/game_server /game_server

# --- pack: the asset pack and the client, WebP + precompressed ----------------
FROM --platform=$BUILDPLATFORM python:3.13-slim-trixie AS pack
RUN apt-get update \
    && apt-get install -y --no-install-recommends brotli \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir pillow==11.3.0
COPY example/assets/pipeline/webp_pass.py example/game/deploy/precompress.sh /tools/
# The pack first: its slow passes stay cached while only code changes.
COPY example/assets/dist /srv/assets
RUN python3 /tools/webp_pass.py /srv/assets && /tools/precompress.sh /srv/assets
COPY --from=engine /src/example/assets/dist/engine.bundle.js /srv/assets/
COPY --from=client /src/example/game/client/build/web /srv/web
RUN /tools/precompress.sh /srv/web /srv/assets/engine.bundle.js \
    && test -f /srv/assets/engine.bundle.js.br && test -f /srv/web/main.dart.js.br

# --- runtime -------------------------------------------------------------------
# cc: glibc + libgcc for the binary (the server stage is trixie too).
# Non-root (uid 65532). No shell: the binary is its own health check, and
# PID 1 handles SIGTERM itself (announce, save every player, exit).
FROM gcr.io/distroless/cc-debian13:nonroot
ARG BUILD_ID=dev
ARG GIT_HASH=""
ARG VERSION=0.0.0
ARG CREATED=""
LABEL org.opencontainers.image.title="Mobbed!" \
      org.opencontainers.image.description="Co-op horde survival in the browser: the game server with the web client and assets." \
      org.opencontainers.image.url="https://mob.plugfox.dev" \
      org.opencontainers.image.source="https://github.com/PlugFox/ws" \
      org.opencontainers.image.documentation="https://github.com/PlugFox/ws/blob/master/example/game/DEPLOY.md" \
      org.opencontainers.image.vendor="PlugFox" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${GIT_HASH}" \
      org.opencontainers.image.created="${CREATED}"
COPY --from=server /game_server /app/game_server
COPY --from=pack /srv /srv
WORKDIR /app
ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=8080 \
    GAME_ASSETS_DIR=/srv/assets \
    GAME_CLIENT_DIR=/srv/web \
    GAME_ASSETS_VERSION=${BUILD_ID} \
    RUST_LOG=game_server=info,tower_http=warn
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 CMD ["/app/game_server", "healthcheck"]
ENTRYPOINT ["/app/game_server"]
