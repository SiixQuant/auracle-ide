//! The Strategy Cockpit: a per-strategy editor-toolbar item.
//!
//! When the active editor buffer is a `.py` strategy file, the cockpit shows
//! Backtest / Validate / Deploy buttons plus a data-feed indicator scoped to
//! that strategy. Each action calls the user's self-hosted Houston engine over
//! loopback through the shared `auracle_connections` transport (which resolves
//! the engine URL + API key from the launcher-provisioned connect config).
//!
//! The cockpit mirrors `BasedPyrightBanner` (a `ToolbarItemView` that self-hides
//! based on the active item being a `.py` editor) and the `BrokerWizard`'s
//! `cx.spawn` + `WeakEntity::update(..).ok()` async-update idiom.

use std::path::PathBuf;

use auracle_backtest_results::{BacktestSummary, open_backtest_results};
use auracle_connections::{get_json, post_json};
use auracle_deploy_gate::{DeployDecision, decide_deploy, poll_live_allowed};
use editor::Editor;
use gpui::{Context, EventEmitter, Task, WeakEntity};
use ui::{Tooltip, prelude::*};
use workspace::{ItemHandle, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace};

/// The outcome of resolving the active `.py` against the engine's listing.
/// Distinguishing these is what lets the cockpit be honest: "can't reach the
/// engine" and "this file isn't a strategy" are different problems.
#[derive(Clone, Debug, PartialEq)]
enum ResolveState {
    /// Resolve in flight.
    Resolving,
    /// Matched a dotted engine strategy path (`strategies.module.Class`).
    Strategy(String),
    /// Engine reachable, but no strategy matches this file.
    NotAStrategy,
    /// The engine could not be reached.
    EngineUnreachable,
}

/// Whether the cockpit is showing, and for which strategy.
enum CockpitVisibility {
    Hidden,
    Shown {
        /// Absolute path of the active `.py` buffer.
        file_path: PathBuf,
        /// Resolution of this file against the engine's strategy listing.
        resolve: ResolveState,
    },
}

/// The state of a single cockpit action.
#[derive(Clone)]
enum ActionState {
    Idle,
    Running,
    Ok(SharedString),
    Error(SharedString),
}

/// The per-strategy data-feed status. `Stale` is intentionally never produced:
/// the engine exposes per-strategy data *presence* (via the deploy preflight's
/// `data` block) but not per-symbol freshness, so the cockpit reports Ok /
/// Missing honestly and leaves staleness to the dedicated docks.
#[derive(Clone, Copy, PartialEq)]
enum FeedState {
    Unknown,
    Ok,
    Missing,
}

pub struct StrategyCockpit {
    /// Handle to the workspace, used to open the Studio results tab after a
    /// backtest completes.
    workspace: WeakEntity<Workspace>,
    visibility: CockpitVisibility,
    backtest: ActionState,
    validate: ActionState,
    deploy: ActionState,
    /// Whether the engine reports that live trading is allowed for the active
    /// broker (the same `live_allowed` truth the engine status chip renders as
    /// "live ok" vs "paper only"). Deploy is relabeled "Deploy (paper)" and
    /// only ever submits a paper deploy when this is false.
    deploy_live_allowed: bool,
    /// Two-step guard for live (real-money) deploys: armed on the first click
    /// when live is permitted, sent only on the confirming second click.
    /// Cleared when the active strategy/context changes.
    awaiting_live_confirm: bool,
    feed: FeedState,
    /// Holds the in-flight resolve/feed/capability poll so it is cancelled when
    /// the active item changes or the cockpit is dropped.
    _context_task: Option<Task<()>>,
    /// Holds the in-flight action request (backtest / validate / deploy) so a
    /// new click cancels the prior one and the task isn't dropped early.
    _action_task: Option<Task<()>>,
}

