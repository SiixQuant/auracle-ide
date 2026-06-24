//! The Studio backtest-results tab.
//!
//! After a backtest, the results open as a new `Item` (a tab) beside the
//! strategy code, rendering a stat strip from the engine's response. Out-of-
//! sample / robustness fields arrive once the engine diagnostics route lands;
//! until then this view degrades honestly to the in-sample stats the existing
//! `/backtest` route returns — it never fabricates a number the engine didn't
//! send (missing stats render as an em dash).
//!
//! Parsing, formatting, and the robustness verdict all live in the gpui-free
//! `auracle_studio_results` reducer crate, shared with the cockpit toolbar so
//! the same engine payload reads identically on both surfaces. This file is the
//! render + async-I/O shell over those pure decisions.

use auracle_actions::{DeployDecision, decide_deploy, poll_live_permission};
use auracle_connections::post_json;
use auracle_studio_results::{
    DiagFetchOutcome, StudioDiagnostics, classify_diag_fetch, fmt_count, fmt_money, fmt_pct,
    fmt_ratio, fmt_turns, robustness_verdict, verdict_text,
};
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Task, Window};
use ui::{Banner, Callout, Severity, TintColor, prelude::*};
use workspace::{Workspace, item::Item};

/// The numbers shown in the results tab's stat strip. Re-exported from the
/// reducer crate so the cockpit (which builds the summary) and this tab (which
/// renders it) share one type and one parser, and the `open_backtest_results`
/// handoff stays type-compatible across the crate boundary.
pub use auracle_studio_results::BacktestSummary;

/// Whether the out-of-sample diagnostics have loaded. `Failed` is distinct from
/// `Unavailable`: a transient engine error is retryable (it offers a Retry),
/// while a route-absent `Unavailable` is an honest "not deployed yet" with no
/// retry, because retrying a missing route can't help.
#[derive(Clone, Default)]
enum DiagState {
    #[default]
    Loading,
    Ready(StudioDiagnostics),
    /// The diagnostics route isn't deployed (404) — non-retryable.
    Unavailable,
    /// A transient server / transport error — retryable.
    Failed(SharedString),
}

/// A single stat strip cell: a large value over a muted label.
fn stat(label: &'static str, value: SharedString) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(Label::new(value).size(LabelSize::Large))
        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
}

/// The state of the results tab's deploy verb.
#[derive(Clone, Default)]
enum DeployState {
    #[default]
    Idle,
    Running,
    Done(SharedString),
    Failed(SharedString),
}

/// The results tab view. Opened via [`open_backtest_results`].
pub struct BacktestResultsView {
    focus_handle: FocusHandle,
    summary: BacktestSummary,
    diagnostics: DiagState,
    deploy: DeployState,
    /// Two-step guard for a live (real-money) deploy: armed on the first click,
    /// sent only on the confirming second click — the exact contract the cockpit
    /// uses, via the shared `auracle_actions` verb core.
    awaiting_live_confirm: bool,
    _deploy_task: Option<Task<()>>,
    _diagnostics_task: Option<Task<()>>,
}

