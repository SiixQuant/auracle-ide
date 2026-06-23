//! gpui-free reducer for the Desk overview dashboard. Turns a snapshot of account
//! + strategy facts into the labelled rows the panel renders, honestly: a value
//! the engine actually reported (including a real `0`) passes through truthfully;
//! a missing value (`None`) or a non-finite one becomes an Unknown row, never a
//! fabricated zero. Pure + unit-tested; the GPUI panel is a thin render over this.

/// How a metric reads at a glance; the panel maps this to a theme colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Positive,
    Negative,
    Neutral,
    /// The engine did not report this value — shown as "Unknown", never a 0.
    Unknown,
}

/// One labelled metric the Desk renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricRow {
    pub label: &'static str,
    pub value: String,
    pub tone: Tone,
}

/// A per-strategy row: name + raw status + a P&L metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyRow {
    pub name: String,
    pub status: String,
    pub pnl: MetricRow,
}

/// Raw per-strategy input.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyInput {
    pub name: String,
    pub status: String,
    pub pnl: Option<f64>,
}

/// The overview snapshot, every account figure optional so absence is honest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeskInput {
    pub equity: Option<f64>,
    pub open_pnl: Option<f64>,
    pub day_pnl: Option<f64>,
    pub buying_power: Option<f64>,
    pub positions: Option<u32>,
    pub strategies: Vec<StrategyInput>,
}

/// The reduced view the Desk panel renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskView {
    pub metrics: Vec<MetricRow>,
    pub strategies: Vec<StrategyRow>,
}

fn fmt_money(value: f64) -> String {
    format!("${:.2}", value)
}

fn fmt_signed(value: f64) -> String {
    if value >= 0.0 {
        format!("+${:.2}", value)
    } else {
        format!("-${:.2}", value.abs())
    }
}

fn pnl_tone(value: f64) -> Tone {
    if value > 0.0 {
        Tone::Positive
    } else if value < 0.0 {
        Tone::Negative
    } else {
        Tone::Neutral
    }
}

/// A plain currency metric (equity, buying power): Neutral when present, Unknown
/// when missing or non-finite.
fn plain_money_row(label: &'static str, value: Option<f64>) -> MetricRow {
    match value {
        Some(v) if v.is_finite() => MetricRow {
            label,
            value: fmt_money(v),
            tone: Tone::Neutral,
        },
        _ => unknown_row(label),
    }
}

/// A signed P&L metric: tone tracks the sign; Unknown when missing/non-finite.
fn pnl_row(label: &'static str, value: Option<f64>) -> MetricRow {
    match value {
        Some(v) if v.is_finite() => MetricRow {
            label,
            value: fmt_signed(v),
            tone: pnl_tone(v),
        },
        _ => unknown_row(label),
    }
}

fn unknown_row(label: &'static str) -> MetricRow {
    MetricRow {
        label,
        value: "Unknown".to_string(),
        tone: Tone::Unknown,
    }
}

fn positions_row(value: Option<u32>) -> MetricRow {
    match value {
        Some(n) => MetricRow {
            label: "Positions",
            value: n.to_string(),
            tone: Tone::Neutral,
        },
        None => unknown_row("Positions"),
    }
}

/// Reduce an overview snapshot into the Desk's rows.
pub fn build_desk(input: DeskInput) -> DeskView {
    let metrics = vec![
        plain_money_row("Equity", input.equity),
        pnl_row("Day P&L", input.day_pnl),
        pnl_row("Open P&L", input.open_pnl),
        plain_money_row("Buying Power", input.buying_power),
        positions_row(input.positions),
    ];

    let strategies = input
        .strategies
        .into_iter()
        .map(|s| StrategyRow {
            name: s.name,
            status: s.status,
            pnl: pnl_row("P&L", s.pnl),
        })
        .collect();

    DeskView {
        metrics,
        strategies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_missing_reads_unknown_never_zero() {
        let view = build_desk(DeskInput::default());
        for row in &view.metrics {
            assert_eq!(row.tone, Tone::Unknown, "{} should be Unknown", row.label);
            assert_eq!(row.value, "Unknown");
        }
        assert!(view.strategies.is_empty());
    }

    #[test]
    fn a_real_zero_passes_through_truthfully() {
        let view = build_desk(DeskInput {
            equity: Some(0.0),
            day_pnl: Some(0.0),
            positions: Some(0),
            ..Default::default()
        });
        let equity = &view.metrics[0];
        assert_eq!(equity.value, "$0.00");
        assert_eq!(equity.tone, Tone::Neutral);
        let day = &view.metrics[1];
        assert_eq!(day.value, "+$0.00");
        assert_eq!(day.tone, Tone::Neutral);
        let pos = &view.metrics[4];
        assert_eq!(pos.value, "0");
        assert_eq!(pos.tone, Tone::Neutral);
    }

    #[test]
    fn pnl_sign_drives_tone_and_display() {
        let view = build_desk(DeskInput {
            day_pnl: Some(1234.5),
            open_pnl: Some(-42.0),
            ..Default::default()
        });
        assert_eq!(view.metrics[1].value, "+$1234.50");
        assert_eq!(view.metrics[1].tone, Tone::Positive);
        assert_eq!(view.metrics[2].value, "-$42.00");
        assert_eq!(view.metrics[2].tone, Tone::Negative);
    }

    #[test]
    fn non_finite_is_unknown_not_a_number() {
        let view = build_desk(DeskInput {
            equity: Some(f64::NAN),
            buying_power: Some(f64::INFINITY),
            ..Default::default()
        });
        assert_eq!(view.metrics[0].tone, Tone::Unknown);
        assert_eq!(view.metrics[3].tone, Tone::Unknown);
    }

    #[test]
    fn strategy_rows_carry_their_own_pnl_tone() {
        let view = build_desk(DeskInput {
            strategies: vec![
                StrategyInput {
                    name: "mean_reversion".into(),
                    status: "live".into(),
                    pnl: Some(88.0),
                },
                StrategyInput {
                    name: "carry".into(),
                    status: "paper".into(),
                    pnl: None,
                },
            ],
            ..Default::default()
        });
        assert_eq!(view.strategies.len(), 2);
        assert_eq!(view.strategies[0].pnl.tone, Tone::Positive);
        assert_eq!(view.strategies[0].status, "live");
        assert_eq!(view.strategies[1].pnl.tone, Tone::Unknown);
        assert_eq!(view.strategies[1].pnl.value, "Unknown");
    }
}
