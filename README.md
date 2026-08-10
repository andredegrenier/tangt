# tangt

A native Gantt chart app built around a plain-text DSL. Charts are `.tangt`
files you can edit as code or directly on the canvas — both at once, with one
undo history spanning both. Export to vector PDF, SVG, or PNG.

Built on the architecture of [gridflow](../claude/gridflow): the text is the
single source of truth, and every canvas gesture (drag a bar, resize it, draw
a dependency, reorder rows) is a *surgical splice* of exactly the bytes that
encode that fact — comments, ordering, and formatting elsewhere survive by
construction.

## The language

```tangt
title "Website Redesign"
axis auto                       // auto | day | week | month
today 2026-08-10                // today-marker override
exclude weekends                // durations count working days
exclude 2026-12-24..2027-01-02  // holidays (date or range)

section Design
task "Wireframes"      : wf, 2026-08-03, 5d
task "Visual design"   : vis, after wf, 10d, #4fa3ff, 60%
milestone "Sign-off"   : ds, after vis

section Build
task "Frontend" : fe, after ds + 2d, until 2026-10-01
task "QA"       : qa, after fe be, 5d, crit
```

One statement per line. Task fields are comma-separated and may appear in any
order — every field is lexically distinct:

| Field | Meaning |
|---|---|
| `2026-08-10` | explicit start date |
| `after id [id…]` | start after those tasks end (multiple predecessors drawn as arrows) |
| `after id + 2d` / `- 2d` | …with lag / lead, in working days |
| `10d`, `2w` | duration (working days when `exclude weekends` is on) |
| `until 2026-09-01` | explicit inclusive end date |
| `60%` | progress fill |
| `#4fa3ff` | bar color |
| `crit` / `done` / `active` | tags |

Broken lines never blank the chart — the parser recovers per statement and
underlines the problem in the editor. Dependency cycles, unknown ids, and
duplicate ids are diagnosed the same way.

## The app

- **Split view**: code on one side, live chart on the other; swap sides with
  ⌘⇧L, hide the editor with ⌘1. Layout and camera persist across launches.
- **Visual editing** — every gesture writes the text:
  - drag a bar → moves its date (an `after` task keeps its dependency; the
    lag is spliced instead — ⌥drag to convert to a fixed date)
  - drag a bar's edges → resizes its duration
  - drag the dot at a bar's right edge onto another bar → creates a dependency
  - double-click → rename; drag a gutter row → reorder; right-click → add /
    delete tasks and sections, set progress
- **Timeline**: pinned month/day header and task gutter; weekends and holidays
  shaded; today marker. Scroll rows, Shift+scroll pans time, pinch/⌘scroll
  zooms time around the pointer.
- **Unified undo**: ⌘Z walks back typing and canvas gestures alike; a drag is
  one undo step no matter how many frames it lasted.
- **Export**: File → Export PDF (single-page vector, auto-sized, selectable
  text), SVG, or PNG (2×). All three serialize the same `Scene` the canvas
  paints, so exports always match the screen.
- ⌘/ opens the in-app reference.

## Building

```sh
cargo run            # launch the app
cargo test           # unit + property + golden tests
```

Two crates: `tangt-core` (lexer → parser → scheduler → layout → scene, zero
GUI deps, all the rewrite logic) and `tangt-app` (egui shell, canvas, editor,
export backends). Rust 2021, eframe 0.35.

## License

MIT
