#!/usr/bin/env bash
# Builds, tries and publishes the game image (example/game/DEPLOY.md).
#
#   deploy/image.sh build   # docker buildx, loaded into the local engine
#   deploy/image.sh run     # the built image on http://127.0.0.1:${PORT:-8088},
#                           # against the dev Postgres (mise run game:db:up)
#   deploy/image.sh push    # docker push <version>-<sha> and latest
#
# Tags: plugfox/mobbed:<server version>-<git sha> and :latest; "-dirty" when
# the working tree (or the assets submodule) has changes, which push refuses
# unless FORCE=1. PLATFORM (default linux/arm64, the production host)
# selects the target; another architecture builds the server under emulation.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
IMAGE=${IMAGE:-plugfox/mobbed}
PLATFORM=${PLATFORM:-linux/arm64}

die() {
  echo "error: $*" >&2
  exit 1
}

version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/example/game/server/Cargo.toml" | head -1
}

tag() {
  local t
  t="$(version)-$(git -C "$ROOT" rev-parse --short=8 HEAD)"
  [[ -z $(git -C "$ROOT" status --porcelain) ]] || t="$t-dirty"
  echo "$t"
}

build() {
  local tag sha id
  tag=$(tag)
  sha=$(git -C "$ROOT" rev-parse --short=8 HEAD)
  # The build id versions the static URLs: unique per build, so a rebuild
  # of one commit with a re-baked pack is never served from old caches.
  id="$tag.$(date -u +%Y%m%d%H%M%S)"
  [[ -d "$ROOT/example/assets/dist/client" ]] || die "no baked pack in example/assets/dist (npm --prefix example/assets run bake)"
  echo "==> $IMAGE:$tag ($PLATFORM, build $id)"
  docker buildx build --platform "$PLATFORM" --load \
    -f "$ROOT/example/game/deploy/Dockerfile" \
    --build-arg "BUILD_ID=$id" \
    --build-arg "GIT_HASH=$sha" \
    --build-arg "VERSION=$tag" \
    --build-arg "CREATED=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    -t "$IMAGE:$tag" -t "$IMAGE:latest" \
    "$ROOT"
  docker image ls "$IMAGE:$tag"
}

run() {
  local port=${PORT:-8088}
  echo "==> $IMAGE:latest on http://127.0.0.1:$port"
  # -it only from a terminal (scripts and CI have no TTY).
  local tty=(); [[ -t 0 && -t 1 ]] && tty=(-it)
  docker run --rm ${tty[@]+"${tty[@]}"} --name mobbed-local -p "127.0.0.1:$port:8080" \
    -e DATABASE_URL="${DATABASE_URL:-postgres://mobbed:mobbed@host.docker.internal:55432/mobbed}" \
    -e JWT_SECRET="${JWT_SECRET:-local-image-test-secret-0123456789abcdef}" \
    "$IMAGE:latest"
}

push() {
  local tag
  tag=$(tag)
  [[ $tag != *-dirty || ${FORCE:-0} == 1 ]] || die "$tag: commit first (or FORCE=1)"
  docker image inspect "$IMAGE:$tag" >/dev/null 2>&1 || die "$IMAGE:$tag is not built (deploy:build)"
  echo "==> pushing $IMAGE:$tag and :latest"
  docker push "$IMAGE:$tag"
  docker push "$IMAGE:latest"
}

case "${1:-}" in
build) build ;;
run) run ;;
push) push ;;
*) die "usage: $0 build|run|push" ;;
esac
