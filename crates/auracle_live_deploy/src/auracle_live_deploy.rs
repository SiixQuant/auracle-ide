//! Pure reducer for the Live Deploy wizard + Live Algorithms view.
//!
//! gpui-free so every decision is unit-tested in isolation. The GPUI panels
//! (the wizard modal + the Live Algorithms Desk view) wrap this state and call
//! these functions for all decisions — they invent nothing.
//!
//! Mirrors the engine's invariants (docs/specs/live-deploy in the SiixQuant/
//! Auracle repo): a LIVE deployment cannot deploy without a positive AUM
//! (the keystone), and the lifecycle verbs available on a running algorithm
//! match the engine's state machine.

// ── Wizard form state ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Paper,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeKind {
    Local,
    Oci,
    Aws,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WizardState {
    pub mode: Mode,
    pub broker: Option<String>,
    /// Starting capital. REQUIRED for live (the deploy keystone).
    pub aum: Option<f64>,
    pub compute_kind: ComputeKind,
    pub resolution: String,
    pub data_providers: Vec<String>,
    pub auto_restart: bool,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            mode: Mode::Paper,
            broker: None,
            aum: None,
            compute_kind: ComputeKind::Local,
            resolution: "minute".to_string(),
            data_providers: Vec::new(),
            auto_restart: true,
        }
    }
}

/// The result of checking whether the wizard can deploy: a single boolean the
/// Deploy button binds its enabled-state to, plus the human reasons it's
/// blocked (shown inline so the user knows what to fix).
#[derive(Debug, Clone, PartialEq)]
pub struct DeployReadiness {
    pub ready: bool,
    pub reasons: Vec<String>,
}

/// Decide whether the Deploy button is enabled, mirroring the engine preflight.
/// The keystone: a LIVE deployment with no positive AUM can never deploy.
pub fn deploy_readiness(state: &WizardState) -> DeployReadiness {
    let mut reasons = Vec::new();

    if state.broker.as_deref().unwrap_or("").is_empty() {
        reasons.push("Select a brokerage.".to_string());
    }
    if state.data_providers.is_empty() {
        reasons.push("Select at least one data source.".to_string());
    }
    if state.mode == Mode::Live && !(state.aum.unwrap_or(0.0) > 0.0) {
        reasons.push("Enter the starting capital (AUM) — required for live.".to_string());
    }

    DeployReadiness {
        ready: reasons.is_empty(),
        reasons,
    }
}

pub fn can_deploy(state: &WizardState) -> bool {
    deploy_readiness(state).ready
}

// ── Live Algorithms list / detail ────────────────────────────────────

/// A lifecycle action offered on a live algorithm. Maps 1:1 to the engine's
/// stop / liquidate / restart verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stop,
    Liquidate,
    Restart,
}

/// Which actions the detail view should enable for a deployment in `state`.
/// Mirrors the engine lifecycle: stop only while running; restart only from a
/// halted (resumable) state; liquidate from any of those; nothing once
/// liquidating/archived.
pub fn available_actions(state: &str) -> Vec<Action> {
    match state {
        "running" => vec![Action::Stop, Action::Liquidate],
        "stopped" | "errored" => vec![Action::Restart, Action::Liquidate],
        // preflight/provisioning/starting/restarting: in-flight, no verbs yet.
        // liquidating/archived: terminal-ish, nothing to offer.
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveRow {
    pub id: i64,
    pub name: String,
    pub state: String,
    pub broker: String,
    pub aum: Option<f64>,
    pub return_pct: Option<f64>,
}

/// Format a return for the row, honestly: `None` (no marks yet) renders as a
/// dash, never a fabricated 0.0%.
pub fn format_return(return_pct: Option<f64>) -> String {
    match return_pct {
        Some(v) => format!("{:+.2}%", v),
        None => "—".to_string(),
    }
}

/// A short status dot label the view colors. Pure mapping, no guessing.
pub fn status_label(state: &str) -> &'static str {
    match state {
        "running" => "Running",
        "stopped" => "Stopped",
        "errored" => "Error",
        "liquidating" => "Liquidating",
        "archived" => "Archived",
        "preflight" | "provisioning" | "starting" | "restarting" => "Starting",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_without_aum_blocks_deploy() {
        let mut s = WizardState {
            mode: Mode::Live,
            broker: Some("ibkr".into()),
            data_providers: vec!["alpaca".into()],
            ..Default::default()
        };
        let r = deploy_readiness(&s);
        assert!(!r.ready);
        assert!(r.reasons.iter().any(|x| x.to_lowercase().contains("capital")));

        s.aum = Some(100_000.0);
        assert!(can_deploy(&s));
    }

    #[test]
    fn paper_does_not_require_aum() {
        let s = WizardState {
            mode: Mode::Paper,
            broker: Some("alpaca".into()),
            data_providers: vec!["alpaca".into()],
            aum: None,
            ..Default::default()
        };
        assert!(can_deploy(&s));
    }

    #[test]
    fn missing_broker_or_data_blocks() {
        let s = WizardState {
            mode: Mode::Paper,
            ..Default::default()
        };
        let r = deploy_readiness(&s);
        assert!(!r.ready);
        assert!(r.reasons.iter().any(|x| x.contains("brokerage")));
        assert!(r.reasons.iter().any(|x| x.contains("data source")));
    }

    #[test]
    fn default_is_paper_minute_local_autorestart() {
        let s = WizardState::default();
        assert_eq!(s.mode, Mode::Paper);
        assert_eq!(s.resolution, "minute");
        assert_eq!(s.compute_kind, ComputeKind::Local);
        assert!(s.auto_restart);
    }

    #[test]
    fn actions_track_lifecycle() {
        assert_eq!(available_actions("running"), vec![Action::Stop, Action::Liquidate]);
        assert_eq!(available_actions("stopped"), vec![Action::Restart, Action::Liquidate]);
        assert_eq!(available_actions("errored"), vec![Action::Restart, Action::Liquidate]);
        assert!(available_actions("starting").is_empty());
        assert!(available_actions("archived").is_empty());
    }

    #[test]
    fn return_and_status_are_honest() {
        assert_eq!(format_return(None), "—");
        assert_eq!(format_return(Some(0.41)), "+0.41%");
        assert_eq!(format_return(Some(-2.1)), "-2.10%");
        assert_eq!(status_label("running"), "Running");
        assert_eq!(status_label("preflight"), "Starting");
        assert_eq!(status_label("weird"), "Unknown");
    }
}
