mod camera;
mod rule;
mod stack;
mod theme;
mod world;

use std::cell::Cell as SharedSlot;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, size, App, Application, Bounds, Context,
    FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, SharedString, Timer, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use camera::Camera;
use rule::{builtin_rules, BsRule, NamedRule};
use stack::{EditCommand, StackEditor, StackEntry};
use theme::{themes, Theme};
use world::World;

const TICK: Duration = Duration::from_millis(80);

const DURATION_PRESETS: [u32; 9] = [1, 2, 5, 10, 25, 50, 100, 250, 500];

fn next_preset(current: u32) -> u32 {
    DURATION_PRESETS
        .iter()
        .copied()
        .find(|&p| p > current)
        .unwrap_or(current)
}

fn prev_preset(current: u32) -> u32 {
    DURATION_PRESETS
        .iter()
        .rev()
        .copied()
        .find(|&p| p < current)
        .unwrap_or(current)
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

struct SimulatorView {
    world: World,
    camera: Camera,
    rules: Vec<NamedRule>,
    rule_index: usize,
    themes: Vec<Theme>,
    theme_index: usize,
    editor: StackEditor,
    stack_enabled: bool,
    running: bool,
    // While the mouse is held on the canvas, the cell value being painted:
    // alive if the stroke started on a dead cell, dead otherwise.
    painting: Option<bool>,
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
            Timer::after(TICK).await;
            let alive = this.update(cx, |this, cx| {
                if this.running {
                    this.world.step(this.current_rule());
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();

        Self {
            world,
            camera: Camera::default(),
            rules: builtin_rules(),
            rule_index: 0,
            themes: themes(),
            theme_index: 0,
            editor: StackEditor::default(),
            stack_enabled: false,
            running: true,
            painting: None,
            pan_last: None,
            focus_handle,
            canvas_bounds: Rc::new(SharedSlot::new(Bounds::default())),
        }
    }

    fn theme(&self) -> Theme {
        self.themes[self.theme_index]
    }

    fn active_stack_entry(&self) -> Option<(usize, &StackEntry)> {
        if !self.stack_enabled {
            return None;
        }
        self.editor.stack.entry_at(self.world.generation)
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

    /// Randomize the cells currently on screen (capped so an extreme
    /// zoom-out doesn't allocate millions of cells).
    fn randomize_visible(&mut self) {
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
        self.world
            .randomize_region(min_x, min_y, max_x, max_y, 0.2, &mut rand::rng());
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
            "n" => self.world.step(self.current_rule()),
            "r" => self.randomize_visible(),
            "c" => self.world.clear(),
            "[" => {
                self.rule_index = (self.rule_index + self.rules.len() - 1) % self.rules.len();
            }
            "]" => self.rule_index = (self.rule_index + 1) % self.rules.len(),
            "t" => self.theme_index = (self.theme_index + 1) % self.themes.len(),
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
        let value = !self.world.get(x, y);
        self.painting = Some(value);
        self.world.set(x, y, value);
        cx.notify();
    }

    fn continue_paint(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        match event.pressed_button {
            Some(MouseButton::Left) => {
                let Some(value) = self.painting else { return };
                let (x, y) = self.cell_at(event.position);
                if self.world.get(x, y) != value {
                    self.world.set(x, y, value);
                    cx.notify();
                }
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
                generations: 50,
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
                    .child(format!("{generations} gen")),
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
}

impl Render for SimulatorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let camera = self.camera;
        let bounds_slot = self.canvas_bounds.clone();

        // Visible-region query: only cells on screen are collected and drawn.
        let (w, h) = self.canvas_size();
        let (min_x, min_y, max_x, max_y) = camera.visible_world_rect(w, h);
        let live = self.world.live_cells_in(min_x, min_y, max_x, max_y);

        let rule_label = match self.active_stack_entry() {
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
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event, _, cx| this.on_key(event, cx)))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_sm()
                    .text_color(rgb(t.text))
                    .child(if self.running {
                        "▶ running"
                    } else {
                        "⏸ paused"
                    })
                    .child(format!("gen {}", self.world.generation))
                    .child(format!("pop {}", self.world.population()))
                    .child(format!("chunks {}", self.world.chunk_count()))
                    .child(format!("{:.1}×", self.camera.zoom))
                    .child(rule_label)
                    .child(div().flex_1().text_right().child(
                        "space pause · n step · r randomize · c clear · scroll zoom · middle-drag pan · arrows pan · 0 reset · t theme",
                    ))
                    .child(
                        div().flex().items_center().gap_1().children(
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
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .m_2()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event, _, cx| this.begin_paint(event, cx)),
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
                                cx.listener(|this, _, _, _| this.painting = None),
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
                                        for (x, y) in live {
                                            let (sx, sy) = camera.world_to_screen(
                                                x as f64, y as f64, w, h,
                                            );
                                            let cell_bounds = Bounds {
                                                origin: point(
                                                    bounds.origin.x + px(sx as f32),
                                                    bounds.origin.y + px(sy as f32),
                                                ),
                                                size: size(px(cell - gap), px(cell - gap)),
                                            };
                                            window.paint_quad(fill(
                                                cell_bounds,
                                                t.cell_color(x, y),
                                            ));
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
                            .child(self.render_rules_panel(cx))
                            .child(self.render_stack_panel(cx)),
                    ),
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
