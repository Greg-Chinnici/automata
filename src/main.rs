mod camera;
mod material;
mod physics;
mod rule;
mod stack;
mod theme;
mod world;

use std::cell::Cell as SharedSlot;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, size, App, Application, Bounds, Context,
    ExternalPaths, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Timer, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};

use camera::Camera;
use material::{builtin_materials, Material, MaterialId, EMPTY, SAND, STONE, WATER};
use physics::PhysicsWorld;
use rule::{builtin_rules, BsRule, NamedRule};
use stack::{EditCommand, StackEditor, StackEntry};
use theme::{themes, Theme};
use world::World;

const DEFAULT_TICK_MS: u64 = 80;
const MIN_TICK_MS: u64 = 10;
const MAX_TICK_MS: u64 = 1280;

const DURATION_PRESETS: [u32; 9] = [1, 2, 5, 10, 25, 50, 100, 250, 500];

/// Step up through the presets; past the largest comes `None` (infinite).
fn next_preset(current: Option<u32>) -> Option<u32> {
    let current = current?;
    DURATION_PRESETS.iter().copied().find(|&p| p > current)
}

/// Step down through the presets; from infinite, back to the largest.
fn prev_preset(current: Option<u32>) -> Option<u32> {
    match current {
        None => DURATION_PRESETS.last().copied(),
        Some(current) => Some(
            DURATION_PRESETS
                .iter()
                .rev()
                .copied()
                .find(|&p| p < current)
                .unwrap_or(current),
        ),
    }
}

/// Which simulation is active. Both worlds are kept alive; switching modes
/// just changes which one steps, paints, and renders.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SimMode {
    CellularAutomaton,
    Physics,
}

/// Payload for dragging a rule out of the library into the stack.
#[derive(Clone)]
struct DraggedRule {
    name: &'static str,
    rule: BsRule,
}

/// Payload for dragging a stack entry to reorder it.
#[derive(Clone)]
struct DraggedEntry {
    index: usize,
}

/// The floating chip rendered under the cursor while dragging.
struct DragPreview {
    label: SharedString,
    theme: Theme,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(rgb(self.theme.chip_bg))
            .rounded_sm()
            .text_sm()
            .text_color(rgb(self.theme.accent))
            .child(self.label.clone())
    }
}

/// Starter physics scene: a stone basin holding a sand mound and a pool of
/// water, so the mode is immediately alive when switched to.
fn default_physics_scene() -> PhysicsWorld {
    let mut world = PhysicsWorld::new();
    for x in -60..=60 {
        world.set(x, 42, STONE);
    }
    for y in 5..=42 {
        world.set(-60, y, STONE);
        world.set(60, y, STONE);
    }
    let mut rng = rand::rng();
    world.sprinkle((-45, 8, -15, 30), SAND, 0.5, &mut rng);
    world.sprinkle((10, 15, 50, 38), WATER, 0.7, &mut rng);
    world
}

struct SimulatorView {
    mode: SimMode,
    world: World,
    physics: PhysicsWorld,
    // World state captured by the save button; restore rolls back to it.
    // One slot per mode so switching modes doesn't clobber the other's save.
    snapshot: Option<World>,
    physics_snapshot: Option<PhysicsWorld>,
    materials: Vec<Material>,
    // The material painted by left-click/drag in physics mode.
    selected_material: MaterialId,
    camera: Camera,
    rules: Vec<NamedRule>,
    rule_index: usize,
    themes: Vec<Theme>,
    theme_index: usize,
    editor: StackEditor,
    stack_enabled: bool,
    // Generation at which the stack (re)started; replay resets it to "now".
    stack_origin: u64,
    running: bool,
    // Simulation pacing and health metrics.
    tick_ms: u64,
    // Exponential moving averages so the readouts don't flicker.
    step_ms_ema: f64,
    rate_ema: f64,
    last_step: Option<Instant>,
    pop_delta: i64,
    // When true, dropped images stamp bright pixels as live cells instead
    // of dark ones.
    invert_import: bool,
    // While the mouse is held on the canvas, the cell value being painted:
    // alive if the stroke started on a dead cell, dead otherwise.
    painting: Option<bool>,
    // Physics-mode equivalent: the material being stroked (Empty = erasing).
    paint_material: Option<MaterialId>,
    // Last mouse position while panning with the middle button.
    pan_last: Option<Point<Pixels>>,
    focus_handle: FocusHandle,
    // Written by the canvas prepaint closure each frame, read by the mouse
    // handlers to map window coordinates back to world cells.
    canvas_bounds: Rc<SharedSlot<Bounds<Pixels>>>,
}

