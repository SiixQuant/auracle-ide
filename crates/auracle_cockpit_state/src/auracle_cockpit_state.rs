//! Gpui-free decision logic for the per-strategy cockpit toolbar.
//!
//! The toolbar's render code used to compute its own state inline — overloading
//! a single `Unknown` feed value as loading, error, and not-yet-polled all at
//! once, and hand-rolling the Deploy button as a separate code path from the
//! other two verbs (which then drifted). This crate lifts those decisions out so
//! they are pure and unit-tested: `serde_json` only, no `gpui`, no `http_client`
//! (the async pollers stay in the gpui crate and call these). See `RUBRIC.md` in
//! the `auracle_view_state` crate (items b, c, e).
//!
//! Honesty is enforced by the tests: the deploy affordance is only "dangerous"
//! at the live-confirm step and never reads a bare "Deploy" while live is
//! allowed; a reachable-but-odd engine payload is `NotAStrategy`, never confused
//! with `EngineUnreachable`.

/// Where a poll-shaped status currently sits. `Unknown` is no longer overloaded
/// as "loading" — `Polling` is the in-flight state, and `Unknown` means the
/// engine genuinely returned no `data` block to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedState {
    /// A poll is in flight; the toolbar shows a "Checking data…" skeleton.
    Polling,
    /// The strategy's universe is fully available.
    Ok,
    /// The engine reported missing symbols for the strategy's universe.
    Missing,
    /// The engine returned no `data` block — nothing to report.
    Unknown,
}

/// Derive feed presence from the deploy-preflight `data` block. This is the pure
/// core the async `poll_feed` wrapper calls; it never returns `Polling` (that is
/// the wrapper's pre-fetch state). `Ok` only when the `missing` array is empty,
/// `Missing` when it is non-empty, and `Unknown` only when there is no `data`
/// block at all — never `Ok` by default.
pub fn feed_from_preflight(value: &serde_json::Value) -> FeedState {
    match value.get("data") {
        Some(data) => {
            let missing = data
                .get("missing")
                .and_then(|missing| missing.as_array())
                .map(|missing| missing.len())
                .unwrap_or(0);
            if missing == 0 {
                FeedState::Ok
            } else {
                FeedState::Missing
            }
        }
        None => FeedState::Unknown,
    }
}

/// The resolve outcome for the file in the editor. `EngineUnreachable` is a
/// transport error (the engine couldn't be reached); a reachable engine whose
/// listing simply doesn't contain this file is `NotAStrategy` — the two are
/// never conflated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveState {
    /// Resolution is in flight.
    Resolving,
    /// The file is a known strategy at this dotted path.
    Strategy(String),
    /// The engine is reachable but this file is not a registered strategy.
    NotAStrategy,
    /// The engine couldn't be reached to resolve the file.
    EngineUnreachable,
}

/// Derive the dotted Python module of a strategy file relative to a
/// `strategies/` root, e.g. `/opt/auracle/strategies/example_ma.py` ->
/// `strategies.example_ma`. Returns `None` when the file is not under a
/// `strategies/` directory or isn't a `.py` file.
pub fn strategy_module_from_path(file_path: &std::path::Path) -> Option<String> {
    let components: Vec<String> = file_path
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
        .collect();
    let strategies_index = components.iter().rposition(|part| part == "strategies")?;
    let mut module_parts: Vec<String> = components[strategies_index..].to_vec();
    let last = module_parts.last_mut()?;
    *last = last.strip_suffix(".py")?.to_owned();
    Some(module_parts.join("."))
}

/// Pure matcher over the engine's strategy listing (engine reachable). Yields
/// `Strategy` on a match, `NotAStrategy` otherwise. It never returns
/// `EngineUnreachable`: that state belongs only to the transport-error path in
/// the async wrapper, so a reachable engine with an unexpected payload shape is
/// honestly "not a strategy".
pub fn match_strategy(listing: &serde_json::Value, module: &str) -> ResolveState {
    let module_prefix = format!("{module}.");
    let matched = listing
        .get("strategies")
        .and_then(|strategies| strategies.as_array())
        .and_then(|strategies| {
            strategies
                .iter()
                .filter_map(|item| item.get("path").and_then(|path| path.as_str()))
                .find(|path| path.starts_with(&module_prefix) || *path == module)
                .map(str::to_owned)
        });
    match matched {
        Some(path) => ResolveState::Strategy(path),
        None => ResolveState::NotAStrategy,
    }
}

