# Zed-parity production rubric

The acceptance gate for every Auracle-authored surface in the IDE. A surface is
"done" only when it passes all five items below. Every surface PR cites which
items it satisfies and attaches a screenshot from the built app.

The goal is **Zed-grade quality, Auracle-branded** — not stock Zed, and not a new
identity. We keep the brand (theme tokens, colour) and raise the craft to the bar
Zed sets for its own editor chrome.

---

## 1. No placeholders

No `soon`, `coming soon`, `TODO`, dead-end, or stub labels ship on a surface.
Every control is either real, or honestly describes a state the engine can prove.

- **Pass:** a stage the engine can't yet prove reads "not tracked yet" (backed by
  engine truth), or shows a designed empty state.
- **Fail:** a button that does nothing; a "soon" tag; a label that promises a
  capability that isn't wired.

## 2. One visual system

Every surface is built from Zed's `ui`-crate components and the shared Auracle
theme tokens (spacing, typography, density, colour). Two surfaces of the same
kind (e.g. two list panels) look like one app, not two hand-rolled layouts.

- **Pass:** spacing/type/density match across surfaces; colour comes from theme
  tokens, not literals.
- **Fail:** bespoke one-off paddings; hard-coded colours; a panel that visibly
  doesn't match its neighbours.

## 3. Every state designed

A surface designs all four states — never a blank panel or a raw spinner. The
mapping from a fetch outcome to one of these states is the
[`ViewState`](src/auracle_view_state.rs) seam, so the decision is unit-tested
without rendering:

- **Loading** — a skeleton while the engine fetch is in flight.
- **Empty** — the fetch succeeded with nothing to show; says what would appear
  here and (where useful) how to make it appear.
- **Error** — the fetch failed; an honest message and a Retry affordance when
  `should_retry()` is true.
- **Ready** — the real content.

- **Pass:** all four states exist and are designed; the state decision has a test.
- **Fail:** an empty result renders blank; an error shows nothing or a raw code;
  loading is a bare spinner.

## 4. Native chrome

Surfaces use Zed's real containers and primitives (panels, toolbars, status-bar
items, the settings-window page framework) rather than bespoke shells.

- **Pass:** settings live as pages in Zed's native settings window; status reads
  as native status-bar items; toolbars match Zed toolbar density.
- **Fail:** a bespoke dock panel that mimics settings; a custom status strip.

## 5. Honest

No dead controls, no fabricated data, no secret shown/logged/placed in a URL. A
surface never invents progress or numbers it can't back with engine truth.

- **Pass:** every figure traces to engine data; disconnected states degrade
  gracefully and say so.
- **Fail:** a placeholder metric; a control with no effect; a secret on screen.

---

## How a surface is graded

1. Implement the surface's state decision via `ViewState` (item 3) — tested.
2. Render each state with the shared visual system (items 2, 4).
3. Remove every placeholder (item 1) and confirm honesty (item 5).
4. In the PR, list items 1–5 with a one-line note each, and attach a screenshot
   from the built `.dmg` (GPUI rendering can't be verified without the Metal
   toolchain, so the screenshot is the visual evidence; the `ViewState` decision
   is the automated evidence).
