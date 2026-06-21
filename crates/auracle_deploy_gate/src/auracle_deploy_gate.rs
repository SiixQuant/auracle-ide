//! The shared live-deploy safety gate.
//!
//! A live (real-money) deploy is never sent on a first click: it must be ARMED
//! and then CONFIRMED, and `live_allowed` is re-checked FRESH against the engine
//! at click time (the context-resolve snapshot can be up to ~30s stale and this
//! decision can authorize real money). This module is the single source of truth
//! for that decision so every surface — the strategy cockpit and the Studio
//! results tab — shares one gate instead of forking a second, drift-prone copy.

use auracle_connections::get_json;

/// What a Deploy click should do, given whether a live deploy is already armed
/// (`awaiting_confirm`) and the FRESH `live_allowed` re-checked at click time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployDecision {
    /// First click on a live-capable strategy: arm a confirmation, never auto-send live.
    ArmConfirm,
    /// Confirming click while live is still permitted: send the live deploy.
    SubmitLive,
    /// Live not permitted (or revoked between arming and confirming): safe paper deploy.
    SubmitPaper,
}

/// Decide what a Deploy click does. Pure, so the real-money safety contract is
/// unit-tested in isolation.
pub fn decide_deploy(awaiting_confirm: bool, live_allowed: bool) -> DeployDecision {
    match (awaiting_confirm, live_allowed) {
        // Confirming click + still permitted → send the live deploy.
        (true, true) => DeployDecision::SubmitLive,
        // Live was revoked between arming and confirming → fall back to paper.
        (true, false) => DeployDecision::SubmitPaper,
        // First click, live permitted → arm a confirmation, never auto-send live.
        (false, true) => DeployDecision::ArmConfirm,
        // Live not permitted → a paper deploy is safe to send immediately.
        (false, false) => DeployDecision::SubmitPaper,
    }
}

/// Re-check, against the engine, whether a live deploy is permitted right now.
/// Fails closed (returns `false`) on any error so a degraded connection can
/// never silently authorize real money.
pub async fn poll_live_allowed(http: std::sync::Arc<dyn http_client::HttpClient>) -> bool {
    match get_json(http, "/ui/api/capabilities").await {
        Ok(value) => value
            .get("live_allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_gate_never_auto_sends_live() {
        // A first click never sends live — it only arms a confirmation.
        assert_eq!(decide_deploy(false, true), DeployDecision::ArmConfirm);
        // The confirming click sends live only while still permitted.
        assert_eq!(decide_deploy(true, true), DeployDecision::SubmitLive);
        // Live not permitted → a safe paper deploy, no confirmation dance.
        assert_eq!(decide_deploy(false, false), DeployDecision::SubmitPaper);
        // Live revoked between arming and confirming → fall back to paper.
        assert_eq!(decide_deploy(true, false), DeployDecision::SubmitPaper);
    }
}
