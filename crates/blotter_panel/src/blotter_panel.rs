//! Blotter — your order activity, in plain words.
//!
//! Polls the engine's order-activity feed (`/ui/api/orders`, the
//! orders table of record) and shows each as a status dot + the
//! engine's plain sentence ("Order done: buy 10 AAPL (about $1,850)").
//! It reflects what Auracle has tried to do and is honest with no
//! broker reachable — the live broker-state snapshot is a separate
//! concern. The panel renders truth; it never invents an order.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Pixels,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    blotter_panel,
    [
        /// Toggle focus on the blotter panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<BlotterPanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone)]
struct Order {
    status: SharedString,
    plain: SharedString,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

pub struct BlotterPanel {
    focus_handle: FocusHandle,
    orders: Vec<Order>,
    status: Status,
    _poll: Task<()>,
}

impl BlotterPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(
                    |this: &mut Self, cx| {
                        this.status = Status::Loading;
                        this.orders.clear();
                        cx.notify();
                    },
                )
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    orders: Vec::new(),
                    status: if auracle_connect::load_config().api_key.is_some() {
                        Status::Loading
                    } else {
                        Status::NotConnected
                    },
                    _poll: poll,
                }
            })
        })
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_orders(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            FetchResult::NotConnected => {
                                this.status = Status::NotConnected;
                                this.orders.clear();
                            }
                            FetchResult::Unreachable => {
                                this.status = Status::Unreachable;
                            }
                            FetchResult::Ok(items) => {
                                this.status = Status::Connected;
                                this.orders = items;
                            }
                        }
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

    fn status_color(&self, status: &str, cx: &App) -> Hsla {
        let colors = cx.theme().status();
        match status.to_lowercase().as_str() {
            "filled" | "executed" => colors.success,
            "rejected" | "failed" | "error" => colors.error,
            "cancelled" | "canceled" => colors.ignored,
            _ => colors.info, // submitted / pending / routed — on its way
        }
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<Order>),
}

async fn fetch_orders(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<Order>> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/orders"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let mut out = Vec::new();
        if let Some(items) = value.get("orders").and_then(|v| v.as_array()) {
            for it in items {
                let plain = it.get("plain").and_then(|v| v.as_str()).unwrap_or_default();
                if plain.is_empty() {
                    continue;
                }
                out.push(Order {
                    status: SharedString::from(
                        it.get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    plain: SharedString::from(plain.to_string()),
                });
            }
        }
        Ok(out)
    }
    .await;
    match attempt {
        Ok(items) => FetchResult::Ok(items),
        Err(_) => FetchResult::Unreachable,
    }
}

impl EventEmitter<PanelEvent> for BlotterPanel {}

impl Focusable for BlotterPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BlotterPanel {
    fn persistent_name() -> &'static str {
        "BlotterPanel"
    }

    fn panel_key() -> &'static str {
        "BlotterPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(200.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ListTodo)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Blotter — order activity")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        13
    }
}

impl Render for BlotterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("BLOTTER")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("order activity")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        let body: AnyElement = match self.status {
            Status::NotConnected => v_flex()
                .p_3()
                .gap_2()
                .child(Label::new("Not connected to your Auracle engine yet."))
                .child(
                    Button::new("blotter-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                Box::new(auracle_connect::Connect),
                                cx,
                            );
                        }),
                )
                .into_any_element(),
            Status::Loading => v_flex()
                .p_3()
                .child(Label::new("Checking…").color(Color::Muted))
                .into_any_element(),
            Status::Unreachable => v_flex()
                .p_3()
                .child(
                    Label::new("Your engine didn't answer. It may be stopped.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected if self.orders.is_empty() => v_flex()
                .p_3()
                .child(
                    Label::new("No orders yet. Trades show up here once a strategy places them.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected => v_flex()
                .id("blotter-scroll")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_0p5()
                .children(self.orders.iter().map(|order| {
                    let dot = self.status_color(&order.status, cx);
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_start()
                        .rounded_sm()
                        .hover(|s| s.bg(cx.theme().colors().ghost_element_hover))
                        .child(
                            div()
                                .mt_1()
                                .size_1p5()
                                .rounded_full()
                                .flex_none()
                                .bg(dot),
                        )
                        .child(
                            Label::new(order.plain.clone()).size(LabelSize::Small),
                        )
                }))
                .into_any_element(),
        };

        v_flex()
            .key_context("BlotterPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}
