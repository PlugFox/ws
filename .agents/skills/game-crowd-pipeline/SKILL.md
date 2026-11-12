---
name: game-crowd-pipeline
description: >-
  Provides end-to-end guidance for the survivors-like game example in ws:
  baking assets from recipe.toml & GRF archives, data-oriented Rust server
  architecture (SoA, SIMD, tiered simulation LOD, double-box AoI filter),
  hierarchical crowd pathfinding through wide portals, and TypeScript Three.js engine runtime.
---

# Game Crowd Pipeline & Server Architecture Guide

Use this skill when developing, debugging, or extending the real-time survivors-like game in `example/`.

## 1. Asset Baking Workflow

The asset pipeline is located in `example/assets/` (submodule pointing to `PlugFox/ws-game-assets`).

### Configuration: `recipe.toml`
Located at `example/assets/recipe.toml`. Defines:
- `grf_archives`: Path to `.grf` archives (e.g. `grf/data.grf`).
- `sources`: Local databases (`data/mob_db.yml`, `data/mob_skill_db.txt`).
- `locations.<id>`: GAT/GND/RSW paths, BGM, camera constraints (`free` vs `fixed`), portals.
- `monsters.<id>`: SPR/ACT paths, animation categories, sound bindings, rAthena stats, skills, and AI Director spawn budgets.

### Running the Baker
```bash
cd example/assets/pipeline && python3 bake.py
# or from example/assets/
npm run bake
```

### Outputs (`dist/`)
- `client/locations/<id>/heightmap.png`: 16-bit grayscale PNG.
- `client/locations/<id>/manifest.json`: Camera bounds, lighting, water, portals, BGM.
- `client/audio/bgm/<id>.ogg`: Transcoded with ffmpeg using libopus at 96 kbps.
- `client/audio/sfx/<mob>/<action>.ogg`: Monster sound effects.
- `client/monsters/<id>/manifest.json`: Action categories (8 directions), skills, preview frame.
- `server/locations/<id>/walkability.bin`: `[u32 width, u32 height, u8 cells...]` (0 = walkable, 1 = obstacle).
- `server/monsters.json`: Full combat stats, AI Director budgets, and skill parameters for the Rust ECS.

---

## 2. Rust Game Server Architecture

### Data-Oriented Design (SoA)
Monsters must be stored in Struct-of-Arrays (SoA) layout to enable auto-vectorization (SIMD) and optimal CPU cache locality:
```rust
pub struct HordeStorage {
    pub pos_x: Vec<f32>,
    pub pos_y: Vec<f32>,
    pub vel_x: Vec<f32>,
    pub vel_y: Vec<f32>,
    pub hp: Vec<i16>,
    pub flags: Vec<u8>,
    pub free_slots: Vec<u16>,
}
```

### Double-Box Radial Filter (Zero Sqrt)
For testing if entity $(x, y)$ is within player light radius $R$:
1. **Fast Reject**: $|dx| > R \lor |dy| > R \implies$ discard ($0$ multiplications).
2. **Fast Accept**: $|dx| \le 0.7071 R \land |dy| \le 0.7071 R \implies$ accept immediately (~63.6% of area, $0$ multiplications).
3. **Annular Check**: $dx^2 + dy^2 \le R^2$ ($2$ mults, $1$ add, no `sqrt`).

### Simulation LOD (Tiered Ticks)
- **Tier 0 (Active)**: within player radius + 2m; 30 Hz full simulation tick, grid collision, boids separation, exact hitboxes.
- **Tier 1 (Periphery)**: 1.0–1.5× radius; 10 Hz (every 3rd tick), linear velocity extrapolation.
- **Tier 2 (Distant)**: > 1.5× radius off-screen; 1–2 Hz macro-step, ghosting through minor obstacles along macro-flow vectors, batch SIMD movement.
- **Bosses (0..5 units)**: Always Tier 0 (30 Hz full fidelity regardless of distance).

### Hierarchical Pathfinding (HPA* + Wide Portals)
- **Macro level**: Topological room graph ($N_{\text{rooms}} \ll N_{\text{cells}}$).
- **Micro level**: Wide-portal integration field: zero-distance boundary across the entire door width so hordes fan out instead of single-file queueing.
- **Continuum Crowds**: Dynamic density field penalizes congested door centers, naturally steering followers to open portal edges.

---

## 3. TypeScript Engine Runtime (`example/assets/engine/`)

The TypeScript runtime implements the `GameView` façade:
```typescript
export interface GameView {
  init(canvas: HTMLCanvasElement): Promise<void>;
  loadLocation(manifestUrl: string): Promise<void>;
  renderFrame(alpha: number, entityBuffer: Float32Array): void;
  spawnVfx(id: string, x: number, y: number): void;
  playSfx(id: string, x: number, y: number): void;
  updateHud(hp: number, xp: number, level: number, netStats: Record<string, unknown>): void;
}
```

- **Build command**: `npm run build:engine` (outputs to `dist/engine.bundle.js`).
- **Interop**: Dart calls `GameView` via `dart:js_interop` extension types, passing entity snapshot buffers zero-copy (`Float32List` → `JSFloat32Array`).
