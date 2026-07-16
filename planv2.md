# Physics Sim Mode — Implementation Plan (addendum to v2 plan)

## Overview

Add a second simulation mode alongside the existing Cellular Automaton mode:
a falling-sand style **Physics Mode**, where cells are materials with
properties (density, phase, flammability, etc.) that move, displace, and
react with each other — a simple Noita/Powder Toy-like sandbox.

This is additive, not a replacement. `SimMode` becomes an enum with two
variants; both share `Chunk`/`World`/`Camera`/rendering, but each has its
own op vocabulary and its own step function, because the two computations
are fundamentally different:

- **CA step**: read-only pass over current generation → write to new buffer
  → swap. Order-independent, embarrassingly parallel.
- **Physics step**: cells *move* (swap positions). Order-dependent — two
  adjacent falling cells can both target the same empty cell below them.
  Needs ordered scanning or claim-then-commit, not a naive parallel
  double-buffer pass.

```
SimMode
├── CellularAutomaton(RuleOp stack)   — existing
└── Physics(PhysicsOp stack)          — new
```

---

## Data Model

### Material definition

Materials replace the raw `state: u8` semantics in physics mode. Each
material is a data record, not code — so materials can be authored,
saved, and shared the same way rule stacks are.

```rust
struct Material {
    id: MaterialId,           // interned/small integer, indexes into a table
    name: String,
    phase: Phase,             // Solid | Powder | Liquid | Gas | Empty
    density: f32,             // determines displacement order
    color: Rgba,
    flammable: bool,
    ignition_temp: Option<f32>,
    conducts_heat: bool,
    // reactions: see below
}

enum Phase {
    Empty,
    Solid,      // static, never moves on its own (e.g. stone, wood)
    Powder,     // falls, piles up, respects angle of repose (e.g. sand)
    Liquid,     // falls, spreads sideways to find level (e.g. water)
    Gas,        // rises, diffuses (e.g. steam, smoke)
}
```

`Cell` gains a material reference instead of (or alongside) its CA `state: u8`:

```rust
struct PhysicsCell {
    material: MaterialId,
    temperature: f32,     // optional, only needed if heat/fire is in scope
    velocity: Option<Vec2>, // optional, for later "juicier" falling behavior
}
```

Keep this separate from the CA `Cell` type rather than unifying them —
they serve different models and forcing a shared struct will bloat both.

### Reactions

Contact-based transformations, not neighbor-count based:

```rust
struct Reaction {
    a: MaterialId,
    b: MaterialId,
    result_a: MaterialId,   // what `a` becomes
    result_b: MaterialId,   // what `b` becomes
    // e.g. water + lava -> stone + steam
    // e.g. fire + wood  -> fire + ash (spreading)
}
```

A small reaction table (start with 5-10 entries) covers most of the
"satisfying" Noita-like interactions: water douses fire, lava + water
makes stone/steam, fire spreads to flammable materials, acid dissolves
things. Resist the urge to build a general chemistry system up front.

### Physics ops (no-code authoring, mirrors `RuleOp`)

For users who want to define **custom materials'** behavior beyond the
built-in phases, in the same spirit as the CA rule stack:

```rust
enum PhysicsOp {
    TryMove { directions: Vec<Direction>, priority: MovePriority },
    DisplaceIfDenser { than: MaterialId },
    ReactOnContact { with: MaterialId, becomes_self: MaterialId, becomes_other: MaterialId },
    ChangeStateIfTemp { above: Option<f32>, below: Option<f32>, becomes: MaterialId },
    Emit { material: MaterialId, chance: f32 }, // e.g. fire emitting smoke
}
```

Most users will just pick a `Phase` + density + a couple of reactions
via the material palette UI (below) rather than hand-author `PhysicsOp`
stacks — treat the ops as the advanced/expert path, same relationship
`RuleOp` has to the built-in B/S rules.

---

## Material Palette (UI)

This is the new piece the CA mode didn't need — a way to **select which
material you're painting with** before clicking/dragging on the canvas.

```
PhysicsView
├── Canvas               (existing, shared with CA mode)
├── MaterialPalette      (new)
│   ├── swatches for each defined material (color + name)
│   ├── selected material highlighted
│   ├── scroll/paginate if many materials
│   └── "Edit Material" → opens MaterialEditor
├── MaterialEditor       (new, for authoring/customizing)
│   ├── name, color picker
│   ├── phase dropdown (Solid/Powder/Liquid/Gas)
│   ├── density slider
│   ├── flammable toggle + ignition temp
│   ├── reaction list (with other materials)
│   └── save/load as JSON (serde, same pattern as rule stacks)
├── BrushControls        (size, shape — mostly reusable from CA's brush)
└── Statistics           (existing)
```

Interaction model:
- Click a swatch in the palette → sets "current paint material."
- Click/drag on canvas → paints cells with the selected material (same
  input plumbing as the CA mode's cell-toggle brush, just writing a
  `MaterialId` instead of toggling a boolean state).
- Right-click or a dedicated "eraser" swatch → paints `Empty`.
- Ship with a small built-in palette (Sand, Water, Stone, Wood, Fire,
  Smoke, Acid, Lava, Empty) so it's immediately playable, then let users
  extend it via the Material Editor and save custom palettes as JSON
  (`palette.json`, list of `Material`), same serialization pattern as
  rule stacks.

---

## Stepping Model

This is the part that actually forks the engine — not the data, the
control flow.

### Ordering strategy

Naive parallel double-buffering breaks for movement (two grains claiming
the same target cell). Options, roughly in order of implementation
effort:

