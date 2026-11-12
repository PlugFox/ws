#!/usr/bin/env node
// Generates placeholder client assets in the agreed formats, so the engine
// and client can be developed before the real art exists:
//
//   client/locations/<id>/sprites.png + sprites.json  flipbook atlas
//   client/locations/<id>/terrain.glb                 ground mesh (vertex colours)
//   client/locations/<id>/props.glb                   props, EXT_mesh_gpu_instancing
//   client/locations/<id>/manifest.json              + npcs (+ heightScale)
//
// Existing files are kept unless --force is given, so real assets dropped
// into dist are never overwritten. Zero dependencies (node >= 22).
//
//   node gen.mjs [--dist ../../../assets/dist] [--force]

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const args = process.argv.slice(2);
const force = args.includes('--force');
const distArg = args.indexOf('--dist');
const here = path.dirname(new URL(import.meta.url).pathname);
const dist = path.resolve(distArg >= 0 ? args[distArg + 1] : path.join(here, '../../../assets/dist'));

// Sprites that may appear on each kind of location ("we know every monster
// on a location" → one atlas per location, one draw call for all units).
const PLAYERS = ['novice_m', 'novice_f'];
const MONSTERS = ['poring', 'lunatic', 'fabre', 'zombie', 'baphomet'];
const SPRITES_BY_KIND = {
  hub: [...PLAYERS, 'mission_npc'],
  indoor: [...PLAYERS],
  field: [...PLAYERS, ...MONSTERS],
};

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(td) >>> 0);
  return Buffer.concat([len, td, crc]);
}

function encodePngRgba(w, h, rgba) {
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 4 + 1)] = 0;
    rgba.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

/** Decodes a 16-bit grayscale PNG (the heightmap format). */
function decodePngGray16(buf) {
  let pos = 8;
  let w = 0, h = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString('ascii', pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === 'IHDR') {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      if (data[8] !== 16 || data[9] !== 0) throw new Error('heightmap must be 16-bit grayscale');
    } else if (type === 'IDAT') idat.push(data);
    pos += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const bpp = 2, stride = w * bpp;
  const out = Buffer.alloc(stride * h);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)];
    const src = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? out[y * stride + i - bpp] : 0;
      const b = y > 0 ? out[(y - 1) * stride + i] : 0;
      const c = y > 0 && i >= bpp ? out[(y - 1) * stride + i - bpp] : 0;
      let v = src[i];
      if (f === 1) v += a;
      else if (f === 2) v += b;
      else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) {
        const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      out[y * stride + i] = v & 0xff;
    }
  }
  const samples = new Uint16Array(w * h);
  for (let i = 0; i < w * h; i++) samples[i] = out.readUInt16BE(i * 2);
  return { w, h, samples };
}

// ---------------------------------------------------------------------------
// Tiny RGBA rasteriser for the placeholder flipbooks
// ---------------------------------------------------------------------------

class Canvas {
  constructor(w, h) {
    this.w = w;
    this.h = h;
    this.px = Buffer.alloc(w * h * 4);
  }
  set(x, y, [r, g, b, a = 255]) {
    x = Math.round(x);
    y = Math.round(y);
    if (x < 0 || y < 0 || x >= this.w || y >= this.h) return;
    const i = (y * this.w + x) * 4;
    this.px[i] = r; this.px[i + 1] = g; this.px[i + 2] = b; this.px[i + 3] = a;
  }
  ellipse(cx, cy, rx, ry, fill, outline) {
    for (let y = Math.floor(cy - ry - 1); y <= cy + ry + 1; y++) {
      for (let x = Math.floor(cx - rx - 1); x <= cx + rx + 1; x++) {
        const d = ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2;
        if (d <= 1) this.set(x, y, d > 0.72 && outline ? outline : fill);
      }
    }
  }
  rect(x0, y0, w, h, c) {
    for (let y = y0; y < y0 + h; y++) for (let x = x0; x < x0 + w; x++) this.set(x, y, c);
  }
  triangle(ax, ay, bx, by, cx, cy, c) {
    const minX = Math.floor(Math.min(ax, bx, cx)), maxX = Math.ceil(Math.max(ax, bx, cx));
    const minY = Math.floor(Math.min(ay, by, cy)), maxY = Math.ceil(Math.max(ay, by, cy));
    const e = (x0, y0, x1, y1, x, y) => (x1 - x0) * (y - y0) - (y1 - y0) * (x - x0);
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const px = x + 0.5, py = y + 0.5;
        const w0 = e(bx, by, cx, cy, px, py), w1 = e(cx, cy, ax, ay, px, py), w2 = e(ax, ay, bx, by, px, py);
        if ((w0 >= 0 && w1 >= 0 && w2 >= 0) || (w0 <= 0 && w1 <= 0 && w2 <= 0)) this.set(x, y, c);
      }
    }
  }
}