/// One toolbar verb's full render decision. A single struct for all three verbs
/// (backtest, validate, deploy) so there is one button renderer and no drift —
/// deploy's danger styling and tooltip arms ride the same path as the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionAffordance {
    /// The button label, including deploy's "Deploy (paper)" / "Confirm live
    /// deploy" variants.
    pub label: String,
    /// Whether the verb can run (requires a resolved strategy).
    pub enabled: bool,
    /// Whether to render the button with error/danger styling. Only the live
    /// deploy confirm step is dangerous.
    pub danger: bool,
    /// The hover explanation shown when the verb is unavailable.
    pub tooltip: Option<String>,
}

/// A verb that only needs a resolved strategy to run (backtest, validate). The
/// label is fixed; when unresolved it is disabled with an honest tooltip.
fn simple_affordance(label: &str, resolve: &ResolveState) -> ActionAffordance {
    match resolve {
        ResolveState::Strategy(_) => ActionAffordance {
            label: label.to_string(),
            enabled: true,
            danger: false,
            tooltip: None,
        },
        other => ActionAffordance {
            label: label.to_string(),
            enabled: false,
            danger: false,
            tooltip: Some(unresolved_message(other).to_string()),
        },
    }
}

/// The Backtest verb's affordance.
pub fn backtest_affordance(resolve: &ResolveState) -> ActionAffordance {
    simple_affordance("Backtest", resolve)
}

/// The Validate verb's affordance.
pub fn validate_affordance(resolve: &ResolveState) -> ActionAffordance {
    simple_affordance("Validate", resolve)
}

/// The Deploy verb's affordance. The label is "Deploy (paper)" whenever live is
/// not allowed (never a bare "Deploy" that hides which target it hits); plain
/// "Deploy" once live is allowed; and "Confirm live deploy" — the only danger
/// state — while awaiting the live confirmation click. Enabled requires a
/// resolved strategy.
pub fn deploy_affordance(
    resolve: &ResolveState,
    live_allowed: bool,
    awaiting_live_confirm: bool,
) -> ActionAffordance {
    let label = if awaiting_live_confirm {
        "Confirm live deploy"
    } else if live_allowed {
        "Deploy"
    } else {
        "Deploy (paper)"
    };
    match resolve {
        ResolveState::Strategy(_) => ActionAffordance {
            label: label.to_string(),
            enabled: true,
            danger: awaiting_live_confirm,
            tooltip: None,
        },
        other => ActionAffordance {
            label: label.to_string(),
            enabled: false,
            danger: false,
            tooltip: Some(unresolved_message(other).to_string()),
        },
    }
}

/// The tooltip/banner copy for an unresolved verb, in one place. Each non-ready
/// resolve state reads distinctly so the user knows why a verb is unavailable
/// without guessing; `Strategy` has nothing to explain.
pub fn unresolved_message(resolve: &ResolveState) -> &'static str {
    match resolve {
        ResolveState::Resolving => "Resolving this file…",
        ResolveState::EngineUnreachable => "Can't reach the engine.",
        ResolveState::NotAStrategy => "This file isn't a registered strategy.",
        ResolveState::Strategy(_) => "",
    }
}

