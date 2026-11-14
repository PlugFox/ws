//! Baked location data: walkability grid, 16-bit heightmap and manifest.
//!
//! All three files are produced by the asset bake (`example/assets`) and share
//! one coordinate system: cell `(x, y)` is stored at index `y * width + x`,
//! `x` grows east and `y` grows north (row 0 is the southern edge). Portal
//! triggers and arrival points in the manifest use the same cell coordinates.
//!
//! Entity positions are continuous `f32` cell coordinates: cell `(3, 4)` covers
//! `[3, 4) × [4, 5)` and its centre is `(3.5, 4.5)`.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;
use thiserror::Error;

/// Largest map side the snapshot fixed-point encoding can represent
/// (`u16` with 6 fractional bits → 1023.98 cells).
pub const MAX_MAP_SIDE: u32 = 1023;

#[derive(Debug, Error)]
pub enum MapError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}: invalid manifest: {1}")]
    Manifest(String, serde_json::Error),
    #[error("walkability grid: {0}")]
    Walkability(String),
    #[error("heightmap: {0}")]
    Heightmap(String),
    #[error("map {map}: {message}")]
    Inconsistent { map: String, message: String },
}

// ---------------------------------------------------------------------------
// Manifest (client/locations/<id>/manifest.json)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationKind {
    Hub,
    Indoor,
    Field,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
    #[serde(rename = "cellSize")]
    pub cell_size: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct HeightBounds {
    pub min: f32,
    pub max: f32,
}

/// Camera rules for a location. The server does not render anything; it sends
/// this to the client on every map change and the client must enforce it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CameraConstraint {
    /// Player may rotate freely and zoom within `[min_zoom, max_zoom]`;
    /// pitch is fixed.
    Free {
        pitch: f32,
        min_zoom: f32,
        max_zoom: f32,
        default_zoom: f32,
    },
    /// Yaw, pitch and zoom are locked (interiors: walls would occlude a
    /// rotated camera).
    Fixed { yaw: f32, pitch: f32, zoom: f32 },
}

impl CameraConstraint {
    fn validate(&self) -> Result<(), String> {
        let finite = |v: f32, name: &str| {
            if v.is_finite() { Ok(()) } else { Err(format!("camera.{name} is not finite")) }
        };
        match *self {
            CameraConstraint::Free { pitch, min_zoom, max_zoom, default_zoom } => {
                finite(pitch, "pitch")?;
                finite(min_zoom, "min_zoom")?;
                finite(max_zoom, "max_zoom")?;
                finite(default_zoom, "default_zoom")?;
                if !(min_zoom > 0.0 && min_zoom <= default_zoom && default_zoom <= max_zoom) {
                    return Err(format!(
                        "camera zoom must satisfy 0 < min ({min_zoom}) <= default ({default_zoom}) <= max ({max_zoom})"
                    ));
                }
                Ok(())
            }
            CameraConstraint::Fixed { yaw, pitch, zoom } => {
                finite(yaw, "yaw")?;
                finite(pitch, "pitch")?;
                finite(zoom, "zoom")?;
                if zoom <= 0.0 {
                    return Err(format!("camera.zoom must be positive, got {zoom}"));
                }
                Ok(())
            }
        }
    }
}

/// A trigger circle that moves an entity to another map.
#[derive(Clone, Debug, Deserialize)]
pub struct PortalDef {
    pub name: String,
    /// `[x, y, radius]` in cells.
    pub trigger: [u32; 3],
    pub target_map: String,
    /// `[x, y]` arrival cell on the target map.
    pub target_pos: [u32; 2],
}

impl PortalDef {
    /// Trigger centre in continuous cell coordinates.
    pub fn center(&self) -> (f32, f32) {
        (self.trigger[0] as f32 + 0.5, self.trigger[1] as f32 + 0.5)
    }

    pub fn radius(&self) -> f32 {
        self.trigger[2] as f32
    }

    /// Inclusive circle test; zero-sqrt.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (cx, cy) = self.center();
        let (dx, dy) = (x - cx, y - cy);
        let r = self.radius();
        dx * dx + dy * dy <= r * r
    }

    /// Arrival point on the target map (cell centre).
    pub fn arrival(&self) -> (f32, f32) {
        (self.target_pos[0] as f32 + 0.5, self.target_pos[1] as f32 + 0.5)
    }
}

