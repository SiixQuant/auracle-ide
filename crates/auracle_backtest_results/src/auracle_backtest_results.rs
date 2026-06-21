//! The Studio backtest-results tab.
//!
//! After a backtest, the results open as a new `Item` (a tab) beside the
//! strategy code, rendering a stat strip from the engine's response. Out-of-
//! sample / robustness fields arrive once the engine diagnostics route lands;
//! until then this view degrades honestly to the in-sample stats the existing
//! `/backtest` route returns — it never fabricates a number the engine didn't
//! send (missing stats render as an em dash).

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Window};
use ui::prelude::*;
use workspace::{Workspace, item::Item};

/// The numbers shown in the results tab's stat strip. Every field is optional so
/// the view renders exactly what the engine returned — no fabricated stats.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacktestSummary {
    pub strategy: SharedString,
    pub net_profit: Option<f64>,
    pub total_return: Option<f64>,
    pub sharpe: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub win_rate: Option<f64>,
    pub turnover: Option<f64>,
    pub num_trades: Option<u64>,
}

impl BacktestSummary {
    /// Parse the engine `/backtest` response (`{ "stats": { … } }`) into a
    /// summary. Missing fields stay `None`.
    pub fn from_engine(strategy: impl Into<SharedString>, value: &serde_json::Value) -> Self {
        let stats = value.get("stats");
        let number = |key: &str| stats.and_then(|s| s.get(key)).and_then(|v| v.as_f64());
        let integer = |key: &str| stats.and_then(|s| s.get(key)).and_then(|v| v.as_u64());
        Self {
            strategy: strategy.into(),
            net_profit: number("net_profit"),
            total_return: number("total_return"),
            sharpe: number("sharpe"),
            max_drawdown: number("max_drawdown"),
            win_rate: number("win_rate"),
            turnover: number("turnover"),
            num_trades: integer("num_trades"),
        }
    }
}

const MISSING: &str = "—";

fn pct(value: Option<f64>) -> SharedString {
    match value {
        Some(v) => format!("{:+.1}%", v * 100.0).into(),
        None => MISSING.into(),
    }
}

fn ratio(value: Option<f64>) -> SharedString {
    match value {
        Some(v) => format!("{v:.2}").into(),
        None => MISSING.into(),
    }
}

fn money(value: Option<f64>) -> SharedString {
    match value {
        Some(v) => format!("{}${:.0}", if v < 0.0 { "-" } else { "+" }, v.abs()).into(),
        None => MISSING.into(),
    }
}

fn turns(value: Option<f64>) -> SharedString {
    match value {
        Some(v) => format!("{v:.1}×").into(),
        None => MISSING.into(),
    }
}

fn count(value: Option<u64>) -> SharedString {
    match value {
        Some(v) => v.to_string().into(),
        None => MISSING.into(),
    }
}

/// A single stat strip cell: a large value over a muted label.
fn stat(label: &'static str, value: SharedString) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(Label::new(value).size(LabelSize::Large))
        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
}

/// The results tab view. Opened via [`open_backtest_results`].
pub struct BacktestResultsView {
    focus_handle: FocusHandle,
    summary: BacktestSummary,
}

impl BacktestResultsView {
    pub fn new(summary: BacktestSummary, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            summary,
        }
    }

    fn title(&self) -> SharedString {
        if self.summary.strategy.is_empty() {
            "Backtest results".into()
        } else {
            self.summary.strategy.clone()
        }
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
        let summary = &self.summary;
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
            .child(
                h_flex()
                    .gap_6()
                    .flex_wrap()
                    .child(stat("Net profit", money(summary.net_profit)))
                    .child(stat("Return", pct(summary.total_return)))
                    .child(stat("Sharpe", ratio(summary.sharpe)))
                    .child(stat("Max drawdown", pct(summary.max_drawdown)))
                    .child(stat("Win rate", pct(summary.win_rate)))
                    .child(stat("Turnover", turns(summary.turnover)))
                    .child(stat("Trades", count(summary.num_trades))),
            )
            .child(
                Label::new("Out-of-sample robustness arrives with the diagnostics route.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_present_stats_and_leaves_missing_as_none() {
        let value = serde_json::json!({
            "stats": { "total_return": 0.161, "sharpe": 1.3, "num_trades": 142 }
        });
        let summary = BacktestSummary::from_engine("OvernightGapReversion", &value);
        assert_eq!(summary.strategy, SharedString::from("OvernightGapReversion"));
        assert_eq!(summary.total_return, Some(0.161));
        assert_eq!(summary.sharpe, Some(1.3));
        assert_eq!(summary.num_trades, Some(142));
        // Absent fields are never fabricated.
        assert_eq!(summary.max_drawdown, None);
        assert_eq!(summary.win_rate, None);
    }

    #[test]
    fn missing_stats_block_yields_all_none() {
        let summary = BacktestSummary::from_engine("S", &serde_json::json!({}));
        assert_eq!(summary.total_return, None);
        assert_eq!(summary.sharpe, None);
        assert_eq!(summary.num_trades, None);
    }

    #[test]
    fn formatters_render_missing_as_em_dash() {
        assert_eq!(pct(None), SharedString::from("—"));
        assert_eq!(ratio(None), SharedString::from("—"));
        assert_eq!(money(None), SharedString::from("—"));
        assert_eq!(count(None), SharedString::from("—"));
        assert_eq!(pct(Some(0.161)), SharedString::from("+16.1%"));
        assert_eq!(money(Some(-456.0)), SharedString::from("-$456"));
        assert_eq!(ratio(Some(1.3)), SharedString::from("1.30"));
    }
}