const shade = ([r, g, b], k) => [Math.min(255, r * k) | 0, Math.min(255, g * k) | 0, Math.min(255, b * k) | 0];

// Look of each placeholder: colour, frame size in px, body shape.
const LOOKS = {
  novice_m: { color: [70, 120, 220], w: 32, h: 52, shape: 'human' },
  novice_f: { color: [220, 90, 150], w: 32, h: 50, shape: 'human' },
  mission_npc: { color: [230, 180, 60], w: 40, h: 60, shape: 'human' },
  poring: { color: [250, 140, 170], w: 30, h: 28, shape: 'blob' },
  lunatic: { color: [240, 240, 240], w: 28, h: 30, shape: 'blob' },
  fabre: { color: [140, 200, 90], w: 34, h: 24, shape: 'blob' },
  zombie: { color: [120, 150, 110], w: 36, h: 60, shape: 'human' },
  baphomet: { color: [90, 60, 70], w: 80, h: 110, shape: 'human' },
};

const PLAYER_ACTIONS = {
  idle: { frames: 3, frameMs: 220, loop: true },
  walk: { frames: 4, frameMs: 110, loop: true },
  sit: { frames: 1, frameMs: 1000, loop: true },
};
const MONSTER_ACTIONS = {
  idle: { frames: 3, frameMs: 200, loop: true },
  walk: { frames: 4, frameMs: 120, loop: true },
  attack: { frames: 4, frameMs: 80, loop: false },
  hit: { frames: 2, frameMs: 100, loop: false },
  die: { frames: 3, frameMs: 150, loop: false },
};

/**
 * Draws one frame. `dir` is camera-relative: 0 = facing away from the camera
 * (up the screen), clockwise in 45° steps. An arrow on the body shows it and
 * the frame number is shown as dots, so animation bugs are visible at once.
 */
function drawFrame(look, action, dir, frame) {
  const c = new Canvas(look.w, look.h);
  const base = look.color;
  const dark = shade(base, 0.45);
  const cx = look.w / 2;
  const bob = action === 'walk' ? (frame % 2 === 0 ? 0 : -2) : action === 'idle' ? (frame === 1 ? -1 : 0) : 0;
  const squash = action === 'die' ? 1 - 0.3 * frame : action === 'hit' ? 0.85 : 1;
  const tint = action === 'hit' ? [255, 80, 80] : base;
  const foot = look.h - 2;

  if (look.shape === 'blob') {
    const ry = (look.h / 2 - 2) * squash;
    c.ellipse(cx, foot - ry + bob, look.w / 2 - 2, ry, tint, dark);
  } else {
    const bodyH = look.h * 0.42 * squash;
    const headR = look.w * 0.24;
    if (action === 'walk') {
      const s = frame % 2 === 0 ? 3 : -3;
      c.rect(Math.round(cx - 5 + s), foot - 9, 4, 9, dark);
      c.rect(Math.round(cx + 1 - s), foot - 9, 4, 9, dark);
    } else if (action !== 'sit') {
      c.rect(Math.round(cx - 5), foot - 9, 4, 9, dark);
      c.rect(Math.round(cx + 1), foot - 9, 4, 9, dark);
    }
    const bodyBottom = action === 'sit' ? foot : foot - 8;
    c.ellipse(cx, bodyBottom - bodyH / 2 + bob, look.w * 0.32, bodyH / 2, tint, dark);
    c.ellipse(cx, bodyBottom - bodyH - headR + 2 + bob, headR, headR, [245, 215, 185], dark);
    if (action === 'attack') {
      const reach = 4 + frame * 3;
      const a = (dir * Math.PI) / 4;
      c.rect(Math.round(cx + Math.sin(a) * reach) - 2, Math.round(bodyBottom - bodyH * 0.6 - Math.cos(a) * reach), 5, 5, [250, 250, 250]);
    }
  }

  // Direction arrow, centred on the body.
  const a = (dir * Math.PI) / 4;
  const ax = Math.sin(a), ay = -Math.cos(a);
  const mcx = cx, mcy = foot - look.h * 0.35 + bob;
  const len = Math.min(look.w, look.h) * 0.28;
  c.triangle(
    mcx + ax * len, mcy + ay * len,
    mcx - ay * len * 0.5 - ax * len * 0.3, mcy + ax * len * 0.5 - ay * len * 0.3,
    mcx + ay * len * 0.5 - ax * len * 0.3, mcy - ax * len * 0.5 - ay * len * 0.3,
    [255, 255, 255],
  );
  for (let i = 0; i <= frame; i++) c.rect(1 + i * 3, 1, 2, 2, [20, 20, 20]);
  return c;
}

