//! Pure ViewState for Live Deploy — the gpui-free seam.
//!
//! This crate models everything the Live Deploy wizard, the Live Algorithms
//! dashboard, and the per-strategy ledger need, with **no gpui dependency** so
//! it builds and unit-tests on a machine without Xcode. The GPUI panel renders
//! these types and dispatches the reducer methods; all the rules (the
//! AUM-required gate, which lifecycle actions a state allows, the deploy-request
//! body, the ledger shape) live here where they can be tested.
//!
//! Types mirror the engine's Live Deploy API
//! (`auracle/houston/routes/live.py`, surfaced under `/ui/api`):
//!   GET  /deployments                  -> Vec<Deployment>
//!   GET  /deployments/{id}             -> Deployment
//!   GET  /deployments/{id}/orders      -> DeploymentOrders
//!   POST /deploy/live                  <- DeployRequest (to_request())
//!   POST /deployments/{id}/{verb}      -> stop | liquidate | restart

use serde::{Deserialize, Serialize};

// ── Engine wire types ───────────────────────────────────────────────────

/// One row in the per-deployment book (positions[] on the detail + ledger).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    #[serde(default)]
    pub quantity: f64,
    #[serde(default)]
    pub avg_cost: f64,
}

/// A live/paper deployment as the engine reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub strategy_path: String,
    #[serde(default)]
    pub strategy_cls: Option<String>,
    #[serde(default)]
    pub broker: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub aum: Option<f64>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub equity: Option<f64>,
    #[serde(default)]
    pub return_pct: Option<f64>,
    #[serde(default)]
    pub positions: Vec<Position>,
}

/// One order row in a deployment's ledger.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LedgerOrder {
    #[serde(default)]
    pub id: i64,
    pub symbol: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub quantity: Option<f64>,
    #[serde(default)]
    pub filled_quantity: Option<f64>,
    #[serde(default)]
    pub avg_fill_price: Option<f64>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub broker: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// `GET /deployments/{id}/orders` — the per-strategy ledger payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeploymentOrders {
    pub deployment_id: i64,
    #[serde(default)]
    pub orders: Vec<LedgerOrder>,
    #[serde(default)]
    pub positions: Vec<Position>,
}

// ── Deploy wizard reducer ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Paper,
    Live,
}

impl Mode {
    pub fn wire(self) -> &'static str {
        match self {
            Mode::Paper => "paper",
            Mode::Live => "live",
        }
    }
}

/// Where the deployment runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compute {
    Local,
    Oci,
    Aws,
}

impl Compute {
    pub fn wire(self) -> &'static str {
        match self {
            Compute::Local => "local",
            Compute::Oci => "oci",
            Compute::Aws => "aws",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Compute::Local => "This machine",
            Compute::Oci => "Oracle Cloud",
            Compute::Aws => "AWS",
        }
    }
}

/// The mutable form behind the Deploy wizard tab. The view binds inputs to
/// these fields and calls [`DeployWizard::validate`] to gate the Deploy button.
#[derive(Debug, Clone, PartialEq)]
pub struct DeployWizard {
    pub name: String,
    pub strategy_path: String,
    pub strategy_cls: String,
    pub mode: Mode,
    pub broker: Option<String>,
    /// The single most important field. Required (>0) for a live deployment.
    pub aum: Option<f64>,
    pub compute: Compute,
    pub data_providers: Vec<String>,
    pub resolution: String,
    pub auto_restart: bool,
}

impl Default for DeployWizard {
    fn default() -> Self {
        Self {
            name: String::new(),
            strategy_path: String::new(),
            strategy_cls: String::new(),
            mode: Mode::Paper,
            broker: None,
            aum: None,
            compute: Compute::Local,
            data_providers: Vec::new(),
            resolution: "minute".into(),
            auto_restart: true,
        }
    }
}