impl BacktestResultsView {
    pub fn new(summary: BacktestSummary, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            summary,
            diagnostics: DiagState::Loading,
            deploy: DeployState::Idle,
            awaiting_live_confirm: false,
            _deploy_task: None,
            _diagnostics_task: None,
        };
        view.fetch_diagnostics(cx);
        view
    }

    /// Fetch out-of-sample diagnostics from the engine. The fetch outcome is
    /// classified by the shared reducer so a 404 (route not deployed) reads as
    /// the honest, non-retryable `Unavailable`, while a 5xx / transport error
    /// reads as a retryable `Failed`. The transport collapses the response to
    /// `Result<Value, _>` with no separate HTTP status, so we pass `None` for
    /// the status and let the classifier sniff the error body — the documented
    /// fallback.
    fn fetch_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.strategy_path() else {
            self.diagnostics = DiagState::Unavailable;
            return;
        };
        // A retry from a failed/loaded state should visibly return to loading.
        self.diagnostics = DiagState::Loading;
        cx.notify();
        let http = cx.http_client();
        self._diagnostics_task = Some(cx.spawn(async move |this, cx| {
            let result = post_json(
                http,
                "/backtest/studio",
                serde_json::json!({ "strategy_path": strategy_path }),
            )
            .await;
            this.update(cx, |this, cx| {
                this.diagnostics = match result {
                    Ok(value) => DiagState::Ready(StudioDiagnostics::from_engine(&value)),
                    Err(error) => {
                        // No HTTP status is surfaced by the transport, so rely on
                        // the body-sniff fallback: a route-absent body is the
                        // only thing treated as Unavailable; everything else is
                        // retryable. With status `None` the classifier only ever
                        // yields Unavailable or Failed — success comes through the
                        // Ok arm above — so a Ready classification here would be a
                        // contradiction we surface honestly as a failure.
                        let message = error.to_string();
                        match classify_diag_fetch(None, Some(&message)) {
                            DiagFetchOutcome::Unavailable => DiagState::Unavailable,
                            DiagFetchOutcome::Failed { message } => {
                                DiagState::Failed(message.into())
                            }
                            DiagFetchOutcome::Ready => DiagState::Failed(message.into()),
                        }
                    }
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn title(&self) -> SharedString {
        if self.summary.strategy.is_empty() {
            "Backtest results".into()
        } else {
            self.summary.strategy.clone().into()
        }
    }

    /// The dotted engine strategy path this result belongs to, if known.
    fn strategy_path(&self) -> Option<SharedString> {
        if self.summary.strategy.is_empty() {
            None
        } else {
            Some(self.summary.strategy.clone().into())
        }
    }

    /// Submit a paper deploy immediately — paper is always safe, one click.
    fn paper_trade(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.submit_deploy(true, cx);
    }

    /// A live (real-money) deploy: re-check permission FRESH at click time and
    /// arm-then-confirm via the shared gate. Never auto-sends live on a first
    /// click; falls back to paper if live was revoked between arm and confirm.
    fn deploy_live(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.strategy_path() else {
            return;
        };
        let http = cx.http_client();
        let awaiting = self.awaiting_live_confirm;
        self.deploy = DeployState::Running;
        cx.notify();
        self._deploy_task = Some(cx.spawn(async move |this, cx| {
            let permission = poll_live_permission(http.clone()).await;
            let decision = decide_deploy(awaiting, permission);
            if decision == DeployDecision::ArmConfirm {
                this.update(cx, |this, cx| {
                    this.awaiting_live_confirm = true;
                    this.deploy = DeployState::Idle;
                    cx.notify();
                })
                .ok();
                return;
            }
            // Live permission couldn't be verified (engine outage / malformed
            // capabilities): surface it honestly and stop — never silently fall
            // through to a paper deploy the user didn't ask for.
            if decision == DeployDecision::BlockedUnverified {
                this.update(cx, |this, cx| {
                    this.awaiting_live_confirm = false;
                    this.deploy = DeployState::Failed(
                        "Couldn't verify live permission — check the engine and retry.".into(),
                    );
                    cx.notify();
                })
                .ok();
                return;
            }
            let paper = decision == DeployDecision::SubmitPaper;
            let result = post_json(
                http,
                "/ui/api/deploy/new",
                serde_json::json!({ "strategy_path": strategy_path, "paper": paper }),
            )
            .await;
            this.update(cx, |this, cx| {
                this.awaiting_live_confirm = false;
                this.deploy = deploy_state_from(result, paper);
                cx.notify();
            })
            .ok();
        }));
    }

    fn submit_deploy(&mut self, paper: bool, cx: &mut Context<Self>) {
        let Some(strategy_path) = self.strategy_path() else {
            return;
        };
        let http = cx.http_client();
        self.deploy = DeployState::Running;
        cx.notify();
        self._deploy_task = Some(cx.spawn(async move |this, cx| {
            let result = post_json(
                http,
                "/ui/api/deploy/new",
                serde_json::json!({ "strategy_path": strategy_path, "paper": paper }),
            )
            .await;
            this.update(cx, |this, cx| {
                this.deploy = deploy_state_from(result, paper);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The in-sample stat strip, or — when the engine returned no statistics at
    /// all — a designed empty state instead of a row of seven em-dashes that
    /// looks broken.
    fn render_summary(&self) -> impl IntoElement {
        let summary = &self.summary;
        if summary.is_empty() {
            return Callout::new()
                .severity(Severity::Info)
                .icon(IconName::Info)
                .title("No statistics for this run")
                .description("The engine returned no statistics for this backtest.")
                .into_any_element();
        }
        h_flex()
            .gap_6()
            .flex_wrap()
            .child(stat("Net profit", fmt_money(summary.net_profit).into()))
            .child(stat("Return", fmt_pct(summary.total_return).into()))
            .child(stat("Sharpe", fmt_ratio(summary.sharpe).into()))
            .child(stat("Max drawdown", fmt_pct(summary.max_drawdown).into()))
            .child(stat("Win rate", fmt_pct(summary.win_rate).into()))
            .child(stat("Turnover", fmt_turns(summary.turnover).into()))
            .child(stat("Trades", fmt_count(summary.num_trades).into()))
            .into_any_element()
    }

    /// The one gated verb: Paper-trade (one click) + Deploy live (arm/confirm).
    /// Only shown when the result belongs to a known strategy. The live deploy
    /// is a real-money action, so once the confirm step is armed the button is
    /// styled with the error tint and a warning callout sits above the bar — no
    /// label-only signal for an action that moves money.
    fn render_deploy_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_strategy = self.strategy_path().is_some();
        let running = matches!(self.deploy, DeployState::Running);
        let live_label = if self.awaiting_live_confirm {
            "Confirm live deploy"
        } else {
            "Deploy live"
        };
        let status: Option<SharedString> = match &self.deploy {
            DeployState::Idle => None,
            DeployState::Running => Some("Deploying…".into()),
            DeployState::Done(message) | DeployState::Failed(message) => Some(message.clone()),
        };

        let bar = h_flex()
            .gap_2()
            .child(
                // "Open Runs" is honest: it opens the Runs dock, it does not
                // select THIS run. The reverse link (a Runs row opening its
                // results tab) needs richer run events from the engine and is
                // tracked separately.
                Button::new("studio-open-runs", "Open Runs").on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(runs_dock::ToggleFocus), cx);
                }),
            )
            .when(has_strategy, |this| {
                this.child(
                    Button::new("studio-paper-trade", "Paper-trade")
                        .label_size(LabelSize::Small)
                        .disabled(running)
                        .on_click(cx.listener(|this, _, window, cx| this.paper_trade(window, cx))),
                )
                .child({
                    let mut live = Button::new("studio-deploy-live", live_label)
                        .label_size(LabelSize::Small)
                        .disabled(running)
                        .on_click(cx.listener(|this, _, window, cx| this.deploy_live(window, cx)));
                    if self.awaiting_live_confirm {
                        live = live.style(ButtonStyle::Tinted(TintColor::Error));
                    }
                    live
                })
            })
            .children(status.map(|message| {
                Label::new(message)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
            }));

        v_flex()
            .gap_2()
            .when(has_strategy && self.awaiting_live_confirm, |this| {
                this.child(
                    Callout::new()
                        .severity(Severity::Warning)
                        .icon(IconName::Warning)
                        .title("This sends a LIVE, real-money deploy")
                        .description(
                            "Click Confirm live deploy to proceed, or switch away to cancel.",
                        ),
                )
            })
            .child(bar)
    }

    /// The out-of-sample readout. Four designed states: a loading skeleton, a
    /// route-absent Callout (no retry — retrying a missing route can't help), a
    /// retryable Failed Banner with a Retry, and the loaded verdict + pairing.
    fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.diagnostics {
            DiagState::Loading => render_diagnostics_skeleton().into_any_element(),
            DiagState::Unavailable => Callout::new()
                .severity(Severity::Info)
                .icon(IconName::Info)
                .title("Out-of-sample robustness")
                .description("Arrives once the engine diagnostics route is deployed.")
                .into_any_element(),
            DiagState::Failed(message) => Banner::new()
                .severity(Severity::Warning)
                .child(
                    Label::new(format!("Couldn't load robustness diagnostics: {message}"))
                        .size(LabelSize::Small),
                )
                .action_slot(
                    Button::new("studio-diagnostics-retry", "Retry")
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.fetch_diagnostics(cx))),
                )
                .into_any_element(),
            DiagState::Ready(diagnostics) => {
                let verdict = verdict_text(robustness_verdict(diagnostics));
                let sensitivity: SharedString = diagnostics
                    .sensitivity
                    .clone()
                    .map(SharedString::from)
                    .unwrap_or_else(|| auracle_studio_results::MISSING.into());
                v_flex()
                    .gap_2()
                    .child(Label::new(verdict))
                    .child(
                        h_flex()
                            .gap_6()
                            .flex_wrap()
                            .child(stat(
                                "Sharpe (in-sample)",
                                fmt_ratio(diagnostics.in_sample_sharpe).into(),
                            ))
                            .child(stat(
                                "Sharpe (out-of-sample)",
                                fmt_ratio(diagnostics.out_of_sample_sharpe).into(),
                            ))
                            .child(stat("Robustness", fmt_ratio(diagnostics.robustness).into()))
                            .child(stat("Sensitivity", sensitivity)),
                    )
                    .into_any_element()
            }
        }
    }
}

/// A designed loading skeleton for the diagnostics section — placeholder cells,
/// not a bare spinner — so the loading state reads as "checking", consistent
/// with the settings-page exemplar.
fn render_diagnostics_skeleton() -> impl IntoElement {
    let skeleton_cell = || {
        v_flex().gap_1().child(
            Label::new("Checking out-of-sample robustness…")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
    };
    v_flex().gap_2().child(skeleton_cell())
}

fn deploy_state_from<E: std::fmt::Display>(
    result: Result<serde_json::Value, E>,
    paper: bool,
) -> DeployState {
    let target = if paper { "paper" } else { "live" };
    match result {
        Ok(_) => DeployState::Done(format!("Deployed to {target}.").into()),
        Err(error) => DeployState::Failed(format!("Deploy failed: {error}.").into()),
    }
}

impl Focusable for BacktestResultsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for BacktestResultsView {}

impl Item for BacktestResultsView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.title()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::GitGraph))
    }
}

impl Render for BacktestResultsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .gap_4()
            .p_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new(self.title()).size(LabelSize::Large))
                    .child(
                        Label::new("In-sample backtest")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(self.render_summary())
            .child(self.render_diagnostics(cx))
            .when(!self.summary.equity.is_empty(), |this| {
                this.child(auracle_charts::EquityChart::new(
                    self.summary
                        .equity
                        .iter()
                        .map(|p| (p.t, p.equity))
                        .collect(),
                ))
            })
            .child(self.render_deploy_bar(cx))
    }
}

/// Open a backtest-results tab in the workspace's active pane.
pub fn open_backtest_results(
    workspace: &mut Workspace,
    summary: BacktestSummary,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let view = cx.new(|cx| BacktestResultsView::new(summary, cx));
    workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
}
