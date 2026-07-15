use std::collections::{HashMap, HashSet};

use crate::rule::BsRule;

pub const CHUNK_SIZE: i64 = 64;
const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Split a world coordinate into (chunk coordinate, local coordinate).
fn split(coord: i64) -> (i64, i64) {
    (coord.div_euclid(CHUNK_SIZE), coord.rem_euclid(CHUNK_SIZE))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChunkPos {
    pub x: i64,
    pub y: i64,
}

/// A dense square of cells. Only chunks containing live cells are stored.
#[derive(Clone)]
struct Chunk {
    cells: Box<[bool]>,
    live: u32,
}

impl Chunk {
    fn new() -> Self {
        Self {
            cells: vec![false; CHUNK_AREA].into_boxed_slice(),
            live: 0,
        }
    }

    fn get(&self, lx: i64, ly: i64) -> bool {
        self.cells[(ly * CHUNK_SIZE + lx) as usize]
    }

    fn set(&mut self, lx: i64, ly: i64, value: bool) {
        let cell = &mut self.cells[(ly * CHUNK_SIZE + lx) as usize];
        if *cell != value {
            *cell = value;
            if value {
                self.live += 1;
            } else {
                self.live -= 1;
            }
        }
    }
}

/// Effectively infinite, sparse world. Empty space is not stored: chunks are
/// allocated when cells are born and dropped when they empty out.
///
/// Stepping is double-buffered — each generation is computed from an
/// immutable read of the previous one. Neighbor counts at chunk edges read
/// halo (ghost) cells from the eight adjacent chunks.
#[derive(Clone)]
pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
    pub generation: u64,
}

