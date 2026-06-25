//! The gpui-free core of the **Flow** shell — the third co-equal presentation of
//! a session (Desk / Copilot / Flow), a node canvas whose signature moves are
//! **fork** and **compare** (decision D3). See [`auracle_session`] for the shell
//! switch model.
//!
//! This crate owns the graph state and the pure decisions over it, so they are
//! unit-tested without any rendering:
//!
//! * [`build_flow`] lays the user's strategies out as a grid of nodes.
//! * [`compare_metrics`] diffs two runs **honestly** — a delta is produced only
//!   for a metric BOTH runs report; anything absent on either side stays `None`
//!   so the canvas renders an em-dash, never a fabricated difference. This is the
//!   client-side answer to the architecture's open "compare must not fabricate
//!   deltas" question: we diff only the numbers the engine actually returned.
//! * [`fork`] adds a client-side draft child of a node, linked by a fork edge —
//!   a *starting point* to iterate in the editor, never a fabricated engine run.
//!
//! Nodes are the user's strategies (from `/ui/api/backtest/strategies`); a
//! node's metrics are filled only after its backtest runs ([`set_summary`]),
//! so an un-run node honestly shows no numbers rather than zeros.

use auracle_strategies::StrategyListItem;
use auracle_studio_results::BacktestSummary;

/// Logical-pixel position of a node on the canvas, measured from the canvas
/// origin. Mutated by dragging; the layout in [`build_flow`] only seeds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

/// What a node represents. A `Strategy` mirrors an engine strategy; a `Draft` is
/// a client-side fork starting point that has no engine run of its own yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Strategy,
    Draft,
}

/// One node on the Flow canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowNode {
    /// Stable identity: the strategy's dotted path, or `draft:N` for a fork.
    pub id: String,
    /// Display name (last dotted segment, or "<name> (fork)" for a draft).
    pub name: String,
    /// Dotted module path to run/open. `None` only if a draft had no parent path.
    pub path: Option<String>,
    /// First line of the strategy docstring, or "".
    pub doc: String,
    /// True for bundled example strategies (sorted after user-written ones).
    pub bundled: bool,
    pub kind: NodeKind,
    /// Backtest metrics — `None` until this node's backtest has been run.
    pub summary: Option<BacktestSummary>,
    pub pos: Pos,
}

/// The kind of relationship an edge encodes. Only forks are drawn today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Fork,
}

/// A directed link between two nodes (parent → fork child).
#[derive(Clone, Debug, PartialEq)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// The whole canvas: every node and every edge. The active shell renders this;
/// switching shells never resets it (decision D2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowView {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

/// Card geometry, shared with the panel so hit-testing and edge endpoints line
/// up with what's drawn.
pub const NODE_W: f32 = 220.0;
pub const NODE_H: f32 = 132.0;
const COL_GAP: f32 = 48.0;
const ROW_GAP: f32 = 40.0;
const MARGIN: f32 = 24.0;
/// Columns in the seed grid layout. Three keeps a readable board on a docked
/// canvas; dragging can rearrange freely afterward.
const COLS: usize = 3;

/// Seed the canvas from the strategy list: one node per strategy, laid out in a
/// left-to-right, top-to-bottom grid. Order is the navigator's order (user
/// strategies before bundled, then by name) so the board is deterministic.
pub fn build_flow(rows: Vec<StrategyListItem>) -> FlowView {
    let nodes = rows
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let col = i % COLS;
            let row = i / COLS;
            FlowNode {
                id: item.path.clone(),
                name: item.name,
                path: Some(item.path),
                doc: item.doc,
                bundled: item.bundled,
                kind: NodeKind::Strategy,
                summary: None,
                pos: Pos {
                    x: MARGIN + col as f32 * (NODE_W + COL_GAP),
                    y: MARGIN + row as f32 * (NODE_H + ROW_GAP),
                },
            }
        })
        .collect();
    FlowView {
        nodes,
        edges: Vec::new(),
    }
}