impl SimulatorView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut world = World::new();
        world.randomize_region(-70, -45, 70, 45, 0.2, &mut rand::rng());

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        cx.spawn(async move |this, cx| loop {
            let Ok(tick_ms) = this.update(cx, |this, cx| {
                if this.running {
                    this.step_timed(cx);
                }
                this.tick_ms
            }) else {
                break;
            };
            Timer::after(Duration::from_millis(tick_ms)).await;
        })
        .detach();

        Self {
            mode: SimMode::CellularAutomaton,
            world,
            physics: default_physics_scene(),
            snapshot: None,
            physics_snapshot: None,
            materials: builtin_materials(),
            selected_material: SAND,
            camera: Camera::default(),
            rules: builtin_rules(),
            rule_index: 0,
            themes: themes(),
            theme_index: 0,
            editor: StackEditor::default(),
            stack_enabled: false,
            stack_origin: 0,
            running: true,
            tick_ms: DEFAULT_TICK_MS,
            step_ms_ema: 0.,
            rate_ema: 0.,
            last_step: None,
            pop_delta: 0,
            invert_import: false,
            painting: None,
            paint_material: None,
            pan_last: None,
            focus_handle,
            canvas_bounds: Rc::new(SharedSlot::new(Bounds::default())),
        }
    }

    fn theme(&self) -> Theme {
        self.themes[self.theme_index]
    }

    fn active_generation(&self) -> u64 {
        match self.mode {
            SimMode::CellularAutomaton => self.world.generation,
            SimMode::Physics => self.physics.generation,
        }
    }

    fn active_population(&self) -> u64 {
        match self.mode {
            SimMode::CellularAutomaton => self.world.population(),
            SimMode::Physics => self.physics.population(),
        }
    }

    fn active_chunk_count(&self) -> usize {
        match self.mode {
            SimMode::CellularAutomaton => self.world.chunk_count(),
            SimMode::Physics => self.physics.chunk_count(),
        }
    }

    fn active_stack_entry(&self) -> Option<(usize, &StackEntry)> {
        if !self.stack_enabled {
            return None;
        }
        // saturating_sub: restore/clear can move the generation below the
        // origin; treat that as the stack having just started.
        self.editor
            .stack
            .entry_at(self.world.generation.saturating_sub(self.stack_origin))
    }

    /// Restart the stack sequence from its first entry, as of now.
    fn replay_stack(&mut self) {
        self.stack_origin = self.world.generation;
        self.stack_enabled = true;
    }

    fn current_rule(&self) -> BsRule {
        self.active_stack_entry()
            .map(|(_, entry)| entry.rule)
            .unwrap_or(self.rules[self.rule_index].rule)
    }

    fn canvas_size(&self) -> (f64, f64) {
        let bounds = self.canvas_bounds.get();
        (
            f32::from(bounds.size.width) as f64,
            f32::from(bounds.size.height) as f64,
        )
    }

    /// Visible world rectangle, capped so an extreme zoom-out doesn't touch
    /// millions of cells.
    fn visible_rect_capped(&self) -> (i64, i64, i64, i64) {
        let (w, h) = self.canvas_size();
        let (mut min_x, mut min_y, mut max_x, mut max_y) = self.camera.visible_world_rect(w, h);
        const MAX_SPAN: i64 = 600;
        if max_x - min_x > MAX_SPAN {
            let cx = (min_x + max_x) / 2;
            min_x = cx - MAX_SPAN / 2;
            max_x = cx + MAX_SPAN / 2;
        }
        if max_y - min_y > MAX_SPAN {
            let cy = (min_y + max_y) / 2;
            min_y = cy - MAX_SPAN / 2;
            max_y = cy + MAX_SPAN / 2;
        }
        (min_x, min_y, max_x, max_y)
    }

    /// Randomize the cells currently on screen: random live cells in CA
    /// mode, a sprinkle of the selected material in physics mode.
    fn randomize_visible(&mut self) {
        let (min_x, min_y, max_x, max_y) = self.visible_rect_capped();
        match self.mode {
            SimMode::CellularAutomaton => {
                self.world
                    .randomize_region(min_x, min_y, max_x, max_y, 0.2, &mut rand::rng());
            }
            SimMode::Physics => {
                let material = if self.selected_material == EMPTY {
                    SAND
                } else {
                    self.selected_material
                };
                self.physics.sprinkle(
                    (min_x, min_y, max_x, max_y),
                    material,
                    0.2,
                    &mut rand::rng(),
                );
            }
        }
    }

    /// Step the world once, updating the health metrics shown in the top bar.
    fn step_timed(&mut self, cx: &mut Context<Self>) {
        let ema = |old: f64, new: f64| {
            if old == 0. {
                new
            } else {
                old * 0.8 + new * 0.2
            }
        };
        if let Some(last) = self.last_step {
            self.rate_ema = ema(self.rate_ema, 1. / last.elapsed().as_secs_f64().max(1e-6));
        }
        self.last_step = Some(Instant::now());

        let population_before = self.active_population() as i64;
        let start = Instant::now();
        match self.mode {
            SimMode::CellularAutomaton => self.world.step(self.current_rule()),
            SimMode::Physics => self.physics.step(&self.materials, &mut rand::rng()),
        }
        self.step_ms_ema = ema(self.step_ms_ema, start.elapsed().as_secs_f64() * 1000.);
        self.pop_delta = self.active_population() as i64 - population_before;
        cx.notify();
    }

    /// Stamp dropped image files into the world, centered on the current
    /// view. Naive luminance threshold: dark opaque pixels become live
    /// cells (or bright ones with `invert_import`); transparent pixels
    /// always stay dead.
    fn drop_images(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        const MAX_DIM: u32 = 256;
        let mut placed = false;
        for path in paths.paths() {
            let img = match image::open(path) {
                Ok(img) => img,
                Err(err) => {
                    eprintln!("could not load {}: {err}", path.display());
                    continue;
                }
            };
            let img = img.thumbnail(MAX_DIM, MAX_DIM).to_luma_alpha8();
            let (w, h) = img.dimensions();
            let origin_x = self.camera.center_x.round() as i64 - w as i64 / 2;
            let origin_y = self.camera.center_y.round() as i64 - h as i64 / 2;
            // In physics mode, stamp with the selected material (stone when
            // the eraser is selected, so a drop always leaves something).
            let stamp = if self.selected_material == EMPTY {
                STONE
            } else {
                self.selected_material
            };
            for (px_x, px_y, pixel) in img.enumerate_pixels() {
                let [luma, alpha] = pixel.0;
                if alpha >= 128 && ((luma < 128) != self.invert_import) {
                    let (x, y) = (origin_x + px_x as i64, origin_y + px_y as i64);
                    match self.mode {
                        SimMode::CellularAutomaton => self.world.set(x, y, true),
                        SimMode::Physics => self.physics.set(x, y, stamp),
                    }
                }
            }
            placed = true;
        }
        if placed {
            cx.notify();
        }
    }

    fn save_snapshot(&mut self) {
        match self.mode {
            SimMode::CellularAutomaton => self.snapshot = Some(self.world.clone()),
            SimMode::Physics => self.physics_snapshot = Some(self.physics.clone()),
        }
    }

    fn restore_snapshot(&mut self) -> bool {
        match self.mode {
            SimMode::CellularAutomaton => {
                if let Some(snapshot) = &self.snapshot {
                    self.world = snapshot.clone();
                    return true;
                }
            }
            SimMode::Physics => {
                if let Some(snapshot) = &self.physics_snapshot {
                    self.physics = snapshot.clone();
                    return true;
                }
            }
        }
        false
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            let changed = match (keystroke.key.as_str(), keystroke.modifiers.shift) {
                ("z", false) => self.editor.undo(),
                ("z", true) => self.editor.redo(),
                _ => false,
            };
            if changed {
                cx.notify();
            }
            return;
        }
        let (w, h) = self.canvas_size();
        const PAN_STEP: f64 = 60.;
        match keystroke.key.as_str() {
            "space" => self.running = !self.running,
            "n" => self.step_timed(cx),
            "," => self.tick_ms = (self.tick_ms * 2).min(MAX_TICK_MS),
            "." => self.tick_ms = (self.tick_ms / 2).max(MIN_TICK_MS),
            "r" => self.randomize_visible(),
            "c" => match self.mode {
                SimMode::CellularAutomaton => {
                    self.world.clear();
                    self.stack_origin = 0;
                }
                SimMode::Physics => self.physics.clear(),
            },
            "m" => {
                self.mode = match self.mode {
                    SimMode::CellularAutomaton => SimMode::Physics,
                    SimMode::Physics => SimMode::CellularAutomaton,
                };
            }
            "s" => self.save_snapshot(),
            "b" => {
                if !self.restore_snapshot() {
                    return;
                }
            }
            "[" => match self.mode {
                SimMode::CellularAutomaton => {
                    self.rule_index = (self.rule_index + self.rules.len() - 1) % self.rules.len();
                }
                SimMode::Physics => {
                    let len = self.materials.len();
                    self.selected_material =
                        ((self.selected_material as usize + len - 1) % len) as MaterialId;
                }
            },
            "]" => match self.mode {
                SimMode::CellularAutomaton => {
                    self.rule_index = (self.rule_index + 1) % self.rules.len();
                }
                SimMode::Physics => {
                    self.selected_material =
                        ((self.selected_material as usize + 1) % self.materials.len())
                            as MaterialId;
                }
            },
            "t" => self.theme_index = (self.theme_index + 1) % self.themes.len(),
            "i" => self.invert_import = !self.invert_import,
            "left" => self.camera.pan_pixels(-PAN_STEP, 0.),
            "right" => self.camera.pan_pixels(PAN_STEP, 0.),
            "up" => self.camera.pan_pixels(0., -PAN_STEP),
            "down" => self.camera.pan_pixels(0., PAN_STEP),
            "-" => self.camera.zoom_by(1. / 1.25, w / 2., h / 2., w, h),
            "=" => self.camera.zoom_by(1.25, w / 2., h / 2., w, h),
            "0" => self.camera = Camera::default(),
            _ => return,
        }
        cx.notify();
    }

    fn cell_at(&self, position: Point<Pixels>) -> (i64, i64) {
        let bounds = self.canvas_bounds.get();
        let (w, h) = self.canvas_size();
        let sx = f32::from(position.x - bounds.origin.x) as f64;
        let sy = f32::from(position.y - bounds.origin.y) as f64;
        let (wx, wy) = self.camera.screen_to_world(sx, sy, w, h);
        (wx.floor() as i64, wy.floor() as i64)
    }

    fn begin_paint(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let (x, y) = self.cell_at(event.position);
        match self.mode {
            SimMode::CellularAutomaton => {
                let value = !self.world.get(x, y);
                self.painting = Some(value);
                self.world.set(x, y, value);
            }
            SimMode::Physics => {
                self.paint_material = Some(self.selected_material);
                self.physics.set(x, y, self.selected_material);
            }
        }
        cx.notify();
    }

    /// Right-button stroke: always erases, in either mode.
    fn begin_erase(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let (x, y) = self.cell_at(event.position);
        match self.mode {
            SimMode::CellularAutomaton => {
                self.painting = Some(false);
                self.world.set(x, y, false);
            }
            SimMode::Physics => {
                self.paint_material = Some(EMPTY);
                self.physics.set(x, y, EMPTY);
            }
        }
        cx.notify();
    }

    fn apply_stroke(&mut self, x: i64, y: i64, cx: &mut Context<Self>) {
        match self.mode {
            SimMode::CellularAutomaton => {
                let Some(value) = self.painting else { return };
                if self.world.get(x, y) != value {
                    self.world.set(x, y, value);
                    cx.notify();
                }
            }
            SimMode::Physics => {
                let Some(material) = self.paint_material else {
                    return;
                };
                if self.physics.get(x, y) != material {
                    self.physics.set(x, y, material);
                    cx.notify();
                }
            }
        }
    }

    fn continue_paint(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        match event.pressed_button {
            Some(MouseButton::Left) | Some(MouseButton::Right) => {
                let (x, y) = self.cell_at(event.position);
                self.apply_stroke(x, y, cx);
            }
            Some(MouseButton::Middle) => {
                if let Some(last) = self.pan_last {
                    let dx = f32::from(last.x - event.position.x) as f64;
                    let dy = f32::from(last.y - event.position.y) as f64;
                    self.camera.pan_pixels(dx, dy);
                    self.pan_last = Some(event.position);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y) as f64,
            ScrollDelta::Lines(delta) => delta.y as f64 * 24.,
        };
        if dy == 0. {
            return;
        }
        let bounds = self.canvas_bounds.get();
        let (w, h) = self.canvas_size();
        let sx = f32::from(event.position.x - bounds.origin.x) as f64;
        let sy = f32::from(event.position.y - bounds.origin.y) as f64;
        self.camera.zoom_by((dy / 160.).exp2(), sx, sy, w, h);
        cx.notify();
    }

    /// Move a stack entry so it sits where the drop landed, accounting for
    /// the shift caused by removing it first.
    fn move_entry(&mut self, from: usize, drop_index: usize, cx: &mut Context<Self>) {
        let to = if from < drop_index {
            drop_index - 1
        } else {
            drop_index
        };
        if from != to {
            self.editor.apply(EditCommand::Move { from, to });
            cx.notify();
        }
    }

    fn insert_rule(&mut self, index: usize, dragged: &DraggedRule, cx: &mut Context<Self>) {
        self.editor.apply(EditCommand::Add {
            index,
            entry: StackEntry {
                name: dragged.name.to_string(),
                rule: dragged.rule,
                generations: Some(50),
            },
        });
        self.stack_enabled = true;
        cx.notify();
    }

    fn render_rules_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        div()
            .p_2()
            .rounded_md()
            .bg(rgb(t.panel_bg))
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .child(div().text_color(rgb(t.text)).child("Rule Library"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(self.rules.iter().enumerate().map(|(index, named)| {
                        let selected =
                            index == self.rule_index && self.active_stack_entry().is_none();
                        let dragged = DraggedRule {
                            name: named.name,
                            rule: named.rule,
                        };
                        div()
                            .id(SharedString::from(named.name))
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(if selected { t.accent } else { t.text }))
                            .when(selected, |el| el.bg(rgb(t.canvas_bg)))
                            .hover(move |el| el.bg(rgb(t.canvas_bg)))
                            .child(named.name)
                            .on_drag(dragged, {
                                let name = SharedString::from(named.name);
                                move |_, _, _, cx| {
                                    cx.new(|_| DragPreview {
                                        label: name.clone(),
                                        theme: t,
                                    })
                                }
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rule_index = index;
                                cx.notify();
                            }))
                    })),
            )
    }

    fn render_stack_entry(
        &self,
        index: usize,
        entry: &StackEntry,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();
        let generations = entry.generations;
        let removed = entry.clone();
        let button = move |id: SharedString, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(t.text))
                .hover(move |el| el.bg(rgb(t.chip_bg)).text_color(rgb(t.accent)))
                .child(label)
        };

        div()
            .id(SharedString::from(format!("stack-entry-{index}")))
            .px_2()
            .py_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded_sm()
            .bg(rgb(t.chip_bg))
            .border_1()
            .border_color(rgb(if active { t.accent } else { t.chip_bg }))
            .cursor_move()
            .on_drag(DraggedEntry { index }, {
                let name = SharedString::from(entry.name.clone());
                move |_, _, _, cx| {
                    cx.new(|_| DragPreview {
                        label: name.clone(),
                        theme: t,
                    })
                }
            })
            .drag_over::<DraggedRule>(move |style, _, _, _| style.bg(rgb(t.hover_bg)))
            .drag_over::<DraggedEntry>(move |style, _, _, _| style.bg(rgb(t.hover_bg)))
            .on_drop(cx.listener(move |this, dragged: &DraggedRule, _, cx| {
                this.insert_rule(index, dragged, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedEntry, _, cx| {
                this.move_entry(dragged.index, index, cx);
            }))
            .child(div().text_color(rgb(t.text_dim)).child("⠿"))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(t.text))
                    .child(entry.name.clone()),
            )
            .child(
                button(format!("dur-down-{index}").into(), "−").on_click(cx.listener(
                    move |this, _, _, cx| {
                        let new = prev_preset(generations);
                        if new != generations {
                            this.editor.apply(EditCommand::SetGenerations {
                                index,
                                old: generations,
                                new,
                            });
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                div()
                    .min_w(px(44.))
                    .text_center()
                    .text_color(rgb(t.accent))
                    .child(match generations {
                        Some(n) => format!("{n} gen"),
                        None => "∞".to_string(),
                    }),
            )
            .child(
                button(format!("dur-up-{index}").into(), "+").on_click(cx.listener(
                    move |this, _, _, cx| {
                        let new = next_preset(generations);
                        if new != generations {
                            this.editor.apply(EditCommand::SetGenerations {
                                index,
                                old: generations,
                                new,
                            });
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                button(format!("remove-{index}").into(), "×").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.editor.apply(EditCommand::Remove {
                            index,
                            entry: removed.clone(),
                        });
                        cx.notify();
                    },
                )),
            )
    }

    fn render_stack_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let active_index = self.active_stack_entry().map(|(index, _)| index);
        let entry_count = self.editor.stack.entries.len();

        let header_button = move |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_color(rgb(if enabled { t.text } else { t.text_dim }))
                .when(enabled, move |el| {
                    el.cursor_pointer()
                        .hover(move |el| el.bg(rgb(t.chip_bg)).text_color(rgb(t.accent)))
                })
                .child(label)
        };

        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(rgb(t.panel_bg))
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_color(rgb(t.text)).child("Rule Stack"))
                    .child(
                        header_button("stack-replay", "↻", !self.editor.stack.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.editor.stack.is_empty() {
                                    this.replay_stack();
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        header_button("undo", "↩", self.editor.can_undo()).on_click(cx.listener(
                            |this, _, _, cx| {
                                if this.editor.undo() {
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(
                        header_button("redo", "↪", self.editor.can_redo()).on_click(cx.listener(
                            |this, _, _, cx| {
                                if this.editor.redo() {
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(
                        div()
                            .id("stack-toggle")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(t.chip_bg))
                            .text_color(rgb(if self.stack_enabled {
                                t.accent
                            } else {
                                t.text_dim
                            }))
                            .child(if self.stack_enabled { "on" } else { "off" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stack_enabled = !this.stack_enabled;
                                cx.notify();
                            })),
                    ),
            )
            .children(
                self.editor
                    .stack
                    .entries
                    .clone()
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        self.render_stack_entry(index, entry, active_index == Some(index), cx)
                    }),
            )
            .child(
                div()
                    .id("stack-append")
                    .flex_1()
                    .min_h(px(60.))
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_dashed()
                    .border_color(rgb(t.text_dim))
                    .text_color(rgb(t.text_dim))
                    .drag_over::<DraggedRule>(move |style, _, _, _| {
                        style.border_color(rgb(t.accent))
                    })
                    .drag_over::<DraggedEntry>(move |style, _, _, _| {
                        style.border_color(rgb(t.accent))
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedRule, _, cx| {
                        this.insert_rule(entry_count, dragged, cx);
                    }))
                    .on_drop(cx.listener(move |this, dragged: &DraggedEntry, _, cx| {
                        this.move_entry(dragged.index, entry_count, cx);
                    }))
                    .child(if self.editor.stack.is_empty() {
                        "drag rules here from the library — each entry runs for its generation count, then the stack loops"
                    } else {
                        "drop here to append"
                    }),
            )
    }

    fn render_palette_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        div()
            .p_2()
            .rounded_md()
            .bg(rgb(t.panel_bg))
            .flex()
            .flex_col()
            .gap_1()
            .text_sm()
            .child(div().mb_1().text_color(rgb(t.text)).child("Materials"))
            .children(self.materials.iter().enumerate().map(|(index, mat)| {
                let id = index as MaterialId;
                let selected = id == self.selected_material;
                let label = if id == EMPTY {
                    "Eraser".to_string()
                } else {
                    mat.name.clone()
                };
                div()
                    .id(SharedString::from(format!("material-{index}")))
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(if selected { t.accent } else { t.panel_bg }))
                    .when(selected, |el| el.bg(rgb(t.chip_bg)))
                    .hover(move |el| el.bg(rgb(t.hover_bg)))
                    .child(
                        div()
                            .size(px(14.))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(t.text_dim))
                            .bg(rgb(if id == EMPTY { t.canvas_bg } else { mat.color })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(if selected { t.accent } else { t.text }))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_color(rgb(t.text_dim))
                            .child(mat.phase_label()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_material = id;
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_1()
                    .text_color(rgb(t.text_dim))
                    .child("left-drag paints · right-drag erases"),
            )
    }
}

impl Render for SimulatorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let camera = self.camera;
        let bounds_slot = self.canvas_bounds.clone();

        // Visible-region query: only cells on screen are collected and drawn.
        // Colors are resolved here so the paint closure just draws quads.
        let (w, h) = self.canvas_size();
        let (min_x, min_y, max_x, max_y) = camera.visible_world_rect(w, h);
        let cells: Vec<(i64, i64, Hsla)> = match self.mode {
            SimMode::CellularAutomaton => self
                .world
                .live_cells_in(min_x, min_y, max_x, max_y)
                .into_iter()
                .map(|(x, y)| (x, y, t.cell_color(x, y)))
                .collect(),
            SimMode::Physics => self
                .physics
                .cells_in(min_x, min_y, max_x, max_y)
                .into_iter()
                .map(|(x, y, m)| (x, y, rgb(self.materials[m as usize].color).into()))
                .collect(),
        };

        let mode_label = match self.mode {
            SimMode::CellularAutomaton => match self.active_stack_entry() {
                Some((index, entry)) => format!(
                    "stack {}/{}: {} ({})",
                    index + 1,
                    self.editor.stack.entries.len(),
                    entry.name,
                    entry.rule.notation()
                ),
                None => format!(
                    "{} ({})",
                    self.rules[self.rule_index].name,
                    self.current_rule().notation()
                ),
            },
            SimMode::Physics => format!(
                "painting {}",
                self.materials[self.selected_material as usize].name
            ),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event, _, cx| this.on_key(event, cx)))
            .child(
                // Row 1: live metrics. Each stat sits in a fixed-width slot
                // so the row doesn't shift as the numbers change.
                div()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(rgb(t.text))
                    .child(div().min_w(px(110.)).child(format!(
                        "gen {}",
                        self.active_generation()
                    )))
                    .child(div().min_w(px(150.)).child(format!(
                        "pop {} ({:+})",
                        self.active_population(),
                        self.pop_delta
                    )))
                    .child(div().min_w(px(90.)).child(format!(
                        "chunks {}",
                        self.active_chunk_count()
                    )))
                    .child(
                        div()
                            .min_w(px(90.))
                            .child(format!("{:.1} gen/s", self.rate_ema)),
                    )
                    .child(
                        div()
                            .min_w(px(100.))
                            .child(format!("step {:.1}ms", self.step_ms_ema)),
                    )
                    .child({
                        // Health: how much of the tick budget a step consumes.
                        let load = 100. * self.step_ms_ema / self.tick_ms as f64;
                        div()
                            .min_w(px(80.))
                            .text_color(rgb(if load < 70. { t.accent } else { 0xd9534f }))
                            .child(format!("load {load:.0}%"))
                    }),
            )
            .child(
                // Row 2: state and controls that rarely change.
                div()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_sm()
                    .text_color(rgb(t.text))
                    .child(div().min_w(px(70.)).child(if self.running {
                        "▶ running"
                    } else {
                        "⏸ paused"
                    }))
                    .child(
                        div()
                            .id("mode-toggle")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(t.chip_bg))
                            .text_color(rgb(t.accent))
                            .hover(move |el| el.bg(rgb(t.hover_bg)))
                            .child(match self.mode {
                                SimMode::CellularAutomaton => "▦ automata",
                                SimMode::Physics => "⌛ physics",
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mode = match this.mode {
                                    SimMode::CellularAutomaton => SimMode::Physics,
                                    SimMode::Physics => SimMode::CellularAutomaton,
                                };
                                cx.notify();
                            })),
                    )
                    .child(format!("{:.1}×", self.camera.zoom))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("speed-down")
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .bg(rgb(t.chip_bg))
                                    .hover(move |el| el.text_color(rgb(t.accent)))
                                    .child("−")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tick_ms = (this.tick_ms * 2).min(MAX_TICK_MS);
                                        cx.notify();
                                    })),
                            )
                            .child(div().min_w(px(36.)).text_center().child(format!(
                                "{:.0}/s",
                                1000. / self.tick_ms as f64
                            )))
                            .child(
                                div()
                                    .id("speed-up")
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .bg(rgb(t.chip_bg))
                                    .hover(move |el| el.text_color(rgb(t.accent)))
                                    .child("+")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tick_ms = (this.tick_ms / 2).max(MIN_TICK_MS);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("invert-import")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(t.chip_bg))
                            .hover(move |el| el.text_color(rgb(t.accent)))
                            .child(if self.invert_import {
                                "img: light→live"
                            } else {
                                "img: dark→live"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.invert_import = !this.invert_import;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().child(mode_label))
                    .child(
                        div()
                            .id("save-state")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(t.chip_bg))
                            .hover(move |el| el.text_color(rgb(t.accent)))
                            .child("⬇ save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_snapshot();
                                cx.notify();
                            })),
                    )
                    .child({
                        let snapshot_generation = match self.mode {
                            SimMode::CellularAutomaton => {
                                self.snapshot.as_ref().map(|s| s.generation)
                            }
                            SimMode::Physics => {
                                self.physics_snapshot.as_ref().map(|s| s.generation)
                            }
                        };
                        let has_snapshot = snapshot_generation.is_some();
                        let label = match snapshot_generation {
                            Some(generation) => format!("⟲ gen {generation}"),
                            None => "⟲ restore".to_string(),
                        };
                        div()
                            .id("restore-state")
                            .px_2()
                            .rounded_sm()
                            .bg(rgb(t.chip_bg))
                            .text_color(rgb(if has_snapshot { t.text } else { t.text_dim }))
                            .when(has_snapshot, move |el| {
                                el.cursor_pointer()
                                    .hover(move |el| el.text_color(rgb(t.accent)))
                            })
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.restore_snapshot() {
                                    cx.notify();
                                }
                            }))
                    })
                    .child(div().flex().items_center().gap_1().children(
                        self.themes.iter().enumerate().map(|(index, theme)| {
                            let selected = index == self.theme_index;
                            div()
                                .id(SharedString::from(theme.name))
                                .size(px(14.))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(rgb(theme.accent))
                                .border_2()
                                .border_color(rgb(if selected { t.text } else { t.bg }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.theme_index = index;
                                    cx.notify();
                                }))
                        }),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .child(
                        div()
                            .id("sim-canvas")
                            .flex_1()
                            .m_2()
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.drop_images(paths, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event, _, cx| this.begin_paint(event, cx)),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event, _, cx| this.begin_erase(event, cx)),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(|this, event: &MouseDownEvent, _, _| {
                                    this.pan_last = Some(event.position);
                                }),
                            )
                            .on_mouse_move(
                                cx.listener(|this, event, _, cx| this.continue_paint(event, cx)),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| {
                                    this.painting = None;
                                    this.paint_material = None;
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, _, _, _| {
                                    this.painting = None;
                                    this.paint_material = None;
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Middle,
                                cx.listener(|this, _, _, _| this.pan_last = None),
                            )
                            .on_scroll_wheel(
                                cx.listener(|this, event, _, cx| this.on_scroll(event, cx)),
                            )
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds_slot.set(bounds),
                                    move |bounds, _, window, _| {
                                        window.paint_quad(fill(bounds, rgb(t.canvas_bg)));
                                        let w = f32::from(bounds.size.width) as f64;
                                        let h = f32::from(bounds.size.height) as f64;
                                        let cell = camera.zoom as f32;
                                        let gap = if cell >= 3. { 1. } else { 0. };
                                        for (x, y, color) in cells {
                                            let (sx, sy) =
                                                camera.world_to_screen(x as f64, y as f64, w, h);
                                            let cell_bounds = Bounds {
                                                origin: point(
                                                    bounds.origin.x + px(sx as f32),
                                                    bounds.origin.y + px(sy as f32),
                                                ),
                                                size: size(px(cell - gap), px(cell - gap)),
                                            };
                                            window.paint_quad(fill(cell_bounds, color));
                                        }
                                    },
                                )
                                .size_full(),
                            ),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .m_2()
                            .ml_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(self.mode == SimMode::CellularAutomaton, |el| {
                                el.child(self.render_rules_panel(cx))
                                    .child(self.render_stack_panel(cx))
                            })
                            .when(self.mode == SimMode::Physics, |el| {
                                el.child(self.render_palette_panel(cx))
                            }),
                    ),
            )
            .child(
                div()
                    .h(px(26.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(t.text_dim))
                    .child(match self.mode {
                        SimMode::CellularAutomaton => {
                            "space pause · n step · , . speed · m mode · r randomize · c clear · s save · b back · \
                             scroll zoom · middle-drag pan · arrows pan · 0 reset camera · \
                             [ ] rule · t theme · i invert import · drag paint · right-drag erase · ⌘Z undo stack edit"
                        }
                        SimMode::Physics => {
                            "space pause · n step · , . speed · m mode · r sprinkle · c clear · s save · b back · \
                             scroll zoom · middle-drag pan · arrows pan · 0 reset camera · \
                             [ ] material · t theme · drag paint · right-drag erase"
                        }
                    }),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Cellular Automata Explorer".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: true,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| SimulatorView::new(window, cx)),
        )
        .unwrap();
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
