# Developer & Agent Guidelines — `ws` Repository

Welcome to the `ws` repository. This file provides architectural context, strict rules, and workflows for human developers and autonomous AI coding agents working on this codebase.

---

## 1. Repository Structure & Separation of Concerns

```
ws/
├── lib/               Core Dart package:ws library (WebSocket & WebTransport)
├── native/            Rust transport library (tokio, tungstenite, rustls) via FFI
├── benchmark/         Performance test harness and mobile Flutter benchmark app
├── server/            Standalone Rust benchmark server (echo, flood, latency)
├── docs/              Architecture, decisions, roadmap, and test plans for package:ws
└── example/           Showcase survivors-like web game exercising package:ws
    ├── docs/          Game roadmap and specifications (gitignored from public repo)
    ├── game/          Game source code (Dart web client, ECS, Rust game server)
    └── assets/        Git submodule -> PlugFox/ws-game-assets (gitignored private repo)
        ├── grf/       Raw input archives (tracked with Git LFS)
        ├── data/      Local rAthena databases (mob_db.yml, mob_skill_db.txt)
        ├── recipe.toml Declarative asset & balance bake recipe
        ├── pipeline/  ETL pipeline (Python / Node.js)
        ├── engine/    TypeScript Three.js engine runtime (Vite bundle)
        └── dist/      Baked client & server packs (KTX2, heightmaps, walkability.bin)
```

---

## 2. Critical Rules & Legal Hygiene

1. **Zero Proprietary Formats in Public Code**:
   - The main repository (`ws`) MUST NOT contain any loaders, parser code, assets, or data structures specific to third-party proprietary games (such as Ragnarok Online `.grf`, `.gat`, `.gnd`, `.rsw`, `.spr`, `.act`).
   - All proprietary asset processing logic belongs strictly inside `example/assets/` (`ws-game-assets` submodule).
2. **Clean Game Engine Contracts**:
   - The game client and server in `example/` only consume generic, industry-standard game formats:
     - Textures: KTX2 texture arrays / atlases (`.ktx2`)
     - Animations: JSON manifests (`manifest.json`)
     - Terrains: 16-bit PNG heightmaps (`heightmap.png`)
     - Physics / Pathfinding: Binary walkability grids (`walkability.bin`)
     - Audio: Opus in OGG container (`.ogg`)
3. **Submodule Workflow**:
   - When editing pipeline, recipe, or TypeScript engine code in `example/assets/`, commit and push to `PlugFox/ws-game-assets` first, then commit the submodule pointer bump in the parent `ws` repo.

---

## 3. Architecture & Implementation Guidelines

### 3.1 Dart Web Client (`example/game/client/`)
- **Language**: Modern Dart 3 with strict types and `dart:js_interop` (compatible with both `dart2js` and `dart2wasm`).
- **Façade Pattern**: Do NOT create hundreds of direct Three.js or WebAudio class bindings in Dart. Talk exclusively to the high-level `GameView` façade (`example/game/view/game_view.dart`).
- **Zero-Copy Data Transfer**: Pass entity snapshot and state arrays as typed views (`Float32List` → `JSFloat32Array`) via `dart:js_interop`. Under `dart2wasm` / `dart2js`, this shares the underlying `ArrayBuffer` without memory copies.
- **Client ECS & Simulation**: Client handles local player prediction, input queueing, snapshot interpolation buffer (~100 ms with adaptive jitter compensation), and network status reporting.

### 3.2 Rust Game Server (`example/game/server/`)
- **Data-Oriented Design (DoD)**:
  - Do NOT use heavy object-oriented hierarchies (`Vec<Box<dyn Monster>>`).
  - Use **Struct-of-Arrays (SoA)**: parallel contiguous arrays for positions, velocities, health, and state flags (`HordeStorage`) to maximize CPU L1/L2 cache locality and enable automatic SIMD vectorization (AVX2 / ARM NEON).
  - Use `rayon` for parallel iteration over spatial sectors.
- **Zero-Allocation Runtime**:
  - Pre-allocate entity pools using generational indexing / free-list stacks (`free_slots: Vec<u16>`). Spawning and killing entities must never call system `malloc`/`free`.
- **Spatial Partitioning**:
  - Use a **Uniform Spatial Hash Grid** (16×16m / 32×32m sectors) with $O(1)$ bitshift indexing (`x >> 4`) instead of pointer-chasing QuadTrees.
- **Simulation LOD (Tiered Simulation Frequency)**:
  - **Tier 0 (Active / Inside light radius + 2m buffer)**: 30 Hz full simulation tick, discrete cell collisions, boids separation, exact hitboxes.
  - **Tier 1 (Periphery / 1.0–1.5× radius)**: 10 Hz (every 3rd tick), linear velocity prediction.
  - **Tier 2 (Distant / > 1.5× radius off-screen)**: 1–2 Hz macro-step, ghosting through minor obstacles (trees, bushes) along macro-flow vectors, batch SIMD movement.
  - **Bosses (0..5 entities)**: Always Tier 0 (30 Hz full fidelity regardless of distance).
- **Double-Box Radial Filter (Zero-Sqrt)**:
  - When testing if entity $(x, y)$ is inside player circular light radius $R$:
    1. *Fast Reject*: $|dx| > R \lor |dy| > R \implies$ discard ($0$ multiplications).
    2. *Fast Accept*: $|dx| \le 0.7071 R \land |dy| \le 0.7071 R \implies$ accept immediately (inscribed square covers ~63.6% of circle area, $0$ multiplications).
    3. *Annular Check*: only for the remaining ~36.4% boundary band, evaluate $dx^2 + dy^2 \le R^2$ ($2$ mults, $1$ add, no `sqrt`).
- **Hierarchical Pathfinding for Choke-Point Maps**:
  - Distance Transform + Watershed segmentation breaks dungeon maps into convex rooms and wide portals.
  - Macro A* finds room sequences ($N_{\text{rooms}} \ll N_{\text{cells}}$).
  - Micro Flow Field initializes a zero-distance boundary across the entire width of the doorway so hordes fan out instead of single-file queueing.
  - Continuum Crowds density fields dynamically penalize congested door centers, naturally steering followers to open portal edges.

### 3.3 TypeScript Engine Runtime (`example/assets/engine/`)
- Implements `GameView` interface using Three.js, WebAudio, and `fflate`.
- Uses `InstancedMesh` per texture array (entire horde rendered in a single draw call).
- GPU frame selection via data-texture vertex pulling (`texelFetch` / `ShaderMaterial`).
- Bundled into `dist/engine.bundle.js` via Vite (`npm run build:engine`).

---

## 4. Common Commands & Runbooks

```bash
# Run core Dart library tests
dart test

# Run Rust transport tests
cd native/rust && cargo test

# Bake game assets & server databases from recipe.toml
cd example/assets/pipeline && python3 bake.py
# or from example/assets root:
npm run bake

# Build TypeScript game engine runtime bundle
npm run build:engine

# Start TypeScript engine dev server
npm run dev:engine
```
