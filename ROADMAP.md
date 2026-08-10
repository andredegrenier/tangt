# tangt roadmap

tangt's niche is **Gantt-as-code for developers**: a single-user, file-first
tool where the plain-text `.tangt` file is the source of truth. This roadmap is
organized around winning that niche on distribution and correctness before
adding project-management depth. It is drawn from a market gap analysis (vs.
Mermaid, frappe-gantt, MS Project, OmniPlan, Linear, Markwhen, and others) and
a UI/UX heuristic review; items already shipped in v0.1.0 are omitted.

Deliberately **out of scope** (to protect the positioning): real-time
collaboration/multi-user, resource leveling, per-resource calendars,
portfolio/multi-project rollups, cost/budget tracking, SF dependencies.

## P0 — the wedge

1. **Headless CLI render** — `tangt render plan.tangt -o out.svg|png|pdf`.
   Native Rust means sub-second, zero-Chromium rendering (Mermaid needs ~170MB
   headless Chromium). Unlocks CI checks, README embeds, and pre-commit chart
   generation. The `export::{export_svg,export_png,export_pdf}` functions
   already exist; this is a `clap` front-end plus a `MonoMeasurer`/fontdb text
   path so it needs no GPU/windowing. (S–M)
2. **Watch mode** — `tangt render --watch` re-renders on file change; pairs
   with #1 for "edit in any editor, preview updates". (S)
3. **Mermaid gantt import** — `tangt import mermaid`. Mermaid's syntax is small
   and close to tangt's; one-command migration captures the largest
   text-Gantt user base. (S–M)
4. **VS Code extension, staged** — TextMate grammar → webview preview (via the
   CLI's SVG) → LSP with diagnostics/id+date completion. Markwhen proves the
   split-editing model works as an extension. (S → M)
5. **Critical path computation + highlight** — forward/backward CPM pass over
   the dependency graph tangt already owns; show computed critical tasks and
   per-task slack in the tooltip, keeping the manual `crit` tag as an override.
   The #1 "real scheduling" feature across every serious competitor. (M)

## P1 — planning depth

6. **Summary / rollup task hierarchy** — nested tasks (indent- or
   `parent`-based) with summary bars and duration/%-complete rollup;
   collapse/expand on the canvas. The most-requested step from "diagram" to
   "plan". (M)
7. **SS/FF dependency types + explicit negative lag** — cheap DSL extension
   (`after x.start`, `with x.end`); skip the rare SF type. (S–M)
8. **Baselines** — `tangt baseline set` snapshots dates into the file (or a
   `baseline:` block); render ghost bars + drift. File-first twist nobody else
   has: **diff against a git revision** of the same `.tangt`. (M)
9. **Share/export pack** — copy chart image to clipboard (`arboard`);
   dark-mode + transparent-background SVG/PDF for embedding in dark docs via a
   light/dark `<picture>` pair. High daily-use value, low effort. (S)

## P2 — reach and interop

10. **Interop exports** — CSV (spreadsheets/stakeholders), `.ics` (milestones
    into the dev's calendar), and MS Project MSPDI XML as a stakeholder escape
    hatch. All text formats; MSPDI is a hand-rolled writer. (S–M)
11. **Quarter/year zoom + compact density** — enables Linear-style "roadmap
    communication" charts. (S)
12. **Print pagination / tiling** for charts beyond ~100 tasks. (M)
13. **Lightweight resource tags** — `@alice` in the DSL, color/filter by
    assignee, a simple overallocation strip. Stops well short of leveling. (M)

## AI assist (SLM + MCP) — shipped, with follow-ups

The MCP server (stdio + token-gated loopback HTTP) and the local-model plan
assistant are implemented; see the README. Ten tools, one shared
implementation in `tangt-core::tools`, every mutation applied as a single
undoable `Op`. Remaining work:

- **Bundled local model** (M–L) — today the assistant needs an endpoint the
  user runs (Ollama or OpenAI-compatible). Ship an in-process llama.cpp
  backend behind a `builtin-llm` feature with a small GGUF catalogue,
  resumable download and SHA-256 verification, so a fresh install can use the
  assistant with nothing else set up. Weighed against a C++ toolchain on every
  CI runner and a much slower release build.
- **A `render` tool** (S) — depends on the headless CLI (P0 #1); would let an
  agent produce a PNG/SVG of the plan it just edited.
- **MCP client** (M) — let the in-app assistant call *other* MCP servers, so
  it can pull a ticket list or a calendar while planning.
- **Streaming tool feedback in the canvas** (S) — highlight the bar a tool call
  is about to change, so a multi-step edit is watchable rather than sudden.
- **Prompt-injection posture** (S) — the assistant reads task labels, which can
  contain instructions. Tools are already narrow and the model cannot reach the
  filesystem or network through tangt, but the system prompt should say so
  explicitly.

## UI/UX polish backlog (beyond the quick wins already shipped)

- **Multi-select** — shift-click / marquee / group move+delete. The event
  model (one `Op` per gesture) already supports batching, so unified undo
  stays intact.
- **Auto-pan when dragging near the viewport edge** during long moves/reorders.
- **Native menu bar on macOS** (NSMenu via `muda`) + a proper Edit menu
  surfacing Undo/Redo; the in-window egui bar stays on Windows/Linux.
- **First-run onboarding** — a one-time dismissible "try dragging a bar" hint.
- **Explicit resize grips + always-visible dep dot on the selected bar** for
  discoverability.
- **Label decluttering at low zoom** — hide outside-bar labels below a
  px-per-day threshold and rely on the gutter + tooltip.
- **Redundant (non-color) encodings** for `crit`/`done`/`active` so status
  survives colorblindness and grayscale printing (hatching, glyphs).
- **Problems popover** listing all diagnostics (the status bar now jumps to the
  first error).
- **Screen-reader story** — the text editor is a complete, accessible
  representation of the chart; document this and keep every canvas action
  backed by a text path (it is, by architecture).