function buildAtlas(spriteIds) {
  const images = []; // { canvas, ax, ay }
  const sprites = {};
  for (const id of spriteIds) {
    const look = LOOKS[id];
    const actions = id === 'mission_npc'
      ? { idle: { frames: 2, frameMs: 450, loop: true } }
      : MONSTERS.includes(id) ? MONSTER_ACTIONS : PLAYER_ACTIONS;
    const outActions = {};
    for (const [name, spec] of Object.entries(actions)) {
      const dirs = [];
      if (id === 'mission_npc') {
        // One direction reused for all 8, as the contract allows.
        const frames = [];
        for (let f = 0; f < spec.frames; f++) {
          images.push({ canvas: drawFrame(look, name, 4, f), ax: look.w / 2, ay: look.h - 2 });
          frames.push(images.length - 1);
        }
        for (let d = 0; d < 8; d++) dirs.push(frames);
      } else {
        for (let d = 0; d < 8; d++) {
          const frames = [];
          for (let f = 0; f < spec.frames; f++) {
            images.push({ canvas: drawFrame(look, name, d, f), ax: look.w / 2, ay: look.h - 2 });
            frames.push(images.length - 1);
          }
          dirs.push(frames);
        }
      }
      outActions[name] = { frameMs: spec.frameMs, loop: spec.loop, dirs };
    }
    sprites[id] = { actions: outActions };
  }

  // Shelf packing, 2 px padding.
  const pad = 2, width = 1024;
  let x = pad, y = pad, shelf = 0;
  const frames = images.map(({ canvas, ax, ay }) => {
    if (x + canvas.w + pad > width) {
      x = pad;
      y += shelf + pad;
      shelf = 0;
    }
    const f = { x, y, w: canvas.w, h: canvas.h, ax, ay, flipX: false };
    x += canvas.w + pad;
    shelf = Math.max(shelf, canvas.h);
    return f;
  });
  let height = 1;
  while (height < y + shelf + pad) height *= 2;
  const atlas = Buffer.alloc(width * height * 4);
  images.forEach(({ canvas }, i) => {
    const f = frames[i];
    for (let row = 0; row < canvas.h; row++) {
      canvas.px.copy(atlas, ((f.y + row) * width + f.x) * 4, row * canvas.w * 4, (row + 1) * canvas.w * 4);
    }
  });
  return {
    png: encodePngRgba(width, height, atlas),
    json: { image: 'sprites.png', pixelsPerMeter: 32, frames, sprites },
  };
}

// ---------------------------------------------------------------------------
// glTF (GLB) writer
// ---------------------------------------------------------------------------

