# PRD — Complete the Auracle identifier debrand

Status: ready-for-agent
Owner decisions: locked via `/grill-with-docs` 2026-06-23
Target repo: `SiixQuant/auracle-ide` (trunk `ide-startup-autoconnect`)

## Problem Statement

Auracle is a rebranded fork of the Zed editor. The visible app name, icon, fonts,
binary (`auracle`), config directory (`~/.config/auracle`), and most chrome are
already Auracle. But the rename is incomplete: a user editing Settings, opening a
"learn more" link, reading a tooltip, or inspecting a project config file still
runs into the **Zed** name and **zed.dev** URLs. Recent examples the owner hit:

- Settings → "How `zed <path>` opens directories…" (fixed in v0.7.3, one of four).
- "Zed AI" edit-prediction provider label; a "Zed Keybind Context" filter.
- ~103 clickable `https://zed.dev/docs/...` "learn more" links across the debugger,
  extensions, edit-prediction and agent panels.
- Functional identifiers users can encounter: the `.zed/` project-settings folder,
  the `zed://settings/…` deep-link scheme, `ZED_*` task variables.

The owner wants **no "Zed" anywhere a user can see it**, the IDE's own paths/links
pointed at Auracle, and everything still working — without breaking existing
installs or projects.

## Solution

A staged debrand that swaps every **user-visible** "Zed"/`zed.dev` reference to
Auracle, repoints doc links to an Auracle docs base, and renames the
**user-facing functional identifiers** to Auracle names while **dual-reading** the
old names so nothing existing breaks. Deep-internal identifiers that no user ever
sees (the `zed::` action namespace, agent protocol URIs, build/task env vars, the
cloud-provider wire id) are kept or dual-read only — renaming them is invisible,
high-volume, and risky. Every change is verified live in the built IDE.

## Owner decisions (locked)

1. **Scope:** deepest — user-visible copy IDE-wide **plus** user-facing functional
   identifiers. (Owner chose "Also rename functional IDs".)
2. **Back-compat:** **dual-read shims**. New Auracle names are canonical; the IDE
   keeps reading the old names as fallbacks so existing installs/projects/links
   keep working forever. (Owner: "whatever you recommend" → dual-read.)
3. **Doc links:** **repoint to Auracle docs.** Base URL standardized to
   `https://docs.aurapointcapital.com` (parallel to the `ide.aurapointcapital.com`
   engine), mapping `zed.dev/docs/X → docs.aurapointcapital.com/X` 1:1, behind a
   single constant. NOTE: these links 404 until that docs site exists — see Risks.
4. **Verification:** thorough, runtime, in the built IDE — not just compile gates.
   (Owner: "make sure everything works within the IDE.")

## Inventory (grounded in the code, 2026-06-23)

Categorized by what each reference IS, which determines its treatment:

| Category | Approx scale | Treatment |
|---|---|---|
| **Display copy** — quoted user-visible strings containing "Zed" | ~164 across ~30 crates (hotspots: `ui`, `editor`, `edit_prediction`, `agent_ui`, `markdown`, `settings_ui`) | **Rename** "Zed"→"Auracle" (judgment per string). |
| **External doc URLs** — `https://zed.dev/docs/...` and friends | ~103 | **Repoint** through the docs mapper. Non-docs zed.dev URLs (releases, CLA, cloud) handled case-by-case (see below). |
| **User-facing functional ids** — `.zed/` project folder (`settings.json`/`tasks.json`/`debug.json`), `zed://settings/…` deep-links | small, centralized in `crates/paths` + `settings_ui` | **Rename + dual-read** old. |
| **Deep-internal functional ids** — `ZED_*` env/task vars (51× `ZED_AGENT_ID`, `ZED_WORKTREE_ROOT`, `ZED_FILE`…), agent protocol URIs (`zed:///agent/…`), `zed.dev` cloud-provider wire id, `.zed_server`/`.zed_wsl_server` remote dirs | ~2k refs | **Dual-read / keep** (do NOT hard-rename — invisible, high-volume, breaks user task configs and wire compat). |
| **Internal `zed::` action namespace** | ~151 | **Keep.** The command palette already display-rewrites the prefix to `auracle:` (`command_palette.rs:697`). |
| **Test fixtures / dev-only** (`hover_links.rs` tests, agent eval fixtures, flatpak Linux packaging) | many | **Leave** (not user-visible; mac is the shipped target). |
| **Theme `$schema` URLs** (`zed.dev/schema/themes/...`) | 4 | **Repoint** to the docs/schema base if hosted, else leave (low-pri; only visible when editing theme JSON). |

