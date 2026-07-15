use gpui::{hsla, rgb, Hsla};

/// How live cells are colored.
#[derive(Clone, Copy, PartialEq)]
pub enum CellStyle {
    /// Every live cell uses the theme accent color.
    Solid,
    /// Hue sweeps diagonally across the grid, so patterns shift color as
    /// they travel.
    Rainbow,
}

/// A complete UI palette. Colors are 0xRRGGBB.
#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: u32,
    pub canvas_bg: u32,
    pub panel_bg: u32,
    pub chip_bg: u32,
    pub hover_bg: u32,
    pub text: u32,
    pub text_dim: u32,
    /// Live cells (when solid), selection highlights, active accents.
    pub accent: u32,
    pub cell_style: CellStyle,
}

impl Theme {
    pub fn cell_color(&self, x: i64, y: i64) -> Hsla {
        match self.cell_style {
            CellStyle::Solid => rgb(self.accent).into(),
            CellStyle::Rainbow => {
                // Diagonal hue sweep repeating every PERIOD cells, stable
                // across the infinite plane (rem_euclid handles negatives).
                const PERIOD: i64 = 160;
                let phase = (x + y).rem_euclid(PERIOD);
                hsla(phase as f32 / PERIOD as f32, 0.85, 0.62, 1.0)
            }
        }
    }
}

pub fn themes() -> Vec<Theme> {
    vec![
        Theme {
            name: "Terminal",
            bg: 0x14141a,
            canvas_bg: 0x0b0b10,
            panel_bg: 0x101016,
            chip_bg: 0x1d1d26,
            hover_bg: 0x232331,
            text: 0x9aa0b0,
            text_dim: 0x565c68,
            accent: 0x5be37d,
            cell_style: CellStyle::Solid,
        },
        Theme {
            name: "Ocean",
            bg: 0x0d1522,
            canvas_bg: 0x081020,
            panel_bg: 0x0b1220,
            chip_bg: 0x14203a,
            hover_bg: 0x1a2a4a,
            text: 0x92a8c8,
            text_dim: 0x4a5d7a,
            accent: 0x64b5f6,
            cell_style: CellStyle::Solid,
        },
        Theme {
            name: "Grayscale",
            bg: 0x171717,
            canvas_bg: 0x0d0d0d,
            panel_bg: 0x121212,
            chip_bg: 0x232323,
            hover_bg: 0x2e2e2e,
            text: 0x9e9e9e,
            text_dim: 0x5c5c5c,
            accent: 0xe6e6e6,
            cell_style: CellStyle::Solid,
        },
        Theme {
            name: "Rainbow",
            bg: 0x141218,
            canvas_bg: 0x0a090e,
            panel_bg: 0x100e14,
            chip_bg: 0x1e1a26,
            hover_bg: 0x282132,
            text: 0xa89fb8,
            text_dim: 0x5f5870,
            accent: 0xff7ad9,
            cell_style: CellStyle::Rainbow,
        },
        Theme {
            name: "Synthwave",
            bg: 0x1a1030,
            canvas_bg: 0x120a24,
            panel_bg: 0x160d2a,
            chip_bg: 0x271846,
            hover_bg: 0x32205a,
            text: 0xb39ddb,
            text_dim: 0x6a5490,
            accent: 0xff4fd8,
            cell_style: CellStyle::Solid,
        },
        Theme {
            name: "Amber",
            bg: 0x1a1410,
            canvas_bg: 0x100c08,
            panel_bg: 0x15100c,
            chip_bg: 0x261e14,
            hover_bg: 0x32281a,
            text: 0xc0a888,
            text_dim: 0x6e5f4a,
            accent: 0xffb74d,
            cell_style: CellStyle::Solid,
        },
        Theme {
            name: "Paper",
            bg: 0xece8de,
            canvas_bg: 0xf9f7f0,
            panel_bg: 0xe2ddd0,
            chip_bg: 0xd6d0c0,
            hover_bg: 0xc9c2b0,
            text: 0x5a5648,
            text_dim: 0x9a947f,
            accent: 0x3a6b35,
            cell_style: CellStyle::Solid,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_are_unique() {
        let names: Vec<_> = themes().iter().map(|t| t.name).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len());
    }

    #[test]
    fn rainbow_hue_stays_in_range() {
        let rainbow = themes()
            .into_iter()
            .find(|t| t.cell_style == CellStyle::Rainbow)
            .unwrap();
        for (x, y) in [(0, 0), (119, 79), (-500, 40), (i64::MAX / 4, -3)] {
            let color = rainbow.cell_color(x, y);
            assert!(
                (0.0..=1.0).contains(&color.h),
                "hue {} out of range",
                color.h
            );
        }
    }
}
