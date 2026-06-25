//! Flow — the node-canvas shell.
//!
//! The third co-equal shell (Desk / Copilot / Flow). Flow lays the user's
//! strategies out as a board of draggable nodes and gives them its two signature
//! verbs (decision D3): **fork** a node into a new draft to iterate, and
//! **compare** two runs head-to-head. The graph state and every pure decision
//! over it live in the gpui-free [`auracle_flow`] reducer; this file is the
//! render + async-I/O shell over it.
//!
//! Honesty by construction: a node shows metrics only after its backtest runs (a
//! missing stat is an em dash, never a zero), and the compare strip shows a delta
//! only for a metric BOTH runs reported — it never fabricates a difference.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_connections::post_json;
use auracle_flow::{
    CompareRow, FlowNode, FlowView, NODE_H, NODE_W, NodeKind, build_flow, center_of,
    compare_metrics, fork, move_node, set_summary,
};
use auracle_strategies::{StrategyListItem, module_to_relpath, strategy_rows};
use auracle_studio_results::{
    BacktestSummary, fmt_count, fmt_money, fmt_pct, fmt_ratio, fmt_turns,
};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, PathBuilder, Pixels, Point, Task, WeakEntity, Window, actions,
    canvas, point, px,
};
use ui::{TintColor, prelude::*};
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::{OpenOptions, OpenVisible, Workspace};