The executing slices re-run the exhaustive grep; the table is the map, not the
line list.

## User Stories

1. As a trader using Auracle, I want every Settings description, label, and command
   reference to say "Auracle" (and the Auracle CLI/commands), so the product feels
   like one coherent tool, not a reskinned Zed.
2. As a user, I want the "Plain Text / CLI open behavior / performance profiler"
   Settings copy to reference `auracle`, so the commands I'm told to run exist.
3. As a user, I want edit-prediction/AI provider labels to read "Auracle", not
   "Zed AI".
4. As a user clicking a "learn more" link, I want to land on Auracle documentation,
   not zed.dev.
5. As a user reading my project's local settings, I want them in `.auracle/`, the
   Auracle convention — while my existing `.zed/settings.json` keeps working.
6. As an existing user, I want my old `.zed/settings.json`, saved `zed://` links,
   and `ZED_*` task variables to keep functioning after I update, with zero manual
   migration.
7. As a power user, I want new project settings, deep-links, and task variables to
   use the Auracle names going forward.
8. As a user opening a tooltip, error, or menu, I want no stray "Zed" text anywhere.
9. As the owner, I want a single place that defines the canonical Auracle names and
   their legacy fallbacks, so future audits never find a stray identifier.
10. As the owner, I want the doc-link base URL to be one constant I can change when
    the docs site domain is finalized.
11. As a user, I want the IDE to still build, launch, open projects, run the agent,
    use the debugger, and load settings exactly as before — the debrand changes
    names, not behavior.
12. As a user on an existing install, I want the update to be seamless — no lost
    settings, no broken links, no re-typed env vars.
13. As the owner, I want the deep-internal identifiers (`zed::` namespace, agent
    URIs, wire ids) left alone so we don't risk breakage for changes no user sees.
14. As a QA reviewer, I want proof (screenshots / link checks / settings-load tests)
    that each surface is debranded and still works.

## Implementation Decisions

### Seams (test at the highest point; the fewer the better)

1. **Brand constants module** — one source of truth. A small module (e.g.
   `crates/util/src/auracle_brand.rs` or a tiny `auracle_brand` crate) exporting:
   - `DISPLAY_NAME = "Auracle"`,
   - `LOCAL_SETTINGS_FOLDER = ".auracle"`, `LEGACY_LOCAL_SETTINGS_FOLDER = ".zed"`,
   - `URL_SCHEME = "auracle"`, `LEGACY_URL_SCHEME = "zed"`,
   - `DOCS_BASE = "https://docs.aurapointcapital.com"`,
   - `ENV_PREFIX = "AURACLE_"`, `LEGACY_ENV_PREFIX = "ZED_"`.
   Pure data + helpers, unit-tested. Everything else references these — no scattered
   literals.

2. **Docs URL mapper** — `auracle_docs_url(path: &str) -> String` (e.g. in the brand
   module) producing `{DOCS_BASE}{path}`. Every `https://zed.dev/docs/...` literal
   is replaced by a call to it. Pure, unit-tested with a table of inputs→outputs.
   Non-docs zed.dev URLs are handled explicitly: the "Update Zed → zed.dev/releases"
   link is **removed** (Auracle updates via the launcher); CLA/cloud/collab URLs in
   the removed-collab paths are left or removed with their dead code.

3. **Project-local settings dual-read** — extend `crates/paths` (`local_settings_*`)
   to expose both the canonical `.auracle/...` and legacy `.zed/...` relative paths,
   and have the settings/tasks/debug loaders in `settings_store` try canonical then
   legacy. Test at the `settings_store` level: a worktree with only `.zed/settings.json`
   still loads; `.auracle/` wins when both exist; writes go to `.auracle/`.

4. **URL-scheme dual-resolve** — register `auracle://` (bundle Info.plist + handler)
   and update the in-app emitters (`settings_ui` deep-links) to emit `auracle://`,
   while the open-request handler resolves BOTH `auracle://` and `zed://`. Keep the
   internal `zed:///agent/...` protocol URIs as-is (not user-facing). Test the
   handler's scheme-parsing on both prefixes.

5. **Env-var dual-read** — a helper `auracle_env(suffix) -> Option<String>` reading
   `AURACLE_<suffix>` then `ZED_<suffix>`; when the IDE SETS task variables, set both
   names. Do NOT mass-rename the `ZED_*` literals. (Recommendation: this slice is
   optional/low-priority; the env vars are invisible and dual-set is the only
   user-relevant part — task configs referencing `$ZED_FILE` keep working, and new
   docs can show `$AURACLE_FILE`.)

