//! Live Algorithms — what's deployed, and what each strategy is buying.
//!
//! A dock panel that polls the engine's Live Deploy API
//! (`/ui/api/deployments`) and shows each live/paper deployment as a row:
//! status dot + name + mode + AUM + return. Selecting a row loads that
//! deployment's per-strategy ledger (`/ui/api/deployments/{id}/orders`) —
//! the orders it placed and the positions it holds — so "which strategy is
//! buying what" has a concrete answer. Lifecycle buttons (Stop / Liquidate /
//! Restart) are offered only where the engine state machine allows them
//! (via [`auracle_live::available_actions`]), so the UI never offers an
//! action the API would 409.
//!
//! All deploy/ledger RULES live in the gpui-free [`auracle_live`] crate; this
//! file is the render + the engine I/O.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_connect::post_mutation;
use auracle_live::{
    Action, DeploymentOrders, LiveAlgorithms, available_actions, format_return, is_active,
    state_label, verb_endpoint,
};
use auracle_panel_common::{
    PanelStatus as Status, PlaceholderLabels, panel_header, placeholder_body,
};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Pixels,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    auracle_live_ui,
    [
        /// Toggle focus on the Live Algorithms panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(20);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<LiveAlgorithmsPanel>(window, cx);
        });
    })
    .detach();
}

pub struct LiveAlgorithmsPanel {
    focus_handle: FocusHandle,
    algos: LiveAlgorithms,
    /// The selected deployment's ledger (orders + positions), or None until a
    /// row is selected / its fetch lands.
    ledger: Option<DeploymentOrders>,
    status: Status,
    _poll: Task<()>,
    /// Holds an in-flight lifecycle POST or ledger GET so it isn't dropped
    /// (and cancelled) before it finishes.
    _action: Option<Task<()>>,
    last_error: Option<SharedString>,
}

