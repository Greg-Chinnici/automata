use crate::rule::BsRule;

/// Fixed-size grid of a two-state Life-like automaton.
///
/// Double-buffered: `step` reads the current generation and writes the next,
/// never mutating in place. Edges wrap (toroidal) so gliders keep flying.
/// The rule is passed per step, so it can change mid-simulation.
pub struct LifeGrid {
    pub width: usize,
    pub height: usize,
    cells: Vec<bool>,
    next: Vec<bool>,
    pub generation: u64,
}

impl LifeGrid {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![false; width * height],
            next: vec![false; width * height],
            generation: 0,
        }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        self.cells[y * self.width + x]
    }

    #[allow(dead_code)] // used by tests now; brush tools will use it later
    pub fn set(&mut self, x: usize, y: usize, alive: bool) {
        self.cells[y * self.width + x] = alive;
    }

    pub fn clear(&mut self) {
        self.cells.fill(false);
        self.generation = 0;
    }

    pub fn randomize(&mut self, density: f64, rng: &mut impl rand::Rng) {
        for cell in &mut self.cells {
            *cell = rng.random_bool(density);
        }
        self.generation = 0;
    }

    fn live_neighbors(&self, x: usize, y: usize) -> u8 {
        let (w, h) = (self.width, self.height);
        let mut count = 0;
        for dy in [h - 1, 0, 1] {
            for dx in [w - 1, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x + dx) % w;
                let ny = (y + dy) % h;
                count += self.cells[ny * w + nx] as u8;
            }
        }
        count
    }

    /// Advance one generation under the given rule.
    pub fn step(&mut self, rule: BsRule) {
        for y in 0..self.height {
            for x in 0..self.width {
                let n = self.live_neighbors(x, y);
                let alive = self.get(x, y);
                self.next[y * self.width + x] = rule.next_state(alive, n);
            }
        }
        std::mem::swap(&mut self.cells, &mut self.next);
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn life() -> BsRule {
        BsRule::parse("B3/S23").unwrap()
    }

    fn set_all(grid: &mut LifeGrid, cells: &[(usize, usize)]) {
        for &(x, y) in cells {
            grid.set(x, y, true);
        }
    }

    fn live_cells(grid: &LifeGrid) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..grid.height {
            for x in 0..grid.width {
                if grid.get(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn blinker_oscillates_with_period_two() {
        let mut grid = LifeGrid::new(8, 8);
        set_all(&mut grid, &[(2, 3), (3, 3), (4, 3)]);
        grid.step(life());
        assert_eq!(live_cells(&grid), vec![(3, 2), (3, 3), (3, 4)]);
        grid.step(life());
        assert_eq!(live_cells(&grid), vec![(2, 3), (3, 3), (4, 3)]);
    }

    #[test]
    fn block_is_still_life() {
        let mut grid = LifeGrid::new(6, 6);
        set_all(&mut grid, &[(2, 2), (3, 2), (2, 3), (3, 3)]);
        let before = live_cells(&grid);
        grid.step(life());
        assert_eq!(live_cells(&grid), before);
    }

    #[test]
    fn glider_translates_by_one_diagonal_every_four_generations() {
        let mut grid = LifeGrid::new(16, 16);
        set_all(&mut grid, &[(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)]);
        let start = live_cells(&grid);
        for _ in 0..4 {
            grid.step(life());
        }
        let shifted: Vec<_> = start.iter().map(|&(x, y)| (x + 1, y + 1)).collect();
        assert_eq!(live_cells(&grid), shifted);
    }

    #[test]
    fn glider_wraps_around_toroidal_edges() {
        let mut grid = LifeGrid::new(8, 8);
        set_all(&mut grid, &[(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)]);
        // One full lap: 4 generations per diagonal step, 8 steps to wrap.
        let start = live_cells(&grid);
        for _ in 0..32 {
            grid.step(life());
        }
        assert_eq!(live_cells(&grid), start);
    }

    #[test]
    fn rule_can_change_mid_simulation() {
        // Run a block under Life (stable), then switch to Seeds: every live
        // cell dies, and cells with exactly two live neighbors are born.
        let mut grid = LifeGrid::new(8, 8);
        set_all(&mut grid, &[(2, 2), (3, 2), (2, 3), (3, 3)]);
        grid.step(life());
        assert_eq!(live_cells(&grid), vec![(2, 2), (3, 2), (2, 3), (3, 3)]);

        let seeds = BsRule::parse("B2/S").unwrap();
        grid.step(seeds);
        let after = live_cells(&grid);
        assert!(
            !after.contains(&(2, 2)),
            "live cells never survive in Seeds"
        );
        assert!(
            after.contains(&(1, 2)),
            "edge-adjacent cells see exactly two neighbors"
        );
        assert!(
            !after.contains(&(1, 1)),
            "corner-diagonal cells see only one neighbor"
        );
    }
}
