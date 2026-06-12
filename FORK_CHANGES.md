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

"Zed" remains a trademark of Zed Industries; this fork does not imply
any endorsement by them. Remaining Zed marks in source and assets are
scheduled for replacement in subsequent changes, recorded here as
they land.
