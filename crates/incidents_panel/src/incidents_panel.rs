//! Incidents panel — what needs your attention, in plain words.
//!
//! Polls the engine's incident feed (`/ui/api/incidents`) and renders
//! each as an incident card: a severity dot, the engine's plain cause
//! sentence, and the technical detail behind a "show details" toggle.
//! The panel renders truth — it never invents an incident.
//!
//! v0 is read-only: dismissal needs a CSRF-free engine path for
//! first-party clients (tracked as a ledger follow-up), so this run
//! deliberately ships no Dismiss button rather than one that would be
//! refused. Reading and triaging by plain cause is the whole v0 value.

use std::collections::HashSet;
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
    incidents_panel,
    [
        /// Toggle focus on the incidents panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<IncidentsPanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone)]
struct Incident {
    row_id: SharedString, // "<kind>:<id>" — stable across polls
    severity: SharedString,
    cause: SharedString,
    detail: SharedString,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

pub struct IncidentsPanel {
    focus_handle: FocusHandle,
    incidents: Vec<Incident>,
    expanded: HashSet<SharedString>,
    status: Status,
    _poll: Task<()>,
}

impl IncidentsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(
                    |this: &mut Self, cx| {
                        this.status = Status::Loading;
                        this.incidents.clear();
                        cx.notify();
                    },
                )
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    incidents: Vec::new(),
                    expanded: HashSet::new(),
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
                let fetched = fetch_incidents(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            FetchResult::NotConnected => {
                                this.status = Status::NotConnected;
                                this.incidents.clear();
                            }
                            FetchResult::Unreachable => {
                                this.status = Status::Unreachable;
                            }
                            FetchResult::Ok(items) => {
                                this.status = Status::Connected;
                                this.incidents = items;
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

    fn severity_color(&self, severity: &str, cx: &App) -> Hsla {
        let status = cx.theme().status();
        match severity {
            "error" => status.error,
            "warning" => status.warning,
            _ => status.info,
        }
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<Incident>),
}

async fn fetch_incidents(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<Incident>> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/incidents"))
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
        if let Some(items) = value.get("incidents").and_then(|v| v.as_array()) {
            for it in items {
                let kind = it.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let id = it.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                let cause = it
                    .get("cause")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if cause.is_empty() {
                    continue;
                }
                out.push(Incident {
                    row_id: SharedString::from(format!("{kind}:{id}")),
                    severity: SharedString::from(
                        it.get("severity")
                            .and_then(|v| v.as_str())
                            .unwrap_or("info")
                            .to_string(),
                    ),
                    cause: SharedString::from(cause.to_string()),
                    detail: SharedString::from(
                        it.get("detail")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
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

impl EventEmitter<PanelEvent> for IncidentsPanel {}

impl Focusable for IncidentsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for IncidentsPanel {
    fn persistent_name() -> &'static str {
        "IncidentsPanel"
    }

    fn panel_key() -> &'static str {
        "IncidentsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right | DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Warning)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Incidents")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        12
    }
}

impl Render for IncidentsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("INCIDENTS")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        let body: AnyElement = match self.status {
            Status::NotConnected => v_flex()
                .p_3()
                .gap_2()
                .child(Label::new(
                    "Not connected to your Auracle engine yet.",
                ))
                .child(
                    Button::new("incidents-connect", "Connect…")
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
                    Label::new(
                        "Your engine didn't answer. It may be stopped.",
                    )
                    .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected if self.incidents.is_empty() => v_flex()
                .p_3()
                .child(
                    Label::new("Nothing needs your attention right now.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected => v_flex()
                .id("incidents-scroll")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_1()
                .children(self.incidents.iter().map(|incident| {
                    let dot = self.severity_color(&incident.severity, cx);
                    let is_open = self.expanded.contains(&incident.row_id);
                    let row_id = incident.row_id.clone();
                    let has_detail = !incident.detail.is_empty();
                    v_flex()
                        .p_2()
                        .gap_1()
                        .rounded_md()
                        .bg(cx.theme().colors().elevated_surface_background)
                        .border_1()
                        .border_color(cx.theme().colors().border)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_start()
                                .child(
                                    div()
                                        .mt_1()
                                        .size_1p5()
                                        .rounded_full()
                                        .flex_none()
                                        .bg(dot),
                                )
                                .child(
                                    Label::new(incident.cause.clone())
                                        .size(LabelSize::Small),
                                ),
                        )
                        .when(has_detail, |card| {
                            let toggle_id: SharedString =
                                format!("toggle-{row_id}").into();
                            card.child(
                                Button::new(
                                    toggle_id,
                                    if is_open {
                                        "hide details"
                                    } else {
                                        "show details"
                                    },
                                )
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::XSmall)
                                .on_click(cx.listener({
                                    let row_id = row_id.clone();
                                    move |this, _, _, cx| {
                                        if !this.expanded.remove(&row_id) {
                                            this.expanded.insert(row_id.clone());
                                        }
                                        cx.notify();
                                    }
                                })),
                            )
                        })
                        .when(is_open && has_detail, |card| {
                            card.child(
                                Label::new(incident.detail.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                        })
                }))
                .into_any_element(),
        };

        v_flex()
            .key_context("IncidentsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}