6. **Display-copy sweep** — mechanical, reviewed string replacement of user-visible
   "Zed"→"Auracle" with an explicit denylist for: `zed::` code, `"zed.dev"` serde
   ids/constants, test fixtures, and the kept internal identifiers. No new seam;
   guarded by the runtime verification pass.

### Treatment of the `zed.dev` cloud-provider id (special case)

`"zed.dev"` is the wire id of the hosted cloud provider (`ZED_CLOUD_PROVIDER_ID`,
`ZED_WEB_SEARCH_PROVIDER_ID`, serde `rename = "zed.dev"`, the migrator). It is NOT
display copy. **Keep the wire value**; if a user-visible label derives from it,
override the LABEL only (e.g. "Auracle Cloud" / hide it, per the BYO-key posture).
Add a serde alias if we ever change the stored id. Renaming the wire id is out of
scope (breaks existing settings + the migrator for an invisible string).

## Testing Decisions

Good tests here assert **external behavior**, not internals:

- **Brand module + docs mapper:** pure unit tests (input path → expected Auracle URL;
  canonical/legacy folder + scheme constants).
- **Dual-read settings (settings_store):** behavioral tests — legacy-only project
  loads; canonical wins over legacy; new writes target `.auracle/`. Prior art:
  existing `settings_store` load tests and the `.zed/settings.json` test fixtures
  already in `settings_ui` (update them to cover both).
- **Scheme handler:** unit test that both `auracle://settings/x` and
  `zed://settings/x` parse to the same action.
- **Runtime verification (manual, in the built .dmg)** — the acceptance bar:
  build → launch → click through every Settings page (no visible "Zed") → click a
  "learn more" link (resolves to `docs.aurapointcapital.com/...`) → open a project
  with only `.zed/settings.json` (still loads) → add `.auracle/settings.json`
  (wins) → trigger a `zed://settings/` and an `auracle://settings/` deep-link (both
  resolve) → confirm agent/debugger/editor still work. Capture screenshots/log proof.

No test should assert on the literal strings being absent via a brittle repo-grep in
CI; instead, a one-shot `script/check-no-visible-zed` helper (greps display-copy
patterns, excludes the denylist) can be run during review.

## Out of Scope

- The internal `zed::` action namespace, crate/package names, and the `zed` Cargo
  package — kept (palette already display-rewrites to `auracle:`).
- Hard-renaming `ZED_*` env vars, agent `zed:///` protocol URIs, and the `zed.dev`
  cloud-provider wire id (dual-read/keep only).
- Authoring the actual Auracle documentation **site** content at
  `docs.aurapointcapital.com` (owner/separate effort). This PRD only points links at
  it.
- Linux/flatpak packaging strings (mac `.dmg` is the shipped target).
- Test fixtures and dev-only tooling.

## Risks & mitigations

- **Doc links 404 until the docs site exists.** Mitigation: route all unmapped/any
  paths through the mapper so flipping the base (or adding a single
  `docs.aurapointcapital.com` landing page) fixes them at once; consider gating the
  doc-link slice behind the site going live, or shipping a "Docs coming soon"
  landing fallback. Owner to confirm the final docs domain (one constant).
- **Scheme registration is a bundle/Info.plist change** — must ship in the same
  build as the emitter change; dual-resolve protects old links.
- **Over-broad string replacement** could hit an internal identifier. Mitigation:
  denylist + reviewed diffs + the runtime pass.

## Suggested slicing (for `/to-issues`, tracer bullets)

1. Brand-constants module + docs-URL mapper (foundation, fully unit-tested).
2. Display-copy sweep, IDE-wide (highest-visibility win; uses the brand module).
3. Doc-URL repoint — all `zed.dev/docs` links → mapper; remove the "Update Zed" link.
4. `.zed/`→`.auracle/` project settings with dual-read (settings/tasks/debug).
5. `zed://`→`auracle://` scheme: register + emit + dual-resolve.
6. (Optional) env-var dual-read/dual-set; provider-label override.
7. Runtime verification pass + release.

## Further Notes

- Already shipped toward this goal: v0.7.3 (4 Settings strings) and the
  palette-prefix display rewrite. This PRD subsumes and completes that work.
- The binary is already `auracle` (`crates/zed/Cargo.toml`), so CLI-command copy
  should reference `auracle`.
- Keep public commit/PR messages generalized per repo convention (describe it as
  completing Auracle branding, not "hiding Zed").
