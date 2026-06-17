//! Schedules — what's deployed and when it next runs.
//!
//! Polls the engine's schedule feed (`/ui/api/schedules.json`) and shows
//! each deployed strategy with its cron and a live/paused dot. It is a
//! light read-only surface: starting, pausing, and stopping a schedule
//! happen through the agent ("pause the momentum schedule") which calls
//! the engine's mutation tools. The panel renders the engine's truth and
//! never invents a schedule.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    schedules_panel,
    [
        /// Toggle focus on the schedules panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<SchedulesPanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone)]
struct ScheduleRow {
    name: SharedString,
    strategy: SharedString,
    cron: SharedString,
    enabled: bool,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

pub struct SchedulesPanel {
    focus_handle: FocusHandle,
    schedules: Vec<ScheduleRow>,
    status: Status,
    _poll: Task<()>,
}

impl SchedulesPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(
                    |this: &mut Self, cx| {
                        this.status = Status::Loading;
                        this.schedules.clear();
                        cx.notify();
                    },
                )
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    schedules: Vec::new(),
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
                let fetched = fetch_schedules(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            FetchResult::NotConnected => {
                                this.status = Status::NotConnected;
                                this.schedules.clear();
                            }
                            FetchResult::Unreachable => {
                                this.status = Status::Unreachable;
                            }
                            FetchResult::Ok(items) => {
                                this.status = Status::Connected;
                                this.schedules = items;
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
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<ScheduleRow>),
}

async fn fetch_schedules(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<ScheduleRow>> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/schedules.json"))
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
        if let Some(items) = value.get("schedules").and_then(|v| v.as_array()) {
            for it in items {
                let name = it.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                // Show just the class/function tail of the strategy path
                // so the row stays scannable.
                let strategy = it
                    .get("strategy_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let strategy_tail = strategy.rsplit('.').next().unwrap_or(strategy);
                out.push(ScheduleRow {
                    name: SharedString::from(name.to_string()),
                    strategy: SharedString::from(strategy_tail.to_string()),
                    cron: SharedString::from(
                        it.get("cron")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    enabled: it.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
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

impl EventEmitter<PanelEvent> for SchedulesPanel {}

impl Focusable for SchedulesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SchedulesPanel {
    fn persistent_name() -> &'static str {
        "SchedulesPanel"
    }

    fn panel_key() -> &'static str {
        "SchedulesPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
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
        px(260.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Clock)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Schedules")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        7
    }
}

impl Render for SchedulesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("SCHEDULES")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("· what's deployed")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        let body: AnyElement = match self.status {
            Status::NotConnected => v_flex()
                .p_3()
                .gap_2()
                .child(Label::new("Not connected to your Auracle engine yet."))
                .child(
                    Button::new("schedules-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element(),
            Status::Loading => v_flex()
                .p_3()
                .child(Label::new("Loading…").color(Color::Muted))
                .into_any_element(),
            Status::Unreachable => v_flex()
                .p_3()
                .child(
                    Label::new("Your engine didn't answer. It may be stopped.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected if self.schedules.is_empty() => v_flex()
                .p_3()
                .child(
                    Label::new("Nothing deployed yet. Deploy a strategy and it appears here.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected => v_flex()
                .id("schedules-scroll")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_0p5()
                .children(self.schedules.iter().map(|row| {
                    let dot = if row.enabled {
                        cx.theme().status().success
                    } else {
                        cx.theme().status().ignored
                    };
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_start()
                        .rounded_sm()
                        .child(div().mt_1().size_1p5().rounded_full().flex_none().bg(dot))
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(Label::new(row.name.clone()).size(LabelSize::Small))
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .child(
                                            Label::new(row.strategy.clone())
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                        .child(
                                            Label::new(row.cron.clone())
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                        .when(!row.enabled, |s| {
                                            s.child(
                                                Label::new("paused")
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                ),
                        )
                }))
                .into_any_element(),
        };

        v_flex()
            .key_context("SchedulesPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}