class Glb {
  constructor() {
    this.json = {
      asset: { version: '2.0', generator: 'ws placeholder gen' },
      scenes: [{ nodes: [] }],
      scene: 0,
      nodes: [], meshes: [], materials: [], accessors: [], bufferViews: [], buffers: [],
    };
    this.chunks = [];
    this.length = 0;
  }
  view(typed, target) {
    const bytes = Buffer.from(typed.buffer, typed.byteOffset, typed.byteLength);
    const offset = this.length;
    this.chunks.push(bytes);
    this.length += bytes.length;
    const pad = (4 - (this.length % 4)) % 4;
    if (pad) {
      this.chunks.push(Buffer.alloc(pad));
      this.length += pad;
    }
    const v = { buffer: 0, byteOffset: offset, byteLength: bytes.length };
    if (target) v.target = target;
    this.json.bufferViews.push(v);
    return this.json.bufferViews.length - 1;
  }
  accessor(typed, type, componentType, { target, normalized, minmax } = {}) {
    const comps = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4 }[type];
    const a = { bufferView: this.view(typed, target), componentType, count: typed.length / comps, type };
    if (normalized) a.normalized = true;
    if (minmax) {
      const min = Array(comps).fill(Infinity), max = Array(comps).fill(-Infinity);
      for (let i = 0; i < typed.length; i++) {
        const k = i % comps;
        min[k] = Math.min(min[k], typed[i]);
        max[k] = Math.max(max[k], typed[i]);
      }
      a.min = min;
      a.max = max;
    }
    this.json.accessors.push(a);
    return this.json.accessors.length - 1;
  }
  material(m) {
    this.json.materials.push(m);
    return this.json.materials.length - 1;
  }
  mesh({ positions, normals, colors, indices, material, name }) {
    const attributes = {
      POSITION: this.accessor(positions, 'VEC3', 5126, { target: 34962, minmax: true }),
      NORMAL: this.accessor(normals, 'VEC3', 5126, { target: 34962 }),
    };
    if (colors) attributes.COLOR_0 = this.accessor(colors, 'VEC4', 5121, { target: 34962, normalized: true });
    const prim = { attributes, indices: this.accessor(indices, 'SCALAR', 5125, { target: 34963 }), material };
    this.json.meshes.push({ name, primitives: [prim] });
    return this.json.meshes.length - 1;
  }
  node(n) {
    this.json.nodes.push(n);
    const i = this.json.nodes.length - 1;
    this.json.scenes[0].nodes.push(i);
    return i;
  }
  write(file) {
    const bin = Buffer.concat(this.chunks);
    this.json.buffers.push({ byteLength: bin.length });
    let jsonBuf = Buffer.from(JSON.stringify(this.json));
    jsonBuf = Buffer.concat([jsonBuf, Buffer.alloc((4 - (jsonBuf.length % 4)) % 4, 0x20)]);
    const header = Buffer.alloc(12);
    header.writeUInt32LE(0x46546c67, 0);
    header.writeUInt32LE(2, 4);
    header.writeUInt32LE(12 + 8 + jsonBuf.length + 8 + bin.length, 8);
    const ch = (len, type) => {
      const b = Buffer.alloc(8);
      b.writeUInt32LE(len, 0);
      b.writeUInt32LE(type, 4);
      return b;
    };
    fs.writeFileSync(file, Buffer.concat([header, ch(jsonBuf.length, 0x4e4f534a), jsonBuf, ch(bin.length, 0x004e4942), bin]));
  }
}

// ---------------------------------------------------------------------------
// Location data
// ---------------------------------------------------------------------------

function loadLocation(id) {
  const dir = path.join(dist, 'client/locations', id);
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
  const hm = decodePngGray16(fs.readFileSync(path.join(dir, 'heightmap.png')));
  const walkBuf = fs.readFileSync(path.join(dist, 'server/locations', id, 'walkability.bin'));
  const w = walkBuf.readUInt32LE(0), h = walkBuf.readUInt32LE(4);
  const walk = walkBuf.subarray(8);
  if (w !== hm.w || h !== hm.h) throw new Error(`${id}: walkability and heightmap differ`);
  return { id, dir, manifest, w, h, samples: hm.samples, walk };
}

/** Height in metres of cell (x, y), honouring an optional heightScale. */
function heightFn(loc, scale) {
  const { min, max } = loc.manifest.heightBounds;
  return (x, y) => {
    x = Math.max(0, Math.min(loc.w - 1, x));
    y = Math.max(0, Math.min(loc.h - 1, y));
    return (min + (loc.samples[y * loc.w + x] / 65535) * (max - min)) * scale;
  };
}

const walkable = (loc, x, y) => x >= 0 && y >= 0 && x < loc.w && y < loc.h && loc.walk[y * loc.w + x] === 0;

