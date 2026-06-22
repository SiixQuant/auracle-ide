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
//!
//! All decision logic — which verbs are enabled, how the feed status reads, how
//! a `/backtest` response becomes a one-line status — lives in the gpui-free
//! `auracle_cockpit_state` / `auracle_studio_results` reducer crates, so this
//! file is purely the render + async-I/O shell. Keeping the decisions pure is
//! what lets them be unit-tested without a graphics toolchain and keeps the
//! cockpit and the results tab formatting the same engine payload identically.

use std::path::PathBuf;

use auracle_backtest_results::open_backtest_results;
use auracle_cockpit_state::{
    ActionAffordance, FeedState, ResolveState, backtest_affordance, deploy_affordance,
    feed_from_preflight, match_strategy, show_unreachable_banner, strategy_module_from_path,
    validate_affordance,
};
use auracle_connections::{get_json, post_json};
use auracle_deploy_gate::{DeployDecision, decide_deploy, poll_live_allowed};
use auracle_studio_results::{BacktestSummary, backtest_oneline};
use editor::Editor;
use gpui::{Context, EventEmitter, Task, WeakEntity};
use ui::{Banner, Indicator, Severity, TintColor, Tooltip, prelude::*};
use workspace::{ItemHandle, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace};

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
            // No poll has run yet: the feed is checking, not "unknown" (which is
            // reserved for the engine genuinely returning no data block).
            feed: FeedState::Polling,
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

    fn resolve_state(&self) -> ResolveState {
        match &self.visibility {
            CockpitVisibility::Shown { resolve, .. } => resolve.clone(),
            // Hidden never renders, but a sensible default keeps the affordance
            // calls total.
            CockpitVisibility::Hidden => ResolveState::Resolving,
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
            file_path: file_path.clone(),
            resolve: ResolveState::Resolving,
        };
        // A poll is in flight: the feed pill reads "Checking data…", not a
        // settled answer.
        self.feed = FeedState::Polling;
        // A pending live-deploy confirmation must not carry across a context
        // change (different strategy/file).
        self.awaiting_live_confirm = false;
        cx.notify();

        let http = cx.http_client();
        // Capture the path this poll resolved for so a fast tab-switch can't
        // paint strategy A's feed / live-allowed onto strategy B: when the
        // result lands we apply it only if the cockpit still shows this path.
        let resolved_for = file_path;
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
                // Guard against a stale poll: if the user switched buffers while
                // this was in flight, `visibility` now points at a different
                // file and applying these values would be a lie about it.
                let still_current = matches!(
                    &this.visibility,
                    CockpitVisibility::Shown { file_path, .. } if *file_path == resolved_for
                );
                if !still_current {
                    return;
                }
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
            // Update ONLY the cockpit's own state here. Opening the results
            // tab must NOT happen inside this update: open_backtest_results
            // adds a workspace item, which synchronously re-enters a
            // StrategyCockpit update and panics ("cannot update ... while it
            // is already being updated"). So capture the summary, let this
            // cockpit update finish, then open the results in a separate
            // workspace update below.
            let summary = this
                .update(cx, |this, cx| match &result {
                    Ok(value) => {
                        // One-liner is formatted by the shared reducer so the
                        // cockpit status and the results tab never disagree.
                        this.backtest = ActionState::Ok(backtest_oneline(value).into());
                        let summary = BacktestSummary::from_engine(title, value);
                        cx.notify();
                        Some(summary)
                    }
                    Err(error) => {
                        this.backtest =
                            ActionState::Error(format!("Backtest failed: {error}.").into());
                        cx.notify();
                        None
                    }
                })
                .ok()
                .flatten();

            if let Some(summary) = summary {
                workspace
                    .update_in(cx, |workspace, window, cx| {
                        open_backtest_results(workspace, summary, window, cx);
                    })
                    .ok();
            }
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

    /// One renderer for all three verbs. The affordance (label / enabled /
    /// danger / tooltip) is computed by the reducer, so the deploy button — with
    /// its danger styling and live-confirm tooltip — rides the exact same path
    /// as Backtest and Validate. There is no second hand-rolled button body to
    /// drift from this one.
    fn render_action_button(
        &self,
        id: &'static str,
        affordance: ActionAffordance,
        state: &ActionState,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let running = matches!(state, ActionState::Running);
        let clickable = affordance.enabled && !running;
        let button_label: SharedString = if running {
            format!("{}…", affordance.label).into()
        } else {
            affordance.label.clone().into()
        };

        let mut button = Button::new(id, button_label)
            .label_size(LabelSize::Small)
            .disabled(!affordance.enabled || running)
            .when(clickable, |this| {
                this.on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            });

        // The only danger style is the live-deploy confirm step: a real-money
        // action must read as dangerous, never as just another button.
        if affordance.danger {
            button = button.style(ButtonStyle::Tinted(TintColor::Error));
        }

        if let Some(tooltip) = affordance.tooltip {
            button = button.tooltip(Tooltip::text(tooltip));
        }

        h_flex()
            .gap_1()
            .child(button)
            .children(render_action_status(state))
    }
}

