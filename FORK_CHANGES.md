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

"Zed" remains a trademark of Zed Industries; this fork does not imply
any endorsement by them. Remaining Zed marks in source and assets are
scheduled for replacement in subsequent changes, recorded here as
they land.
