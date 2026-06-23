//! Shared, gpui-free parsing and formatting for Studio backtest results.
//!
//! The cockpit toolbar and the Studio results tab both render the same engine
//! `/backtest` and diagnostics payloads. When each surface parsed and formatted
//! those payloads independently they drifted (the cockpit said `return 16.1%`
//! while the tab said `+16.1%`). This crate is the single source of truth so
//! both agree, and — because it depends on `serde_json` only — every decision is
//! unit-tested without a graphics toolchain. See `RUBRIC.md` in the
//! `auracle_view_state` crate (item 5, honesty; item b, one visual system).
//!
//! Honesty is the governing rule: an absent or non-numeric engine field becomes
//! `None` (rendered as an em-dash), never a fabricated `0.0` or `"unknown"`
//! string, and the robustness verdict never celebrates an in-sample edge whose
//! out-of-sample behaviour the engine didn't report.

/// One point on the equity curve: an engine timestamp (seconds) and the account
/// value at that time. Only finite values from the engine ever reach this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EquityPoint {
    pub t: f64,
    pub equity: f64,
}

/// Parsed in-sample backtest statistics. Every field is optional and is filled
/// only from a numeric engine value — an absent field stays `None` so the render
/// layer shows an em-dash rather than a fabricated number.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacktestSummary {
    pub strategy: String,
    pub net_profit: Option<f64>,
    pub total_return: Option<f64>,
    pub sharpe: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub win_rate: Option<f64>,
    pub turnover: Option<f64>,
    pub num_trades: Option<u64>,
    /// The equity curve, empty when the engine did not include one (it does not
    /// today). Never synthesised — an absent series stays empty so the chart
    /// shows its honest "no curve yet" state rather than a fabricated line.
    pub equity: Vec<EquityPoint>,
}

impl BacktestSummary {
    /// Parse the engine `/backtest` response (`{ "stats": { … } }`) into a
    /// summary, leaving any absent or non-numeric stat as `None`.
    pub fn from_engine(strategy: impl Into<String>, value: &serde_json::Value) -> Self {
        let stats = value.get("stats");
        let number = |key: &str| stats.and_then(|s| s.get(key)).and_then(|v| v.as_f64());
        let integer = |key: &str| stats.and_then(|s| s.get(key)).and_then(|v| v.as_u64());
        // The equity curve (`{ "equity": { "series": [{ "t", "equity" }] } }`) is
        // not part of today's payload; when absent or non-numeric it stays empty,
        // never synthesised.
        let equity = value
            .get("equity")
            .and_then(|e| e.get("series"))
            .and_then(|s| s.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let t = row.get("t")?.as_f64()?;
                        let equity = row.get("equity")?.as_f64()?;
                        (t.is_finite() && equity.is_finite()).then_some(EquityPoint { t, equity })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            strategy: strategy.into(),
            net_profit: number("net_profit"),
            total_return: number("total_return"),
            sharpe: number("sharpe"),
            max_drawdown: number("max_drawdown"),
            win_rate: number("win_rate"),
            turnover: number("turnover"),
            num_trades: integer("num_trades"),
            equity,
        }
    }

    /// True when the engine returned no statistics at all. This drives the tab's
    /// designed empty state instead of rendering a row of em-dashes that looks
    /// broken. A legitimate `0.0` is `Some(0.0)`, so it is not "empty".
    pub fn is_empty(&self) -> bool {
        self.net_profit.is_none()
            && self.total_return.is_none()
            && self.sharpe.is_none()
            && self.max_drawdown.is_none()
            && self.win_rate.is_none()
            && self.turnover.is_none()
            && self.num_trades.is_none()
    }
}

/// The single em-dash both surfaces show for a missing value, so they match.
pub const MISSING: &str = "—";

/// A signed percentage, e.g. `+16.1%`; `—` when absent.
pub fn fmt_pct(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{:+.1}%", v * 100.0),
        None => MISSING.to_string(),
    }
}

/// A two-decimal ratio, e.g. `1.30`; `—` when absent.
pub fn fmt_ratio(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.2}"),
        None => MISSING.to_string(),
    }
}

/// A signed whole-dollar amount, e.g. `-$456`; `—` when absent.
pub fn fmt_money(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{}${:.0}", if v < 0.0 { "-" } else { "+" }, v.abs()),
        None => MISSING.to_string(),
    }
}

/// A turnover multiple, e.g. `3.2×`; `—` when absent.
pub fn fmt_turns(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.1}×"),
        None => MISSING.to_string(),
    }
}

/// A trade count, e.g. `142`; `—` when absent.
pub fn fmt_count(value: Option<u64>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => MISSING.to_string(),
    }
}

