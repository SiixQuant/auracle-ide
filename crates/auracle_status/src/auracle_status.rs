//! Status bar truths — the engine chip.
//!
//! One small status-bar item that tells the truth about the engine
//! connection, the active broker, and what that broker is allowed to
//! do ("live ok" vs "paper only"), polled from the capability API
//! every thirty seconds and refreshed instantly after a Connect save.
//! It renders four states — not connected, checking, unreachable, and
//! connected-with-broker — and a click opens the Connect dialog. The
//! tooltip carries the engine's full plain capability sentence so the
//! honest answer to "can I go live?" is one hover away. The
//! kill-switch chip is deliberately absent until its engine verb
//! exists (never fake a control).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use gpui::{App, Entity, EventEmitter, Hsla, SharedString, Task, Window};
use ui::Tooltip;
use ui::prelude::*;
use workspace::{StatusItemView, Workspace, item::ItemHandle};

const POLL_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq)]
enum EngineState {
    NotConnected,
    Checking,
    Connected {
        broker: SharedString,
        live_allowed: bool,
        /// The engine's plain capability sentence for the active
        /// broker (or the "no broker yet" line) — the honest answer
        /// to "what can this do / can I go live?".
        capability_plain: SharedString,
    },
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
        auracle_panel_common::spawn_poll(
            cx,
            POLL_EVERY,
            move || poll_once(http.clone()),
            |this, next, _cx| {
                this.state = next;
            },
        )
    }
}

async fn poll_once(http: Arc<dyn http_client::HttpClient>) -> EngineState {
    let config = auracle_connect::load_config();
    if config
        .api_key
        .filter(|key| !key.trim().is_empty())
        .is_none()
    {
        return EngineState::NotConnected;
    }
    let attempt: Result<EngineState> = async {
        let value = auracle_connections::get_json(http, "/ui/api/capabilities").await?;
        let active = value.get("active_broker").and_then(|v| v.as_str());
        let broker = active.unwrap_or("none yet").to_string();
        let live_allowed = value
            .get("live_allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Prefer the active broker's own plain sentence; fall back to
        // the top-level plain (set when no broker is active yet).
        let capability_plain = value
            .get("brokers")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|b| b.get("active").and_then(|a| a.as_bool()).unwrap_or(false))
            })
            .and_then(|b| b.get("plain").and_then(|p| p.as_str()))
            .or_else(|| value.get("plain").and_then(|p| p.as_str()))
            .unwrap_or("")
            .to_string();
        Ok(EngineState::Connected {
            broker: SharedString::from(broker),
            live_allowed,
            capability_plain: SharedString::from(capability_plain),
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
                "Your Auracle engine isn't connected yet. Click to connect.".into(),
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
                capability_plain,
            } => {
                // Glance text answers "can I go live?" in one word;
                // the tooltip carries the engine's full plain sentence.
                let mode = if *live_allowed {
                    "live ok"
                } else {
                    "paper only"
                };
                let license_note = if *live_allowed {
                    "Real-money trading is allowed by your license — \
                     paper stays the default."
                } else {
                    "Real-money trading is not yet enabled on your \
                     license; paper trading works."
                };
                let tip = if capability_plain.is_empty() {
                    license_note.to_string()
                } else {
                    format!("{capability_plain} {license_note}")
                };
                (
                    cx.theme().status().success,
                    // "on" (not "live") so the word never collides with
                    // live trading — the mode token owns that meaning.
                    SharedString::from(format!("engine: on · broker: {broker} · {mode}")),
                    SharedString::from(tip),
                )
            }
        };

        h_flex()
            .id("auracle-engine-chip")
            .gap_1p5()
            .px_1()
            .child(div().size_1p5().rounded_full().bg(dot))
            .child(Label::new(text).size(LabelSize::Small).color(Color::Muted))
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
