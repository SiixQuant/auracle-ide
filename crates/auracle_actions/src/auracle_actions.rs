//! The verb layer (L2) — the one place "what can a user do, and is it allowed"
//! is decided, so every shell (Desk, Copilot, Flow) calls one gate instead of
//! forking drift-prone copies. This subsumes the older `auracle_deploy_gate` and
//! `auracle_cockpit_state::*_affordance` with one richer, honest model.
//!
//! Two safety laws are encoded here, not in any shell:
//! - **Real money is engine-authoritative.** A live deploy is permitted only when
//!   the engine says so, re-checked FRESH at click time. The client gate is a
//!   fail-closed MIRROR for instant UX — never the source of truth.
//! - **Never act on an unverified permission.** Live permission is a TRI-STATE
//!   (`Allowed`/`Denied`/`Unknown`); an engine outage / 401 / malformed body is
//!   `Unknown`, which BLOCKS the deploy with an honest "couldn't verify" — it does
//!   NOT silently fall through to a paper deploy the user didn't ask for.

use std::sync::Arc;

/// The verbs a user can invoke. Shared across all shells; each shell differs only
/// in how a verb is summoned and how its result is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Connect,
    CreateStrategy,
    Backtest,
    Validate,
    Deploy,
    Monitor,
    Cancel,
}

/// Whether the engine permits a LIVE (real-money) deploy right now. Tri-state on
/// purpose: a missing field, malformed body, or transport error is `Unknown`, not
/// `Denied` — the caller must surface "couldn't verify", never quietly downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePermission {
    Allowed,
    Denied,
    Unknown,
}

/// Parse the engine `/ui/api/capabilities` body into a [`LivePermission`]. Only an
/// explicit `live_allowed: true`/`false` yields Allowed/Denied; anything else
/// (absent, null, non-bool) is `Unknown` so a degraded payload can't read as a
/// confident verdict.
pub fn live_permission_from_capabilities(value: &serde_json::Value) -> LivePermission {
    match value.get("live_allowed").and_then(|v| v.as_bool()) {
        Some(true) => LivePermission::Allowed,
        Some(false) => LivePermission::Denied,
        None => LivePermission::Unknown,
    }
}

/// Re-check live permission against the engine at click time. Any error becomes
/// `Unknown` (NOT `Denied`) so the caller blocks honestly rather than silently
/// paper-deploying during an outage.
pub async fn poll_live_permission(http: Arc<dyn http_client::HttpClient>) -> LivePermission {
    match auracle_connections::get_json(http, "/ui/api/capabilities").await {
        Ok(value) => live_permission_from_capabilities(&value),
        Err(_) => LivePermission::Unknown,
    }
}

/// What a Deploy click should do, given whether a live deploy is already armed and
/// the FRESH live permission re-checked at click time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployDecision {
    /// First click on a live-capable strategy: arm a confirmation, never auto-send live.
    ArmConfirm,
    /// Confirming click while live is still permitted: send the live deploy.
    SubmitLive,
    /// Live explicitly not permitted: a paper deploy is safe to send immediately.
    SubmitPaper,
    /// Live permission could not be verified (engine unreachable / malformed):
    /// block and ask the user to retry. NEVER a silent paper or live deploy.
    BlockedUnverified,
}

/// Decide what a Deploy click does. Pure, so the real-money contract is unit-tested
/// in isolation. The `Unknown` arm is the honesty fix: an unverifiable permission
/// blocks rather than silently downgrading to paper.
pub fn decide_deploy(awaiting_confirm: bool, permission: LivePermission) -> DeployDecision {
    match (awaiting_confirm, permission) {
        (false, LivePermission::Allowed) => DeployDecision::ArmConfirm,
        (true, LivePermission::Allowed) => DeployDecision::SubmitLive,
        (_, LivePermission::Denied) => DeployDecision::SubmitPaper,
        (_, LivePermission::Unknown) => DeployDecision::BlockedUnverified,
    }
}

/// Whether a verb can be invoked right now, and if not, the honest reason. Shells
/// render `Disabled { reason }` as a tooltip/inline hint — never a dead control
/// with no explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Affordance {
    /// Invocable now.
    Ready,
    /// Invocable but requires a confirming second action (e.g. armed live deploy).
    NeedsConfirm,
    /// Not invocable; `reason` is shown to the user.
    Disabled { reason: String },
}

impl Affordance {
    pub fn disabled(reason: impl Into<String>) -> Self {
        Affordance::Disabled {
            reason: reason.into(),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Affordance::Ready)
    }
}

/// `connect` is invocable whenever the engine is reachable; otherwise it's the one
/// verb whose disabled-reason points at the engine itself.
pub fn connect_affordance(engine_reachable: bool) -> Affordance {
    if engine_reachable {
        Affordance::Ready
    } else {
        Affordance::disabled("Engine unreachable — start it from the launcher, then retry.")
    }
}