impl StrategyCockpit {
    pub fn new(workspace: WeakEntity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            visibility: CockpitVisibility::Hidden,
            backtest: ActionState::Idle,
            validate: ActionState::Idle,
            deploy: ActionState::Idle,
            deploy_live_allowed: false,
            awaiting_live_confirm: false,
            feed: FeedState::Unknown,
            _context_task: None,
            _action_task: None,
        }
    }

    fn active_strategy_path(&self) -> Option<&str> {
        match &self.visibility {
            CockpitVisibility::Shown {
                resolve: ResolveState::Strategy(path),
                ..
            } => Some(path.as_str()),
            _ => None,
        }
    }

    /// Resolve the active `.py` file to a dotted engine strategy path, then poll
    /// the data-feed status and the live-vs-paper capability for it. Strategy
    /// identity comes from the engine's own listing so we never invent a path:
    /// we derive the module from the file (relative to a `strategies/` root) and
    /// match it against the listing the engine returns.
    fn refresh_context(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        let Some(module) = strategy_module_from_path(&file_path) else {
            self.visibility = CockpitVisibility::Shown {
                file_path,
                resolve: ResolveState::NotAStrategy,
            };
            self.feed = FeedState::Unknown;
            cx.notify();
            return;
        };

        self.visibility = CockpitVisibility::Shown {
            file_path,
            resolve: ResolveState::Resolving,
        };
        self.feed = FeedState::Unknown;
        // A pending live-deploy confirmation must not carry across a context
        // change (different strategy/file).
        self.awaiting_live_confirm = false;
        cx.notify();

        let http = cx.http_client();
        self._context_task = Some(cx.spawn(async move |this, cx| {
            let resolution = resolve_strategy_path(http.clone(), &module).await;

            let (feed, live_allowed) = if let ResolveState::Strategy(strategy_path) = &resolution {
                let feed = poll_feed(http.clone(), strategy_path).await;
                let live_allowed = poll_live_allowed(http.clone()).await;
                (feed, live_allowed)
            } else {
                (FeedState::Unknown, false)
            };

            this.update(cx, |this, cx| {
                if let CockpitVisibility::Shown { resolve, .. } = &mut this.visibility {
                    *resolve = resolution;
                }
                this.feed = feed;
                this.deploy_live_allowed = live_allowed;
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_backtest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.active_strategy_path().map(str::to_owned) else {
            return;
        };
        let http = cx.http_client();
        let workspace = self.workspace.clone();
        let title = strategy_path.clone();
        self.backtest = ActionState::Running;
        cx.notify();
        self._action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let body = serde_json::json!({ "strategy_path": strategy_path });
            let result = post_json(http, "/backtest", body).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(value) => {
                        this.backtest = ActionState::Ok(backtest_summary(&value));
                        // Open the results in a Studio tab beside the code.
                        let summary = BacktestSummary::from_engine(title, &value);
                        workspace
                            .update(cx, |workspace, cx| {
                                open_backtest_results(workspace, summary, window, cx);
                            })
                            .ok();
                    }
                    Err(error) => {
                        this.backtest =
                            ActionState::Error(format!("Backtest failed: {error}.").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_validate(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.active_strategy_path().map(str::to_owned) else {
            return;
        };
        let http = cx.http_client();
        self.validate = ActionState::Running;
        cx.notify();
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let path = format!(
                "/ui/api/validation?strategy_path={}",
                url_encode(&strategy_path)
            );
            let result = get_json(http, &path).await;
            this.update(cx, |this, cx| {
                this.validate = match result {
                    Ok(value) => ActionState::Ok(validation_summary(&value)),
                    Err(error) => ActionState::Error(format!("Validate failed: {error}.").into()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_deploy(&mut self, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.active_strategy_path().map(str::to_owned) else {
            return;
        };
        let http = cx.http_client();
        // Re-check live permission FRESH at click time — the context-resolve
        // snapshot is up to ~30s stale and this can authorize real money. The
        // first click on a live-capable strategy only ARMS a confirmation;
        // the deploy is sent on the confirming second click (or immediately as
        // paper when live isn't permitted).
        let awaiting = self.awaiting_live_confirm;
        self.deploy = ActionState::Running;
        cx.notify();
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let live_allowed = poll_live_allowed(http.clone()).await;
            let decision = decide_deploy(awaiting, live_allowed);
            if decision == DeployDecision::ArmConfirm {
                this.update(cx, |this, cx| {
                    this.deploy_live_allowed = true;
                    this.deploy = ActionState::Idle;
                    this.awaiting_live_confirm = true;
                    cx.notify();
                })
                .ok();
                return;
            }
            let paper = decision == DeployDecision::SubmitPaper;
            let body = serde_json::json!({
                "strategy_path": strategy_path,
                "paper": paper,
            });
            let result = post_json(http, "/ui/api/deploy/new", body).await;
            this.update(cx, |this, cx| {
                this.deploy_live_allowed = live_allowed;
                this.awaiting_live_confirm = false;
                this.deploy = match result {
                    Ok(value) => ActionState::Ok(deploy_summary(&value, paper)),
                    Err(error) => ActionState::Error(format!("Deploy failed: {error}.").into()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_action_button(
        &self,
        id: &'static str,
        label: &'static str,
        state: &ActionState,
        enabled: bool,
        disabled_tooltip: Option<&'static str>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let running = matches!(state, ActionState::Running);
        let button_label: SharedString = if running {
            format!("{label}…").into()
        } else {
            label.into()
        };

        let button = Button::new(id, button_label)
            .label_size(LabelSize::Small)
            .disabled(!enabled || running)
            .when(enabled && !running, |this| {
                this.on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            });

        let button = match disabled_tooltip {
            Some(text) if !enabled => button.tooltip(Tooltip::text(text)),
            _ => button,
        };

        h_flex()
            .gap_1()
            .child(button)
            .children(render_action_status(state))
    }
}

impl EventEmitter<ToolbarItemEvent> for StrategyCockpit {}

impl Render for StrategyCockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let resolved = matches!(
            self.visibility,
            CockpitVisibility::Shown {
                resolve: ResolveState::Strategy(_),
                ..
            }
        );

        let backtest = self.backtest.clone();
        let validate = self.validate.clone();
        let deploy = self.deploy.clone();
        let deploy_label = if self.awaiting_live_confirm {
            "Confirm live deploy"
        } else if self.deploy_live_allowed {
            "Deploy"
        } else {
            "Deploy (paper)"
        };

        let unresolved_tooltip = match &self.visibility {
            CockpitVisibility::Shown {
                resolve: ResolveState::EngineUnreachable,
                ..
            } => "Can't reach the engine — check your connection in Settings.",
            CockpitVisibility::Shown {
                resolve: ResolveState::Resolving,
                ..
            } => "Resolving this file against the engine…",
            _ => "No engine strategy matches this file. Add a Strategy subclass under the strategies path.",
        };

        h_flex()
            .id("strategy-cockpit")
            .gap_2()
            .px_1()
            .child(
                self.render_action_button(
                    "cockpit-backtest",
                    "Backtest",
                    &backtest,
                    resolved,
                    Some(unresolved_tooltip),
                    Self::run_backtest,
                    cx,
                ),
            )
            .child(
                self.render_action_button(
                    "cockpit-validate",
                    "Validate",
                    &validate,
                    resolved,
                    Some(unresolved_tooltip),
                    Self::run_validate,
                    cx,
                ),
            )
            .child({
                let label = deploy_label;
                let running = matches!(deploy, ActionState::Running);
                let button_label: SharedString = if running {
                    format!("{label}…").into()
                } else {
                    label.into()
                };
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("cockpit-deploy", button_label)
                            .label_size(LabelSize::Small)
                            .disabled(!resolved || running)
                            .when(resolved && !running, |this| {
                                this.on_click(
                                    cx.listener(|this, _, _, cx| this.run_deploy(cx)),
                                )
                            })
                            .when(!resolved, |this| {
                                this.tooltip(Tooltip::text(unresolved_tooltip))
                            })
                            .when(resolved && !self.deploy_live_allowed && !self.awaiting_live_confirm, |this| {
                                this.tooltip(Tooltip::text(
                                    "Live trading is not allowed for the active broker; this deploys to paper.",
                                ))
                            })
                            .when(self.awaiting_live_confirm, |this| {
                                this.tooltip(Tooltip::text(
                                    "Sends a LIVE, real-money deploy — click again to confirm.",
                                ))
                            }),
                    )
                    .children(render_action_status(&deploy))
            })
            .child(render_feed_pill(self.feed))
    }
}

impl ToolbarItemView for StrategyCockpit {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        let active_py_path = active_pane_item.and_then(|item| {
            let editor = item.act_as::<Editor>(cx)?;
            let path = editor.update(cx, |editor, cx| editor.target_file_abs_path(cx))?;
            let file_name = path.file_name()?;
            file_name
                .as_encoded_bytes()
                .ends_with(b".py")
                .then_some(path)
        });

        match active_py_path {
            Some(path) => {
                let already_showing = matches!(
                    &self.visibility,
                    CockpitVisibility::Shown { file_path, .. } if *file_path == path
                );
                if !already_showing {
                    // New strategy buffer: reset action states and re-resolve.
                    self.backtest = ActionState::Idle;
                    self.validate = ActionState::Idle;
                    self.deploy = ActionState::Idle;
                    self.refresh_context(path, cx);
                }
                ToolbarItemLocation::Secondary
            }
            None => {
                if !matches!(self.visibility, CockpitVisibility::Hidden) {
                    self.visibility = CockpitVisibility::Hidden;
                    self._context_task = None;
                    cx.notify();
                }
                ToolbarItemLocation::Hidden
            }
        }
    }
}

fn render_action_status(state: &ActionState) -> Option<impl IntoElement> {
    match state {
        ActionState::Idle | ActionState::Running => None,
        ActionState::Ok(message) => Some(
            Label::new(message.clone())
                .size(LabelSize::Small)
                .color(Color::Success),
        ),
        ActionState::Error(message) => Some(
            Label::new(message.clone())
                .size(LabelSize::Small)
                .color(Color::Error),
        ),
    }
}

fn render_feed_pill(feed: FeedState) -> impl IntoElement {
    let (text, color) = match feed {
        FeedState::Unknown => ("Data: unknown", Color::Muted),
        FeedState::Ok => ("Data: ok", Color::Success),
        FeedState::Missing => ("Data: missing", Color::Warning),
    };
    h_flex().child(Label::new(text).size(LabelSize::Small).color(color))
}

/// Derive the dotted Python module of a strategy file relative to a
/// `strategies/` root, e.g. `/opt/auracle/strategies/example_ma.py` ->
/// `strategies.example_ma`. Returns `None` when the file is not under a
/// `strategies/` directory.
fn strategy_module_from_path(file_path: &std::path::Path) -> Option<String> {
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

/// Match the derived module against the engine's strategy listing. Returns
/// `EngineUnreachable` on a transport error so the cockpit can say so honestly
/// instead of conflating it with "this file isn't a strategy".
async fn resolve_strategy_path(
    http: std::sync::Arc<dyn http_client::HttpClient>,
    module: &str,
) -> ResolveState {
    match get_json(http, "/ui/api/backtest/strategies").await {
        Ok(value) => match_strategy(&value, module),
        Err(_) => ResolveState::EngineUnreachable,
    }
}

/// Pure matcher over the engine's strategy listing (engine reachable). Yields
/// `Strategy` on a match, `NotAStrategy` otherwise.
fn match_strategy(value: &serde_json::Value, module: &str) -> ResolveState {
    let module_prefix = format!("{module}.");
    let matched = value
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

/// Report per-strategy data-feed presence from the deploy preflight's `data`
/// block (`universe` + `missing`). Presence only; freshness is out of scope.
async fn poll_feed(
    http: std::sync::Arc<dyn http_client::HttpClient>,
    strategy_path: &str,
) -> FeedState {
    let path = format!(
        "/ui/api/deploy/preflight?strategy_path={}",
        url_encode(strategy_path)
    );
    match get_json(http, &path).await {
        Ok(value) => match value.get("data") {
            Some(data) => {
                let missing = data
                    .get("missing")
                    .and_then(|m| m.as_array())
                    .map(|m| m.len())
                    .unwrap_or(0);
                if missing == 0 {
                    FeedState::Ok
                } else {
                    FeedState::Missing
                }
            }
            None => FeedState::Unknown,
        },
        Err(_) => FeedState::Unknown,
    }
}

fn backtest_summary(value: &serde_json::Value) -> SharedString {
    if let Some(stats) = value.get("stats") {
        let total = stats
            .get("total_return")
            .and_then(|v| v.as_f64())
            .map(|v| format!("return {:.1}%", v * 100.0));
        let sharpe = stats
            .get("sharpe")
            .and_then(|v| v.as_f64())
            .map(|v| format!("sharpe {v:.2}"));
        let parts: Vec<String> = [total, sharpe].into_iter().flatten().collect();
        if !parts.is_empty() {
            return format!("Backtest done — {}", parts.join(", ")).into();
        }
    }
    "Backtest done.".into()
}

fn validation_summary(value: &serde_json::Value) -> SharedString {
    if let Some(plain) = value.get("plain").and_then(|v| v.as_str()) {
        return plain.to_owned().into();
    }
    let red = value
        .get("signals")
        .and_then(|s| s.as_array())
        .map(|signals| {
            signals
                .iter()
                .filter(|signal| signal.get("tier").and_then(|t| t.as_str()) == Some("red"))
                .count()
        })
        .unwrap_or(0);
    if red == 0 {
        "Validation passed.".into()
    } else {
        format!("Validation: {red} red signal(s).").into()
    }
}

fn deploy_summary(value: &serde_json::Value, paper: bool) -> SharedString {
    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(true);
    let target = if paper { "paper" } else { "live" };
    if ok {
        format!("Deployed to {target}.").into()
    } else {
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("engine refused the deploy");
        format!("Deploy not accepted: {message}.").into()
    }
}

/// Minimal percent-encoding for the strategy path in a query string. Strategy
/// paths are dotted identifiers (`strategies.module.Class`), so only `.` and
/// the structural characters need escaping; we encode the few characters that
/// would otherwise break the query.
fn url_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
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
    fn encodes_strategy_path_query() {
        assert_eq!(
            url_encode("strategies.example_ma.MACrossover"),
            "strategies.example_ma.MACrossover"
        );
        assert_eq!(url_encode("a b&c"), "a%20b%26c");
    }
}