impl World {
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            generation: 0,
        }
    }

    pub fn get(&self, x: i64, y: i64) -> bool {
        let (cx, lx) = split(x);
        let (cy, ly) = split(y);
        self.chunks
            .get(&ChunkPos { x: cx, y: cy })
            .is_some_and(|chunk| chunk.get(lx, ly))
    }

    pub fn set(&mut self, x: i64, y: i64, value: bool) {
        let (cx, lx) = split(x);
        let (cy, ly) = split(y);
        let pos = ChunkPos { x: cx, y: cy };
        if !value {
            // Never allocate a chunk just to store a dead cell.
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                chunk.set(lx, ly, false);
                if chunk.live == 0 {
                    self.chunks.remove(&pos);
                }
            }
            return;
        }
        self.chunks
            .entry(pos)
            .or_insert_with(Chunk::new)
            .set(lx, ly, true);
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.generation = 0;
    }

    pub fn population(&self) -> u64 {
        self.chunks.values().map(|c| c.live as u64).sum()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn randomize_region(
        &mut self,
        min_x: i64,
        min_y: i64,
        max_x: i64,
        max_y: i64,
        density: f64,
        rng: &mut impl rand::Rng,
    ) {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                self.set(x, y, rng.random_bool(density));
            }
        }
    }

    /// All live cells inside the (inclusive) world-coordinate rectangle.
    pub fn live_cells_in(&self, min_x: i64, min_y: i64, max_x: i64, max_y: i64) -> Vec<(i64, i64)> {
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
                        if x >= min_x && x <= max_x && chunk.get(lx, ly) {
                            out.push((x, y));
                        }
                    }
                }
            }
        }
        out
    }

    /// Advance one generation under the given rule.
    pub fn step(&mut self, rule: BsRule) {
        // Any birth must be adjacent to a live cell, so only chunks with live
        // cells and their eight neighbors can change.
        let mut candidates = HashSet::new();
        for pos in self.chunks.keys() {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    candidates.insert(ChunkPos {
                        x: pos.x + dx,
                        y: pos.y + dy,
                    });
                }
            }
        }

        let mut next = HashMap::new();
        for pos in candidates {
            // The 3x3 chunk neighborhood supplying halo cells at the edges.
            let mut neighborhood: [Option<&Chunk>; 9] = [None; 9];
            for dy in -1..=1i64 {
                for dx in -1..=1i64 {
                    neighborhood[((dy + 1) * 3 + dx + 1) as usize] = self.chunks.get(&ChunkPos {
                        x: pos.x + dx,
                        y: pos.y + dy,
                    });
                }
            }
            // Local read covering -1..=CHUNK_SIZE on both axes.
            let read = |lx: i64, ly: i64| -> bool {
                let (dx, lx) = if lx < 0 {
                    (-1, lx + CHUNK_SIZE)
                } else if lx >= CHUNK_SIZE {
                    (1, lx - CHUNK_SIZE)
                } else {
                    (0, lx)
                };
                let (dy, ly) = if ly < 0 {
                    (-1, ly + CHUNK_SIZE)
                } else if ly >= CHUNK_SIZE {
                    (1, ly - CHUNK_SIZE)
                } else {
                    (0, ly)
                };
                neighborhood[((dy + 1) * 3 + dx + 1) as usize].is_some_and(|c| c.get(lx, ly))
            };

            let mut new_chunk = Chunk::new();
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let mut count = 0u8;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx != 0 || dy != 0 {
                                count += read(lx + dx, ly + dy) as u8;
                            }
                        }
                    }
                    if rule.next_state(read(lx, ly), count) {
                        new_chunk.set(lx, ly, true);
                    }
                }
            }
            if new_chunk.live > 0 {
                next.insert(pos, new_chunk);
            }
        }
        self.chunks = next;
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn life() -> BsRule {
        BsRule::parse("B3/S23").unwrap()
    }

    fn set_all(world: &mut World, cells: &[(i64, i64)]) {
        for &(x, y) in cells {
            world.set(x, y, true);
        }
    }

    fn live_cells(world: &World) -> Vec<(i64, i64)> {
        let mut cells = world.live_cells_in(-10_000, -10_000, 10_000, 10_000);
        cells.sort();
        cells
    }

    fn glider(origin_x: i64, origin_y: i64) -> Vec<(i64, i64)> {
        [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)]
            .iter()
            .map(|&(x, y)| (origin_x + x, origin_y + y))
            .collect()
    }

    #[test]
    fn blinker_oscillates_across_chunk_boundary() {
        // Horizontal blinker straddling the x = 0 chunk edge.
        let mut world = World::new();
        set_all(&mut world, &[(-1, 3), (0, 3), (1, 3)]);
        world.step(life());
        assert_eq!(live_cells(&world), vec![(0, 2), (0, 3), (0, 4)]);
        world.step(life());
        assert_eq!(live_cells(&world), vec![(-1, 3), (0, 3), (1, 3)]);
    }

    #[test]
    fn block_is_still_life_at_negative_coordinates() {
        let mut world = World::new();
        set_all(
            &mut world,
            &[(-70, -70), (-69, -70), (-70, -69), (-69, -69)],
        );
        let before = live_cells(&world);
        world.step(life());
        assert_eq!(live_cells(&world), before);
    }

    #[test]
    fn glider_crosses_chunk_boundary_intact() {
        // Starts near the corner of chunk (0,0); after 16 generations it has
        // moved (+4,+4) and crossed into neighboring chunks.
        let mut world = World::new();
        set_all(&mut world, &glider(60, 60));
        for _ in 0..8 {
            world.step(life());
        }
        assert!(
            world.chunk_count() > 1,
            "mid-crossing, the glider straddles the boundary"
        );
        for _ in 0..8 {
            world.step(life());
        }
        let mut expected = glider(64, 64);
        expected.sort();
        assert_eq!(live_cells(&world), expected);
    }

    #[test]
    fn empty_chunks_are_dropped() {
        let mut world = World::new();
        world.set(5, 5, true); // lone cell dies of underpopulation
        assert_eq!(world.chunk_count(), 1);
        world.step(life());
        assert_eq!(world.population(), 0);
        assert_eq!(world.chunk_count(), 0);
    }

    #[test]
    fn erasing_last_cell_frees_the_chunk() {
        let mut world = World::new();
        world.set(5, 5, true);
        world.set(5, 5, false);
        assert_eq!(world.chunk_count(), 0);
        // Setting a cell dead in empty space allocates nothing.
        world.set(9999, 9999, false);
        assert_eq!(world.chunk_count(), 0);
    }

    #[test]
    fn birth_can_allocate_a_new_chunk() {
        // Blinker at the very edge of chunk (0,0): its vertical phase pokes
        // into the chunk above/below.
        let mut world = World::new();
        set_all(&mut world, &[(9, 0), (10, 0), (11, 0)]);
        assert_eq!(world.chunk_count(), 1);
        world.step(life());
        assert_eq!(live_cells(&world), vec![(10, -1), (10, 0), (10, 1)]);
        assert_eq!(world.chunk_count(), 2);
    }

    #[test]
    fn rule_can_change_mid_simulation() {
        let mut world = World::new();
        set_all(&mut world, &[(2, 2), (3, 2), (2, 3), (3, 3)]);
        world.step(life());
        assert_eq!(live_cells(&world).len(), 4, "block stable under Life");
        world.step(BsRule::parse("B2/S").unwrap());
        let after = live_cells(&world);
        assert!(
            !after.contains(&(2, 2)),
            "live cells never survive in Seeds"
        );
        assert!(
            after.contains(&(1, 2)),
            "cells born where two neighbors meet"
        );
    }

    #[test]
    fn live_cells_in_respects_bounds() {
        let mut world = World::new();
        set_all(&mut world, &[(0, 0), (100, 100), (-100, -100)]);
        assert_eq!(world.live_cells_in(-10, -10, 10, 10), vec![(0, 0)]);
        assert_eq!(world.live_cells_in(-200, -200, 200, 200).len(), 3);
    }
}