/// `backtest` needs a selected strategy and nothing already running.
pub fn backtest_affordance(has_strategy: bool, in_flight: bool) -> Affordance {
    if !has_strategy {
        Affordance::disabled("Select or create a strategy to backtest.")
    } else if in_flight {
        Affordance::disabled("A backtest is already running.")
    } else {
        Affordance::Ready
    }
}

/// `validate` needs a selected strategy.
pub fn validate_affordance(has_strategy: bool) -> Affordance {
    if has_strategy {
        Affordance::Ready
    } else {
        Affordance::disabled("Select or create a strategy to validate.")
    }
}

/// The Deploy verb's affordance, derived from the same decision used to act, so the
/// button's appearance and behavior can never disagree. Requires a strategy; an
/// unverifiable live permission disables with an honest reason rather than offering
/// a button that would silently paper-deploy.
pub fn deploy_affordance(
    has_strategy: bool,
    awaiting_confirm: bool,
    permission: LivePermission,
) -> Affordance {
    if !has_strategy {
        return Affordance::disabled("Backtest a strategy before deploying it.");
    }
    match decide_deploy(awaiting_confirm, permission) {
        DeployDecision::ArmConfirm | DeployDecision::SubmitLive => Affordance::NeedsConfirm,
        DeployDecision::SubmitPaper => Affordance::Ready,
        DeployDecision::BlockedUnverified => {
            Affordance::disabled("Couldn't verify live permission — check the engine and retry.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_permission_parses_from_absent_or_malformed() {
        assert_eq!(
            live_permission_from_capabilities(&serde_json::json!({ "live_allowed": true })),
            LivePermission::Allowed
        );
        assert_eq!(
            live_permission_from_capabilities(&serde_json::json!({ "live_allowed": false })),
            LivePermission::Denied
        );
        // Absent, null, or non-bool → Unknown, never a confident verdict.
        assert_eq!(
            live_permission_from_capabilities(&serde_json::json!({})),
            LivePermission::Unknown
        );
        assert_eq!(
            live_permission_from_capabilities(&serde_json::json!({ "live_allowed": "yes" })),
            LivePermission::Unknown
        );
    }

    #[test]
    fn deploy_never_auto_sends_live() {
        // First click, permitted → arm only.
        assert_eq!(
            decide_deploy(false, LivePermission::Allowed),
            DeployDecision::ArmConfirm
        );
        // Confirming click, still permitted → live.
        assert_eq!(
            decide_deploy(true, LivePermission::Allowed),
            DeployDecision::SubmitLive
        );
        // Explicitly not permitted → safe paper, either click.
        assert_eq!(
            decide_deploy(false, LivePermission::Denied),
            DeployDecision::SubmitPaper
        );
        assert_eq!(
            decide_deploy(true, LivePermission::Denied),
            DeployDecision::SubmitPaper
        );
    }

    #[test]
    fn unverified_permission_blocks_instead_of_silent_paper() {
        // The audit's real-money bug: an outage must NOT fall through to paper.
        assert_eq!(
            decide_deploy(false, LivePermission::Unknown),
            DeployDecision::BlockedUnverified
        );
        assert_eq!(
            decide_deploy(true, LivePermission::Unknown),
            DeployDecision::BlockedUnverified
        );
    }

    #[test]
    fn deploy_affordance_tracks_the_decision() {
        // With a strategy: appearance follows the decision exactly.
        assert!(matches!(
            deploy_affordance(true, false, LivePermission::Allowed),
            Affordance::NeedsConfirm
        )); // arm
        assert!(matches!(
            deploy_affordance(true, true, LivePermission::Allowed),
            Affordance::NeedsConfirm
        )); // confirm/live
        assert!(deploy_affordance(true, false, LivePermission::Denied).is_ready()); // paper
        assert!(matches!(
            deploy_affordance(true, false, LivePermission::Unknown),
            Affordance::Disabled { .. }
        )); // can't verify → blocked, not silent paper
    }

    #[test]
    fn no_strategy_disables_money_and_run_verbs() {
        assert!(matches!(
            backtest_affordance(false, false),
            Affordance::Disabled { .. }
        ));
        assert!(matches!(
            validate_affordance(false),
            Affordance::Disabled { .. }
        ));
        assert!(matches!(
            deploy_affordance(false, false, LivePermission::Denied),
            Affordance::Disabled { .. }
        ));
        assert!(backtest_affordance(true, false).is_ready());
        assert!(!backtest_affordance(true, true).is_ready());
    }

    #[test]
    fn connect_points_at_the_engine_when_down() {
        assert!(connect_affordance(true).is_ready());
        assert!(matches!(
            connect_affordance(false),
            Affordance::Disabled { .. }
        ));
    }
}