/// Which way a metric improves — drives the delta's tone. `CloserToZeroBetter`
/// exists for drawdown, where the better run is the one with the smaller
/// *magnitude* regardless of whether the engine encodes it as a negative number
/// (-0.30) or a positive one (0.30) — so we never claim a direction the engine's
/// sign convention doesn't support. `Neutral` metrics (e.g. trade count) get no
/// tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    HigherBetter,
    LowerBetter,
    CloserToZeroBetter,
    Neutral,
}

/// One row of a two-run comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct CompareRow {
    pub label: &'static str,
    pub a: Option<f64>,
    pub b: Option<f64>,
    /// `b - a`, present ONLY when both sides have a value — never fabricated.
    pub delta: Option<f64>,
    pub direction: Direction,
}

impl CompareRow {
    /// Whether `b` is an improvement over `a`, given the metric's direction.
    /// `None` when there's no delta or the direction is neutral, so the renderer
    /// can stay tone-neutral rather than guess.
    pub fn improved(&self) -> Option<bool> {
        match self.direction {
            Direction::Neutral => None,
            Direction::CloserToZeroBetter => {
                let (a, b) = (self.a?, self.b?);
                let (a_mag, b_mag) = (a.abs(), b.abs());
                if a_mag == b_mag {
                    None
                } else {
                    Some(b_mag < a_mag)
                }
            }
            Direction::HigherBetter => {
                let delta = self.delta?;
                if delta == 0.0 {
                    None
                } else {
                    Some(delta > 0.0)
                }
            }
            Direction::LowerBetter => {
                let delta = self.delta?;
                if delta == 0.0 {
                    None
                } else {
                    Some(delta < 0.0)
                }
            }
        }
    }
}

fn compare_row(
    label: &'static str,
    a: Option<f64>,
    b: Option<f64>,
    direction: Direction,
) -> CompareRow {
    let delta = match (a, b) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    CompareRow {
        label,
        a,
        b,
        delta,
        direction,
    }
}

/// Diff two runs into a fixed set of rows. A row's `delta` exists only when the
/// engine returned that metric for BOTH runs — the honesty invariant.
pub fn compare_metrics(a: &BacktestSummary, b: &BacktestSummary) -> Vec<CompareRow> {
    vec![
        compare_row(
            "Net profit",
            a.net_profit,
            b.net_profit,
            Direction::HigherBetter,
        ),
        compare_row(
            "Return",
            a.total_return,
            b.total_return,
            Direction::HigherBetter,
        ),
        compare_row("Sharpe", a.sharpe, b.sharpe, Direction::HigherBetter),
        compare_row(
            "Max drawdown",
            a.max_drawdown,
            b.max_drawdown,
            Direction::CloserToZeroBetter,
        ),
        compare_row("Win rate", a.win_rate, b.win_rate, Direction::HigherBetter),
        compare_row("Turnover", a.turnover, b.turnover, Direction::LowerBetter),
        compare_row(
            "Trades",
            a.num_trades.map(|n| n as f64),
            b.num_trades.map(|n| n as f64),
            Direction::Neutral,
        ),
    ]
}

/// Attach (or replace) a node's backtest metrics after its run completes.
/// Returns whether a matching node was found.
pub fn set_summary(view: &mut FlowView, id: &str, summary: BacktestSummary) -> bool {
    if let Some(node) = view.nodes.iter_mut().find(|n| n.id == id) {
        node.summary = Some(summary);
        true
    } else {
        false
    }
}

/// Move a node by a drag delta. Returns whether a matching node was found.
pub fn move_node(view: &mut FlowView, id: &str, dx: f32, dy: f32) -> bool {
    if let Some(node) = view.nodes.iter_mut().find(|n| n.id == id) {
        node.pos.x += dx;
        node.pos.y += dy;
        true
    } else {
        false
    }
}

