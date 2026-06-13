//! Status bar truths — the engine chip.
//!
//! One small status-bar item that tells the truth about the engine
//! connection and the active broker, polled from the capability API
//! every thirty seconds and refreshed instantly after a Connect
//! save. It renders exactly three states — not connected, checking,
//! and connected-with-broker — and a click opens the Connect dialog.
//! The kill-switch chip is deliberately absent until its engine verb
//! exists (never fake a control).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{App, Entity, EventEmitter, Hsla, SharedString, Task, WeakEntity, Window};
use ui::prelude::*;
use ui::Tooltip;
use workspace::{StatusItemView, Workspace, item::ItemHandle};

const POLL_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq)]
enum EngineState {
    NotConnected,
    Checking,
    Connected { broker: SharedString, live_allowed: bool },
    Unreachable,
}

pub struct AuracleStatus {
    state: EngineState,
    _poll: Task<()>,
}

impl AuracleStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
            this.state = EngineState::Checking;
            cx.notify();
        })
        .detach();
        let poll = Self::spawn_poll(cx);
        Self {
            state: if auracle_connect::load_config().api_key.is_some() {
                EngineState::Checking
            } else {
                EngineState::NotConnected
            },
            _poll: poll,
        }
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let next = poll_once(http.clone()).await;
                if this
                    .update(cx, |this, cx| {
                        this.state = next.clone();
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(POLL_EVERY).await;
            }
        })
    }
}

async fn poll_once(http: Arc<dyn http_client::HttpClient>) -> EngineState {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return EngineState::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<EngineState> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/capabilities"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let broker = value
            .get("active_broker")
            .and_then(|v| v.as_str())
            .unwrap_or("none yet")
            .to_string();
        let live_allowed = value
            .get("live_allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(EngineState::Connected {
            broker: SharedString::from(broker),
            live_allowed,
        })
    }
    .await;
    attempt.unwrap_or(EngineState::Unreachable)
}

impl EventEmitter<()> for AuracleStatus {}

impl Render for AuracleStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, text, tip): (Hsla, SharedString, SharedString) = match &self.state {
            EngineState::NotConnected => (
                cx.theme().colors().text_muted,
                "engine: not connected".into(),
                "Your Auracle engine isn't connected yet. Click to connect."
                    .into(),
            ),
            EngineState::Checking => (
                cx.theme().status().warning,
                "engine: checking…".into(),
                "Asking your engine how it's doing — usually a moment.".into(),
            ),
            EngineState::Unreachable => (
                cx.theme().status().error,
                "engine: unreachable".into(),
                "Your engine didn't answer. It may be stopped — start it, \
                 or click to check the connection details."
                    .into(),
            ),
            EngineState::Connected {
                broker,
                live_allowed,
            } => (
                cx.theme().status().success,
                SharedString::from(format!("engine: live · broker: {broker}")),
                SharedString::from(if *live_allowed {
                    "Connected. Real-money trading is allowed by your \
                     license — paper stays the default."
                        .to_string()
                } else {
                    "Connected. Real-money trading is not yet enabled on \
                     your license; paper trading works."
                        .to_string()
                }),
            ),
        };

        h_flex()
            .id("auracle-engine-chip")
            .gap_1p5()
            .px_1()
            .child(div().size_1p5().rounded_full().bg(dot))
            .child(
                Label::new(text)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .tooltip(Tooltip::text(tip))
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(auracle_connect::Connect), cx);
            })
    }
}

impl StatusItemView for AuracleStatus {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<workspace::HideStatusItem> {
        // Engine truth must stay visible; there is no setting to
        // hide it.
        None
    }
}

pub fn register(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let item: Entity<AuracleStatus> = cx.new(|cx| AuracleStatus::new(cx));
    workspace.status_bar().update(cx, |status_bar, cx| {
        status_bar.add_right_item(item, window, cx);
    });
}

pub fn init(_cx: &mut App) {}