impl DeployWizard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every reason the deployment can't go out yet, in display order. Empty =
    /// ready. Mirrors the engine's `preflight_live` so the UI fails the same
    /// way the server would (no surprise 400 after the user clicks Deploy).
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.name.trim().is_empty() {
            errs.push("Give the deployment a name.".into());
        }
        if self.strategy_path.trim().is_empty() || self.strategy_cls.trim().is_empty() {
            errs.push("Pick a strategy to deploy.".into());
        }
        if self.broker.as_deref().unwrap_or("").is_empty() {
            errs.push("Choose a brokerage.".into());
        }
        if self.mode == Mode::Live {
            match self.aum {
                Some(a) if a > 0.0 => {}
                _ => errs.push(
                    "Starting capital (AUM) is required for live deployment and must be > 0."
                        .into(),
                ),
            }
        }
        if self.compute != Compute::Local && self.data_providers.is_empty() {
            errs.push("A cloud deployment needs at least one data source.".into());
        }
        errs
    }

    pub fn can_deploy(&self) -> bool {
        self.validate().is_empty()
    }

    /// The `POST /deploy/live` body. Only call when [`can_deploy`] is true.
    pub fn to_request(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name.trim(),
            "strategy_path": self.strategy_path.trim(),
            "strategy_cls": self.strategy_cls.trim(),
            "broker": self.broker.clone().unwrap_or_default(),
            "mode": self.mode.wire(),
            "aum": self.aum,
            "data_providers": self.data_providers,
            "resolution": self.resolution,
            "compute_kind": self.compute.wire(),
            "risk": serde_json::Map::new(),
            "notifications": serde_json::Value::Array(vec![]),
            "resilience": { "auto_restart": self.auto_restart },
        })
    }
}

// ── Live Algorithms dashboard reducer ───────────────────────────────────

/// A lifecycle action a user can take on a running/halted deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stop,
    Liquidate,
    Restart,
}

impl Action {
    pub fn verb(self) -> &'static str {
        match self {
            Action::Stop => "stop",
            Action::Liquidate => "liquidate",
            Action::Restart => "restart",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Action::Stop => "Stop",
            Action::Liquidate => "Liquidate",
            Action::Restart => "Restart",
        }
    }
    /// True for actions that flatten or permanently change the book — the view
    /// should confirm before dispatching.
    pub fn is_destructive(self) -> bool {
        matches!(self, Action::Liquidate)
    }
}

/// Actions the engine lifecycle allows from `state` (mirrors
/// `auracle/live/lifecycle.py` ALLOWED). Anything else the API would 409.
pub fn available_actions(state: &str) -> Vec<Action> {
    match state {
        "running" | "starting" | "restarting" => vec![Action::Stop, Action::Liquidate],
        "stopped" | "errored" => vec![Action::Restart, Action::Liquidate],
        _ => Vec::new(), // draft / preflight / provisioning / liquidating / archived
    }
}

/// The control-plane path for `action` on `deployment_id`.
pub fn verb_endpoint(deployment_id: i64, action: Action) -> String {
    format!("/ui/api/deployments/{deployment_id}/{}", action.verb())
}

/// How a state should read + badge in the dashboard.
pub fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "Live",
        "starting" => "Starting",
        "restarting" => "Restarting",
        "preflight" | "provisioning" => "Preparing",
        "stopped" => "Stopped",
        "liquidating" => "Liquidating",
        "errored" => "Errored",
        "archived" => "Archived",
        "draft" => "Draft",
        _ => "Unknown",
    }
}

/// True while the deployment counts as live (drives the dot color / "is it
/// trading" affordance). Mirrors lifecycle.ACTIVE_STATES.
pub fn is_active(state: &str) -> bool {
    matches!(
        state,
        "preflight" | "provisioning" | "starting" | "running" | "restarting"
    )
}

#[derive(Debug, Clone, Default)]
pub struct LiveAlgorithms {
    pub rows: Vec<Deployment>,
    pub selected: Option<i64>,
}

impl LiveAlgorithms {
    pub fn set_rows(&mut self, rows: Vec<Deployment>) {
        // Drop a stale selection if the deployment is gone.
        if let Some(sel) = self.selected {
            if !rows.iter().any(|d| d.id == sel) {
                self.selected = None;
            }
        }
        self.rows = rows;
    }

    pub fn select(&mut self, id: i64) {
        self.selected = Some(id);
    }

    pub fn selected_deployment(&self) -> Option<&Deployment> {
        let sel = self.selected?;
        self.rows.iter().find(|d| d.id == sel)
    }

    pub fn active_count(&self) -> usize {
        self.rows.iter().filter(|d| is_active(&d.state)).count()
    }
}

