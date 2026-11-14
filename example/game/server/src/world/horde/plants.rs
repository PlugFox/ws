//! Field flora: plants and mushrooms planted once when a stage is created
//! (COMBAT.md §4.10).
//!
//! A location that grows them has `<dist>/server/locations/<id>/plants.json`
//! (baked from recipe.toml `[locations.<id>.plants]` by
//! `pipeline/plants_baker.py`):
//!
//! ```json
//! { "count": [26, 34], "spacing": 5.0, "clear": 14.0,
//!   "kinds": { "green_plant": 100, "shining_plant": 2 },
//!   "ground": "plants.bin" }
//! ```
//!
//! * `count`: plants per stage, rolled once; never respawned, never scaled
//!   by mission modifiers.
//! * `spacing`: least distance between two plants (cells).
//! * `clear`: cells kept free around the arrival point, portals and NPCs.
//! * `kinds`: relative pick weights by monster id (`archetype = "plant"` in
//!   `monsters.json`, whose `stats.hits` is the HP range).
//! * `ground`: optional mask in the `walkability.bin` layout, zero = may
//!   grow there (the grass cells of the baked `surface.bin`). Without it any
//!   walkable cell will do.
//!
//! Placement draws from the horde's RNG before anything else does, so a
//! seeded stage plants the same field every time.

use super::storage::FLAG_PLANT;
use super::{in_radius, Horde};
use crate::world::map::{MapData, MapError, WalkGrid};
use rand::Rng;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Most plants a stage may hold (the bake is trusted, but bounded).
pub const MAX_PLANTS: u16 = 128;

/// Placement attempts per plant before the field counts as full.
const TRIES_PER_PLANT: usize = 40;

/// How far a plant sits off its cell centre (cells), so rows do not show.
const JITTER: f32 = 0.3;

#[derive(Deserialize)]
struct RawPlants {
    count: [u16; 2],
    #[serde(default)]
    spacing: f32,
    #[serde(default)]
    clear: f32,
    kinds: BTreeMap<String, u32>,
    #[serde(default)]
    ground: Option<String>,
}

/// What a location grows (`plants.json`).
#[derive(Clone, Debug)]
pub struct PlantRules {
    /// Plants per stage, `(min, max)`.
    pub count: (u16, u16),
    /// Least distance between two plants (cells).
    pub spacing: f32,
    /// Cells kept free around the arrival point, portals and NPCs.
    pub clear: f32,
    /// Monster id and relative weight, in id order.
    pub kinds: Vec<(String, u32)>,
    /// Zero = plants may grow on the cell; `None` = any walkable cell.
    pub ground: Option<WalkGrid>,
}

impl PlantRules {
    /// Parses `plants.json`; `ground` reads the mask file it names.
    pub fn parse(bytes: &[u8], ground: impl FnOnce(&str) -> Result<Vec<u8>, MapError>) -> Result<Self, MapError> {
        let bad = |m: String| MapError::Walkability(format!("plants.json: {m}"));
        let raw: RawPlants = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        let [lo, hi] = raw.count;
        if lo > hi || hi > MAX_PLANTS {
            return Err(bad(format!("count [{lo}, {hi}] must be min <= max <= {MAX_PLANTS}")));
        }
        if !(raw.spacing.is_finite() && raw.spacing >= 0.0 && raw.clear.is_finite() && raw.clear >= 0.0) {
            return Err(bad("spacing and clear must be >= 0".into()));
        }
        let ground = match raw.ground {
            Some(file) => Some(WalkGrid::parse(&ground(&file)?)?),
            None => None,
        };
        Ok(Self {
            count: (lo, hi),
            spacing: raw.spacing,
            clear: raw.clear,
            kinds: raw.kinds.into_iter().filter(|(_, w)| *w > 0).collect(),
            ground,
        })
    }

    /// `<server_dir>/plants.json` when the location has one.
    pub fn load(server_dir: &Path) -> Result<Option<Self>, MapError> {
        let read = |p: std::path::PathBuf| std::fs::read(&p).map_err(|source| MapError::Io { path: p.display().to_string(), source });
        let path = server_dir.join("plants.json");
        if !path.exists() {
            return Ok(None);
        }
        Self::parse(&read(path)?, |file| read(server_dir.join(file))).map(Some)
    }

    /// Whether the mask (if any) lets a plant grow on the cell.
    fn grows_on(&self, x: i32, y: i32) -> bool {
        self.ground.as_ref().is_none_or(|g| g.is_walkable_cell(x, y))
    }
}