/// Whether the cockpit should show the engine-unreachable Retry banner. True
/// only for `EngineUnreachable`, which is a first-class error state with a retry,
/// not the same as "not a strategy".
pub fn show_unreachable_banner(resolve: &ResolveState) -> bool {
    matches!(resolve, ResolveState::EngineUnreachable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn match_strategy_distinguishes_match_from_no_match() {
        let listing = serde_json::json!({
            "strategies": [{ "path": "strategies.gap.OvernightGapReversion" }]
        });
        assert_eq!(
            match_strategy(&listing, "strategies.gap"),
            ResolveState::Strategy("strategies.gap.OvernightGapReversion".to_string())
        );
        assert_eq!(
            match_strategy(&listing, "strategies.other"),
            ResolveState::NotAStrategy
        );
        // A reachable engine with an unexpected shape is "not a strategy",
        // never confused with unreachable (which is the transport-error path).
        assert_eq!(
            match_strategy(&serde_json::json!({}), "strategies.gap"),
            ResolveState::NotAStrategy
        );
    }

    #[test]
    fn derives_module_from_strategies_path() {
        assert_eq!(
            strategy_module_from_path(Path::new("/opt/auracle/strategies/example_ma.py")),
            Some("strategies.example_ma".to_string())
        );
        assert_eq!(
            strategy_module_from_path(Path::new("/home/me/proj/strategies/sub/momentum.py")),
            Some("strategies.sub.momentum".to_string())
        );
    }

    #[test]
    fn rejects_non_strategy_paths() {
        assert_eq!(
            strategy_module_from_path(Path::new("/home/me/notes/scratch.py")),
            None
        );
        assert_eq!(
            strategy_module_from_path(Path::new("/opt/auracle/strategies/readme.txt")),
            None
        );
    }

    #[test]
    fn feed_from_preflight_reads_presence() {
        assert_eq!(
            feed_from_preflight(&serde_json::json!({ "data": { "missing": [] } })),
            FeedState::Ok
        );
        assert_eq!(
            feed_from_preflight(&serde_json::json!({ "data": { "missing": ["AAPL"] } })),
            FeedState::Missing
        );
        assert_eq!(
            feed_from_preflight(&serde_json::json!({})),
            FeedState::Unknown
        );
    }

    #[test]
    fn deploy_affordance_labels_and_danger() {
        let strategy = ResolveState::Strategy("strategies.gap.Reversion".to_string());

        let paper = deploy_affordance(&strategy, false, false);
        assert_eq!(paper.label, "Deploy (paper)");
        assert!(paper.enabled);
        assert!(!paper.danger);

        let live_ready = deploy_affordance(&strategy, true, false);
        assert_eq!(live_ready.label, "Deploy");
        assert!(live_ready.enabled);
        assert!(!live_ready.danger);

        let confirm = deploy_affordance(&strategy, true, true);
        assert_eq!(confirm.label, "Confirm live deploy");
        assert!(confirm.enabled);
        assert!(confirm.danger);

        let unresolved = deploy_affordance(&ResolveState::NotAStrategy, true, false);
        assert!(!unresolved.enabled);
        assert!(unresolved.tooltip.is_some());
    }

    #[test]
    fn backtest_and_validate_enabled_only_when_strategy() {
        let strategy = ResolveState::Strategy("strategies.gap.Reversion".to_string());
        assert!(backtest_affordance(&strategy).enabled);
        assert!(validate_affordance(&strategy).enabled);

        for unresolved in [
            ResolveState::Resolving,
            ResolveState::NotAStrategy,
            ResolveState::EngineUnreachable,
        ] {
            let backtest = backtest_affordance(&unresolved);
            assert!(!backtest.enabled);
            assert!(backtest.tooltip.is_some());
            let validate = validate_affordance(&unresolved);
            assert!(!validate.enabled);
            assert!(validate.tooltip.is_some());
        }

        // EngineUnreachable and NotAStrategy carry distinct copy.
        assert_ne!(
            backtest_affordance(&ResolveState::EngineUnreachable).tooltip,
            backtest_affordance(&ResolveState::NotAStrategy).tooltip
        );
    }

    #[test]
    fn unresolved_message_is_distinct_per_state() {
        let resolving = unresolved_message(&ResolveState::Resolving);
        let unreachable = unresolved_message(&ResolveState::EngineUnreachable);
        let not_a_strategy = unresolved_message(&ResolveState::NotAStrategy);
        assert_ne!(resolving, unreachable);
        assert_ne!(resolving, not_a_strategy);
        assert_ne!(unreachable, not_a_strategy);
    }

    #[test]
    fn unreachable_banner_only_for_engine_unreachable() {
        assert!(show_unreachable_banner(&ResolveState::EngineUnreachable));
        assert!(!show_unreachable_banner(&ResolveState::NotAStrategy));
        assert!(!show_unreachable_banner(&ResolveState::Resolving));
        assert!(!show_unreachable_banner(&ResolveState::Strategy(
            "strategies.gap.Reversion".to_string()
        )));
    }
}
