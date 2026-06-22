# UI production-quality rubric

The acceptance gate for every Auracle-authored surface in the IDE. A surface is
"done" only when it passes all five items below. Every surface PR cites which
items it satisfies and attaches a screenshot from the built app.

The goal is **editor-grade quality, Auracle-branded** — we keep the brand (theme
tokens, colour) and raise the craft to the bar the editor sets for its own
chrome. Not a stock look, and not a new identity.

---

## 1. No placeholders

No `soon`, `coming soon`, `TODO`, dead-end, or stub labels ship on a surface.
Every control is either real, or honestly describes a state the engine can prove.

- **Pass:** a stage the engine can't yet prove reads "not tracked yet" (backed by
  engine truth), or shows a designed empty state.
- **Fail:** a button that does nothing; a "soon" tag; a label that promises a
  capability that isn't wired.

## 2. One visual system

Every surface is built from the shared `ui` components and the Auracle theme
tokens (spacing, typography, density, colour). Two surfaces of the same kind
(e.g. two list panels) look like one app, not two hand-rolled layouts.

This item has two halves — one mechanical, one reviewed:

- **Mechanical (must hold):** the surface's render path contains **zero colour
  literals** — colour comes from theme tokens only. This is grep-checkable and a
  reviewer should confirm it (no `rgb(...)`, `hsla(...)`, `0x..`-as-colour, hex
  strings in the render path).
- **Reviewed:** spacing/type/density match the surface's siblings. Because
  "match" is a judgement, the PR screenshot must place the new surface
  **adjacent to a sibling surface of the same kind**, so the reviewer compares
  rather than guesses.

- **Pass:** zero colour literals in the render path; the adjacency screenshot
  shows consistent spacing/density with siblings.
- **Fail:** any hard-coded colour; bespoke one-off paddings; a panel that
  visibly doesn't match its neighbours.

## 3. Every state designed

A surface designs all four states — never a blank panel or a raw spinner. The
mapping from a fetch outcome to one of these states is the
[`ViewState`](src/auracle_view_state.rs) seam, so the decision is unit-tested
without rendering:

- **Loading** — a skeleton while the engine fetch is in flight.
- **Empty** — the fetch succeeded with nothing to show; says what would appear
  here and (where useful) how to make it appear. Reachable only when the surface
  supplies an emptiness predicate — `into_list_view` does this for `Vec`; a
  non-list surface must define its own emptiness, or it will never produce Empty.
- **Error** — the fetch failed; an honest message and a Retry affordance when
  `should_retry()` is true. `into_view` marks every engine-fetch failure
  retryable; a permanent error (e.g. unsupported platform) is constructed
  directly with `ViewState::permanent_error`.
- **Ready** — the real content.

What the automated test proves vs. what the screenshot proves:

- **Automated (the `ViewState` decision):** which state is selected — i.e. that
  no state is silently forgotten. This is branch selection only.
- **Screenshot-verified:** the *quality* of each state — that Loading is a
  skeleton not a bare spinner, that the Empty hint actually says what would
  appear here (never blank), and that the error message is honest, not a raw
  code. The seam cannot prove these; the reviewer must.

- **Pass:** all four states exist, the state decision has a test, and the
  screenshot shows each state designed.
- **Fail:** an empty result renders blank or with no hint; an error shows
  nothing or a raw code; loading is a bare spinner.

## 4. Native chrome

Surfaces use the editor's real containers and primitives (panels, toolbars,
status-bar items, the native settings-window page framework) rather than bespoke
shells.

- **Pass:** settings live as pages in the native settings window; status reads as
  native status-bar items; toolbars match the editor's toolbar density.
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
   from the built `.dmg` — including the item-2 adjacency shot. (GPUI rendering
   can't be verified without the platform graphics toolchain, so the screenshot
   is the visual evidence; the `ViewState` decision is the automated evidence for
   branch selection.)
