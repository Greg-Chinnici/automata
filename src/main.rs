mod life;
mod rule;
mod stack;

use std::cell::Cell as SharedSlot;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, size, App, Application, Bounds, Context,
    FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    SharedString, Timer, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use life::LifeGrid;
use rule::{builtin_rules, BsRule, NamedRule};
use stack::{EditCommand, StackEditor, StackEntry};

const GRID_WIDTH: usize = 120;
const GRID_HEIGHT: usize = 80;
const TICK: Duration = Duration::from_millis(80);

const BG: u32 = 0x14141a;
const CANVAS_BG: u32 = 0x0b0b10;
const PANEL_BG: u32 = 0x101016;
const CHIP_BG: u32 = 0x1d1d26;
const CELL_COLOR: u32 = 0x5be37d;
const TEXT_COLOR: u32 = 0x9aa0b0;
const TEXT_DIM: u32 = 0x565c68;

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

/// Fit a `cols` x `rows` grid inside `bounds`, centered. Returns the grid's
/// top-left corner and the cell edge length in pixels.
fn cell_geometry(bounds: Bounds<Pixels>, cols: usize, rows: usize) -> (Point<Pixels>, f32) {
    let cell = (f32::from(bounds.size.width) / cols as f32)
        .min(f32::from(bounds.size.height) / rows as f32);
    let origin = point(
        bounds.origin.x + px((f32::from(bounds.size.width) - cell * cols as f32) / 2.),
        bounds.origin.y + px((f32::from(bounds.size.height) - cell * rows as f32) / 2.),
    );
    (origin, cell)
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
struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(rgb(CHIP_BG))
            .rounded_sm()
            .text_sm()
            .text_color(rgb(CELL_COLOR))
            .child(self.0.clone())
    }
}

struct SimulatorView {
    grid: LifeGrid,
    rules: Vec<NamedRule>,
    rule_index: usize,
    editor: StackEditor,
    stack_enabled: bool,
    running: bool,
    // While the mouse is held on the canvas, the cell value being painted:
    // alive if the stroke started on a dead cell, dead otherwise.
    painting: Option<bool>,
    focus_handle: FocusHandle,
    // Written by the canvas prepaint closure each frame, read by the mouse
    // handlers to map window coordinates back to grid cells.
    canvas_bounds: Rc<SharedSlot<Bounds<Pixels>>>,
}

