use serde::{Deserialize, Serialize};

/// Index into the material table. 0 is always Empty.
pub type MaterialId = u8;

pub const EMPTY: MaterialId = 0;
pub const SAND: MaterialId = 1;
pub const WATER: MaterialId = 2;
pub const STONE: MaterialId = 3;

/// Broad behavioral class of a material; the step function dispatches on
/// this, with `density` refining displacement order within/between phases.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Phase {
    Empty,
    /// Static, never moves on its own (stone, wood).
    Solid,
    /// Falls, piles up diagonally (sand).
    Powder,
    /// Falls, spreads sideways to find its level (water).
    Liquid,
    /// Rises, diffuses (steam, smoke).
    Gas,
}

/// A material is a data record, not code — so palettes can be authored,
/// saved, and shared the same way rule stacks are.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub phase: Phase,
    /// Determines displacement order: denser sinks through lighter fluids.
    pub density: f32,
    /// 0xRRGGBB.
    pub color: u32,
}

impl Material {
    pub fn phase_label(&self) -> &'static str {
        match self.phase {
            Phase::Empty => "empty",
            Phase::Solid => "solid",
            Phase::Powder => "powder",
            Phase::Liquid => "liquid",
            Phase::Gas => "gas",
        }
    }
}

/// Built-in palette. Index in the returned Vec is the `MaterialId`, matching
/// the `EMPTY`/`SAND`/... constants above.
pub fn builtin_materials() -> Vec<Material> {
    let mat = |name: &str, phase, density, color| Material {
        name: name.to_string(),
        phase,
        density,
        color,
    };
    vec![
        mat("Empty", Phase::Empty, 0.0, 0x000000),
        mat("Sand", Phase::Powder, 1.5, 0xd8b45a),
        mat("Water", Phase::Liquid, 1.0, 0x4a90d9),
        mat("Stone", Phase::Solid, 3.0, 0x8b8b93),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_table_order() {
        let materials = builtin_materials();
        assert_eq!(materials[EMPTY as usize].name, "Empty");
        assert_eq!(materials[SAND as usize].name, "Sand");
        assert_eq!(materials[WATER as usize].name, "Water");
        assert_eq!(materials[STONE as usize].name, "Stone");
    }

    #[test]
    fn material_round_trips_through_json() {
        let materials = builtin_materials();
        let json = serde_json::to_string(&materials).unwrap();
        let back: Vec<Material> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), materials.len());
        assert_eq!(back[SAND as usize].phase, Phase::Powder);
        assert_eq!(back[SAND as usize].color, materials[SAND as usize].color);
    }
}
