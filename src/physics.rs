use std::collections::{HashMap, HashSet};

use rand::Rng;

use crate::material::{Material, MaterialId, Phase, EMPTY};
use crate::world::{split, ChunkPos, CHUNK_SIZE};

const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// A dense square of material ids. Only chunks containing non-empty cells
/// are stored.
#[derive(Clone)]
struct Chunk {
    cells: Box<[MaterialId]>,
    occupied: u32,
}

impl Chunk {
    fn new() -> Self {
        Self {
            cells: vec![EMPTY; CHUNK_AREA].into_boxed_slice(),
            occupied: 0,
        }
    }

    fn get(&self, lx: i64, ly: i64) -> MaterialId {
        self.cells[(ly * CHUNK_SIZE + lx) as usize]
    }

    fn set(&mut self, lx: i64, ly: i64, value: MaterialId) {
        let cell = &mut self.cells[(ly * CHUNK_SIZE + lx) as usize];
        if *cell == EMPTY && value != EMPTY {
            self.occupied += 1;
        } else if *cell != EMPTY && value == EMPTY {
            self.occupied -= 1;
        }
        *cell = value;
    }
}

/// Sparse infinite grid of materials for the falling-sand physics mode.
///
/// Unlike the CA `World`, stepping mutates **in place** with a well-defined
/// scan order (bottom-to-top, alternating horizontal direction per
/// generation). A "move" is a clear+set pair, which doesn't map cleanly onto
/// the CA's double-buffered model — this divergence is intentional, and is
/// the standard approach for falling-sand sims (Noita, Powder Toy).
#[derive(Clone)]
pub struct PhysicsWorld {
    chunks: HashMap<ChunkPos, Chunk>,
    pub generation: u64,
}