actions!(
    auracle_flow_panel,
    [
        /// Toggle focus on the Flow canvas.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<FlowPanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

/// An in-progress node drag: which node, and the last mouse position seen so the
/// next move can be applied as a delta.
struct Drag {
    node_id: String,
    last: Point<Pixels>,
}

pub struct FlowPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    view: FlowView,
    status: Status,
    /// Up to two node ids chosen for comparison (oldest drops when a third
    /// joins).
    selected: Vec<String>,
    /// The node whose backtest is currently running, if any.
    running: Option<String>,
    /// Monotonic counter so repeated forks get unique draft ids.
    fork_seq: usize,
    drag: Option<Drag>,
    /// A transient note (e.g. "open a project to edit this file") — honest
    /// feedback instead of a silent no-op.
    note: Option<SharedString>,
    _poll: Task<()>,
}

impl FlowPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        let handle = workspace.clone();
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.status = Status::Loading;
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    workspace: handle,
                    view: FlowView::default(),
                    status: if auracle_connect::load_config().api_key.is_some() {
                        Status::Loading
                    } else {
                        Status::NotConnected
                    },
                    selected: Vec::new(),
                    running: None,
                    fork_seq: 0,
                    drag: None,
                    note: None,
                    _poll: poll,
                }
            })
        })
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_strategies(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        this.apply_fetch(fetched);
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    return;
                }
                cx.background_executor().timer(POLL_EVERY).await;
            }
        })
    }

    /// Apply a fetch outcome. Re-fetching preserves any metrics already attached
    /// to surviving nodes and any drafts the user has forked: only brand-new
    /// strategies are added and vanished ones removed, so a background poll never
    /// wipes the board out from under the user.
    fn apply_fetch(&mut self, fetched: FetchResult) {
        match fetched {
            FetchResult::NotConnected => self.status = Status::NotConnected,
            FetchResult::Unreachable => self.status = Status::Unreachable,
            FetchResult::Ok(rows) => {
                self.status = Status::Connected;
                self.merge_strategies(rows);
            }
        }
    }

    /// Reconcile the freshly-fetched strategy list into the canvas. Existing
    /// nodes keep their position, summary, and selection; new strategies are laid
    /// out by the reducer; drafts (which have no engine strategy) are always
    /// kept.
    fn merge_strategies(&mut self, rows: Vec<StrategyListItem>) {
        if self.view.nodes.is_empty() {
            self.view = build_flow(rows);
            return;
        }
        let fresh = build_flow(rows);
        // Drop strategy nodes the engine no longer lists; keep drafts.
        let still_present: Vec<&str> = fresh.nodes.iter().map(|n| n.id.as_str()).collect();
        self.view
            .nodes
            .retain(|n| n.kind == NodeKind::Draft || still_present.contains(&n.id.as_str()));
        // Add strategy nodes we didn't have before, at their seeded positions.
        let existing: Vec<String> = self.view.nodes.iter().map(|n| n.id.clone()).collect();
        for node in fresh.nodes {
            if !existing.contains(&node.id) {
                self.view.nodes.push(node);
            }
        }
        // Prune selections / edges that point at nodes that no longer exist.
        let ids: Vec<String> = self.view.nodes.iter().map(|n| n.id.clone()).collect();
        self.selected.retain(|id| ids.contains(id));
        self.view
            .edges
            .retain(|e| ids.contains(&e.from) && ids.contains(&e.to));
    }

    fn refetch(&mut self, cx: &mut Context<Self>) {
        self.status = Status::Loading;
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let fetched = fetch_strategies(http).await;
            this.update(cx, |this, cx| {
                this.apply_fetch(fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── verbs ────────────────────────────────────────────────────────────

    fn begin_drag(&mut self, id: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag = Some(Drag {
            node_id: id,
            last: position,
        });
        cx.notify();
    }

    fn on_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((id, dx, dy)) = self.drag.as_mut().map(|drag| {
            let dx = f32::from(position.x - drag.last.x);
            let dy = f32::from(position.y - drag.last.y);
            drag.last = position;
            (drag.node_id.clone(), dx, dy)
        }) else {
            return;
        };
        move_node(&mut self.view, &id, dx, dy);
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    /// Add or remove a node from the (max-two) comparison set.
    fn toggle_compare(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.selected.iter().position(|s| s == &id) {
            self.selected.remove(pos);
        } else {
            self.selected.push(id);
            if self.selected.len() > 2 {
                self.selected.remove(0);
            }
        }
        cx.notify();
    }

    /// Run a node's backtest and attach the metrics. Errors leave the node
    /// un-run (no fabricated numbers) and surface an honest note.
    fn run_node(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(path) = self
            .view
            .nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.path.clone())
        else {
            return;
        };
        self.running = Some(id.clone());
        self.note = None;
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let body = serde_json::json!({ "strategy_path": path.clone() });
            let result = post_json(http, "/backtest", body).await;
            this.update(cx, |this, cx| {
                match &result {
                    Ok(value) => {
                        let summary = BacktestSummary::from_engine(path.clone(), value);
                        set_summary(&mut this.view, &id, summary);
                    }
                    Err(error) => {
                        this.note = Some(format!("Backtest failed: {error}.").into());
                    }
                }
                this.running = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fork a node: add a draft child on the canvas and open the parent's file so
    /// the user can start iterating the variant immediately.
    fn fork_node(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.fork_seq += 1;
        let seq = self.fork_seq;
        if fork(&mut self.view, &id, seq).is_none() {
            return;
        }
        let path = self
            .view
            .nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.path.clone());
        cx.notify();
        if let Some(path) = path {
            self.open_strategy(path.into(), window, cx);
        }
    }

    fn resolve_abs(&self, module_path: &str, cx: &App) -> Option<PathBuf> {
        let rel = module_to_relpath(module_path)?;
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().clone();
        let worktree = project.read(cx).visible_worktrees(cx).next()?;
        let root = worktree.read(cx).abs_path();
        Some(root.join(rel))
    }

    fn open_strategy(
        &mut self,
        module_path: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(abs) = self.resolve_abs(&module_path, cx) else {
            self.note = Some("Open this folder as a project to edit strategy files.".into());
            cx.notify();
            return;
        };
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |_this, cx| {
            workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_abs_path(
                        abs,
                        OpenOptions {
                            visible: Some(OpenVisible::None),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                })?
                .await?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    // ── render ───────────────────────────────────────────────────────────

    fn render_canvas(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.view.nodes.is_empty() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .child(Label::new("No strategies yet").color(Color::Muted))
                .child(
                    Label::new("Create a strategy in Build, then fork and compare runs here.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        }

        let content_w = self
            .view
            .nodes
            .iter()
            .map(|n| n.pos.x + NODE_W)
            .fold(0.0_f32, f32::max)
            + 24.0;
        let content_h = self
            .view
            .nodes
            .iter()
            .map(|n| n.pos.y + NODE_H)
            .fold(0.0_f32, f32::max)
            + 24.0;
        let nodes = self.view.nodes.clone();
        // Render the edges and each node to owned `AnyElement`s FIRST, so the
        // `&mut cx` borrow each one needs is released before the next — under
        // edition 2024 an `impl IntoElement` return captures the cx lifetime, so
        // these must be materialised sequentially, not inside a `.children(map)`
        // closure that would hold cx across iterations.
        let edges = self.render_edges(cx);
        let mut node_els: Vec<AnyElement> = Vec::with_capacity(nodes.len());
        for node in &nodes {
            node_els.push(self.render_node(node, cx));
        }

        div()
            .id("flow-canvas")
            .size_full()
            .overflow_scroll()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.on_drag_move(event.position, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _, cx| this.end_drag(cx)),
            )
            .child(
                div()
                    .relative()
                    .w(px(content_w.max(360.0)))
                    .h(px(content_h.max(240.0)))
                    .child(edges)
                    .children(node_els),
            )
            .into_any_element()
    }

    /// The fork edges, painted behind the nodes as thin connectors between card
    /// centers — mirrors the equity chart's `canvas` + `PathBuilder` painter.
    fn render_edges(&self, cx: &mut Context<Self>) -> AnyElement {
        let color = cx.theme().colors().border;
        let segments: Vec<(f32, f32, f32, f32)> = self
            .view
            .edges
            .iter()
            .filter_map(|edge| {
                let a = center_of(&self.view, &edge.from)?;
                let b = center_of(&self.view, &edge.to)?;
                Some((a.x, a.y, b.x, b.y))
            })
            .collect();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let ox = bounds.origin.x;
                let oy = bounds.origin.y;
                for (ax, ay, bx, by) in &segments {
                    let mut line = PathBuilder::stroke(px(1.5));
                    line.move_to(point(ox + px(*ax), oy + px(*ay)));
                    line.line_to(point(ox + px(*bx), oy + px(*by)));
                    if let Ok(path) = line.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    fn render_node(&self, node: &FlowNode, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected.contains(&node.id);
        let running = self.running.as_deref() == Some(node.id.as_str());
        let is_draft = node.kind == NodeKind::Draft;
        let has_path = node.path.is_some();
        let tag = if is_draft {
            "fork"
        } else if node.bundled {
            "example"
        } else {
            "strategy"
        };

        let drag_id = node.id.clone();
        let run_id = node.id.clone();
        let fork_id = node.id.clone();
        let cmp_id = node.id.clone();

        let metrics: AnyElement = match &node.summary {
            Some(summary) if !summary.is_empty() => h_flex()
                .gap_3()
                .flex_wrap()
                .child(mini_metric("Return", fmt_pct(summary.total_return)))
                .child(mini_metric("Sharpe", fmt_ratio(summary.sharpe)))
                .child(mini_metric("Max DD", fmt_pct(summary.max_drawdown)))
                .into_any_element(),
            Some(_) => Label::new("No statistics returned")
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element(),
            None => Label::new("Not run yet")
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element(),
        };

        let border_color = if selected {
            cx.theme().colors().text_accent
        } else {
            cx.theme().colors().border_variant
        };

        v_flex()
            .absolute()
            .left(px(node.pos.x))
            .top(px(node.pos.y))
            .w(px(NODE_W))
            .h(px(NODE_H))
            .rounded_md()
            .border_1()
            .border_color(border_color)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                // Drag handle: grab the header to reposition the node.
                h_flex()
                    .id(SharedString::from(format!("flow-head-{}", node.id)))
                    .px_2()
                    .py_1()
                    .gap_1()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().colors().border_variant)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.begin_drag(drag_id.clone(), event.position, cx);
                        }),
                    )
                    .child(
                        Label::new(node.name.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
                    .child(Label::new(tag).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_2()
                    .gap_2()
                    .justify_between()
                    .child(metrics)
                    .child(
                        h_flex()
                            .gap_1()
                            .when(has_path, |this| {
                                this.child(
                                    Button::new(
                                        SharedString::from(format!("flow-run-{}", node.id)),
                                        if running { "Running…" } else { "Run" },
                                    )
                                    .label_size(LabelSize::XSmall)
                                    .size(ButtonSize::Compact)
                                    .disabled(running)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.run_node(run_id.clone(), cx),
                                    )),
                                )
                            })
                            .when(has_path && !is_draft, |this| {
                                this.child(
                                    Button::new(
                                        SharedString::from(format!("flow-fork-{}", node.id)),
                                        "Fork",
                                    )
                                    .label_size(LabelSize::XSmall)
                                    .size(ButtonSize::Compact)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.fork_node(fork_id.clone(), window, cx)
                                        },
                                    )),
                                )
                            })
                            .child(
                                Button::new(
                                    SharedString::from(format!("flow-cmp-{}", node.id)),
                                    "Compare",
                                )
                                .label_size(LabelSize::XSmall)
                                .size(ButtonSize::Compact)
                                .toggle_state(selected)
                                .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.toggle_compare(cmp_id.clone(), cx),
                                )),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The two-run comparison strip, shown only when exactly two nodes are
    /// selected. A row's delta appears only when both runs have that metric.
    fn render_compare_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selected.len() != 2 {
            return None;
        }
        let a = self.view.nodes.iter().find(|n| n.id == self.selected[0])?;
        let b = self.view.nodes.iter().find(|n| n.id == self.selected[1])?;

        let header = h_flex()
            .gap_2()
            .items_center()
            .child(Label::new(a.name.clone()).size(LabelSize::Small))
            .child(Label::new("vs").size(LabelSize::XSmall).color(Color::Muted))
            .child(Label::new(b.name.clone()).size(LabelSize::Small));

        let body: AnyElement = match (&a.summary, &b.summary) {
            (Some(sa), Some(sb)) => v_flex()
                .gap_1()
                .children(compare_metrics(sa, sb).into_iter().map(render_compare_row))
                .into_any_element(),
            _ => Label::new("Run both nodes to compare their metrics.")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        };

        Some(
            v_flex()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }
}

/// A small "value over label" cell for a node card.
fn mini_metric(label: &'static str, value: String) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(Label::new(value).size(LabelSize::Small))
        .child(
            Label::new(label)
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
}

/// One row of the compare strip: metric label, each side's value, and the delta
/// tinted by whether it's an improvement (muted when there's no honest delta).
fn render_compare_row(row: CompareRow) -> impl IntoElement {
    let delta_color = match row.improved() {
        Some(true) => Color::Success,
        Some(false) => Color::Error,
        None => Color::Muted,
    };
    let delta_text = match row.delta {
        Some(_) => format!("Δ {}", fmt_metric(row.label, row.delta)),
        None => "—".to_string(),
    };
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div().w(px(96.)).child(
                Label::new(row.label)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            ),
        )
        .child(
            div()
                .w(px(72.))
                .child(Label::new(fmt_metric(row.label, row.a)).size(LabelSize::Small)),
        )
        .child(
            div()
                .w(px(72.))
                .child(Label::new(fmt_metric(row.label, row.b)).size(LabelSize::Small)),
        )
        .child(
            Label::new(delta_text)
                .size(LabelSize::Small)
                .color(delta_color),
        )
}

/// Format a metric value the way its label expects, so the compare strip reads
/// the same as the results tab. `None` becomes an em dash via the shared
/// formatters — never a fabricated figure.
fn fmt_metric(label: &str, value: Option<f64>) -> String {
    match label {
        "Net profit" => fmt_money(value),
        "Return" | "Win rate" | "Max drawdown" => fmt_pct(value),
        "Sharpe" => fmt_ratio(value),
        "Turnover" => fmt_turns(value),
        "Trades" => fmt_count(value.map(|v| v as u64)),
        _ => value
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "—".to_string()),
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<StrategyListItem>),
}

async fn fetch_strategies(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<StrategyListItem>> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/backtest/strategies"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let raw: Vec<(String, String, bool)> = value
            .get("strategies")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(|it| {
                        (
                            it.get("path")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("doc")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("bundled").and_then(|v| v.as_bool()).unwrap_or(false),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(strategy_rows(
            raw.iter().map(|(p, d, b)| (p.as_str(), d.as_str(), *b)),
        ))
    }
    .await;
    match attempt {
        Ok(items) => FetchResult::Ok(items),
        Err(_) => FetchResult::Unreachable,
    }
}

impl EventEmitter<PanelEvent> for FlowPanel {}

impl Focusable for FlowPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FlowPanel {
    fn persistent_name() -> &'static str {
        "FlowPanel"
    }

    fn panel_key() -> &'static str {
        "FlowPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right | DockPosition::Bottom)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(720.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::GitGraph)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Flow — fork & compare")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        13
    }
}

impl Render for FlowPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("FLOW")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("· fork & compare")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        let body: AnyElement = match self.status {
            Status::NotConnected => v_flex()
                .p_3()
                .gap_2()
                .child(Label::new("Not connected to your Auracle engine yet."))
                .child(
                    Button::new("flow-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element(),
            Status::Loading => v_flex()
                .p_3()
                .child(
                    Label::new("Loading strategies…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Unreachable => v_flex()
                .p_3()
                .gap_2()
                .child(
                    Label::new("Your engine didn't answer. It may be stopped.").color(Color::Muted),
                )
                .child(
                    Button::new("flow-retry", "Retry")
                        .style(ButtonStyle::Outlined)
                        .label_size(LabelSize::Small)
                        .size(ButtonSize::Compact)
                        .on_click(cx.listener(|this, _, _, cx| this.refetch(cx))),
                )
                .into_any_element(),
            Status::Connected => self.render_canvas(cx),
        };

        v_flex()
            .key_context("FlowPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .children(self.note.clone().map(|note| {
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted))
            }))
            .child(div().flex_1().min_h_0().child(body))
            .children(self.render_compare_strip(cx))
    }
}
