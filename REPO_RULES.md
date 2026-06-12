# Repository rules — auracle-ide

1. **License.** GPL-3.0-or-later for the application (upstream
   inheritance); Apache-2.0 crates keep their licenses. Every
   distributed binary's complete corresponding source is this public
   repository. Upstream copyright notices are never removed.
2. **Boundary.** Zero proprietary code, ever. The Auracle engine
   (license tiers, gates, brokers, backtests, data) lives in a
   separate closed repository and is reached ONLY over its HTTP/MCP
   APIs. Nothing in this repo may embed engine source, schemas with
   secrets, tokens, or credentials. The reverse also holds: nothing
   from this GPL tree may be copied into the engine repository.
3. **Pinning.** The `auracle` branch derives from upstream release
   tags only (current: v1.6.3). Never merge or track upstream
   `main`. Upstream updates land in scheduled rebase windows,
   recorded in FORK_CHANGES.md.
4. **Diff discipline.** Keep diffs in upstream files minimal;
   Auracle-native functionality goes in clearly bounded new crates.
5. **Commits.** Lowercase conventional subjects ≤72 chars,
   generalized wording, no Co-Authored-By trailers, one author.
6. **Honesty.** The IDE renders engine truths. It never fakes
   capability, numbers, or readiness; paper/dry-run are defaults.
7. **Building (macOS dev).** `cargo build -p zed --features
   gpui_platform/runtime_shaders` from the repo root — no full Xcode
   needed for dev builds (runtime shader compilation); release
   builds use precompiled shaders in CI. Build into the LOCAL
   `target/` only; never point CARGO_TARGET_DIR at another checkout
   — a stale external cache means you may run a binary that does not
   match this tree.