impl PhysicsWorld {
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            generation: 0,
        }
    }

    pub fn get(&self, x: i64, y: i64) -> MaterialId {
        let (cx, lx) = split(x);
        let (cy, ly) = split(y);
        self.chunks
            .get(&ChunkPos { x: cx, y: cy })
            .map_or(EMPTY, |chunk| chunk.get(lx, ly))
    }

    pub fn set(&mut self, x: i64, y: i64, value: MaterialId) {
        let (cx, lx) = split(x);
        let (cy, ly) = split(y);
        let pos = ChunkPos { x: cx, y: cy };
        if value == EMPTY {
            // Never allocate a chunk just to store an empty cell.
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                chunk.set(lx, ly, EMPTY);
                if chunk.occupied == 0 {
                    self.chunks.remove(&pos);
                }
            }
            return;
        }
        self.chunks
            .entry(pos)
            .or_insert_with(Chunk::new)
            .set(lx, ly, value);
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.generation = 0;
    }

    pub fn population(&self) -> u64 {
        self.chunks.values().map(|c| c.occupied as u64).sum()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// All non-empty cells inside the (inclusive) world-coordinate rectangle.
    pub fn cells_in(
        &self,
        min_x: i64,
        min_y: i64,
        max_x: i64,
        max_y: i64,
    ) -> Vec<(i64, i64, MaterialId)> {
        let mut out = Vec::new();
        let (cx0, _) = split(min_x);
        let (cx1, _) = split(max_x);
        let (cy0, _) = split(min_y);
        let (cy1, _) = split(max_y);
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let Some(chunk) = self.chunks.get(&ChunkPos { x: cx, y: cy }) else {
                    continue;
                };
                let base_x = cx * CHUNK_SIZE;
                let base_y = cy * CHUNK_SIZE;
                for ly in 0..CHUNK_SIZE {
                    let y = base_y + ly;
                    if y < min_y || y > max_y {
                        continue;
                    }
                    for lx in 0..CHUNK_SIZE {
                        let x = base_x + lx;
                        if x < min_x || x > max_x {
                            continue;
                        }
                        let material = chunk.get(lx, ly);
                        if material != EMPTY {
                            out.push((x, y, material));
                        }
                    }
                }
            }
        }
        out
    }

    /// Scatter `material` over the inclusive rectangle at the given density,
    /// leaving cells that don't pass the roll untouched.
    pub fn sprinkle(
        &mut self,
        (min_x, min_y, max_x, max_y): (i64, i64, i64, i64),
        material: MaterialId,
        density: f64,
        rng: &mut impl Rng,
    ) {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if rng.random_bool(density) {
                    self.set(x, y, material);
                }
            }
        }
    }

    /// Advance one tick: gravity for powders/liquids, buoyancy for gases,
    /// density displacement between fluids.
    ///
    /// Sequential ordered scan — rows bottom-to-top so falling resolves in a
    /// single pass, horizontal direction alternating per generation so piles
    /// and spreads don't develop a directional bias. Cells that already moved
    /// this tick are skipped so nothing moves twice.
    pub fn step(&mut self, materials: &[Material], rng: &mut impl Rng) {
        let left_to_right = self.generation.is_multiple_of(2);
        let mut moved: HashSet<(i64, i64)> = HashSet::new();

        // Snapshot the chunk grid up front; chunks created mid-step only
        // ever contain cells that already moved this tick.
        let mut chunk_ys: Vec<i64> = self.chunks.keys().map(|p| p.y).collect();
        chunk_ys.sort_unstable();
        chunk_ys.dedup();

        for &cy in chunk_ys.iter().rev() {
            let mut chunk_xs: Vec<i64> = self
                .chunks
                .keys()
                .filter(|p| p.y == cy)
                .map(|p| p.x)
                .collect();
            chunk_xs.sort_unstable();
            if !left_to_right {
                chunk_xs.reverse();
            }
            for ly in (0..CHUNK_SIZE).rev() {
                let y = cy * CHUNK_SIZE + ly;
                for &cx in &chunk_xs {
                    if !self.chunks.contains_key(&ChunkPos { x: cx, y: cy }) {
                        continue; // emptied out mid-step
                    }
                    for i in 0..CHUNK_SIZE {
                        let lx = if left_to_right {
                            i
                        } else {
                            CHUNK_SIZE - 1 - i
                        };
                        let x = cx * CHUNK_SIZE + lx;
                        self.step_cell(x, y, materials, &mut moved, rng);
                    }
                }
            }
        }
        self.generation += 1;
    }

    fn step_cell(
        &mut self,
        x: i64,
        y: i64,
        materials: &[Material],
        moved: &mut HashSet<(i64, i64)>,
        rng: &mut impl Rng,
    ) {
        let id = self.get(x, y);
        if id == EMPTY || moved.contains(&(x, y)) {
            return;
        }
        let mover = &materials[id as usize];

        // Candidate directions in priority order; diagonal / sideways pairs
        // are shuffled so piles and spreads stay symmetric on average.
        let mut dirs: [(i64, i64); 5] = match mover.phase {
            Phase::Powder => [(0, 1), (-1, 1), (1, 1), (0, 0), (0, 0)],
            Phase::Liquid => [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0)],
            Phase::Gas => [(0, -1), (-1, -1), (1, -1), (0, 0), (0, 0)],
            Phase::Empty | Phase::Solid => return,
        };
        if rng.random_bool(0.5) {
            dirs.swap(1, 2);
        }
        if dirs[3] != (0, 0) && rng.random_bool(0.5) {
            dirs.swap(3, 4);
        }

        for (dx, dy) in dirs {
            if (dx, dy) == (0, 0) {
                continue;
            }
            let (tx, ty) = (x + dx, y + dy);
            let target_id = self.get(tx, ty);
            if !can_displace(mover, target_id, dy, materials) {
                continue;
            }
            // Swap: covers plain moves (target empty) and displacement
            // (sand sinking through water, which bubbles up in its place).
            self.set(x, y, target_id);
            self.set(tx, ty, id);
            moved.insert((tx, ty));
            if target_id != EMPTY {
                moved.insert((x, y));
            }
            return;
        }
    }
}