function terrainGlb(loc, scale, file) {
  const { w, h } = loc;
  const cs = loc.manifest.dimensions.cellSize;
  const hAt = heightFn(loc, scale);
  // One vertex per cell corner; corner height = mean of the 4 cells around it.
  const vw = w + 1, vh = h + 1;
  const positions = new Float32Array(vw * vh * 3);
  const colors = new Uint8Array(vw * vh * 4);
  const heights = new Float32Array(vw * vh);
  for (let y = 0; y < vh; y++) {
    for (let x = 0; x < vw; x++) {
      const hv = (hAt(x - 1, y - 1) + hAt(x, y - 1) + hAt(x - 1, y) + hAt(x, y)) / 4;
      heights[y * vw + x] = hv;
      const i = y * vw + x;
      positions.set([x * cs, hv, -y * cs], i * 3);
      let walkN = 0;
      for (const [dx, dy] of [[-1, -1], [0, -1], [-1, 0], [0, 0]]) if (walkable(loc, x + dx, y + dy)) walkN++;
      const checker = ((x >> 2) + (y >> 2)) % 2 ? 1 : 0.93;
      const base = walkN >= 2 ? [120, 170, 90] : [125, 120, 115];
      colors.set([...shade(base, checker), 255], i * 4);
    }
  }
  const normals = new Float32Array(vw * vh * 3);
  for (let y = 0; y < vh; y++) {
    for (let x = 0; x < vw; x++) {
      const hl = heights[y * vw + Math.max(0, x - 1)], hr = heights[y * vw + Math.min(vw - 1, x + 1)];
      const hd = heights[Math.max(0, y - 1) * vw + x], hu = heights[Math.min(vh - 1, y + 1) * vw + x];
      const nx = hl - hr, nz = hu - hd, ny = 4 * cs;
      // z points south (−y in cells), hence the sign on nz.
      const len = Math.hypot(nx, ny, nz);
      normals.set([nx / len, ny / len, nz / len], (y * vw + x) * 3);
    }
  }
  const indices = new Uint32Array(w * h * 6);
  let k = 0;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const a = y * vw + x, b = a + 1, c = a + vw, d = c + 1;
      // Counter-clockwise seen from above (+Y), with −Z = north.
      indices.set([a, b, d, a, d, c], k);
      k += 6;
    }
  }
  const glb = new Glb();
  const material = glb.material({
    name: 'terrain',
    pbrMetallicRoughness: { baseColorFactor: [1, 1, 1, 1], metallicFactor: 0, roughnessFactor: 1 },
  });
  glb.node({ name: 'terrain', mesh: glb.mesh({ positions, normals, colors, indices, material, name: 'terrain' }) });
  glb.write(file);
}

/** Unit primitives for the placeholder props, base at y = 0. */
function boxGeometry(sx, sy, sz, color) {
  const positions = [], normals = [], colors = [], indices = [];
  const faces = [
    [[1, 0, 0], [0, 0, -1], [0, 1, 0]], [[-1, 0, 0], [0, 0, 1], [0, 1, 0]],
    [[0, 1, 0], [1, 0, 0], [0, 0, -1]], [[0, -1, 0], [1, 0, 0], [0, 0, 1]],
    [[0, 0, 1], [1, 0, 0], [0, 1, 0]], [[0, 0, -1], [-1, 0, 0], [0, 1, 0]],
  ];
  for (const [n, u, v] of faces) {
    const base = positions.length / 3;
    for (const [a, b] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
      positions.push(((n[0] + a * u[0] + b * v[0]) * sx) / 2, ((n[1] + a * u[1] + b * v[1] + 1) * sy) / 2, ((n[2] + a * u[2] + b * v[2]) * sz) / 2);
      normals.push(...n);
      colors.push(...shade(color, n[1] > 0 ? 1 : 0.8), 255);
    }
    indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
  }
  return { positions, normals, colors, indices };
}