/// A one-line cockpit status string for a `/backtest` response, formatted with
/// the same helpers the results tab uses so the two surfaces never disagree.
/// Falls back to a bare "Backtest done." when the engine returned no stats.
pub fn backtest_oneline(value: &serde_json::Value) -> String {
    let summary = BacktestSummary::from_engine(String::new(), value);
    if summary.is_empty() {
        return "Backtest done.".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if summary.total_return.is_some() {
        parts.push(format!("return {}", fmt_pct(summary.total_return)));
    }
    if summary.sharpe.is_some() {
        parts.push(format!("sharpe {}", fmt_ratio(summary.sharpe)));
    }
    if parts.is_empty() {
        return "Backtest done.".to_string();
    }
    format!("Backtest done — {}", parts.join(", "))
}

/// Parsed out-of-sample / robustness diagnostics. `sensitivity` is `None` when
/// absent — never the fabricated `"unknown"` string the gpui crate used to
/// coerce it to, so the render layer shows an honest em-dash.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StudioDiagnostics {
    pub in_sample_sharpe: Option<f64>,
    pub out_of_sample_sharpe: Option<f64>,
    pub robustness: Option<f64>,
    pub sensitivity: Option<String>,
}

impl StudioDiagnostics {
    /// Parse the engine diagnostics payload, leaving absent fields as `None`.
    pub fn from_engine(value: &serde_json::Value) -> Self {
        let sharpe = |block: &str| {
            value
                .get(block)
                .and_then(|s| s.get("sharpe"))
                .and_then(|v| v.as_f64())
        };
        Self {
            in_sample_sharpe: sharpe("in_sample"),
            out_of_sample_sharpe: sharpe("out_of_sample"),
            robustness: value.get("robustness").and_then(|v| v.as_f64()),
            sensitivity: value
                .get("sensitivity")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        }
    }
}

/// An honest plain-English verdict on out-of-sample robustness. The variants
/// distinguish "the engine ran but out-of-sample wasn't evaluated" from "the
/// engine gave us nothing", and never claim an edge holds unless both sharpes
/// are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobustnessVerdict {
    /// Out-of-sample sharpe is negative — the edge does not survive.
    DoesNotSurvive,
    /// Out-of-sample sharpe is non-negative but well below in-sample.
    WeakensSharply,
    /// Out-of-sample sharpe holds up against in-sample.
    Holds,
    /// In-sample present, but the engine reported no out-of-sample sharpe.
    NotEvaluated,
    /// Neither sharpe is present — nothing to judge.
    Unavailable,
}

/// Classify the robustness of a run. `Holds`/`WeakensSharply`/`DoesNotSurvive`
/// require both sharpes; with only an in-sample sharpe the result is
/// `NotEvaluated`, and with neither it is `Unavailable`.
pub fn robustness_verdict(diagnostics: &StudioDiagnostics) -> RobustnessVerdict {
    match (
        diagnostics.in_sample_sharpe,
        diagnostics.out_of_sample_sharpe,
    ) {
        (Some(_), Some(out_of_sample)) if out_of_sample < 0.0 => RobustnessVerdict::DoesNotSurvive,
        (Some(in_sample), Some(out_of_sample)) if out_of_sample < in_sample * 0.5 => {
            RobustnessVerdict::WeakensSharply
        }
        (Some(_), Some(_)) => RobustnessVerdict::Holds,
        (Some(_), None) => RobustnessVerdict::NotEvaluated,
        _ => RobustnessVerdict::Unavailable,
    }
}

/// The exact sentence each verdict renders as.
pub fn verdict_text(verdict: RobustnessVerdict) -> &'static str {
    match verdict {
        RobustnessVerdict::DoesNotSurvive => {
            "The edge does not survive out-of-sample — I would not deploy this."
        }
        RobustnessVerdict::WeakensSharply => {
            "The edge weakens sharply out-of-sample — treat it with caution."
        }
        RobustnessVerdict::Holds => "The edge holds out-of-sample.",
        RobustnessVerdict::NotEvaluated => "Out-of-sample not evaluated for this run.",
        RobustnessVerdict::Unavailable => "Diagnostics unavailable.",
    }
}

/// The outcome of a diagnostics fetch, classified so the render layer can tell a
/// route-not-deployed-yet case (non-retryable, honest "coming via engine") apart
/// from a transient failure (retryable, with a Retry affordance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagFetchOutcome {
    /// A 2xx response — diagnostics are present.
    Ready,
    /// The route is absent (404 / "route not found") — non-retryable.
    Unavailable,
    /// A server or transport error — retryable.
    Failed { message: String },
}