/// Cells a plant may take: reachable, its 8 neighbours walkable (no plant
/// in a wall or a one-cell gap), on the grown ground, and `clear` away
/// from where players arrive, the portals and the NPCs.
fn candidate_cells(map: &MapData, rules: &PlantRules) -> Vec<(u16, u16)> {
    let walk = &map.walk;
    let mut keep: Vec<(f32, f32)> = vec![map.spawn];
    keep.extend(map.manifest.spawn.map(|[x, y]| (x as f32 + 0.5, y as f32 + 0.5)));
    keep.extend(map.manifest.portals.iter().map(|p| p.center()));
    keep.extend(map.manifest.npcs.iter().map(|n| n.center()));
    let mut out = Vec::new();
    for y in 0..walk.height() as i32 {
        for x in 0..walk.width() as i32 {
            if !map.is_reachable_cell(x as u32, y as u32) || !rules.grows_on(x, y) {
                continue;
            }
            if !(-1..=1).all(|dy| (-1..=1).all(|dx| walk.is_walkable_cell(x + dx, y + dy))) {
                continue;
            }
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            if keep.iter().any(|&(kx, ky)| in_radius(cx - kx, cy - ky, rules.clear)) {
                continue;
            }
            out.push((x as u16, y as u16));
        }
    }
    out
}

impl Horde {
    /// Plants the location's flora (`PlantRules` of its map), once, when
    /// the horde is made. Returns how many grew.
    pub(super) fn plant_flora(&mut self) -> usize {
        let world = std::sync::Arc::clone(&self.world);
        let map = world.map(self.map);
        let Some(rules) = map.plants.as_ref() else { return 0 };
        // Weighted kinds this catalogue knows as plants.
        let mut picks: Vec<(u8, u32)> = Vec::new();
        let mut total = 0u32;
        for (id, weight) in &rules.kinds {
            match self.catalog.kind_of(id) {
                Some(kind) if self.catalog.get(kind).is_plant() => {
                    total += weight;
                    picks.push((kind, total));
                }
                _ => tracing::warn!("plants of {}: {id} is not a plant of the catalogue, skipped", map.id()),
            }
        }
        if total == 0 {
            return 0;
        }
        let cells = candidate_cells(map, rules);
        if cells.is_empty() {
            return 0;
        }
        let want = self.rng.gen_range(rules.count.0..=rules.count.1) as usize;
        let mut placed: Vec<(f32, f32)> = Vec::with_capacity(want);
        let mut tries = want * TRIES_PER_PLANT;
        while placed.len() < want && tries > 0 {
            tries -= 1;
            let (cx, cy) = cells[self.rng.gen_range(0..cells.len())];
            let x = cx as f32 + 0.5 + self.rng.gen_range(-JITTER..JITTER);
            let y = cy as f32 + 0.5 + self.rng.gen_range(-JITTER..JITTER);
            if placed.iter().any(|&(px, py)| in_radius(x - px, y - py, rules.spacing)) {
                continue;
            }
            let roll = self.rng.gen_range(0..total);
            let kind = picks.iter().find(|(_, acc)| roll < *acc).expect("roll below the total").0;
            let (lo, hi) = self.catalog.get(kind).hits;
            let hp = self.rng.gen_range(lo..=hi);
            if self.spawn_plant(kind, x, y, hp).is_none() {
                break; // pool full
            }
            placed.push((x, y));
        }
        self.rebuild_index();
        placed.len()
    }

    /// A rooted plant of `kind` that takes `hits` blows.
    pub(super) fn spawn_plant(&mut self, kind: u8, x: f32, y: f32, hits: u16) -> Option<u16> {
        let slot = self.store.spawn(kind, x, y, hits.max(1) as i32, FLAG_PLANT)?;
        self.plants += 1;
        if !self.index.hold && !self.index.fresh.contains(&slot) && self.index.fresh.len() < super::MAX_FRESH {
            self.index.fresh.push(slot);
        } else if !self.index.hold {
            self.rebuild_index();
        }
        Some(slot)
    }

    /// Live plants on the field.
    pub fn plants(&self) -> usize {
        self.plants as usize
    }