/// Fork a node: add a client-side draft child placed just below-right of its
/// parent and linked by a fork edge. `draft_seq` makes the new id unique across
/// repeated forks. Returns the new draft's id, or `None` if the parent is gone.
///
/// A draft is a *starting point* — it carries the parent's path so the panel can
/// open that file to iterate, but it has no metrics of its own until run.
pub fn fork(view: &mut FlowView, parent_id: &str, draft_seq: usize) -> Option<String> {
    let (name, path, doc, pos) = {
        let parent = view.nodes.iter().find(|n| n.id == parent_id)?;
        (
            parent.name.clone(),
            parent.path.clone(),
            parent.doc.clone(),
            parent.pos,
        )
    };
    let draft_id = format!("draft:{draft_seq}");
    view.nodes.push(FlowNode {
        id: draft_id.clone(),
        name: format!("{name} (fork)"),
        path,
        doc,
        bundled: false,
        kind: NodeKind::Draft,
        summary: None,
        pos: Pos {
            x: pos.x + 40.0,
            y: pos.y + NODE_H + 24.0,
        },
    });
    view.edges.push(FlowEdge {
        from: parent_id.to_string(),
        to: draft_id.clone(),
        kind: EdgeKind::Fork,
    });
    Some(draft_id)
}

/// The center point of a node's card — the anchor edges are drawn between.
pub fn node_center(node: &FlowNode) -> Pos {
    Pos {
        x: node.pos.x + NODE_W / 2.0,
        y: node.pos.y + NODE_H / 2.0,
    }
}