/// Classify a diagnostics fetch from its HTTP status and/or error body.
///
/// Only a 404 (or, when the status is unavailable, an error body that names a
/// missing route) is treated as `Unavailable`; everything else — 5xx, other
/// non-2xx, and transport errors with no status — is `Failed` and therefore
/// retryable. A transport error (`status == None`) is never `Unavailable`,
/// because a connection that never reached the engine tells us nothing about
/// whether the route exists.
///
/// The `error` body is the documented fallback for transports that collapse the
/// response to `Result<Value, _>` without surfacing a status code: when `status`
/// is `None` we sniff the body for "404"/"not found" so a route-absent error can
/// still be reported honestly as `Unavailable` rather than as a retryable
/// failure.
pub fn classify_diag_fetch(status: Option<u16>, error: Option<&str>) -> DiagFetchOutcome {
    match status {
        Some(code) if (200..300).contains(&code) => DiagFetchOutcome::Ready,
        Some(404) => DiagFetchOutcome::Unavailable,
        Some(code) => DiagFetchOutcome::Failed {
            message: error
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Engine returned HTTP {code}.")),
        },
        None => {
            let body = error.unwrap_or("");
            if body_names_missing_route(body) {
                DiagFetchOutcome::Unavailable
            } else {
                DiagFetchOutcome::Failed {
                    message: if body.is_empty() {
                        "Couldn't reach the engine.".to_string()
                    } else {
                        body.to_string()
                    },
                }
            }
        }
    }
}

