# Game web client

Dart web client for the hub: login, prediction, interpolation, NPCs. Rendering
goes through the TypeScript `GameView` façade (`example/assets/engine`).

## Run locally

```bash
# 1. Placeholder art in the baked pack (skips files that already exist)
node example/game/tools/placeholders/gen.mjs

# 2. Engine bundle -> example/assets/dist/engine.bundle.js
cd example/assets/engine && npm install && npm run build && cd -

# 3. Client -> example/game/client/build/web
cd example/game/client && dart pub get && tool/build.sh && cd -

# 4. Server (serves the client at /, the pack at /assets)
cd example/game/server && cargo run
```

Open http://127.0.0.1:8080, register, create a character and select it.
WASD/arrows move, Q/E or right-drag rotate the camera (free maps only), the
wheel zooms, F talks to the NPC in range, F10 opens the settings.

Bots: `dart run tool/bot.dart --server http://127.0.0.1:8080 --count 10`.
Tests: `dart test`.