/// A static NPC placed by the manifest. The server uses it for interaction
/// range checks; the client renders it from the same manifest.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NpcDef {
    pub id: String,
    /// Sprite id in the location's `sprites.json`.
    pub sprite: String,
    pub name: String,
    /// What talking to the NPC opens: `"kafra"` (missions, skill reset,
    /// restyle), `"mission_board"`, `"stylist"`, `"skill_master"`,
    /// `"changelog"`, `"talk"`.
    #[serde(default = "NpcDef::default_kind")]
    pub kind: String,
    /// What a `"shop"` NPC sells (`combat::items::SHOPS` id).
    #[serde(default)]
    pub shop: String,
    /// Cell coordinates; the NPC stands on the cell centre.
    pub x: u32,
    pub y: u32,
    /// 0 = north, clockwise in 45° steps (same as player facing).
    #[serde(default = "NpcDef::default_facing")]
    pub facing: u8,
    /// Interaction radius in cells.
    #[serde(default = "NpcDef::default_radius")]
    pub interact_radius: f32,
}

impl NpcDef {
    fn default_kind() -> String {
        "talk".into()
    }

    fn default_facing() -> u8 {
        4
    }

    fn default_radius() -> f32 {
        3.0
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x as f32 + 0.5, self.y as f32 + 0.5)
    }

    /// Inclusive circle test; zero-sqrt.
    pub fn in_reach(&self, x: f32, y: f32) -> bool {
        let (cx, cy) = self.center();
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy <= self.interact_radius * self.interact_radius
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LocationManifest {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: LocationKind,
    pub dimensions: Dimensions,
    #[serde(rename = "heightBounds")]
    pub height_bounds: HeightBounds,
    pub camera: CameraConstraint,
    #[serde(default)]
    pub portals: Vec<PortalDef>,
    #[serde(default)]
    pub bgm: Option<String>,
    #[serde(default)]
    pub npcs: Vec<NpcDef>,
    /// Hub interest radius in cells: snapshots carry only entities within
    /// it (plus a margin) of the recipient. 0 = the whole map.
    #[serde(default, rename = "lightRadius")]
    pub light_radius: f32,
    /// Authored spots and pets for the town residents (hubs).
    #[serde(default)]
    pub residents: ResidentsDef,
    /// Authored arrival cell for players entering without a saved position;
    /// absent (or blocked) = the walkable cell nearest the map centre.
    #[serde(default)]
    pub spawn: Option<[u32; 2]>,
    /// Where players come back to from a mission (scattered a few cells
    /// around it); absent = `spawn`.
    #[serde(default, rename = "returnPoint")]
    pub return_point: Option<[u32; 2]>,
}

/// `residents` of a manifest: where townsfolk go and which pets roam.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ResidentsDef {
    #[serde(default)]
    pub spots: Vec<SpotDef>,
    #[serde(default)]
    pub pets: Vec<PetDef>,
}

/// A place a resident walks to (cell coordinates).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SpotDef {
    pub x: f32,
    pub y: f32,
    /// `"sit"`, `"stand"` or `"wander"` (anything else stands).
    #[serde(default = "SpotDef::default_kind")]
    pub kind: String,
    /// 0 = north, clockwise in 45° steps.
    #[serde(default = "NpcDef::default_facing")]
    pub facing: u8,
    /// What is there (`"bench"`, `"stall"`, …); informational.
    #[serde(default)]
    pub tag: Option<String>,
}

impl SpotDef {
    fn default_kind() -> String {
        "stand".into()
    }
}

/// A town animal: drawn from the location atlas sprite `sprite`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PetDef {
    pub sprite: String,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Walkability grid (server/locations/<id>/walkability.bin)
// ---------------------------------------------------------------------------

/// Binary layout: `u32 LE width`, `u32 LE height`, then `width * height`
/// bytes. **A zero byte is walkable, any non-zero byte is blocked** — the
/// bake writes the source cell type, where type 0 is the only walkable one.
#[derive(Clone, Debug)]
pub struct WalkGrid {
    width: u32,
    height: u32,
    blocked: Vec<u8>,
}

impl WalkGrid {
    pub fn parse(bytes: &[u8]) -> Result<Self, MapError> {
        if bytes.len() < 8 {
            return Err(MapError::Walkability(format!(
                "file is {} bytes, header needs 8",
                bytes.len()
            )));
        }
        let width = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if width == 0 || height == 0 || width > MAX_MAP_SIDE || height > MAX_MAP_SIDE {
            return Err(MapError::Walkability(format!(
                "dimensions {width}x{height} outside 1..={MAX_MAP_SIDE}"
            )));
        }
        let expected = (width as usize) * (height as usize);
        let body = &bytes[8..];
        if body.len() != expected {
            return Err(MapError::Walkability(format!(
                "{width}x{height} needs {expected} cell bytes, file has {}",
                body.len()
            )));
        }
        Ok(Self { width, height, blocked: body.to_vec() })
    }

