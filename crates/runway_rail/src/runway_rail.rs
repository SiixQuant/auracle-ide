//! The Quant Runway rail — the persistent six-stage spine of the
//! Auracle IDE (Research → Build → Validate → Paper → Go live →
//! Monitor).
//!
//! When connected, the rail renders the ENGINE's runway truth
//! (`/ui/api/runway`): stages with real evidence light up and carry
//! their evidence sentence; stages the engine cannot prove stay
//! locked with the engine's honest "can't tell yet" line. The rail
//! never invents progress. Unconnected installs keep the teaching
//! placeholder.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::Tooltip;
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    runway_rail,
    [
        /// Toggle focus on the runway rail.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(60);

const STAGES: [(&str, &str, &str); 6] = [
    ("research", "Research", "Look at markets, data, and ideas."),
    ("build", "Build", "Shape an idea into a strategy."),
    (
        "validate",
        "Validate",
        "Test it against the past, honestly.",
    ),
    ("paper", "Paper", "Practice with pretend money."),
    (
        "go_live",
        "Go live",
        "Real money — only after every gate is green.",
    ),
    ("monitor", "Monitor", "Watch everything that runs."),
];

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<RunwayRail>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone, Default)]
struct StageTruth {
    reached: SharedString,  // "yes" | "no" | "unknown"
    evidence: SharedString, // the engine's plain sentence
}

#[derive(Clone, Default)]
struct RunwayTruth {
    stages: std::collections::HashMap<String, StageTruth>,
    current: Option<SharedString>,
}

pub struct RunwayRail {
    focus_handle: FocusHandle,
    truth: Option<RunwayTruth>,
    connected: bool,
    _poll: Task<()>,
}

impl RunwayRail {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.truth = None;
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    truth: None,
                    connected: false,
                    _poll: poll,
                }
            })
        })
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        auracle_panel_common::spawn_poll(
            cx,
            POLL_EVERY,
            move || fetch_runway(http.clone()),
            |this, fetched, _cx| match fetched {
                Some(truth) => {
                    this.truth = Some(truth);
                    this.connected = true;
                }
                None => {
                    this.connected = false;
                }
            },
        )
    }
}

async fn fetch_runway(http: Arc<dyn http_client::HttpClient>) -> Option<RunwayTruth> {
    let config = auracle_connect::load_config();
    let key = config.api_key.filter(|k| !k.trim().is_empty())?;
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<RunwayTruth> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/runway"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let mut truth = RunwayTruth {
            current: value
                .get("current")
                .and_then(|v| v.as_str())
                .map(|s| SharedString::from(s.to_string())),
            ..Default::default()
        };
        if let Some(stages) = value.get("stages").and_then(|v| v.as_object()) {
            for (name, stage) in stages {
                truth.stages.insert(
                    name.clone(),
                    StageTruth {
                        reached: SharedString::from(
                            stage
                                .get("reached")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                        ),
                        evidence: SharedString::from(
                            stage
                                .get("evidence")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        ),
                    },
                );
            }
        }
        Ok(truth)
    }
    .await;
    attempt.ok()
}

impl EventEmitter<PanelEvent> for RunwayRail {}

impl Focusable for RunwayRail {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for RunwayRail {
    fn persistent_name() -> &'static str {
        "RunwayRail"
    }

    fn panel_key() -> &'static str {
        "RunwayRail"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(192.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayOutlined)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Runway")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}

impl Render for RunwayRail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let truth = self.truth.clone();
        let connected = self.connected && truth.is_some();

        v_flex()
            .key_context("RunwayRail")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .p_2()
            .gap_1()
            .child(
                h_flex().px_1().pb_1().items_center().gap_2().child(
                    Label::new("RUNWAY")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .children(STAGES.iter().enumerate().map(|(ix, (key, name, hint))| {
                let stage = truth
                    .as_ref()
                    .and_then(|t| t.stages.get(*key))
                    .cloned()
                    .unwrap_or_default();
                let reached = stage.reached.as_ref() == "yes";
                let is_current = truth
                    .as_ref()
                    .and_then(|t| t.current.as_ref())
                    .map(|c| c.as_ref() == *key)
                    .unwrap_or(false);
                let tooltip_text: SharedString = if !stage.evidence.is_empty() {
                    // The engine's own sentence — for reached AND for
                    // honestly-unproven stages (it carries the "not
                    // tracked yet" wording).
                    stage.evidence.clone()
                } else if connected {
                    format!("{hint} Nothing here yet.").into()
                } else {
                    format!(
                        "{hint} This stage lights up after you connect \
                         your Auracle engine."
                    )
                    .into()
                };
                let (icon, icon_color, label_color) = if reached {
                    (
                        IconName::Check,
                        Color::Success,
                        if is_current {
                            Color::Accent
                        } else {
                            Color::Default
                        },
                    )
                } else {
                    (IconName::LockOutlined, Color::Disabled, Color::Disabled)
                };
                // A one-word glance marker so lock states read without a
                // hover: the current rung, a not-tracked-yet stage, and
                // a not-done-yet stage are each distinct — none "broken".
                let (marker, marker_color): (Option<&str>, Color) = if is_current {
                    (Some("now"), Color::Accent)
                } else if reached {
                    (None, Color::Muted)
                } else if stage.reached.as_ref() == "unknown" {
                    (Some("soon"), Color::Muted)
                } else {
                    (Some("to do"), Color::Muted)
                };
                // Informational rows — no background hover highlight (a
                // stage isn't a button yet; its document doesn't exist).
                // The tooltip carries the engine's evidence sentence.
                h_flex()
                    .id(ix)
                    .px_1()
                    .py_0p5()
                    .gap_2()
                    .rounded_sm()
                    .child(Icon::new(icon).size(IconSize::XSmall).color(icon_color))
                    .child(Label::new(*name).color(label_color))
                    .child(div().flex_1())
                    .when_some(marker, |row, m| {
                        row.child(Label::new(m).size(LabelSize::XSmall).color(marker_color))
                    })
                    .tooltip(Tooltip::text(tooltip_text))
            }))
            .child(div().flex_1())
            .when(connected, |rail| {
                rail.child(
                    div().px_1().pb_1().child(
                        Label::new("Live from your engine.")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
                )
            })
            .when(!connected, |rail| {
                // Not connected: give the user the same forward action
                // every other panel offers, so the locked rail never
                // strands a first-time user (council B-14).
                rail.child(
                    v_flex()
                        .px_1()
                        .pb_1()
                        .gap_1()
                        .child(
                            Label::new(
                                "This rail lights up once it can reach \
                                 your Auracle engine.",
                            )
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                        )
                        .child(
                            Button::new("runway-connect", "Connect…")
                                .style(ButtonStyle::Filled)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                                }),
                        ),
                )
            })
    }
}
