# Fork notice (GPL-3.0 §5a — statement of changes)

This repository is **Auracle IDE**, a fork of
[zed-industries/zed](https://github.com/zed-industries/zed), carrying
all upstream licenses unchanged (see LICENSE-GPL, LICENSE-APACHE and
per-crate license files). The `auracle` branch is cut from the
upstream release tag **v1.6.3** (commit 601ecb3) and never tracks
upstream `main`; upstream merges happen in deliberate rebase windows
and are recorded here.

## Changes relative to upstream v1.6.3
- 2026-06-12 — fork point. Bundled the Auracle theme family
  (`assets/themes/auracle/`, original work) and made it the default;
  default settings ship telemetry (diagnostics + metrics) OFF and
  auto-update OFF; release-channel display names read "Auracle IDE".
  Added this file and REPO_RULES.md. No other source changes.
- 2026-06-12 (2) — startup network silenced: no extension
  auto-installs or update checks at boot (manual paths remain); the
  remote third-party agent-catalog refresh is disabled; default
  agent model provider is key-based (anthropic), not cloud-account.
  Clean-profile boot opens zero non-local connections.
- 2026-06-12 (3) — theme token pack: collaboration/agent presence
  colors (agent is never a verdict color), light-mode contrast set,
  light elevation step, tabular figures pinned in UI font features.
- 2026-06-12 (4) — first-boot experience: new `runway_rail` crate
  (left-dock placeholder rail: six locked stages with plain-word
  hints; lights up only when the engine exposes stage tracking);
  welcome headline/subtitle now Auracle's (the cloud-plan onboarding
  strings under ai_onboarding refer to Zed's own cloud products and
  become unreachable with the sign-in affordance hidden by default —
  scheduled for removal in the deep debrand pass); the dev-container
  suggestion toast no longer auto-prompts (CLI-initiated only);
  title-bar Sign In hidden by default; app_id is Auracle's.
- 2026-06-13 (5) — deep debrand of user-facing surfaces: "About Zed"
  → "About Auracle IDE"; launch-failure notifications, title-bar
  update/collab prompts, and the screen-share permission text now say
  "Auracle IDE"; AI-panel headline "Welcome to Zed AI" → "Welcome to
  Auracle AI"; generic "tokens in Zed agent" → "tokens in the agent".
  Packaged as /Applications/Auracle IDE.app (CFBundleName "Auracle
  IDE", bundle id com.aurapointcapital.AuracleIDE, Auracle logo
  icon) so the Dock, ⌘-Tab, and app menu read "Auracle IDE".
  PRINCIPLED EXCLUSION: Zed's specific paid-plan screens (Zed Pro /
  Business / Student and their token grants) are NOT relabeled to
  "Auracle Pro/…" — Auracle does not resell those tiers and faking
  them would violate the never-fake-capability rule. Those screens
  belong to Zed's cloud sign-in, which is hidden by default and
  therefore unreachable in normal Auracle use; they are left inert
  rather than turned into false Auracle products.
- 2026-06-13 (6) — debrand completion of the remaining user-facing
  surfaces: the in-app logo (`assets/images/zed_logo.svg`, rendered on
  the Welcome and onboarding screens) is now the Auracle flame mark;
  the move-to-Applications dialog ("Move Auracle to Applications?" /
  "Failed to move Auracle to Applications") and the title-bar update
  button ("Checking for / Downloading / Installing Auracle Update…")
  read Auracle. PRINCIPLED EXCLUSIONS (left intentionally, not
  user-facing product chrome): test fixtures and component-gallery
  examples; internal identifiers (e.g. the `InstallingZedModal` type,
  `AgentId::from("Zed")`); bundled-asset names referenced by path
  ("Zed Mono" font, "Zed Icons" icon theme); GPU/telemetry diagnostic
  strings; and the Zed-cloud paid-plan/account screens covered by the
  prior exclusion. The on-disk binary remains `zed` and the config /
  cache directories remain Zed's — these are non-user-facing and
  renaming them would fork the data dirs / break upstream-rebase
  ergonomics for no customer-visible gain.
- 2026-06-17 (7) — deep string + asset rebrand of the remaining
  user-facing chrome. All 13 `zed_*`/`ai_zed` icon and logo SVG assets
  renamed to `auracle_*`/`ai_auracle` (with their `IconName` /
  `VectorName` enum variants and all call sites); the macOS Finder
  file-type name ("Auracle Text Document"), all 12 macOS permission
  prompts ("…in Auracle…"), the window-tabbing group identifier, the
  REPL setup tooltip, the command-palette status/feedback descriptions,
  the AI-provider configuration copy ("To use Auracle's agent with …",
  "restart Auracle"), the MCP client name and OAuth/DAP client
  identifiers, the crash-reporter binary label and notification id, and
  the Windows product-name/dialog strings all read Auracle. The
  command palette now displays actions as "auracle: …" while the
  underlying action identity stays `zed::…` (the exact registry/keymap
  lookup key — rewriting it would silently break keybindings).
  PRINCIPLED EXCLUSIONS (unchanged, deliberately): bundled-asset names
  referenced by path ("Zed Mono"/"Zed Plex Sans" fonts, "Zed Icons"
  theme, the "Zed Keybind Context" language), HTTP User-Agent strings
  some servers validate, serialized settings keys (`zed_dot_dev`,
  `EditPredictionProvider::Zed`), the Zed-cloud paid-plan/account
  screens (per the entry-5 exclusion), internal identifiers, tests and
  component-gallery examples, and all GPL/Apache copyright headers. The
  on-disk binary name and config/data directories are addressed in a
  separate coordinated change (lockstep with the launcher + a first-run
  config migration), recorded here when it lands.

"Zed" remains a trademark of Zed Industries; this fork does not imply
any endorsement by them. Remaining Zed marks in source and assets are
scheduled for replacement in subsequent changes, recorded here as
they land.