    /// Builds a grid from rows written north-to-south (top row first), which is
    /// how a human draws a map. `#` is blocked, anything else walkable.
    pub fn from_ascii(rows: &[&str]) -> Self {
        let height = rows.len() as u32;
        let width = rows.first().map_or(0, |r| r.len()) as u32;
        let mut blocked = vec![0u8; (width * height) as usize];
        for (row_idx, row) in rows.iter().enumerate() {
            assert_eq!(row.len() as u32, width, "ragged ascii map");
            let y = height - 1 - row_idx as u32;
            for (x, ch) in row.bytes().enumerate() {
                blocked[(y * width) as usize + x] = u8::from(ch == b'#');
            }
        }
        Self { width, height, blocked }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Walkability of a cell; out-of-bounds cells are blocked.
    #[inline]
    pub fn is_walkable_cell(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        self.blocked[y as usize * self.width as usize + x as usize] == 0
    }

    /// Walkability of the cell containing a continuous position.
    #[inline]
    pub fn is_walkable(&self, x: f32, y: f32) -> bool {
        // `floor` so that -0.5 maps to cell -1 (blocked), not 0.
        self.is_walkable_cell(x.floor() as i32, y.floor() as i32)
    }

    pub fn walkable_count(&self) -> usize {
        self.blocked.iter().filter(|&&b| b == 0).count()
    }

    /// 4-connected component labels of walkable cells (`u32::MAX` for blocked).
    /// Returns the labels and the size of each component.
    pub fn components(&self) -> (Vec<u32>, Vec<usize>) {
        let (w, h) = (self.width as i32, self.height as i32);
        let mut labels = vec![u32::MAX; self.blocked.len()];
        let mut sizes = Vec::new();
        let mut queue = VecDeque::new();
        for start in 0..self.blocked.len() {
            if self.blocked[start] != 0 || labels[start] != u32::MAX {
                continue;
            }
            let label = sizes.len() as u32;
            let mut size = 0usize;
            labels[start] = label;
            queue.push_back(start);
            while let Some(idx) = queue.pop_front() {
                size += 1;
                let (x, y) = ((idx as i32) % w, (idx as i32) / w);
                for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let n = (ny * w + nx) as usize;
                    if self.blocked[n] == 0 && labels[n] == u32::MAX {
                        labels[n] = label;
                        queue.push_back(n);
                    }
                }
            }
            sizes.push(size);
        }
        (labels, sizes)
    }
}

// ---------------------------------------------------------------------------
// Heightmap (client/locations/<id>/heightmap.png)
// ---------------------------------------------------------------------------

/// 16-bit grayscale heights, normalised over `heightBounds` from the manifest.
/// PNG row 0 is cell row `y = 0` (the bake writes rows in grid order), so a
/// renderer must not flip it implicitly.
#[derive(Clone, Debug)]
pub struct Heightmap {
    width: u32,
    height: u32,
    samples: Vec<u16>,
    min: f32,
    max: f32,
}

impl Heightmap {
    pub fn decode_png(bytes: &[u8], bounds: HeightBounds) -> Result<Self, MapError> {
        let decoder = png::Decoder::new(bytes);
        let mut reader = decoder
            .read_info()
            .map_err(|e| MapError::Heightmap(e.to_string()))?;
        let info = reader.info();
        if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Sixteen {
            return Err(MapError::Heightmap(format!(
                "expected 16-bit grayscale, got {:?} {:?}",
                info.color_type, info.bit_depth
            )));
        }
        let (width, height) = (info.width, info.height);
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let frame = reader
            .next_frame(&mut buf)
            .map_err(|e| MapError::Heightmap(e.to_string()))?;
        let data = &buf[..frame.buffer_size()];
        let row_bytes = frame.line_size;
        let mut samples = Vec::with_capacity((width * height) as usize);
        for row in data.chunks_exact(row_bytes).take(height as usize) {
            for px in row[..(width as usize) * 2].as_chunks::<2>().0 {
                samples.push(u16::from_be_bytes(*px));
            }
        }
        if !(bounds.min.is_finite() && bounds.max.is_finite() && bounds.min <= bounds.max) {
            return Err(MapError::Heightmap(format!(
                "invalid height bounds [{}, {}]",
                bounds.min, bounds.max
            )));
        }
        Ok(Self { width, height, samples, min: bounds.min, max: bounds.max })
    }