1. **Sequential ordered scan (simplest, do this first)**: iterate rows
   bottom-to-top (so falling resolves in one pass), alternating
   left-to-right/right-to-left per row (or randomizing scan direction
   per row) to avoid directional bias in how piles/spreads look.
   Single-threaded per chunk, but chunks can still be processed in
   parallel *if* movement across chunk boundaries is deferred to a
   second pass (see below).
2. **Claim-then-commit**: cells propose a move in parallel, conflicts
   are resolved (e.g. first-writer-wins or randomized tiebreak), then
   moves are applied in a second parallel pass. More complex, revisit
   only if the sequential scan becomes the profiling bottleneck.

Start with (1). It's simpler, well-understood (this is how most
falling-sand toys work), and Phase 7 (Performance) is the right place to
revisit parallelism if needed — don't over-engineer this before it's
proven necessary.

### Chunk boundary movement

The existing CA plan handles chunk boundaries via **halo/ghost cells**
for read-only neighbor counting. Physics needs more: a cell can actually
**move across** a chunk boundary, which means transferring ownership of
data, not just reading a neighbor's state.

Approach:
- Process each chunk's interior first.
- Cells that would move into a neighboring chunk get queued as
  "pending transfers" rather than written immediately.
- After all chunks are processed, apply pending transfers in a
  synchronization pass.
- This keeps the parallel-per-chunk structure intact for the common
  case (movement within a chunk) while still being correct at
  boundaries.

### Mutation strategy

The CA plan mandates "never mutate in place, swap buffers." Physics
mode should mutate in place (with the ordering above), since:
- A "move" is a clear+set pair; representing that as a pure
  read-old/write-new function is awkward and doesn't map to the
  swap-buffer model cleanly.
- In-place mutation with a well-defined scan order is the standard,
  well-tested approach for falling-sand sims (Noita, Powder Toy, etc.)

Document this divergence clearly in code comments — it's an intentional
exception to the CA stepping rule, not an oversight.

---

## Rendering & Input Reuse

Reuse as much of the existing pipeline as possible:
- Camera, spatial tree, visible-chunk culling: unchanged, works the same
  regardless of what's inside a chunk.
- Canvas draw call: same "iterate visible chunks, draw cells" loop,
  just color-by-material instead of color-by-CA-state.
- Brush/input system: same mouse-drag-to-paint plumbing, but writes a
  `MaterialId` field instead of toggling a boolean.

New: **MaterialPalette** and **MaterialEditor** components under
`SimulatorView`, only shown/active when `SimMode::Physics` is selected.

---

## Testing

- Unit tests for each `Phase`'s core movement rule in isolation (a
  single powder cell falls until blocked; a liquid cell spreads to
  find its level; a gas cell rises and diffuses) — analogous to the CA
  plan's oscillator/spaceship tests.
- Reaction tests: verify each entry in the reaction table produces the
  expected material pair on contact.
- Chunk-boundary movement test: place a falling material one row above
  a chunk seam, verify it transfers correctly across chunks over
  several steps.
- Determinism test: same seed + same input sequence → same final state
  (important if scan-direction randomization is used — seed it).

---

## Milestones (extends the existing Phase 0–7 plan)

## Phase 8 — Physics Core
- `Material` + `Phase` + `MaterialId` table, `PhysicsCell` type
- Built-in materials: Sand, Water, Stone, Empty
- Sequential ordered-scan step function (single chunk, no boundary
  crossing yet)
- Basic gravity/falling for Powder and Liquid phases

## Phase 9 — Material Palette UI
- `MaterialPalette` component: swatches, selection state
- Brush painting writes `MaterialId` to cells
- Built-in palette JSON, loaded at startup

## Phase 10 — Reactions & Extended Phases
- Reaction table + contact resolution
- Gas phase (rise + diffuse)
- Fire/flammability + temperature field (optional scope cut if time-boxed)
- Built-in materials expanded: Wood, Fire, Smoke, Lava, Acid

## Phase 11 — Chunk-Boundary Movement
- Pending-transfer queue for cross-chunk moves
- Sync pass to apply transfers after per-chunk step
- Boundary movement tests

## Phase 12 — Material Editor & Custom Palettes
- `MaterialEditor` component: create/edit materials, define reactions
- Save/load custom palettes as JSON (serde, mirrors rule-stack save/load)
- `PhysicsOp` stack (advanced/expert path) for behavior beyond
  phase/density/reaction defaults

## Phase 13 — Physics Performance (parallel evaluation, optional)
- Only if profiling shows sequential scan is a bottleneck at target
  scale
- Claim-then-commit parallel movement resolution
- Revisit alongside existing Phase 7 (CA performance work)

---

## Open Questions / Scope Guards

- **Temperature/heat simulation** is where these projects tend to
  balloon (heat diffusion, conduction, phase changes from temperature).
  Recommend time-boxing: ship flammability as a simple contact trigger
  first (fire touches wood → wood becomes fire) before adding a
  continuous temperature field. Add temperature only if the simpler
  model feels insufficient in practice.
- **Pressure/fluid dynamics** (proper liquid pressure, gas compression)
  is out of scope for an initial version — simple "find level" spread
  behavior is enough for a convincing sandbox.
- **Unifying `Cell` and `PhysicsCell`**: resist merging these into one
  struct even if it's tempting for code reuse. The two modes have
  different invariants (CA state is transition-only; physics state
  includes spatial movement) and a shared struct will accumulate unused
  fields on both sides.
  fields on both sides.
