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
use auracle_runway::{StageTone, stage_marker};
use auracle_view_state::Load;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::Tooltip;
use ui::prelude::*;
use util::ResultExt as _;
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

/// The rail's own connection picture, distinct from the runway payload itself.
/// The two top-level cases the engine truth can't express — no API key at all,
/// and a poll still in flight before any answer — must read differently from a
/// reachable engine's `Load`, so they live here rather than being collapsed into
/// "no truth".
enum RailState {
    /// No API key is configured — the install hasn't been connected.
    Disconnected,
    /// A key is present and a runway fetch is in flight or has been attempted;
    /// the inner `Load` is the honest loading / error / ready picture.
    Linked(Load<RunwayTruth>),
}

pub struct RunwayRail {
    focus_handle: FocusHandle,
    state: RailState,
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
                    // A fresh connect attempt resets the rail to a first-poll
                    // Checking state when a key is now present, so the rail
                    // never strands on stale truth from a previous engine.
                    this.state = Self::initial_state();
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    state: Self::initial_state(),
                    _poll: poll,
                }
            })
        })
    }

    /// The rail's state before the first poll returns: Disconnected when there is
    /// no key, otherwise Linked+Pending (Checking) so a key-present install never
    /// shows the "not connected" CTA while the very first poll is still running.
    fn initial_state() -> RailState {
        if auracle_connect::load_config()
            .api_key
            .filter(|key| !key.trim().is_empty())
            .is_some()
        {
            RailState::Linked(Load::Pending)
        } else {
            RailState::Disconnected
        }
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_runway(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        this.state = match fetched {
                            // No key — the install isn't connected.
                            None => RailState::Disconnected,
                            // A key is present; carry through the honest load
                            // outcome (ready truth or a real fetch error).
                            Some(load) => RailState::Linked(load),
                        };
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

/// Poll the engine's runway truth. `None` means there is no API key (the install
/// isn't connected); `Some(Load::Done)` is a fresh truth; `Some(Load::Failed)` is
/// a reachable-but-erroring engine — a state that must read distinctly from "not
/// connected" rather than being silently collapsed to it.
async fn fetch_runway(http: Arc<dyn http_client::HttpClient>) -> Option<Load<RunwayTruth>> {
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
    // Never silently discard a real fetch failure: log it (the error carries no
    // key — the secret only ever lived in the request Cookie header), and surface
    // a distinct, designed error state to the rail instead of "not connected".
    Some(match attempt.log_err() {
        Some(truth) => Load::Done(truth),
        None => Load::Failed("Couldn't read your runway.".to_string()),
    })
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

/// Map a [`StageTone`] to the theme colour the rail renders a stage in. Only
/// theme `Color::*` — never a colour literal — so the rail tracks the theme.
/// Mirrors `account_setup::tone_color`.
fn tone_color(tone: StageTone) -> Color {
    match tone {
        StageTone::Accent => Color::Accent,
        StageTone::Positive => Color::Success,
        StageTone::Muted => Color::Muted,
        StageTone::Disabled => Color::Disabled,
    }
}

impl Render for RunwayRail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The runway truth is only present once a linked engine has answered; in
        // every other top-level state (no key, first poll in flight, fetch error)
        // there is no truth and the stages render Locked.
        let truth: Option<RunwayTruth> = match &self.state {
            RailState::Linked(Load::Done(truth)) => Some(truth.clone()),
            _ => None,
        };
        let has_truth = truth.is_some();

        v_flex()
            .key_context("RunwayRail")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .px_1()
                    .pb_1()
                    .items_center()
                    .gap_2()
                    .child(
                        Label::new("RUNWAY")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .child(
                        // One-click research from the runway — opens the agent
                        // and runs the arXiv "Research ideas" scan (research_scan).
                        Button::new("runway-research", "Research")
                            .style(ButtonStyle::Filled)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    Box::new(auracle_agent_commands::ResearchIdeas),
                                    cx,
                                );
                            }),
                    ),
            )
            .children(STAGES.iter().enumerate().map(|(ix, (key, name, hint))| {
                let stage = truth
                    .as_ref()
                    .and_then(|t| t.stages.get(*key))
                    .cloned()
                    .unwrap_or_default();
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
                } else if has_truth {
                    format!("{hint} Nothing here yet.").into()
                } else {
                    format!(
                        "{hint} This stage lights up after you connect \
                         your Auracle engine."
                    )
                    .into()
                };
                // The entire per-stage glance — icon, tones, and the one-word
                // marker — is decided by the gpui-free reducer, so the render
                // only maps its tones to theme colours. The marker is honest
                // vocabulary only ("now" | "not tracked" | "to do"), never the
                // banned "soon".
                let marker = stage_marker(stage.reached.as_ref(), is_current, has_truth);
                let icon = if marker.reached_icon {
                    IconName::Check
                } else {
                    IconName::LockOutlined
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
                    .child(
                        Icon::new(icon)
                            .size(IconSize::XSmall)
                            .color(tone_color(marker.mark_tone)),
                    )
                    .child(Label::new(*name).color(tone_color(marker.name_tone)))
                    .child(div().flex_1())
                    .when_some(marker.label, |row, label| {
                        row.child(
                            Label::new(label)
                                .size(LabelSize::XSmall)
                                .color(tone_color(marker.mark_tone)),
                        )
                    })
                    .tooltip(Tooltip::text(tooltip_text))
            }))
            .child(div().flex_1())
            .child(self.render_footer())
    }
}

impl RunwayRail {
    /// The rail footer, one designed state per top-level case: a live note when
    /// the engine answered, a muted "checking" line while the first poll is in
    /// flight (never the Connect CTA — that would read as a false negative), an
    /// honest error with a retry affordance when a reachable engine erred, and
    /// the Connect CTA only when there genuinely is no key.
    fn render_footer(&self) -> AnyElement {
        match &self.state {
            RailState::Linked(Load::Done(_)) => div()
                .px_1()
                .pb_1()
                .child(
                    Label::new("Live from your engine.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            RailState::Linked(Load::Pending) => div()
                .px_1()
                .pb_1()
                .child(
                    Label::new("Checking your engine…")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element(),
            RailState::Linked(Load::Failed(message)) => v_flex()
                .px_1()
                .pb_1()
                .gap_1()
                .child(
                    Label::new(message.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Error),
                )
                .child(
                    Button::new("runway-retry-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element(),
            RailState::Disconnected => v_flex()
                // Not connected: give the user the same forward action
                // every other panel offers, so the locked rail never
                // strands a first-time user (council B-14).
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
                )
                .into_any_element(),
        }
    }
}