    pub fn flat(width: u32, height: u32) -> Self {
        Self { width, height, samples: vec![0; (width * height) as usize], min: 0.0, max: 0.0 }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Height (source units) of the cell containing a position; clamps to the
    /// map edge.
    pub fn height_at(&self, x: f32, y: f32) -> f32 {
        let cx = (x.floor() as i64).clamp(0, self.width as i64 - 1) as usize;
        let cy = (y.floor() as i64).clamp(0, self.height as i64 - 1) as usize;
        let s = self.samples[cy * self.width as usize + cx];
        self.min + (s as f32 / 65535.0) * (self.max - self.min)
    }
}

// ---------------------------------------------------------------------------
// MapData: the three files, cross-validated
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MapData {
    pub manifest: LocationManifest,
    pub walk: WalkGrid,
    pub heights: Heightmap,
    /// Default spawn: walkable cell closest to the map centre inside the
    /// largest connected walkable region.
    pub spawn: (f32, f32),
    /// What grows on the field (`server/locations/<id>/plants.json`):
    /// planted by every stage on it (`horde/plants.rs`).
    pub plants: Option<crate::world::horde::plants::PlantRules>,
    /// Baked hostile ground (`server/locations/<id>/hazards.json`): stages
    /// on it run them with the mission's own (`world::environment`).
    pub hazards: Vec<crate::world::environment::Hazard>,
    component_of: Vec<u32>,
    main_component: u32,
}

impl MapData {
    pub fn new(manifest: LocationManifest, walk: WalkGrid, heights: Heightmap) -> Result<Self, MapError> {
        let err = |message: String| MapError::Inconsistent { map: manifest.id.clone(), message };
        let dims = &manifest.dimensions;
        if (walk.width(), walk.height()) != (dims.width, dims.height) {
            return Err(err(format!(
                "walkability is {}x{}, manifest says {}x{}",
                walk.width(),
                walk.height(),
                dims.width,
                dims.height
            )));
        }
        if (heights.width(), heights.height()) != (dims.width, dims.height) {
            return Err(err(format!(
                "heightmap is {}x{}, manifest says {}x{}",
                heights.width(),
                heights.height(),
                dims.width,
                dims.height
            )));
        }
        manifest.camera.validate().map_err(err)?;
        for p in &manifest.portals {
            let [x, y, r] = p.trigger;
            if r == 0 {
                return Err(err(format!("portal {} has zero radius", p.name)));
            }
            if !walk.is_walkable_cell(x as i32, y as i32) {
                return Err(err(format!("portal {} trigger ({x},{y}) is not walkable", p.name)));
            }
        }

        for n in &manifest.npcs {
            if n.x >= dims.width || n.y >= dims.height {
                return Err(err(format!("npc {} at ({},{}) is outside the map", n.id, n.x, n.y)));
            }
            if n.facing > 7 || !n.interact_radius.is_finite() || n.interact_radius <= 0.0 {
                return Err(err(format!("npc {} has a bad facing or interaction radius", n.id)));
            }
        }

        let (component_of, sizes) = walk.components();
        let main_component = sizes
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| **s)
            .map(|(i, _)| i as u32)
            .ok_or_else(|| err("map has no walkable cells".into()))?;