/// Format a return percent for a cell, e.g. `+12.34%` / `-3.10%` / `—`.
pub fn format_return(return_pct: Option<f64>) -> String {
    match return_pct {
        Some(p) => format!("{:+.2}%", p),
        None => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_live() -> DeployWizard {
        DeployWizard {
            name: "Momentum".into(),
            strategy_path: "strategies.momentum".into(),
            strategy_cls: "Momentum".into(),
            mode: Mode::Live,
            broker: Some("clearstreet".into()),
            aum: Some(100_000.0),
            ..Default::default()
        }
    }

    #[test]
    fn live_requires_positive_aum() {
        let mut w = ready_live();
        assert!(w.can_deploy());

        w.aum = None;
        assert!(!w.can_deploy());
        assert!(w.validate().iter().any(|e| e.contains("Starting capital")));

        w.aum = Some(0.0);
        assert!(!w.can_deploy());
    }

    #[test]
    fn paper_does_not_require_aum() {
        let mut w = ready_live();
        w.mode = Mode::Paper;
        w.aum = None;
        assert!(w.can_deploy(), "{:?}", w.validate());
    }

    #[test]
    fn missing_strategy_or_broker_blocks_deploy() {
        let mut w = ready_live();
        w.broker = None;
        assert!(w.validate().iter().any(|e| e.contains("brokerage")));

        let mut w = ready_live();
        w.strategy_cls = String::new();
        assert!(w.validate().iter().any(|e| e.contains("strategy")));
    }

    #[test]
    fn cloud_requires_a_data_source() {
        let mut w = ready_live();
        w.compute = Compute::Oci;
        assert!(w.validate().iter().any(|e| e.contains("data source")));
        w.data_providers.push("alpaca".into());
        assert!(w.can_deploy());
    }

    #[test]
    fn to_request_carries_the_keystone_fields() {
        let body = ready_live().to_request();
        assert_eq!(body["mode"], "live");
        assert_eq!(body["broker"], "clearstreet");
        assert_eq!(body["aum"], 100_000.0);
        assert_eq!(body["compute_kind"], "local");
        assert_eq!(body["resilience"]["auto_restart"], true);
    }

    #[test]
    fn actions_follow_lifecycle() {
        assert_eq!(
            available_actions("running"),
            vec![Action::Stop, Action::Liquidate]
        );
        assert_eq!(
            available_actions("stopped"),
            vec![Action::Restart, Action::Liquidate]
        );
        assert_eq!(
            available_actions("errored"),
            vec![Action::Restart, Action::Liquidate]
        );
        assert!(available_actions("archived").is_empty());
        assert!(available_actions("preflight").is_empty());
    }

    #[test]
    fn verb_endpoint_and_destructive() {
        assert_eq!(
            verb_endpoint(7, Action::Liquidate),
            "/ui/api/deployments/7/liquidate"
        );
        assert!(Action::Liquidate.is_destructive());
        assert!(!Action::Stop.is_destructive());
    }

    #[test]
    fn selection_drops_when_row_disappears() {
        let mut algos = LiveAlgorithms::default();
        algos.set_rows(vec![Deployment {
            id: 1,
            state: "running".into(),
            ..Default::default()
        }]);
        algos.select(1);
        assert!(algos.selected_deployment().is_some());
        assert_eq!(algos.active_count(), 1);

        algos.set_rows(vec![Deployment {
            id: 2,
            state: "stopped".into(),
            ..Default::default()
        }]);
        assert!(algos.selected_deployment().is_none());
        assert_eq!(algos.active_count(), 0);
    }

    #[test]
    fn deserializes_engine_ledger_payload() {
        let json = r#"{
            "deployment_id": 3,
            "orders": [
                {"id": 10, "symbol": "SPY", "action": "BUY", "quantity": 10.0,
                 "filled_quantity": 10.0, "avg_fill_price": 100.5, "status": "filled",
                 "broker": "clearstreet", "created_at": "2026-06-30T12:00:00+00:00"}
            ],
            "positions": [{"symbol": "SPY", "quantity": 10.0, "avg_cost": 100.5}]
        }"#;
        let parsed: DeploymentOrders = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.deployment_id, 3);
        assert_eq!(parsed.orders.len(), 1);
        assert_eq!(parsed.orders[0].symbol, "SPY");
        assert_eq!(parsed.positions[0].quantity, 10.0);
    }

    #[test]
    fn format_return_handles_missing() {
        assert_eq!(format_return(Some(12.345)), "+12.35%");
        assert_eq!(format_return(Some(-3.1)), "-3.10%");
        assert_eq!(format_return(None), "—");
    }
}