    /// The slot holds a plant.
    pub fn is_plant(&self, slot: u16) -> bool {
        self.store.is_plant(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
    use crate::world::horde::{HordeConfig, PlayerView};
    use crate::world::hub::HubWorld;
    use crate::world::map::{
        CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, PortalDef,
    };
    use std::sync::Arc;

    const W: u32 = 80;
    const H: u32 = 80;

    /// An open 80 × 80 field walled at the rim with a block of wall in the
    /// middle-east, a portal in the south-west; grass (the mask) covers the
    /// west half only.
    fn world(rules: Option<PlantRules>) -> Arc<HubWorld> {
        let mut rows = vec![String::new(); H as usize];
        for (r, row) in rows.iter_mut().enumerate() {
            *row = (0..W)
                .map(|c| {
                    let wall = r == 0 || c == 0 || r as u32 == H - 1 || c == W - 1 || ((30..40).contains(&r) && (50..60).contains(&c));
                    if wall { '#' } else { '.' }
                })
                .collect();
        }
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let walk = WalkGrid::from_ascii(&rows);
        let manifest = LocationManifest {
            id: "field".into(),
            name: "Field".into(),
            kind: LocationKind::Field,
            dimensions: Dimensions { width: W, height: H, cell_size: 0.5 },
            height_bounds: HeightBounds { min: 0.0, max: 1.0 },
            camera: CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 30.0, default_zoom: 20.0 },
            portals: vec![PortalDef {
                name: "exit".into(),
                trigger: [10, 10, 2],
                target_map: "field".into(),
                target_pos: [40, 40],
            }],
            bgm: None,
            npcs: Vec::new(),
            light_radius: 0.0,
            residents: Default::default(),
            spawn: Some([40, 40]),
            return_point: None,
        };
        let mut map = MapData::new(manifest, walk, Heightmap::flat(W, H)).unwrap();
        map.plants = rules;
        Arc::new(HubWorld::new(vec![map], "field").unwrap())
    }

    fn west_grass() -> WalkGrid {
        WalkGrid::parse(&west_grass_bytes()).unwrap()
    }

    fn rules() -> PlantRules {
        PlantRules {
            count: (20, 24),
            spacing: 4.0,
            clear: 8.0,
            kinds: vec![("common".into(), 90), ("rare".into(), 10), ("ghost".into(), 50), ("poring".into(), 50)],
            ground: Some(west_grass()),
        }
    }

    fn catalog() -> Arc<MonsterCatalog> {
        Arc::new(
            MonsterCatalog::new(vec![
                test_plant("common", (3, 5)),
                test_plant("rare", (10, 10)),
                test_def("poring", 2.0, 1, 0.0, false),
            ])
            .unwrap(),
        )
    }

    fn horde(seed: u64, rules: Option<PlantRules>) -> Horde {
        let cfg = HordeConfig { director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
        Horde::new(world(rules), catalog(), cfg)
    }

    fn layout(h: &Horde) -> Vec<(u8, u32, u32, i32)> {
        h.monsters().map(|(s, _, k, x, y)| (k, x.to_bits(), y.to_bits(), h.store().hp[s as usize])).collect()
    }

    #[test]
    fn a_seeded_stage_plants_the_same_field() {
        let a = horde(7, Some(rules()));
        assert_eq!(layout(&a), layout(&horde(7, Some(rules()))));
        assert_ne!(layout(&a), layout(&horde(8, Some(rules()))), "another seed, another field");
        assert!((20..=24).contains(&a.plants()), "{} plants", a.plants());
        assert_eq!(a.plants(), a.len(), "nothing but plants at creation");
        assert_eq!(horde(7, None).len(), 0, "a location without flora grows none");
    }

    #[test]
    fn plants_keep_their_ground_spacing_and_clearings() {
        let h = horde(3, Some(rules()));
        let walk = &h.world.map(h.map).walk;
        let pts: Vec<(u8, f32, f32)> = h.monsters().map(|(_, _, k, x, y)| (k, x, y)).collect();
        let catalog = catalog();
        for (i, &(k, x, y)) in pts.iter().enumerate() {
            assert!(catalog.get(k).is_plant(), "only plant kinds are picked (unknown ids and porings skipped)");
            assert!(x < (W / 2) as f32, "on the grass mask: x = {x}");
            let (cx, cy) = (x.floor() as i32, y.floor() as i32);
            assert!((-1..=1).all(|dy| (-1..=1).all(|dx| walk.is_walkable_cell(cx + dx, cy + dy))), "clear of walls at ({x}, {y})");
            // The clearing is tested on cell centres; the jitter is under half a cell.
            assert!((x - 40.5).hypot(y - 40.5) > 7.5 && (x - 10.5).hypot(y - 10.5) > 7.5, "clear of the arrival and the portal");
            for &(_, ox, oy) in &pts[i + 1..] {
                assert!((x - ox).hypot(y - oy) > 4.0, "spaced");
            }
            let s = h.monsters().find(|m| m.3 == x && m.4 == y).unwrap().0;
            assert!(h.is_plant(s));
            let hp = h.store().hp[s as usize];
            let (lo, hi) = catalog.get(k).hits;
            assert!((lo as i32..=hi as i32).contains(&hp), "hit points from the kind's range");
        }
        // Common ones outnumber rare ones (90 : 10).
        let rare = pts.iter().filter(|p| catalog.get(p.0).id == "rare").count();
        assert!(rare * 3 < pts.len(), "{rare} rare of {}", pts.len());
    }

    #[test]
    fn plants_stay_rooted_and_out_of_the_wave_budget() {
        let mut h = horde(5, Some(rules()));
        let before = layout(&h);
        let (x, y) = (20.0, 40.0);
        for t in 0..100 {
            h.tick_views(t, 20, &[PlayerView { x, y, light: 40.0, id: 1, hunted: true }]);
        }
        assert_eq!(layout(&h), before, "no step, no push, no recycling");
        let s = h.monsters().next().unwrap().0;
        h.place(s, 30.0, 30.0);
        assert_eq!(layout(&h), before, "nothing moves a plant");

        // With the game master on, waves come and the flora stays as it was:
        // never bought, never respawned, and no room taken from the cap.
        let cfg = HordeConfig { seed: Some(5), alive_cap: 10, ..HordeConfig::default() };
        let mut h = Horde::new(world(Some(rules())), catalog(), cfg);
        let plants = h.plants();
        let player = [PlayerView { x, y, light: 10.0, id: 1, hunted: true }];
        for t in 0..20 * 60 {
            h.tick_views(t, 20, &player);
        }
        let catalog = catalog();
        let kinds: Vec<bool> = h.monsters().map(|m| catalog.get(m.2).is_plant()).collect();
        assert_eq!(kinds.iter().filter(|p| **p).count(), plants, "the same plants");
        assert_eq!(kinds.iter().filter(|p| !**p).count(), 10, "the cap counts monsters only");
    }

    /// The real pack (when the asset submodule is baked): Poring Island
    /// plants its table's count of known plant kinds, all on grass.
    #[test]
    fn the_baked_field_grows_its_flora() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let (Ok(world), Ok(catalog)) = (HubWorld::load(&dist, &["prt_fild08".into()], "prt_fild08"), MonsterCatalog::load(&dist))
        else {
            return;
        };
        let Some(rules) = world.map(world.entry()).plants.clone() else { return };
        let world = Arc::new(world);
        let cfg = HordeConfig { director_enabled: false, seed: Some(11), ..HordeConfig::default() };
        let h = Horde::new(Arc::clone(&world), Arc::new(catalog), cfg);
        assert!((rules.count.0 as usize..=rules.count.1 as usize).contains(&h.plants()), "{} plants", h.plants());
        let ground = rules.ground.as_ref().expect("the field names its grass");
        for (s, _, k, x, y) in h.monsters() {
            assert!(h.is_plant(s) && h.catalog().get(k).is_plant());
            assert!(ground.is_walkable(x, y), "{} at ({x}, {y}) on grass", h.catalog().get(k).id);
        }
    }

    #[test]
    fn rules_parse_and_reject_nonsense() {
        let mask = |_: &str| Ok(west_grass_bytes());
        let ok = br#"{"count":[2,4],"spacing":3,"clear":5,"kinds":{"a":3,"b":0},"ground":"plants.bin"}"#;
        let r = PlantRules::parse(ok, mask).unwrap();
        assert_eq!((r.count, r.spacing, r.clear, r.kinds.clone()), ((2, 4), 3.0, 5.0, vec![("a".to_string(), 3)]));
        assert!(r.grows_on(1, 1) && !r.grows_on(W as i32 - 2, 1));
        assert!(PlantRules::parse(br#"{"count":[4,2],"kinds":{}}"#, mask).is_err());
        assert!(PlantRules::parse(br#"{"count":[1,999],"kinds":{}}"#, mask).is_err());
        assert!(PlantRules::parse(br#"{"count":[1,2],"spacing":-1,"kinds":{}}"#, mask).is_err());
        let any = PlantRules::parse(br#"{"count":[1,2],"kinds":{"a":1}}"#, mask).unwrap();
        assert!(any.ground.is_none() && any.grows_on(70, 1));
    }

    /// The mask file: zero (grass) on the west half.
    fn west_grass_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&W.to_le_bytes());
        bytes.extend_from_slice(&H.to_le_bytes());
        for _y in 0..H {
            bytes.extend((0..W).map(|x| u8::from(x >= W / 2)));
        }
        bytes
    }
}