        let (cx, cy) = (walk.width() as f32 / 2.0, walk.height() as f32 / 2.0);
        let mut best = None::<(f32, u32, u32)>;
        for y in 0..walk.height() {
            for x in 0..walk.width() {
                if component_of[(y * walk.width() + x) as usize] != main_component {
                    continue;
                }
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = dx * dx + dy * dy;
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, x, y));
                }
            }
        }
        let (_, mut sx, mut sy) = best.expect("main component is non-empty");
        if let Some([ax, ay]) = manifest.spawn {
            if ax < walk.width() && ay < walk.height() && component_of[(ay * walk.width() + ax) as usize] == main_component {
                (sx, sy) = (ax, ay);
            }
        }

        Ok(Self {
            manifest,
            walk,
            heights,
            spawn: (sx as f32 + 0.5, sy as f32 + 0.5),
            plants: None,
            hazards: Vec::new(),
            component_of,
            main_component,
        })
    }

    /// Loads `<dist>/client/locations/<id>/{manifest.json,heightmap.png}`,
    /// `<dist>/server/locations/<id>/walkability.bin` and, where the field
    /// grows plants, `plants.json` (+ its ground mask) beside it.
    pub fn load(dist_root: &Path, id: &str) -> Result<Self, MapError> {
        let client = dist_root.join("client/locations").join(id);
        let server = dist_root.join("server/locations").join(id);
        let read = |p: std::path::PathBuf| {
            std::fs::read(&p).map_err(|source| MapError::Io { path: p.display().to_string(), source })
        };
        let manifest_path = client.join("manifest.json");
        let manifest: LocationManifest = serde_json::from_slice(&read(manifest_path.clone())?)
            .map_err(|e| MapError::Manifest(manifest_path.display().to_string(), e))?;
        if manifest.id != id {
            return Err(MapError::Inconsistent {
                map: id.to_string(),
                message: format!("manifest id is {:?}", manifest.id),
            });
        }
        let walk = WalkGrid::parse(&read(server.join("walkability.bin"))?)?;
        let heights = Heightmap::decode_png(&read(client.join("heightmap.png"))?, manifest.height_bounds)?;
        let plants = crate::world::horde::plants::PlantRules::load(&server)?;
        if let Some(g) = plants.as_ref().and_then(|p| p.ground.as_ref())
            && (g.width(), g.height()) != (walk.width(), walk.height())
        {
            return Err(MapError::Inconsistent { map: id.to_string(), message: "plants.bin does not match the walk grid".into() });
        }
        let mut data = Self::new(manifest, walk, heights)?;
        data.plants = plants;
        data.hazards = crate::world::environment::load_hazards(&server)?;
        Ok(data)
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// True when the cell is walkable and belongs to the largest connected
    /// region (i.e. a player standing there can reach the rest of the map).
    pub fn is_reachable_cell(&self, x: u32, y: u32) -> bool {
        x < self.walk.width()
            && y < self.walk.height()
            && self.component_of[(y * self.walk.width() + x) as usize] == self.main_component
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, w: u32, h: u32, portals: Vec<PortalDef>) -> LocationManifest {
        LocationManifest {
            id: id.into(),
            name: id.into(),
            kind: LocationKind::Hub,
            dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
            height_bounds: HeightBounds { min: 0.0, max: 0.0 },
            camera: CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
            portals,
            bgm: None,
            npcs: Vec::new(),
            light_radius: 0.0,
            residents: Default::default(),
            spawn: None,
            return_point: None,
        }
    }

    #[test]
    fn walk_grid_parses_header_and_rejects_bad_sizes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 1, 0, 1, 0, 1]);
        let g = WalkGrid::parse(&bytes).unwrap();
        assert_eq!((g.width(), g.height()), (3, 2));
        assert!(g.is_walkable_cell(0, 0));
        assert!(!g.is_walkable_cell(1, 0));
        assert!(g.is_walkable_cell(1, 1));
        assert!(!g.is_walkable_cell(-1, 0), "out of bounds is blocked");
        assert!(!g.is_walkable(-0.25, 0.5), "negative fraction floors to -1");

        assert!(WalkGrid::parse(&bytes[..7]).is_err());
        assert!(WalkGrid::parse(&bytes[..13]).is_err(), "truncated body");
        let mut zero = bytes.clone();
        zero[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(WalkGrid::parse(&zero).is_err());
        let mut huge = bytes.clone();
        huge[0..4].copy_from_slice(&(MAX_MAP_SIDE + 1).to_le_bytes());
        assert!(WalkGrid::parse(&huge).is_err());
    }

    #[test]
    fn ascii_rows_are_north_first() {
        let g = WalkGrid::from_ascii(&["#..", "..."]);
        assert!(!g.is_walkable_cell(0, 1), "top-left char is (0, height-1)");
        assert!(g.is_walkable_cell(0, 0));
    }

    #[test]
    fn heightmap_round_trips_through_png() {
        let (w, h) = (3u32, 2u32);
        let samples: [u16; 6] = [0, 65535, 32768, 1, 2, 3];
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, w, h);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Sixteen);
            let mut wr = enc.write_header().unwrap();
            let data: Vec<u8> = samples.iter().flat_map(|s| s.to_be_bytes()).collect();
            wr.write_image_data(&data).unwrap();
        }
        let hm = Heightmap::decode_png(&png_bytes, HeightBounds { min: -10.0, max: 10.0 }).unwrap();
        assert_eq!(hm.height_at(0.5, 0.5), -10.0);
        assert_eq!(hm.height_at(1.5, 0.5), 10.0);
        assert!((hm.height_at(2.5, 0.5) - 0.0).abs() < 0.001);
        // Clamps outside the map instead of panicking.
        assert_eq!(hm.height_at(-5.0, -5.0), -10.0);
    }

    #[test]
    fn map_data_rejects_mismatches_and_picks_reachable_spawn() {
        // A big room on the left and an isolated pocket in the centre: the
        // spawn must avoid the pocket even though it is closer to the centre.
        let walk = WalkGrid::from_ascii(&[
            "#########",
            "#...#####",
            "#...#.###",
            "#...#####",
            "#########",
        ]);
        let m = MapData::new(manifest("t", 9, 5, vec![]), walk.clone(), Heightmap::flat(9, 5)).unwrap();
        let (sx, sy) = m.spawn;
        assert!(sx < 4.0, "spawn {sx},{sy} must be in the big room");
        assert!(!m.is_reachable_cell(5, 2), "pocket is not reachable");
        assert!(m.is_reachable_cell(1, 1));

        assert!(MapData::new(manifest("t", 8, 5, vec![]), walk.clone(), Heightmap::flat(9, 5)).is_err());
        assert!(MapData::new(manifest("t", 9, 5, vec![]), walk.clone(), Heightmap::flat(9, 4)).is_err());

        let bad_portal = PortalDef {
            name: "into_wall".into(),
            trigger: [0, 0, 1],
            target_map: "x".into(),
            target_pos: [1, 1],
        };
        assert!(MapData::new(manifest("t", 9, 5, vec![bad_portal]), walk, Heightmap::flat(9, 5)).is_err());
    }

    #[test]
    fn camera_validation() {
        let ok = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        assert!(ok.validate().is_ok());
        let inverted = CameraConstraint::Free { pitch: -45.0, min_zoom: 40.0, max_zoom: 10.0, default_zoom: 20.0 };
        assert!(inverted.validate().is_err());
        assert!(CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 0.0 }.validate().is_err());
        assert!(CameraConstraint::Fixed { yaw: f32::NAN, pitch: -45.0, zoom: 1.0 }.validate().is_err());
    }

    #[test]
    fn manifest_camera_deserializes_both_variants() {
        let free: CameraConstraint = serde_json::from_str(
            r#"{"type":"free","pitch":-45.0,"min_zoom":12.0,"max_zoom":40.0,"default_zoom":24.0}"#,
        )
        .unwrap();
        assert!(matches!(free, CameraConstraint::Free { .. }));
        let fixed: CameraConstraint =
            serde_json::from_str(r#"{"type":"fixed","yaw":0.0,"pitch":-45.0,"zoom":18.0}"#).unwrap();
        assert_eq!(fixed, CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 });
    }

    #[test]
    fn manifest_light_radius_and_residents_are_optional() {
        let base = r#""id":"t","name":"T","type":"hub","dimensions":{"width":4,"height":4,"cellSize":0.5},
            "heightBounds":{"min":0,"max":0},"camera":{"type":"fixed","yaw":0,"pitch":-45,"zoom":18}"#;
        let bare: LocationManifest = serde_json::from_str(&format!("{{{base}}}")).unwrap();
        assert_eq!(bare.light_radius, 0.0);
        assert_eq!(bare.residents, ResidentsDef::default());
        let full: LocationManifest = serde_json::from_str(&format!(
            r#"{{{base},"lightRadius":22,"residents":{{"spots":[{{"x":3,"y":4.5,"kind":"sit","facing":2,"tag":"bench"}},{{"x":1,"y":1}}],
                "pets":[{{"sprite":"4_cat","name":"Mittens"}}]}}}}"#
        ))
        .unwrap();
        assert_eq!(full.light_radius, 22.0);
        let r = &full.residents;
        assert_eq!(r.spots[0], SpotDef { x: 3.0, y: 4.5, kind: "sit".into(), facing: 2, tag: Some("bench".into()) });
        assert_eq!((r.spots[1].kind.as_str(), r.spots[1].facing), ("stand", 4));
        assert_eq!(r.pets, vec![PetDef { sprite: "4_cat".into(), name: "Mittens".into() }]);
    }
}
