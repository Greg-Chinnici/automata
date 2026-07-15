# Cellular Automata Explorer — Rust + GPUI Project Plan (v2)

## Overview

A high-performance cellular automata sandbox built in Rust using GPUI.

Goals:

* Support Conway's Game of Life and thousands of rule variations
* Allow changing rules during simulation
* Support a near-infinite zoomable world
* Efficiently render extremely large simulations
* Allow experimentation with custom rules, neighborhoods, and cell states
* Provide a **no-code rule stack editor** with full undo/redo for authoring rules

---

# Development Environment

**All builds, tests, and tooling run inside a Nix flake dev shell.** Do not rely on
system-installed Rust, cargo, or any other tool — everything must be pinned and
reproducible via the flake.

```
project-root/
├── flake.nix
├── flake.lock
└── ...
```

`flake.nix` should provide:

* Pinned Rust toolchain (via `rust-overlay` or `fenix`) matching the project's `rust-toolchain.toml`
* `cargo`, `rustc`, `clippy`, `rustfmt`
* System libraries GPUI needs (graphics/windowing deps — confirm exact list once GPUI is vendored, e.g. Vulkan/Metal loaders, `pkg-config`, `fontconfig`, `freetype`)
* `rust-analyzer` for editor support
* Optional: `cargo-watch`, `cargo-nextest` for fast test iteration

Enter the shell with:

```
nix develop
```

All `cargo build`, `cargo test`, `cargo run`, and `cargo clippy` commands in this
project assume they are run **inside** `nix develop` (or via `nix develop -c <cmd>`
in CI/scripts). No instructions in this plan should be interpreted as running
outside that shell.

CI should invoke the same flake shell rather than a separate toolchain setup, so
local and CI builds stay identical.

---

# Technology Stack

## Language
Rust

## UI Framework
GPUI — window management, UI controls, input handling, rendering coordination.

## Rendering
* Initial: GPUI rendering primitives
* Future: GPU-accelerated pipeline if profiling shows the need

## Data Structures
* Sparse spatial storage, chunk-based world representation
* Spatial tree (quadtree) for visibility management
* Bitpacked cell storage for binary (2-state) automata

---

# High-Level Architecture

```
Application
|
├── GPUI Layer
|   ├── Controls
|   ├── Camera
|   └── Renderer
|
├── Simulation Engine
|   ├── World
|   ├── Rule System (built-in + no-code stack)
|   ├── Generation Manager
|   └── Timeline
|
├── Rule Editor
|   ├── Rule Stack (data model)
|   ├── Edit Command History (undo/redo)
|   └── Compiler (stack → fast execution form)
|
└── Spatial Storage
    ├── Chunk Manager
    ├── Spatial Tree
    └── Cell Storage
```

---

# Core Concepts

## World
The world is effectively infinite. Do not store empty space — only chunks
containing live/non-default cells are allocated.

## Chunk System
The world is divided into chunks, each tracking position, cell data, active
cell count, and a dirty flag.

```rust
struct Chunk {
    position: ChunkPosition,
    cells: CellStorage,   // see Cell Storage section
    dirty: bool,
}
```

### Chunk boundary handling
Neighbor counting across chunk edges requires halo (ghost) cell data pulled
from adjacent chunks. Design this in from Phase 3, not as a retrofit — it
shapes both `Chunk` and the update loop.

## Spatial Tree
Quadtree-style structure for:
* Finding visible chunks quickly
* Loading/unloading distant areas
* Supporting zoom
* Improving rendering performance

## Camera System

```rust
struct Camera {
    position: Vec2,
    zoom: f32,
}
```

`screen = (world - camera_position) * zoom`

Controls: pan, zoom, follow patterns, jump to coordinates.

---

# Cell Storage

For binary (2-state) automata, pack cells into bitboards (e.g. `u64` segments
per row) instead of one bool per byte. This cuts memory ~8x and lets neighbor
counting use bit shifts/SIMD instead of scalar loops — this matters once
simulations reach millions of live cells.

```rust
struct Cell {
    state: u8, // multi-state path: Brian's Brain, Wireworld, custom sims
}
```

Keep two storage paths:
1. **Bitpacked** — fast path for 2-state Life-like rules
2. **`state: u8` per cell** — general path for multi-state automata

---

# Simulation Engine

The simulator does not know about rendering.

```
Simulation
|
├── Current World
├── Current Rule
├── Generation Number
└── Step()
```

## Parallel stepping (Rayon)
Update chunks in parallel by reading from the current generation and writing
to a new buffer — never mutate in place, or updates become order-dependent.
Swap buffers after each step.

---

# Rule System

Two ways to define a rule:

1. **Built-in rules** — compiled Rust implementations of common Life-like
   families (B/S notation), for speed.
2. **No-code rule stack** — user-authored rules built from ops in the GUI.

```rust
trait Rule {
    fn name(&self) -> &str;
    fn next_state(&self, cell: Cell, neighbors: NeighborData) -> Cell;
}
```

Avoid calling through `Box<dyn Rule>` in the hot per-cell loop — vtable
dispatch per cell per generation shows up in profiling fast. Reserve `dyn
Rule` for genuinely custom/scripted rules; use an enum + monomorphized path
for built-ins.

Rules can be swapped anytime, including mid-simulation via the Rule Timeline.

---

# No-Code Rule Stack Editor

## Data model

The rule stack is plain, serializable data — not tied to any UI state.

```rust
enum RuleOp {
    CountNeighbors { radius: u8 },
    Threshold { min: u8, max: u8, then_state: u8 },
    SetState(u8),
    Conditional {
        predicate: Box<RuleOp>,
        if_true: Vec<RuleOp>,
        if_false: Vec<RuleOp>,
    },
    Repeat { times: u32, body: Vec<RuleOp> },
}
```