impl EventEmitter<ToolbarItemEvent> for StrategyCockpit {}

impl Render for StrategyCockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let resolve = self.resolve_state();
        let backtest = self.backtest.clone();
        let validate = self.validate.clone();
        let deploy = self.deploy.clone();

        // Compute every verb's affordance once from the resolve state, so the
        // three buttons stay in lockstep.
        let backtest_affordance = backtest_affordance(&resolve);
        let validate_affordance = validate_affordance(&resolve);
        let deploy_affordance = deploy_affordance(
            &resolve,
            self.deploy_live_allowed,
            self.awaiting_live_confirm,
        );

        let buttons = h_flex()
            .id("strategy-cockpit")
            .gap_2()
            .px_1()
            .child(self.render_action_button(
                "cockpit-backtest",
                backtest_affordance,
                &backtest,
                Self::run_backtest,
                cx,
            ))
            .child(self.render_action_button(
                "cockpit-validate",
                validate_affordance,
                &validate,
                Self::run_validate,
                cx,
            ))
            .child(self.render_action_button(
                "cockpit-deploy",
                deploy_affordance,
                &deploy,
                |this, _window, cx| this.run_deploy(cx),
                cx,
            ))
            .child(render_feed_pill(self.feed));

        // Engine-unreachable is a first-class error state, not just a disabled
        // tooltip: surface it as a Banner with a Retry that re-resolves the same
        // file. The disabled verbs render beneath it.
        v_flex()
            .gap_1()
            .when(show_unreachable_banner(&resolve), |this| {
                this.child(render_unreachable_banner(cx, &self.visibility))
            })
            .child(buttons)
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

/// The per-strategy data-feed pill. The dot color + copy distinguish the four
/// poll states: a "Checking data…" skeleton while in flight, settled Ok /
/// Missing answers, and a muted Unknown only when the engine returned no data
/// block at all — never a single neutral label that conflates all four.
fn render_feed_pill(feed: FeedState) -> impl IntoElement {
    let (text, color) = match feed {
        FeedState::Polling => ("Checking data…", Color::Muted),
        FeedState::Ok => ("Data: ok", Color::Success),
        FeedState::Missing => ("Data: missing", Color::Warning),
        FeedState::Unknown => ("Data: unknown", Color::Muted),
    };
    h_flex()
        .gap_1()
        .child(Indicator::dot().color(color))
        .child(Label::new(text).size(LabelSize::Small).color(color))
}

/// The engine-unreachable banner: a warning strip with a Retry that re-resolves
/// the file the cockpit is currently showing. Distinct from the "not a strategy"
/// case (a disabled tooltip), because the cure is different — retry vs. add a
/// Strategy subclass.
fn render_unreachable_banner(
    cx: &mut Context<StrategyCockpit>,
    visibility: &CockpitVisibility,
) -> impl IntoElement {
    let file_path = match visibility {
        CockpitVisibility::Shown { file_path, .. } => Some(file_path.clone()),
        CockpitVisibility::Hidden => None,
    };
    Banner::new()
        .severity(Severity::Warning)
        .child(
            Label::new("Can't reach the engine — check your connection in Settings.")
                .size(LabelSize::Small),
        )
        .action_slot(
            Button::new("cockpit-retry", "Retry")
                .label_size(LabelSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(file_path) = file_path.clone() {
                        this.refresh_context(file_path, cx);
                    }
                })),
        )
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

/// Report per-strategy data-feed presence from the deploy preflight's `data`
/// block (`universe` + `missing`) via the shared pure classifier. Presence
/// only; freshness is out of scope. A transport error reads as `Unknown` (we
/// learned nothing), never a fabricated Ok/Missing.
async fn poll_feed(
    http: std::sync::Arc<dyn http_client::HttpClient>,
    strategy_path: &str,
) -> FeedState {
    let path = format!(
        "/ui/api/deploy/preflight?strategy_path={}",
        url_encode(strategy_path)
    );
    match get_json(http, &path).await {
        Ok(value) => feed_from_preflight(&value),
        Err(_) => FeedState::Unknown,
    }
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

    #[test]
    fn encodes_strategy_path_query() {
        assert_eq!(
            url_encode("strategies.example_ma.MACrossover"),
            "strategies.example_ma.MACrossover"
        );
        assert_eq!(url_encode("a b&c"), "a%20b%26c");
    }
}