function coneGeometry(r, hgt, y0, color, seg = 8) {
  const positions = [], normals = [], colors = [], indices = [];
  for (let i = 0; i < seg; i++) {
    const a0 = (i / seg) * Math.PI * 2, a1 = ((i + 1) / seg) * Math.PI * 2;
    const base = positions.length / 3;
    const am = (a0 + a1) / 2;
    const n = [Math.cos(am), r / hgt, Math.sin(am)];
    const l = Math.hypot(...n);
    positions.push(Math.cos(a0) * r, y0, Math.sin(a0) * r, 0, y0 + hgt, 0, Math.cos(a1) * r, y0, Math.sin(a1) * r);
    for (let j = 0; j < 3; j++) {
      normals.push(n[0] / l, n[1] / l, n[2] / l);
      colors.push(...shade(color, j === 1 ? 1.15 : 0.9), 255);
    }
    indices.push(base, base + 1, base + 2);
  }
  return { positions, normals, colors, indices };
}

function merge(...parts) {
  const out = { positions: [], normals: [], colors: [], indices: [] };
  for (const p of parts) {
    const base = out.positions.length / 3;
    out.positions.push(...p.positions);
    out.normals.push(...p.normals);
    out.colors.push(...p.colors);
    out.indices.push(...p.indices.map((i) => i + base));
  }
  return {
    positions: new Float32Array(out.positions),
    normals: new Float32Array(out.normals),
    colors: new Uint8Array(out.colors),
    indices: new Uint32Array(out.indices),
  };
}

function propsGlb(loc, scale, file, keepClear = []) {
  const cs = loc.manifest.dimensions.cellSize;
  const hAt = heightFn(loc, scale);
  // Deterministic hash so reruns place the same props.
  const rnd = (x, y, s) => {
    let n = (x * 374761393 + y * 668265263 + s * 2147483647) | 0;
    n = Math.imul(n ^ (n >>> 13), 1274126177);
    return ((n ^ (n >>> 16)) >>> 0) / 4294967296;
  };
  const kinds = { tree: [], crate: [], pillar: [] };
  for (let y = 0; y < loc.h; y++) {
    for (let x = 0; x < loc.w; x++) {
      if (walkable(loc, x, y)) continue;
      // Blocked cells on the edge of walkable space: something visible blocks them.
      let edge = false;
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) if (walkable(loc, x + dx, y + dy)) edge = true;
      if (!edge || rnd(x, y, 1) > 0.12) continue;
      // Keep sight lines around the spawn and NPCs.
      if (keepClear.some(([cx, cy]) => Math.abs(cx - x) <= 6 && Math.abs(cy - y) <= 6)) continue;
      const r = rnd(x, y, 2);
      const kind = loc.manifest.type === 'indoor' ? (r < 0.6 ? 'crate' : 'pillar') : r < 0.65 ? 'tree' : r < 0.85 ? 'crate' : 'pillar';
      kinds[kind].push([(x + 0.5) * cs, hAt(x, y), -(y + 0.5) * cs, rnd(x, y, 3) * Math.PI * 2, 0.8 + rnd(x, y, 4) * 0.5]);
    }
  }
  const glb = new Glb();
  glb.json.extensionsUsed = ['EXT_mesh_gpu_instancing'];
  const material = glb.material({ name: 'props', pbrMetallicRoughness: { baseColorFactor: [1, 1, 1, 1], metallicFactor: 0, roughnessFactor: 1 } });
  const geos = {
    tree: merge(boxGeometry(0.25, 0.8, 0.25, [110, 80, 50]), coneGeometry(0.9, 2.2, 0.6, [50, 120, 60])),
    crate: merge(boxGeometry(0.8, 0.8, 0.8, [170, 130, 80])),
    pillar: merge(boxGeometry(0.5, 2.5, 0.5, [200, 200, 205])),
  };
  for (const [kind, list] of Object.entries(kinds)) {
    if (!list.length) continue;
    const mesh = glb.mesh({ ...geos[kind], material, name: kind });
    const t = new Float32Array(list.length * 3), r = new Float32Array(list.length * 4), s = new Float32Array(list.length * 3);
    list.forEach(([x, y, z, yaw, sc], i) => {
      t.set([x, y, z], i * 3);
      r.set([0, Math.sin(yaw / 2), 0, Math.cos(yaw / 2)], i * 4);
      s.set([sc, sc, sc], i * 3);
    });
    glb.node({
      name: kind,
      mesh,
      extensions: {
        EXT_mesh_gpu_instancing: {
          attributes: {
            TRANSLATION: glb.accessor(t, 'VEC3', 5126),
            ROTATION: glb.accessor(r, 'VEC4', 5126),
            SCALE: glb.accessor(s, 'VEC3', 5126),
          },
        },
      },
    });
  }
  glb.write(file);
  return Object.fromEntries(Object.entries(kinds).map(([k, v]) => [k, v.length]));
}