* Fully `serde`-serializable → users save/load/share rule stacks as JSON.
* Start with a **reorderable linear list UI** (drag handles in GPUI), not a
  node-graph editor. Covers "loop X times" / "list ops in a row" with far
  less UI work; revisit a graph editor later if needed.

## Command pattern — for editing, not simulation stepping

Important distinction: **editing the rule stack** and **stepping the
simulation backward** are different problems, and command pattern only
solves the first one cleanly.

* Most CA rules (including Life) are **not invertible** — a dead cell could
  have come from many neighbor configurations, so there is no general
  `invert()` for a simulation step.
* Simulation undo/rewind stays on the existing **Generation snapshot /
  Timeline system** (Phase 5), not on command pattern.

Command pattern undo/redo applies to **rule-authoring actions** in the
editor:

```rust
enum EditCommand {
    AddOp { index: usize, op: RuleOp },
    RemoveOp { index: usize },
    SetParam { path: Vec<usize>, old: Value, new: Value },
    Reorder { from: usize, to: usize },
}
```

Each variant has a natural inverse (add↔remove, reorder↔reorder-back,
set-param stores the old value), giving full undo/redo over rule
construction in the GUI.

## Execution performance

Tree-walking the `RuleOp` list per cell per generation will be slow at scale
— it doesn't vectorize. In order of effort:

1. **Compile-on-change**: only re-interpret the stack when it's edited;
   compile it once into a flat closure/bytecode array, then hot-loop the
   compiled form.
2. **Fast-path detection**: recognize common patterns (threshold-based
   Life-likes) and lower them into the existing bitboard fast path instead
   of generic interpretation.
3. Keep the generic interpreter as a fallback for truly custom stacks only.

---

# Dynamic Rule Changes (Rule Timeline)

```
Rule Timeline
|
+-- generation 0:   B3/S23
+-- generation 100: B36/S23
+-- generation 200: custom stack "predator-prey-v3"
```

Before each step: check timeline → apply active rule (built-in or compiled
stack) → generate next state.

---

# Generation System

```
Generation
|
├── World State (or diff)
├── Rule Used
└── Timestamp
```

Full per-generation snapshots get expensive fast on large worlds. Prefer:
* Store **dirty-chunk diffs** per generation, or
* Log rule-change events + periodic full keyframes, replaying deterministically between them (like video keyframes)

Enables replay, undo, experiment comparison — and is the actual backing
store for simulation-step undo (see Rule Stack Editor section above).

---

# Rendering Pipeline

```
Camera → Visible Region Query → Spatial Tree → Visible Chunks → Draw Cells
```

Only render what is visible.

---

# GPUI Components

```
App
|
└── SimulatorView
      ├── Canvas
      ├── Toolbar
      ├── RuleSelector
      ├── RuleStackEditor   (no-code stack, drag-reorder list, undo/redo)
      ├── Timeline
      └── Statistics
```

Controls: Start/Pause, Step, Speed, Rule selection, Brush tools, Pattern
loading.

## Input System

Mouse: left-click toggle cells · middle-click pan · scroll zoom.

Keyboard:
```
Space     Start/Pause
N         Next generation
R         Randomize
Ctrl+Z    Undo (rule editor)
Ctrl+Shift+Z  Redo (rule editor)
```

---

# Pattern I/O

Support RLE and plaintext (`.cells`) formats for interoperability with the
existing Life pattern community (e.g. LifeWiki libraries).

---

# Testing

Unit tests using known oscillators/spaceships (glider, blinker, pulsar) per
built-in rule, run via `cargo test` (or `cargo nextest`) inside the flake dev
shell, to catch simulation bugs early — especially around chunk-boundary
neighbor counting.

---

# Development Milestones

## Phase 0 — Environment
* Write `flake.nix` / `flake.lock`
* Confirm `nix develop` provides a working GPUI build (spike GPUI's required
  system libs and expose via the flake)

## Phase 1 — Prototype
* GPUI window, fixed grid, Conway Life, basic renderer

## Phase 2 — Rule Engine
* Rule trait, B/S parser, rule switching, built-in rule library

## Phase 3 — Infinite World
* Chunk storage, sparse world, spatial indexing, halo/ghost-cell boundary handling

## Phase 4 — Camera System
* Infinite scrolling, zoom, visible-region rendering

## Phase 5 — History and Experiments
* Generation diffs/keyframes, rule timelines, replay

## Phase 6 — No-Code Rule Stack Editor
* `RuleOp` data model + serde (de)serialization
* Linear reorderable-list UI in GPUI
* `EditCommand` undo/redo stack
* Stack compiler (compile-on-change → flat execution form)
* Fast-path lowering for recognizable Life-like patterns

## Phase 7 — Performance
* Bitpacked cell storage, parallel simulation (Rayon, double-buffered),
  dirty-chunk updates, GPU rendering, large-scale worlds

---

# Initial Rust Crates

* `gpui` — application framework
* `serde` / `serde_json` — rule stack + save/load formats
* `rayon` — parallel simulation
* `glam` — vector math
* `anyhow` — error handling
* `tracing` — debugging

All resolved and built via the Nix flake dev shell — no ad hoc global installs.

---

# Final Design Principle

The engine is built around four independent systems:

1. **World Storage** — where cells exist
2. **Simulation Rules** — how cells change (built-in or no-code stack)
3. **Rule Authoring** — how users build/undo rule stacks (separate from simulation undo)
4. **Rendering** — how the user sees the world

Keeping these separate lets the project grow from a small Game of Life clone
into a general-purpose, user-extensible cellular automata laboratory.