/// Whether `mover` may enter a cell holding `target_id` when moving with
/// vertical direction `dy`. Empty cells are always enterable; fluids are
/// displaced by density (denser sinks, lighter rises); sideways movement
/// requires empty space.
fn can_displace(mover: &Material, target_id: MaterialId, dy: i64, materials: &[Material]) -> bool {
    if target_id == EMPTY {
        return true;
    }
    if dy == 0 {
        return false;
    }
    let target = &materials[target_id as usize];
    if !matches!(target.phase, Phase::Liquid | Phase::Gas) {
        return false;
    }
    if dy > 0 {
        mover.density > target.density
    } else {
        mover.density < target.density
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{builtin_materials, SAND, STONE, WATER};
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn step_n(world: &mut PhysicsWorld, n: usize) {
        let materials = builtin_materials();
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..n {
            world.step(&materials, &mut rng);
        }
    }

    fn cells(world: &PhysicsWorld) -> Vec<(i64, i64, MaterialId)> {
        let mut out = world.cells_in(-10_000, -10_000, 10_000, 10_000);
        out.sort();
        out
    }

    #[test]
    fn sand_falls_and_rests_on_stone() {
        let mut world = PhysicsWorld::new();
        // Three-wide floor so the grain can't topple off diagonally.
        for x in -1..=1 {
            world.set(x, 10, STONE);
        }
        world.set(0, 0, SAND);
        step_n(&mut world, 15);
        assert_eq!(world.get(0, 9), SAND, "sand came to rest on the stone");
        assert_eq!(world.get(0, 10), STONE);
    }

    #[test]
    fn sand_piles_diagonally() {
        let mut world = PhysicsWorld::new();
        for x in -2..=2 {
            world.set(x, 10, STONE);
        }
        world.set(0, 9, SAND);
        world.set(0, 8, SAND);
        step_n(&mut world, 5);
        assert_eq!(world.get(0, 9), SAND, "bottom grain stays put");
        let toppled = world.get(-1, 9) == SAND || world.get(1, 9) == SAND;
        assert!(toppled, "top grain slid off diagonally: {:?}", cells(&world));
        assert_eq!(world.get(0, 8), EMPTY);
    }

    #[test]
    fn water_spreads_to_find_level() {
        let mut world = PhysicsWorld::new();
        // Walled basin — water jitters sideways forever, so an open floor
        // would let it wander off the edge.
        for x in -4..=4 {
            world.set(x, 10, STONE);
        }
        for y in 6..10 {
            world.set(-4, y, STONE);
            world.set(4, y, STONE);
        }
        world.set(0, 9, WATER);
        world.set(0, 8, WATER);
        step_n(&mut world, 10);
        let water: Vec<_> = cells(&world)
            .into_iter()
            .filter(|&(_, _, m)| m == WATER)
            .collect();
        assert_eq!(water.len(), 2);
        assert!(
            water.iter().all(|&(_, y, _)| y == 9),
            "water leveled out into one layer: {water:?}"
        );
    }

    #[test]
    fn sand_sinks_through_water() {
        // Sealed 1-wide well: floor at y=11, walls up both sides, so the
        // water has nowhere to go but up past the sand.
        let mut world = PhysicsWorld::new();
        for x in -1..=1 {
            world.set(x, 11, STONE);
        }
        for y in 8..=10 {
            world.set(-1, y, STONE);
            world.set(1, y, STONE);
        }
        world.set(0, 10, WATER);
        world.set(0, 9, SAND);
        step_n(&mut world, 5);
        assert_eq!(world.get(0, 10), SAND, "sand displaced the water");
        assert_eq!(world.get(0, 9), WATER, "water bubbled up above it");
    }

    #[test]
    fn falling_crosses_chunk_boundary() {
        // Stone floor in chunk row 1, sand starting in chunk row 0.
        let mut world = PhysicsWorld::new();
        for x in -1..=1 {
            world.set(x, CHUNK_SIZE + 6, STONE);
        }
        world.set(0, CHUNK_SIZE - 2, SAND);
        step_n(&mut world, 12);
        assert_eq!(world.get(0, CHUNK_SIZE + 5), SAND);
    }

    #[test]
    fn cell_moves_at_most_once_per_step() {
        // A lone grain falls exactly one cell per tick, even though the
        // in-place scan revisits the row it lands in... it must not.
        let mut world = PhysicsWorld::new();
        world.set(0, 0, SAND);
        let materials = builtin_materials();
        let mut rng = StdRng::seed_from_u64(1);
        world.step(&materials, &mut rng);
        assert_eq!(world.get(0, 1), SAND);
        assert_eq!(world.population(), 1);
    }

    #[test]
    fn step_is_deterministic_for_a_fixed_seed() {
        let build = || {
            let mut world = PhysicsWorld::new();
            for x in -20..=20 {
                world.set(x, 30, STONE);
            }
            let mut rng = StdRng::seed_from_u64(42);
            world.sprinkle((-15, 0, 15, 20), SAND, 0.3, &mut rng);
            world.sprinkle((-15, 0, 15, 20), WATER, 0.2, &mut rng);
            let materials = builtin_materials();
            for _ in 0..30 {
                world.step(&materials, &mut rng);
            }
            cells(&world)
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn erasing_last_cell_frees_the_chunk() {
        let mut world = PhysicsWorld::new();
        world.set(5, 5, SAND);
        assert_eq!(world.chunk_count(), 1);
        world.set(5, 5, EMPTY);
        assert_eq!(world.chunk_count(), 0);
        world.set(9999, 9999, EMPTY);
        assert_eq!(world.chunk_count(), 0);
    }
}
