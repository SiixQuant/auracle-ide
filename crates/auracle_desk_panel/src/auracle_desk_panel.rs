//! Desk — the account + deployments overview, in one glance.
//!
//! Polls the engine's `/ui/api/overview` snapshot and renders the account
//! metrics (equity, P&L, buying power, positions) and one row per deployed
//! strategy. Honest by construction via the gpui-free `auracle_desk` reducer: a
//! value the engine actually reported (including a real 0) shows truthfully; a
//! missing one reads "Unknown", never a fabricated number. The panel renders
//! truth; it never invents a figure.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_desk::{DeskInput, DeskView, StrategyInput, Tone, build_desk};
use auracle_view_state::{Load, ViewState};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Task,
    WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    auracle_desk_panel,
    [
        /// Toggle focus on the Desk panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(30);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<DeskPanel>(window, cx);
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

pub struct DeskPanel {
    focus_handle: FocusHandle,
    view: DeskView,
    status: Status,
    _poll: Task<()>,
}

impl DeskPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
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
                    view: build_desk(DeskInput::default()),
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
                let fetched = fetch_overview(http.clone()).await;
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

    /// Apply a fetch outcome to panel state. Shared by the poll and the manual
    /// Retry so both reach the same state from the same outcome.
    fn apply_fetch(&mut self, fetched: FetchResult) {
        match fetched {
            FetchResult::NotConnected => self.status = Status::NotConnected,
            FetchResult::Unreachable => self.status = Status::Unreachable,
            FetchResult::Ok(view) => {
                self.status = Status::Connected;
                self.view = view;
            }
        }
    }

    /// Fetch once now, off the 30s cadence — the Retry affordance on the
    /// unreachable error state.
    fn refetch(&mut self, cx: &mut Context<Self>) {
        self.status = Status::Loading;
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let fetched = fetch_overview(http).await;
            this.update(cx, |this, cx| {
                this.apply_fetch(fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_desk(&self, view: &DeskView, cx: &App) -> AnyElement {
        let metric_row = |row: &auracle_desk::MetricRow| {
            h_flex()
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .child(
                    Label::new(row.label.to_string())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(div().flex_1())
                .child(
                    Label::new(row.value.clone())
                        .size(LabelSize::Small)
                        .color(tone_color(row.tone))
                        .buffer_font(cx),
                )
        };

        v_flex()
            .id("desk-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_1()
            .child(section_label("ACCOUNT"))
            .children(view.metrics.iter().map(metric_row))
            .when(!view.strategies.is_empty(), |this| {
                this.child(section_label("STRATEGIES"))
                    .children(view.strategies.iter().map(|strategy| {
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .items_center()
                            .child(Label::new(strategy.name.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(strategy.status.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(div().flex_1())
                            .child(
                                Label::new(strategy.pnl.value.clone())
                                    .size(LabelSize::Small)
                                    .color(tone_color(strategy.pnl.tone))
                                    .buffer_font(cx),
                            )
                    }))
            })
            .into_any_element()
    }
}

/// Maps a reducer tone to a theme colour, so the render path holds no colour
/// literals. Unknown is muted (an honest "—"), never a confident green/red.
fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Positive => Color::Success,
        Tone::Negative => Color::Error,
        Tone::Neutral => Color::Default,
        Tone::Unknown => Color::Muted,
    }
}

fn section_label(text: &'static str) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
}

fn render_skeleton() -> AnyElement {
    let skeleton_row = || {
        h_flex().px_2().py_1().child(
            Label::new("Loading…")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
    };
    v_flex()
        .p_1()
        .gap_0p5()
        .child(skeleton_row())
        .child(skeleton_row())
        .child(skeleton_row())
        .into_any_element()
}

fn render_error(message: &str, retryable: bool, cx: &mut Context<DeskPanel>) -> AnyElement {
    v_flex()
        .p_3()
        .gap_2()
        .child(Label::new(message.to_string()).color(Color::Muted))
        .when(retryable, |this| {
            this.child(
                Button::new("desk-retry", "Retry")
                    .style(ButtonStyle::Outlined)
                    .label_size(LabelSize::XSmall)
                    .size(ButtonSize::Compact)
                    .start_icon(
                        Icon::new(IconName::RotateCcw)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.refetch(cx))),
            )
        })
        .into_any_element()
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(DeskView),
}

async fn fetch_overview(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<DeskView> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/overview"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        Ok(build_desk(parse_overview(&value)))
    }
    .await;
    match attempt {
        Ok(view) => FetchResult::Ok(view),
        Err(_) => FetchResult::Unreachable,
    }
}

/// Turn the `/ui/api/overview` JSON into the reducer's input. Absent / non-numeric
/// fields stay `None` so the reducer renders them "Unknown" — never a fabricated 0.
fn parse_overview(value: &serde_json::Value) -> DeskInput {
    let number = |key: &str| value.get(key).and_then(|v| v.as_f64());
    let positions = value
        .get("positions")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let strategies = value
        .get("strategies")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let name = row.get("name").and_then(|v| v.as_str())?.to_string();
                    Some(StrategyInput {
                        name,
                        status: row
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        pnl: row.get("pnl").and_then(|v| v.as_f64()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    DeskInput {
        equity: number("equity"),
        open_pnl: number("open_pnl"),
        day_pnl: number("day_pnl"),
        buying_power: number("buying_power"),
        positions,
        strategies,
    }
}

impl EventEmitter<PanelEvent> for DeskPanel {}

impl Focusable for DeskPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for DeskPanel {
    fn persistent_name() -> &'static str {
        "DeskPanel"
    }

    fn panel_key() -> &'static str {
        "DeskPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right)
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
        Some(IconName::Sliders)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Desk — account overview")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        14
    }
}

impl Render for DeskPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("DESK")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("· Monitor · from your records")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        let body: AnyElement = if self.status == Status::NotConnected {
            v_flex()
                .p_3()
                .gap_2()
                .child(Label::new("Not connected to your Auracle engine yet."))
                .child(
                    Button::new("desk-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element()
        } else {
            let load = match self.status {
                Status::Loading => Load::Pending,
                Status::Unreachable => {
                    Load::Failed("Your engine didn't answer. It may be stopped.".to_string())
                }
                _ => Load::Done(self.view.clone()),
            };
            // The desk always has account rows to show once connected (a missing
            // figure reads "Unknown"), so a connected fetch is never "empty".
            match load.into_view(|_| false, "") {
                ViewState::Loading => render_skeleton(),
                ViewState::Empty { .. } => render_skeleton(),
                ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
                ViewState::Ready(view) => self.render_desk(&view, cx),
            }
        };

        v_flex()
            .key_context("DeskPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}