/** Walkable cell nearest the map centre in the largest region (same rule as the server spawn). */
function spawnCell(loc) {
  const comp = new Int32Array(loc.w * loc.h).fill(-1);
  const sizes = [];
  for (let i = 0; i < loc.w * loc.h; i++) {
    if (loc.walk[i] !== 0 || comp[i] >= 0) continue;
    const c = sizes.length;
    let n = 0;
    const stack = [i];
    comp[i] = c;
    while (stack.length) {
      const j = stack.pop();
      n++;
      const x = j % loc.w, y = (j / loc.w) | 0;
      for (const [nx, ny] of [[x + 1, y], [x - 1, y], [x, y + 1], [x, y - 1]]) {
        if (walkable(loc, nx, ny) && comp[ny * loc.w + nx] < 0) {
          comp[ny * loc.w + nx] = c;
          stack.push(ny * loc.w + nx);
        }
      }
    }
    sizes.push(n);
  }
  const main = sizes.indexOf(Math.max(...sizes));
  let best = null;
  for (let y = 0; y < loc.h; y++) {
    for (let x = 0; x < loc.w; x++) {
      if (comp[y * loc.w + x] !== main) continue;
      const d = (x + 0.5 - loc.w / 2) ** 2 + (y + 0.5 - loc.h / 2) ** 2;
      if (!best || d < best.d) best = { d, x, y };
    }
  }
  return { ...best, comp, main };
}

// ---------------------------------------------------------------------------

function writeIfMissing(file, write) {
  if (fs.existsSync(file) && !force) {
    console.log(`  keep   ${path.relative(dist, file)}`);
    return false;
  }
  write(file);
  console.log(`  wrote  ${path.relative(dist, file)}`);
  return true;
}

const ids = fs.readdirSync(path.join(dist, 'client/locations'));
for (const id of ids) {
  console.log(`[${id}]`);
  const loc = loadLocation(id);
  const m = loc.manifest;

  // The current bake writes heights in the source map's units (5 per cell,
  // negative = up). Until the real bake emits metres, record the factor.
  let changed = false;
  if (m.heightScale === undefined) {
    m.heightScale = -m.dimensions.cellSize / 5;
    changed = true;
  }
  const scale = m.heightScale ?? 1;

  if (m.type === 'hub' && !m.npcs) {
    // Stand the mission board a few cells north of the spawn.
    const s = spawnCell(loc);
    let pick = { x: s.x, y: s.y };
    for (let dy = 6; dy >= 2; dy--) {
      if (s.comp[(s.y + dy) * loc.w + s.x] === s.main) {
        pick = { x: s.x, y: s.y + dy };
        break;
      }
    }
    m.npcs = [{
      id: 'mission_board', sprite: 'mission_npc', name: 'Mission Board Keeper', kind: 'mission_board',
      x: pick.x, y: pick.y, facing: 4, interactRadius: 3,
    }];
    changed = true;
  }
  if (changed) {
    fs.writeFileSync(path.join(loc.dir, 'manifest.json'), JSON.stringify(m, null, 2));
    console.log(`  patched manifest.json (heightScale=${m.heightScale}, npcs=${(m.npcs ?? []).length})`);
  }

  const spriteIds = SPRITES_BY_KIND[m.type] ?? PLAYERS;
  const atlas = buildAtlas(spriteIds);
  writeIfMissing(path.join(loc.dir, 'sprites.png'), (f) => fs.writeFileSync(f, atlas.png));
  writeIfMissing(path.join(loc.dir, 'sprites.json'), (f) => fs.writeFileSync(f, JSON.stringify(atlas.json)));
  writeIfMissing(path.join(loc.dir, 'terrain.glb'), (f) => terrainGlb(loc, scale, f));
  const sp = spawnCell(loc);
  const clear = [[sp.x, sp.y], ...(m.npcs ?? []).map((n) => [n.x, n.y]), ...(m.portals ?? []).map((p) => p.trigger)];
  writeIfMissing(path.join(loc.dir, 'props.glb'), (f) => console.log('  props', propsGlb(loc, scale, f, clear)));
}