/// Look up a node's center by id (for resolving an edge's endpoints).
pub fn center_of(view: &FlowView, id: &str) -> Option<Pos> {
    view.nodes.iter().find(|n| n.id == id).map(node_center)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, bundled: bool) -> StrategyListItem {
        StrategyListItem {
            name: auracle_strategies::display_name(path),
            path: path.to_string(),
            doc: String::new(),
            bundled,
        }
    }

    fn summary(net: Option<f64>, sharpe: Option<f64>, dd: Option<f64>) -> BacktestSummary {
        BacktestSummary {
            strategy: "s".into(),
            net_profit: net,
            sharpe,
            max_drawdown: dd,
            ..Default::default()
        }
    }

    #[test]
    fn build_flow_makes_one_node_per_strategy() {
        let view = build_flow(vec![item("a.b.C", false), item("a.b.D", false)]);
        assert_eq!(view.nodes.len(), 2);
        assert_eq!(view.nodes[0].name, "C");
        assert_eq!(view.nodes[0].kind, NodeKind::Strategy);
        assert!(view.nodes[0].summary.is_none());
        assert!(view.edges.is_empty());
    }

    #[test]
    fn build_flow_lays_out_a_grid() {
        let rows: Vec<_> = (0..4).map(|i| item(&format!("p.S{i}"), false)).collect();
        let view = build_flow(rows);
        // First three across one row, fourth wraps under the first.
        assert_eq!(
            view.nodes[0].pos,
            Pos {
                x: MARGIN,
                y: MARGIN
            }
        );
        assert!(view.nodes[1].pos.x > view.nodes[0].pos.x);
        assert_eq!(view.nodes[1].pos.y, view.nodes[0].pos.y);
        assert_eq!(view.nodes[3].pos.x, view.nodes[0].pos.x);
        assert!(view.nodes[3].pos.y > view.nodes[0].pos.y);
    }

    #[test]
    fn compare_produces_a_delta_only_when_both_sides_have_the_metric() {
        let a = summary(Some(100.0), Some(1.0), None);
        let b = summary(Some(150.0), None, Some(-0.2));
        let rows = compare_metrics(&a, &b);
        let net = rows.iter().find(|r| r.label == "Net profit").unwrap();
        assert_eq!(net.delta, Some(50.0));
        // Sharpe present only on a → no fabricated delta.
        let sharpe = rows.iter().find(|r| r.label == "Sharpe").unwrap();
        assert_eq!(sharpe.delta, None);
        // Max drawdown present only on b → no delta either.
        let dd = rows.iter().find(|r| r.label == "Max drawdown").unwrap();
        assert_eq!(dd.delta, None);
    }

    #[test]
    fn improvement_tone_respects_metric_direction() {
        // More net profit is better.
        let net = compare_row(
            "Net profit",
            Some(100.0),
            Some(120.0),
            Direction::HigherBetter,
        );
        assert_eq!(net.improved(), Some(true));
        // Drawdown improves when its magnitude shrinks, regardless of sign
        // convention: -0.20 is a smaller drawdown than -0.30.
        let dd = compare_row(
            "Max drawdown",
            Some(-0.30),
            Some(-0.20),
            Direction::CloserToZeroBetter,
        );
        assert_eq!(dd.improved(), Some(true));
        // The same holds for a positive-magnitude encoding (0.20 < 0.30).
        let dd_pos = compare_row(
            "Max drawdown",
            Some(0.30),
            Some(0.20),
            Direction::CloserToZeroBetter,
        );
        assert_eq!(dd_pos.improved(), Some(true));
        // A larger drawdown is worse.
        let worse = compare_row(
            "Max drawdown",
            Some(-0.20),
            Some(-0.35),
            Direction::CloserToZeroBetter,
        );
        assert_eq!(worse.improved(), Some(false));
        // Neutral metrics never claim an improvement.
        let trades = compare_row("Trades", Some(10.0), Some(99.0), Direction::Neutral);
        assert_eq!(trades.improved(), None);
        // No delta → no tone.
        let missing = compare_row("Sharpe", Some(1.0), None, Direction::HigherBetter);
        assert_eq!(missing.improved(), None);
    }

    #[test]
    fn fork_adds_a_draft_child_and_an_edge() {
        let mut view = build_flow(vec![item("strategies.m.Mom", false)]);
        let parent_id = view.nodes[0].id.clone();
        let draft = fork(&mut view, &parent_id, 1).expect("parent exists");
        assert_eq!(view.nodes.len(), 2);
        let child = view.nodes.iter().find(|n| n.id == draft).unwrap();
        assert_eq!(child.kind, NodeKind::Draft);
        assert_eq!(child.name, "Mom (fork)");
        assert_eq!(child.path.as_deref(), Some("strategies.m.Mom"));
        assert!(child.summary.is_none());
        assert_eq!(view.edges.len(), 1);
        assert_eq!(view.edges[0].from, parent_id);
        assert_eq!(view.edges[0].to, draft);
    }

    #[test]
    fn fork_of_a_missing_node_is_a_no_op() {
        let mut view = build_flow(vec![item("a.B", false)]);
        assert_eq!(fork(&mut view, "nope", 1), None);
        assert_eq!(view.nodes.len(), 1);
        assert!(view.edges.is_empty());
    }

    #[test]
    fn set_summary_fills_only_the_named_node() {
        let mut view = build_flow(vec![item("a.B", false), item("a.C", false)]);
        let id = view.nodes[1].id.clone();
        assert!(set_summary(&mut view, &id, summary(Some(1.0), None, None)));
        assert!(view.nodes[0].summary.is_none());
        assert!(view.nodes[1].summary.is_some());
        assert!(!set_summary(
            &mut view,
            "missing",
            summary(None, None, None)
        ));
    }

    #[test]
    fn move_node_translates_position() {
        let mut view = build_flow(vec![item("a.B", false)]);
        let id = view.nodes[0].id.clone();
        let before = view.nodes[0].pos;
        assert!(move_node(&mut view, &id, 12.0, -5.0));
        assert_eq!(view.nodes[0].pos.x, before.x + 12.0);
        assert_eq!(view.nodes[0].pos.y, before.y - 5.0);
        assert!(!move_node(&mut view, "missing", 1.0, 1.0));
    }

    #[test]
    fn center_of_resolves_edge_endpoints() {
        let mut view = build_flow(vec![item("a.B", false)]);
        let id = view.nodes[0].id.clone();
        let center = center_of(&view, &id).unwrap();
        assert_eq!(center.x, MARGIN + NODE_W / 2.0);
        assert_eq!(center.y, MARGIN + NODE_H / 2.0);
        assert_eq!(center_of(&view, "missing"), None);
    }
}