impl LiveAlgorithmsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.status = Status::Loading;
                    this.algos.set_rows(Vec::new());
                    this.ledger = None;
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    algos: LiveAlgorithms::default(),
                    ledger: None,
                    status: Status::initial(),
                    _poll: poll,
                    _action: None,
                    last_error: None,
                }
            })
        })
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        auracle_panel_common::spawn_poll(
            cx,
            POLL_EVERY,
            move || fetch_deployments(http.clone()),
            |this, fetched, cx| match fetched {
                FetchResult::NotConnected => {
                    this.status = Status::NotConnected;
                    this.algos.set_rows(Vec::new());
                    this.ledger = None;
                }
                FetchResult::Unreachable => {
                    this.status = Status::Unreachable;
                }
                FetchResult::Ok(items) => {
                    this.status = Status::Connected;
                    this.algos.set_rows(items);
                    this.last_error = None;
                    // Keep the selected deployment's ledger fresh.
                    this.refresh_ledger(cx);
                }
            },
        )
    }

    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        self.algos.select(id);
        self.ledger = None;
        self.refresh_ledger(cx);
        cx.notify();
    }

    fn refresh_ledger(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.algos.selected else {
            return;
        };
        let http = cx.http_client();
        self._action = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let ledger = fetch_ledger(http, id).await;
            this.update(cx, |this, cx| {
                // Only apply if the selection hasn't moved on under us.
                if this.algos.selected == Some(id) {
                    this.ledger = ledger;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn dispatch_action(&mut self, id: i64, action: Action, cx: &mut Context<Self>) {
        let http = cx.http_client();
        let path = verb_endpoint(id, action);
        self._action = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if let Err(error) = post_mutation(http.clone(), &path).await {
                this.update(cx, |this, cx| {
                    this.last_error = Some(SharedString::from(format!(
                        "Couldn't {}: {error}.",
                        action.verb()
                    )));
                    cx.notify();
                })
                .ok();
            }
            // Refetch so the row reflects the new state honestly either way.
            let fetched = fetch_deployments(http).await;
            this.update(cx, |this, cx| {
                if let FetchResult::Ok(items) = fetched {
                    this.status = Status::Connected;
                    this.algos.set_rows(items);
                    this.last_error = None;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn state_dot(&self, state: &str, cx: &App) -> Hsla {
        let colors = cx.theme().status();
        match state {
            "running" => colors.success,
            "errored" => colors.error,
            "stopped" | "archived" | "liquidating" => colors.ignored,
            _ => colors.info, // preparing / starting / restarting
        }
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<auracle_live::Deployment>),
}

async fn engine_get(http: Arc<dyn http_client::HttpClient>, path: &str) -> Result<serde_json::Value> {
    let config = auracle_connect::load_config();
    let key = config
        .api_key
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("not connected"))?;
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let request = http_client::http::Request::builder()
        .uri(format!("{url}{path}"))
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("status {}", response.status());
    }
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    Ok(serde_json::from_str(&body)?)
}

async fn fetch_deployments(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    if config.api_key.map(|k| k.trim().is_empty()).unwrap_or(true) {
        return FetchResult::NotConnected;
    }
    match engine_get(http, "/ui/api/deployments").await {
        Ok(value) => match serde_json::from_value::<Vec<auracle_live::Deployment>>(value) {
            Ok(items) => FetchResult::Ok(items),
            Err(_) => FetchResult::Unreachable,
        },
        Err(_) => FetchResult::Unreachable,
    }
}

async fn fetch_ledger(http: Arc<dyn http_client::HttpClient>, id: i64) -> Option<DeploymentOrders> {
    let value = engine_get(http, &format!("/ui/api/deployments/{id}/orders"))
        .await
        .ok()?;
    serde_json::from_value::<DeploymentOrders>(value).ok()
}

impl EventEmitter<PanelEvent> for LiveAlgorithmsPanel {}

impl Focusable for LiveAlgorithmsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for LiveAlgorithmsPanel {
    fn persistent_name() -> &'static str {
        "LiveAlgorithmsPanel"
    }

    fn panel_key() -> &'static str {
        "LiveAlgorithmsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(440.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayOutlined)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Live Algorithms — what's deployed + each strategy's ledger")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        12
    }
}

impl LiveAlgorithmsPanel {
    fn render_row(
        &self,
        deployment: &auracle_live::Deployment,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = deployment.id;
        let selected = self.algos.selected == Some(id);
        let dot = self.state_dot(&deployment.state, cx);
        let return_text = format_return(deployment.return_pct);
        let return_color = match deployment.return_pct {
            Some(p) if p > 0.0 => Color::Success,
            Some(p) if p < 0.0 => Color::Error,
            _ => Color::Muted,
        };
        let mode_aum = match deployment.aum {
            Some(a) => format!("{} · ${:.0}", deployment.mode, a),
            None => deployment.mode.clone(),
        };
        let selected_bg = cx.theme().colors().element_selected;

        let actions_row = available_actions(&deployment.state).into_iter().map(move |action| {
            Button::new(
                SharedString::from(format!("live-{}-{}", id, action.verb())),
                action.label(),
            )
            .style(ButtonStyle::Subtle)
            .label_size(LabelSize::XSmall)
            .size(ButtonSize::Compact)
            .when(action.is_destructive(), |b| b.color(Color::Error))
            .on_click(cx.listener(move |this, _, _, cx| this.dispatch_action(id, action, cx)))
        });

        v_flex()
            .id(SharedString::from(format!("live-row-{id}")))
            .w_full()
            .px_2()
            .py_1p5()
            .gap_1()
            .rounded_md()
            .when(selected, |row| row.bg(selected_bg))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(div().size_1p5().rounded_full().flex_none().bg(dot))
                    .child(
                        Label::new(if deployment.name.is_empty() {
                            SharedString::from(format!("Deployment {id}"))
                        } else {
                            SharedString::from(deployment.name.clone())
                        })
                        .size(LabelSize::Small),
                    )
                    .child(div().flex_1())
                    .child(
                        Label::new(SharedString::from(state_label(&deployment.state).to_string()))
                            .size(LabelSize::XSmall)
                            .color(if is_active(&deployment.state) {
                                Color::Default
                            } else {
                                Color::Muted
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        Label::new(SharedString::from(mode_aum))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .child(Label::new(return_text).size(LabelSize::XSmall).color(return_color)),
            )
            .child(h_flex().w_full().gap_1().children(actions_row))
    }

    fn render_ledger(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(deployment) = self.algos.selected_deployment() else {
            return div().into_any_element();
        };
        let muted = Color::Muted;
        let header = h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .child(Label::new("Ledger").size(LabelSize::Small))
            .child(
                Label::new(SharedString::from(format!("· {}", deployment.name)))
                    .size(LabelSize::XSmall)
                    .color(muted),
            );

        let positions = self
            .ledger
            .as_ref()
            .map(|l| l.positions.clone())
            .unwrap_or_default();
        let orders = self
            .ledger
            .as_ref()
            .map(|l| l.orders.clone())
            .unwrap_or_default();

        let position_rows = positions.into_iter().map(|p| {
            h_flex()
                .w_full()
                .px_2()
                .gap_2()
                .child(Label::new(SharedString::from(p.symbol)).size(LabelSize::XSmall))
                .child(div().flex_1())
                .child(
                    Label::new(SharedString::from(format!(
                        "{:.0} @ ${:.2}",
                        p.quantity, p.avg_cost
                    )))
                    .size(LabelSize::XSmall)
                    .color(muted),
                )
        });

        let order_rows = orders.into_iter().map(|o| {
            let filled = o.filled_quantity.unwrap_or(0.0);
            let qty = o.quantity.unwrap_or(0.0);
            let detail = format!(
                "{} {:.0}/{:.0} {} · {}",
                o.action, filled, qty, o.symbol, o.status
            );
            h_flex()
                .w_full()
                .px_2()
                .gap_2()
                .child(Label::new(SharedString::from(detail)).size(LabelSize::XSmall))
        });

        v_flex()
            .w_full()
            .gap_0p5()
            .child(header)
            .when(self.ledger.is_none(), |this| {
                this.child(
                    div().px_2().py_1().child(
                        Label::new("Loading ledger…")
                            .size(LabelSize::XSmall)
                            .color(muted),
                    ),
                )
            })
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        Label::new("Positions")
                            .size(LabelSize::XSmall)
                            .color(muted)
                            .mx_2(),
                    )
                    .children(position_rows),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .pt_1()
                    .child(
                        Label::new("Orders")
                            .size(LabelSize::XSmall)
                            .color(muted)
                            .mx_2(),
                    )
                    .children(order_rows),
            )
            .into_any_element()
    }
}

impl Render for LiveAlgorithmsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.algos.active_count();
        let header = panel_header("LIVE ALGORITHMS", cx).child(
            Label::new(SharedString::from(format!("· {active} live")))
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        );

        let labels = PlaceholderLabels::new(
            "live-connect",
            "Checking…",
            "No deployments yet. Deploy a strategy and it shows up here, live.",
        );
        let rows: Vec<_> = self.algos.rows.clone();
        let body: AnyElement = match placeholder_body(&self.status, rows.is_empty(), &labels) {
            Some(placeholder) => placeholder,
            None => v_flex()
                .id("live-scroll")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_1()
                .children(rows.iter().map(|d| self.render_row(d, cx)))
                .child(self.render_ledger(cx))
                .into_any_element(),
        };

        v_flex()
            .key_context("LiveAlgorithmsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .when_some(self.last_error.clone(), |this, error| {
                this.child(
                    div().px_2().py_1().child(
                        Label::new(error).size(LabelSize::XSmall).color(Color::Error),
                    ),
                )
            })
            .child(body)
    }
}