/// Whether an error body (status unknown) names a missing route. Case-insensitive
/// match on the documented markers; deliberately conservative so a transient
/// failure is never mistaken for a permanently-absent route.
fn body_names_missing_route(body: &str) -> bool {
    let lowered = body.to_ascii_lowercase();
    lowered.contains("404") || lowered.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_engine_parses_present_stats_and_leaves_absent_none() {
        let value = serde_json::json!({
            "stats": { "total_return": 0.161, "sharpe": 1.3, "num_trades": 142 }
        });
        let summary = BacktestSummary::from_engine("Gap", &value);
        assert_eq!(summary.strategy, "Gap");
        assert_eq!(summary.total_return, Some(0.161));
        assert_eq!(summary.sharpe, Some(1.3));
        assert_eq!(summary.num_trades, Some(142));
        assert_eq!(summary.net_profit, None);
        assert_eq!(summary.max_drawdown, None);
        assert!(!summary.is_empty());
    }

    #[test]
    fn equity_series_is_parsed_when_present_and_empty_otherwise() {
        // Absent today: stays empty, never synthesised.
        let bare = BacktestSummary::from_engine("Gap", &serde_json::json!({ "stats": {} }));
        assert!(bare.equity.is_empty());

        // Present + finite: parsed in order; non-finite/malformed points dropped.
        let value = serde_json::json!({
            "equity": { "series": [
                { "t": 1.0, "equity": 100.0 },
                { "t": 2.0, "equity": 101.5 },
                { "t": 3.0, "equity": "nope" },
                { "t": 4.0 }
            ] }
        });
        let summary = BacktestSummary::from_engine("Gap", &value);
        assert_eq!(summary.equity.len(), 2);
        assert_eq!(
            summary.equity[0],
            EquityPoint {
                t: 1.0,
                equity: 100.0
            }
        );
        assert_eq!(summary.equity[1].equity, 101.5);
    }

    #[test]
    fn from_engine_empty_object_is_all_none_and_empty() {
        let summary = BacktestSummary::from_engine("Gap", &serde_json::json!({}));
        assert_eq!(summary.net_profit, None);
        assert_eq!(summary.total_return, None);
        assert_eq!(summary.sharpe, None);
        assert_eq!(summary.max_drawdown, None);
        assert_eq!(summary.win_rate, None);
        assert_eq!(summary.turnover, None);
        assert_eq!(summary.num_trades, None);
        assert!(summary.is_empty());
    }

    #[test]
    fn is_empty_false_when_any_single_field_present() {
        let value = serde_json::json!({ "stats": { "sharpe": 0.0 } });
        let summary = BacktestSummary::from_engine("Gap", &value);
        assert_eq!(summary.sharpe, Some(0.0));
        assert!(!summary.is_empty());
    }

    #[test]
    fn formatters_render_em_dash_for_none_and_exact_format_for_value() {
        assert_eq!(fmt_pct(None), MISSING);
        assert_eq!(fmt_pct(Some(0.161)), "+16.1%");
        assert_eq!(fmt_ratio(None), MISSING);
        assert_eq!(fmt_ratio(Some(1.3)), "1.30");
        assert_eq!(fmt_money(None), MISSING);
        assert_eq!(fmt_money(Some(-456.0)), "-$456");
        assert_eq!(fmt_money(Some(456.0)), "+$456");
        assert_eq!(fmt_turns(None), MISSING);
        assert_eq!(fmt_turns(Some(3.2)), "3.2×");
        assert_eq!(fmt_count(None), MISSING);
        assert_eq!(fmt_count(Some(142)), "142");
    }

    #[test]
    fn backtest_oneline_matches_tab_formatting_for_same_payload() {
        let value = serde_json::json!({
            "stats": { "total_return": 0.161, "sharpe": 1.3, "num_trades": 142 }
        });
        // Anti-drift: the cockpit one-liner uses the same formatters the tab's
        // stat strip uses, so the numbers it shows match the tab exactly.
        let summary = BacktestSummary::from_engine("Gap", &value);
        let line = backtest_oneline(&value);
        assert!(line.contains(&fmt_pct(summary.total_return)));
        assert!(line.contains(&fmt_ratio(summary.sharpe)));
        assert_eq!(line, "Backtest done — return +16.1%, sharpe 1.30");
    }

    #[test]
    fn backtest_oneline_falls_back_when_no_stats() {
        assert_eq!(backtest_oneline(&serde_json::json!({})), "Backtest done.");
    }

    #[test]
    fn diagnostics_absent_sensitivity_is_none_not_unknown_string() {
        let diagnostics = StudioDiagnostics::from_engine(&serde_json::json!({
            "in_sample": { "sharpe": 1.3 }
        }));
        assert_eq!(diagnostics.in_sample_sharpe, Some(1.3));
        assert_eq!(diagnostics.out_of_sample_sharpe, None);
        assert_eq!(diagnostics.sensitivity, None);
    }

    #[test]
    fn robustness_verdict_distinguishes_each_case() {
        let does_not_survive = StudioDiagnostics {
            in_sample_sharpe: Some(2.8),
            out_of_sample_sharpe: Some(-0.2),
            ..Default::default()
        };
        assert_eq!(
            robustness_verdict(&does_not_survive),
            RobustnessVerdict::DoesNotSurvive
        );

        let holds = StudioDiagnostics {
            in_sample_sharpe: Some(1.3),
            out_of_sample_sharpe: Some(1.1),
            ..Default::default()
        };
        assert_eq!(robustness_verdict(&holds), RobustnessVerdict::Holds);

        let weakens = StudioDiagnostics {
            in_sample_sharpe: Some(1.3),
            out_of_sample_sharpe: Some(0.4),
            ..Default::default()
        };
        assert_eq!(
            robustness_verdict(&weakens),
            RobustnessVerdict::WeakensSharply
        );

        let not_evaluated = StudioDiagnostics {
            in_sample_sharpe: Some(1.3),
            out_of_sample_sharpe: None,
            ..Default::default()
        };
        assert_eq!(
            robustness_verdict(&not_evaluated),
            RobustnessVerdict::NotEvaluated
        );

        let unavailable = StudioDiagnostics::default();
        assert_eq!(
            robustness_verdict(&unavailable),
            RobustnessVerdict::Unavailable
        );
    }

    #[test]
    fn classify_diag_fetch_maps_status_and_transport_errors() {
        assert_eq!(
            classify_diag_fetch(Some(404), None),
            DiagFetchOutcome::Unavailable
        );
        assert_eq!(
            classify_diag_fetch(Some(500), None),
            DiagFetchOutcome::Failed {
                message: "Engine returned HTTP 500.".to_string()
            }
        );
        assert_eq!(
            classify_diag_fetch(None, Some("timeout")),
            DiagFetchOutcome::Failed {
                message: "timeout".to_string()
            }
        );
        assert_eq!(
            classify_diag_fetch(Some(200), None),
            DiagFetchOutcome::Ready
        );
    }

    #[test]
    fn classify_diag_fetch_string_sniff_fallback_when_status_absent() {
        // Documented fallback: with no HTTP status, a body naming a missing
        // route is route-absent (Unavailable); anything else is retryable.
        assert_eq!(
            classify_diag_fetch(None, Some("HTTP 404 Not Found")),
            DiagFetchOutcome::Unavailable
        );
        assert_eq!(
            classify_diag_fetch(None, Some("route not found")),
            DiagFetchOutcome::Unavailable
        );
        assert_eq!(
            classify_diag_fetch(None, Some("connection refused")),
            DiagFetchOutcome::Failed {
                message: "connection refused".to_string()
            }
        );
    }
}
