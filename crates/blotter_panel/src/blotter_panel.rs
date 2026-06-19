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
    /// The broker's live order id (IBKR int, ClearStreet UUID), echoed
    /// back to `POST /ui/api/orders/cancel/{order_id}`. The cancel route
    /// addresses the broker's open-orders book, NOT our DB row id, so a
    /// per-row Cancel is only honest when the feed carries this id. The
    /// activity feed omits it today, so per-row Cancel stays hidden until
    /// the engine adds `broker_order_id` to `/ui/api/orders` — the header
    /// "Cancel all" (which is id-agnostic) covers cancel in the meantime.
    broker_order_id: Option<SharedString>,
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
    /// Holds the in-flight cancel POST so it isn't dropped (and cancelled)
    /// before it finishes. Mirrors `auracle_connections::BrokerWizard._task`.
    _action: Option<Task<()>>,
    /// A short plain message shown when a cancel POST fails, cleared on the
    /// next successful refetch so it never lingers as stale.
    last_error: Option<SharedString>,
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
                    _action: None,
                    last_error: None,
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
                                this.last_error = None;
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

    /// Whether an order is still working at the broker and therefore
    /// cancellable. A positive allowlist of the engine's in-flight statuses
    /// (`pending` / `sent` / `partially_filled`, plus the broker-snapshot
    /// words `working` / `open` / `submitted` / `routed`). Notably `dry_run`
    /// is NOT cancellable — it's a preview that never reached a broker — and
    /// the terminal statuses (`executed` / `filled` / `cancelled` /
    /// `rejected` / `failed`) are done.
    fn is_cancellable(status: &str) -> bool {
        matches!(
            status.to_lowercase().as_str(),
            "pending"
                | "sent"
                | "partially_filled"
                | "working"
                | "open"
                | "submitted"
                | "routed"
        )
    }

    fn cancel_order(&mut self, order_id: SharedString, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._action = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            // Refetch regardless of outcome — a failed cancel leaves the row
            // working (the next poll/refetch shows that truth honestly); a
            // successful one flips it to cancelled.
            if let Err(error) =
                post_mutation(http.clone(), &format!("/ui/api/orders/cancel/{order_id}")).await
            {
                this.update(cx, |this, cx| {
                    this.last_error = Some(SharedString::from(format!("Couldn't cancel: {error}.")));
                    cx.notify();
                })
                .ok();
            }
            let fetched = fetch_orders(http).await;
            this.update(cx, |this, cx| {
                if let FetchResult::Ok(items) = fetched {
                    this.status = Status::Connected;
                    this.orders = items;
                    this.last_error = None;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_all(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._action = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if let Err(error) = post_mutation(http.clone(), "/ui/api/orders/cancel-all").await {
                this.update(cx, |this, cx| {
                    this.last_error =
                        Some(SharedString::from(format!("Couldn't cancel all: {error}.")));
                    cx.notify();
                })
                .ok();
            }
            let fetched = fetch_orders(http).await;
            this.update(cx, |this, cx| {
                if let FetchResult::Ok(items) = fetched {
                    this.status = Status::Connected;
                    this.orders = items;
                    this.last_error = None;
                }
                cx.notify();
            })
            .ok();
        }));
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
                // `broker_order_id` is the only id the cancel route accepts
                // (it addresses the broker's open-orders book, not our DB
                // row id). Accept a string or a numeric id; absent/empty
                // means this row can't be cancelled one-off from here.
                let broker_order_id = it
                    .get("broker_order_id")
                    .and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .filter(|s| !s.trim().is_empty())
                    .map(SharedString::from);
                out.push(Order {
                    status: SharedString::from(
                        it.get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    plain: SharedString::from(plain.to_string()),
                    broker_order_id,
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

/// Fetch the double-submit CSRF token: GET `/ui/api/status` so the engine
/// issues an `auracle_csrf` cookie, then return its value to echo back as
/// the `X-CSRF-Token` header on the mutation. Mirrors
/// `auracle_connections::fetch_csrf` — we hit `/ui/api/status` (not an HTML
/// page) so the cookie still flows under the headless web profile.
async fn fetch_csrf(http: Arc<dyn http_client::HttpClient>, base_url: &str, key: &str) -> String {
    let Ok(request) = http_client::http::Request::builder()
        .uri(format!("{base_url}/ui/api/status"))
        .header("X-API-Key", key)
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())
    else {
        return String::new();
    };
    let Ok(response) = http.send(request).await else {
        return String::new();
    };
    for value in response.headers().get_all("set-cookie") {
        let Ok(cookie) = value.to_str() else { continue };
        if let Some(rest) = cookie.strip_prefix("auracle_csrf=") {
            return rest.split(';').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// POST a `/ui/api` mutation with the dual auth headers + the double-submit
/// CSRF token, over loopback. Mirrors `auracle_connections::post_json`; the
/// cancel routes take an empty body. Returns the result so the caller can
/// react — never logs the request (the session key in the headers must not
/// reach the logs).
async fn post_mutation(http: Arc<dyn http_client::HttpClient>, path: &str) -> Result<()> {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        anyhow::bail!("not connected");
    };
    let base_url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let csrf = fetch_csrf(http.clone(), &base_url, &key).await;
    let request = http_client::http::Request::builder()
        .method("POST")
        .uri(format!("{base_url}{path}"))
        .header("Content-Type", "application/json")
        .header("X-API-Key", key.clone())
        .header("X-CSRF-Token", csrf.clone())
        .header(
            "Cookie",
            format!("auracle_session={key}; auracle_csrf={csrf}"),
        )
        .body(http_client::AsyncBody::default())?;
    let response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("status {}", response.status());
    }
    Ok(())
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
        // "Cancel all" is a kill-switch over the broker's whole open book, so
        // it's offered whenever connected with at least one working order —
        // it needs no per-row broker id (unlike single-order cancel).
        let has_cancellable = self.status == Status::Connected
            && self
                .orders
                .iter()
                .any(|order| Self::is_cancellable(&order.status));

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
                // Provenance, so a novice never mistakes this for a
                // live broker feed: it's Auracle's own order records.
                Label::new("· Monitor · from your records")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .when(has_cancellable, |row| {
                row.child(div().flex_1()).child(
                    Button::new("blotter-cancel-all", "Cancel all")
                        .style(ButtonStyle::Outlined)
                        .label_size(LabelSize::XSmall)
                        .size(ButtonSize::Compact)
                        .color(Color::Error)
                        .tooltip(ui::Tooltip::text(
                            "Cancel every working order on the active broker",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_all(cx))),
                )
            });

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
                    // A per-row Cancel is only honest when the row is still
                    // working AND carries the broker order id the cancel
                    // route addresses (the activity feed exposes the DB row
                    // id, which the route would NOT match). Until the engine
                    // adds `broker_order_id` to /ui/api/orders, this stays
                    // hidden and the header "Cancel all" carries cancel.
                    let cancel_target = order
                        .broker_order_id
                        .clone()
                        .filter(|_| Self::is_cancellable(&order.status));
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_start()
                        .rounded_sm()
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
                        .when_some(cancel_target, |row, order_id| {
                            row.child(div().flex_1()).child(
                                Button::new(
                                    SharedString::from(format!("cancel-{order_id}")),
                                    "Cancel",
                                )
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::XSmall)
                                .color(Color::Error)
                                .tooltip(ui::Tooltip::text("Cancel this working order"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_order(order_id.clone(), cx);
                                })),
                            )
                        })
                }))
                .into_any_element(),
        };

        v_flex()
            .key_context("BlotterPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .when_some(self.last_error.clone(), |this, error| {
                this.child(
                    div().px_2().py_1().child(
                        Label::new(error)
                            .size(LabelSize::XSmall)
                            .color(Color::Error),
                    ),
                )
            })
            .child(body)
    }
}