impl SimulatorView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut grid = LifeGrid::new(GRID_WIDTH, GRID_HEIGHT);
        grid.randomize(0.2, &mut rand::rng());

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        cx.spawn(async move |this, cx| loop {
            Timer::after(TICK).await;
            let alive = this.update(cx, |this, cx| {
                if this.running {
                    this.grid.step(this.current_rule());
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();

        Self {
            grid,
            rules: builtin_rules(),
            rule_index: 0,
            editor: StackEditor::default(),
            stack_enabled: false,
            running: true,
            painting: None,
            focus_handle,
            canvas_bounds: Rc::new(SharedSlot::new(Bounds::default())),
        }
    }

    fn active_stack_entry(&self) -> Option<(usize, &StackEntry)> {
        if !self.stack_enabled {
            return None;
        }
        self.editor.stack.entry_at(self.grid.generation)
    }

    fn current_rule(&self) -> BsRule {
        self.active_stack_entry()
            .map(|(_, entry)| entry.rule)
            .unwrap_or(self.rules[self.rule_index].rule)
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
        match keystroke.key.as_str() {
            "space" => self.running = !self.running,
            "n" => self.grid.step(self.current_rule()),
            "r" => self.grid.randomize(0.2, &mut rand::rng()),
            "c" => self.grid.clear(),
            "[" => {
                self.rule_index = (self.rule_index + self.rules.len() - 1) % self.rules.len();
            }
            "]" => self.rule_index = (self.rule_index + 1) % self.rules.len(),
            _ => return,
        }
        cx.notify();
    }

    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let (origin, cell) =
            cell_geometry(self.canvas_bounds.get(), self.grid.width, self.grid.height);
        let x = (f32::from(position.x - origin.x) / cell).floor();
        let y = (f32::from(position.y - origin.y) / cell).floor();
        (x >= 0. && y >= 0. && (x as usize) < self.grid.width && (y as usize) < self.grid.height)
            .then_some((x as usize, y as usize))
    }

    fn begin_paint(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some((x, y)) = self.cell_at(event.position) {
            let value = !self.grid.get(x, y);
            self.painting = Some(value);
            self.grid.set(x, y, value);
            cx.notify();
        }
    }

    fn continue_paint(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(value) = self.painting else { return };
        if let Some((x, y)) = self.cell_at(event.position) {
            if self.grid.get(x, y) != value {
                self.grid.set(x, y, value);
                cx.notify();
            }
        }
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

    fn render_stack_entry(
        &self,
        index: usize,
        entry: &StackEntry,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let generations = entry.generations;
        let removed = entry.clone();
        let button = |id: SharedString, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(TEXT_COLOR))
                .hover(|el| el.bg(rgb(CHIP_BG)).text_color(rgb(CELL_COLOR)))
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
            .bg(rgb(CHIP_BG))
            .border_1()
            .border_color(rgb(if active { CELL_COLOR } else { CHIP_BG }))
            .cursor_move()
            .on_drag(DraggedEntry { index }, {
                let name = SharedString::from(entry.name.clone());
                move |_, _, _, cx| cx.new(|_| DragPreview(name.clone()))
            })
            .drag_over::<DraggedRule>(|style, _, _, _| style.bg(rgb(0x232331)))
            .drag_over::<DraggedEntry>(|style, _, _, _| style.bg(rgb(0x232331)))
            .on_drop(cx.listener(move |this, dragged: &DraggedRule, _, cx| {
                this.insert_rule(index, dragged, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedEntry, _, cx| {
                this.move_entry(dragged.index, index, cx);
            }))
            .child(div().text_color(rgb(TEXT_DIM)).child("⠿"))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(TEXT_COLOR))
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
                    .text_color(rgb(CELL_COLOR))
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
        let active_index = self.active_stack_entry().map(|(index, _)| index);
        let entry_count = self.editor.stack.entries.len();

        let header_button = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_color(rgb(if enabled { TEXT_COLOR } else { TEXT_DIM }))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|el| el.bg(rgb(CHIP_BG)).text_color(rgb(CELL_COLOR)))
                })
                .child(label)
        };

        div()
            .w(px(280.))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .m_2()
            .ml_0()
            .rounded_md()
            .bg(rgb(PANEL_BG))
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_color(rgb(TEXT_COLOR)).child("Rule Stack"))
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
                            .bg(rgb(CHIP_BG))
                            .text_color(rgb(if self.stack_enabled { CELL_COLOR } else { TEXT_DIM }))
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
                    .border_color(rgb(TEXT_DIM))
                    .text_color(rgb(TEXT_DIM))
                    .drag_over::<DraggedRule>(|style, _, _, _| style.border_color(rgb(CELL_COLOR)))
                    .drag_over::<DraggedEntry>(|style, _, _, _| style.border_color(rgb(CELL_COLOR)))
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
        let (cols, rows) = (self.grid.width, self.grid.height);
        let mut live = Vec::new();
        for y in 0..rows {
            for x in 0..cols {
                if self.grid.get(x, y) {
                    live.push((x, y));
                }
            }
        }
        let population = live.len();
        let bounds_slot = self.canvas_bounds.clone();

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
            .bg(rgb(BG))
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
                    .text_color(rgb(TEXT_COLOR))
                    .child(if self.running {
                        "▶ running"
                    } else {
                        "⏸ paused"
                    })
                    .child(format!("gen {}", self.grid.generation))
                    .child(format!("pop {population}"))
                    .child(rule_label)
                    .child(div().flex_1().text_right().child(
                        "space pause · n step · r randomize · c clear · [ ] rule · drag paint · ⌘Z undo",
                    )),
            )
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .text_sm()
                    .children(self.rules.iter().enumerate().map(|(index, named)| {
                        let selected = index == self.rule_index && self.active_stack_entry().is_none();
                        let dragged = DraggedRule {
                            name: named.name,
                            rule: named.rule,
                        };
                        div()
                            .id(SharedString::from(named.name))
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(if selected { CELL_COLOR } else { TEXT_COLOR }))
                            .when(selected, |el| el.bg(rgb(CANVAS_BG)))
                            .hover(|el| el.bg(rgb(CANVAS_BG)))
                            .child(named.name)
                            .on_drag(dragged, {
                                let name = SharedString::from(named.name);
                                move |_, _, _, cx| cx.new(|_| DragPreview(name.clone()))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rule_index = index;
                                cx.notify();
                            }))
                    })),
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
                            .on_mouse_move(
                                cx.listener(|this, event, _, cx| this.continue_paint(event, cx)),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.painting = None),
                            )
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds_slot.set(bounds),
                                    move |bounds, _, window, _| {
                                        window.paint_quad(fill(bounds, rgb(CANVAS_BG)));
                                        let (origin, cell) = cell_geometry(bounds, cols, rows);
                                        let gap = if cell >= 3. { 1. } else { 0. };
                                        for (x, y) in live {
                                            let cell_bounds = Bounds {
                                                origin: point(
                                                    origin.x + px(x as f32 * cell),
                                                    origin.y + px(y as f32 * cell),
                                                ),
                                                size: size(px(cell - gap), px(cell - gap)),
                                            };
                                            window.paint_quad(fill(cell_bounds, rgb(CELL_COLOR)));
                                        }
                                    },
                                )
                                .size_full(),
                            ),
                    )
                    .child(self.render_stack_panel(cx)),
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
