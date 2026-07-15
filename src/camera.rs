pub const MIN_ZOOM: f64 = 1.0;
pub const MAX_ZOOM: f64 = 48.0;

/// Maps between world cell coordinates and canvas-relative pixel
/// coordinates: `screen = (world - center) * zoom + canvas_size / 2`.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub center_x: f64,
    pub center_y: f64,
    /// Pixels per cell.
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center_x: 0.,
            center_y: 0.,
            zoom: 8.,
        }
    }
}

impl Camera {
    pub fn world_to_screen(&self, wx: f64, wy: f64, width: f64, height: f64) -> (f64, f64) {
        (
            (wx - self.center_x) * self.zoom + width / 2.,
            (wy - self.center_y) * self.zoom + height / 2.,
        )
    }

    pub fn screen_to_world(&self, sx: f64, sy: f64, width: f64, height: f64) -> (f64, f64) {
        (
            (sx - width / 2.) / self.zoom + self.center_x,
            (sy - height / 2.) / self.zoom + self.center_y,
        )
    }

    /// Shift the view by a pixel delta (positive = content moves left/up).
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        self.center_x += dx / self.zoom;
        self.center_y += dy / self.zoom;
    }

    /// Zoom by `factor`, keeping the world point under the given canvas
    /// pixel fixed on screen.
    pub fn zoom_by(&mut self, factor: f64, sx: f64, sy: f64, width: f64, height: f64) {
        let (wx, wy) = self.screen_to_world(sx, sy, width, height);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center_x = wx - (sx - width / 2.) / self.zoom;
        self.center_y = wy - (sy - height / 2.) / self.zoom;
    }

    /// Inclusive world-cell rectangle covering the canvas.
    pub fn visible_world_rect(&self, width: f64, height: f64) -> (i64, i64, i64, i64) {
        let (min_x, min_y) = self.screen_to_world(0., 0., width, height);
        let (max_x, max_y) = self.screen_to_world(width, height, width, height);
        (
            min_x.floor() as i64,
            min_y.floor() as i64,
            max_x.ceil() as i64,
            max_y.ceil() as i64,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_screen_round_trip() {
        let camera = Camera {
            center_x: 12.5,
            center_y: -3.,
            zoom: 6.,
        };
        let (sx, sy) = camera.world_to_screen(20., 10., 800., 600.);
        let (wx, wy) = camera.screen_to_world(sx, sy, 800., 600.);
        assert!((wx - 20.).abs() < 1e-9);
        assert!((wy - 10.).abs() < 1e-9);
    }

    #[test]
    fn zoom_keeps_cursor_point_anchored() {
        let mut camera = Camera::default();
        let (cursor_x, cursor_y) = (200., 150.);
        let before = camera.screen_to_world(cursor_x, cursor_y, 800., 600.);
        camera.zoom_by(1.5, cursor_x, cursor_y, 800., 600.);
        let after = camera.screen_to_world(cursor_x, cursor_y, 800., 600.);
        assert!((before.0 - after.0).abs() < 1e-9);
        assert!((before.1 - after.1).abs() < 1e-9);
    }

    #[test]
    fn zoom_clamps_to_limits() {
        let mut camera = Camera::default();
        camera.zoom_by(1000., 0., 0., 800., 600.);
        assert_eq!(camera.zoom, MAX_ZOOM);
        camera.zoom_by(1e-6, 0., 0., 800., 600.);
        assert_eq!(camera.zoom, MIN_ZOOM);
    }

    #[test]
    fn pan_moves_center_in_cells() {
        let mut camera = Camera::default(); // zoom 8
        camera.pan_pixels(80., -40.);
        assert_eq!(camera.center_x, 10.);
        assert_eq!(camera.center_y, -5.);
    }

    #[test]
    fn visible_rect_contains_center_and_scales_with_zoom() {
        let camera = Camera {
            center_x: 100.,
            center_y: 100.,
            zoom: 10.,
        };
        let (min_x, min_y, max_x, max_y) = camera.visible_world_rect(800., 600.);
        assert!(min_x <= 100 && 100 <= max_x);
        assert!(min_y <= 100 && 100 <= max_y);
        assert_eq!(max_x - min_x, 80); // 800px / 10px-per-cell
        assert_eq!(max_y - min_y, 60);
    }
}
