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
use auracle_status_view::{
    ChipTone, ConnectionStatus, EngineFacts, QcFacts, chip_view, rollup_chip_view,
};
use futures::AsyncReadExt as _;
use gpui::{App, Entity, EventEmitter, Hsla, Task, WeakEntity, Window};
use ui::Tooltip;
use ui::prelude::*;
use util::ResultExt as _;
use workspace::{StatusItemView, Workspace, item::ItemHandle};

const POLL_EVERY: Duration = Duration::from_secs(30);

/// The one place a chip tone becomes a status-bar dot colour. Both the engine
/// chip and the connections rollup resolve through here, so the tone→colour
/// mapping lives once instead of being copy-pasted per chip.
fn dot_color(tone: ChipTone, cx: &App) -> Hsla {
    match tone {
        ChipTone::Good => cx.theme().status().success,
        ChipTone::Bad => cx.theme().status().error,
        ChipTone::Checking => cx.theme().status().warning,
        ChipTone::Muted => cx.theme().colors().text_muted,
    }
}

pub struct AuracleStatus {
    /// The already-parsed engine facts the chip decides over — the gpui-free
    /// reducer (`chip_view`) owns the label/tone/tooltip text, so this view only
    /// holds the facts and maps the reducer's tone to a theme colour.
    state: EngineFacts,
    _poll: Task<()>,
}

impl AuracleStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
            this.state = EngineFacts::Checking;
            cx.notify();
        })
        .detach();
        let poll = Self::spawn_poll(cx);
        Self {
            state: if auracle_connect::load_config()
                .api_key
                .filter(|key| !key.trim().is_empty())
                .is_some()
            {
                EngineFacts::Checking
            } else {
                EngineFacts::NotConnected
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

async fn poll_once(http: Arc<dyn http_client::HttpClient>) -> EngineFacts {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return EngineFacts::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<EngineFacts> = async {
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
        // An absent or empty active broker is None — never a broker literally
        // named "none yet"; the reducer renders the honest "no broker yet" form.
        let broker = value
            .get("active_broker")
            .and_then(|v| v.as_str())
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string());
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
        Ok(EngineFacts::Connected {
            broker,
            live_allowed,
            capability_plain,
        })
    }
    .await;
    // Never silently discard a real fetch failure: log it (the error carries no
    // key — the secret only ever lived in the request Cookie header) and fall
    // back to the honest Unreachable chip.
    attempt.log_err().unwrap_or(EngineFacts::Unreachable)
}

impl EventEmitter<()> for AuracleStatus {}

impl Render for AuracleStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The whole chip — label, tone, and tooltip — is decided by the gpui-free
        // reducer; this view only maps the reducer's tone to a theme colour.
        let view = chip_view(self.state.clone());
        let dot = dot_color(view.tone, cx);

        h_flex()
            .id("auracle-engine-chip")
            .gap_1p5()
            .px_1()
            .child(div().size_1p5().rounded_full().bg(dot))
            .child(
                Label::new(view.label)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .tooltip(Tooltip::text(view.tooltip))
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

/// The connections rollup chip — one chip that summarises every non-engine
/// connection (QuantConnect today; brokers and data sources as they gain status
/// probes) instead of one chip per connector. The engine keeps its own chip; this
/// one collapses the rest into a single worst-state + count read, decided by the
/// gpui-free `rollup_chip_view` reducer. Clicking it opens Settings → Connections.
/// It never fabricates a connection: an absent or unreachable endpoint reads
/// honestly through each member's tone.
pub struct ConnectionsRollup {
    /// The non-engine connections being summarised. Push more members here as
    /// other connectors gain a status probe; the reducer collapses the list.
    members: Vec<ConnectionStatus>,
    _poll: Task<()>,
}

/// Map a QuantConnect probe result to its rollup member. `Good`/connected only on
/// an actual authenticated connection — never from a stale or in-flight probe.
fn qc_to_status(facts: QcFacts) -> ConnectionStatus {
    let (tone, connected) = match facts {
        QcFacts::Connected { .. } => (ChipTone::Good, true),
        QcFacts::Checking => (ChipTone::Checking, false),
        QcFacts::NotConnected => (ChipTone::Muted, false),
    };
    ConnectionStatus {
        name: "QuantConnect".to_string(),
        tone,
        connected,
    }
}

impl ConnectionsRollup {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
            // A saved connection invalidates every member until the next probe;
            // show checking rather than a stale read.
            for member in &mut this.members {
                member.tone = ChipTone::Checking;
                member.connected = false;
            }
            cx.notify();
        })
        .detach();
        let poll = Self::spawn_poll(cx);
        let has_key = auracle_connect::load_config()
            .api_key
            .filter(|key| !key.trim().is_empty())
            .is_some();
        Self {
            members: vec![qc_to_status(if has_key {
                QcFacts::Checking
            } else {
                QcFacts::NotConnected
            })],
            _poll: poll,
        }
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let members = poll_connections_once(http.clone()).await;
                if this
                    .update(cx, |this, cx| {
                        this.members = members;
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

/// Probe every non-engine connection once and return their rollup members. Today
/// that is QuantConnect alone; add more probes here as connectors gain status.
async fn poll_connections_once(http: Arc<dyn http_client::HttpClient>) -> Vec<ConnectionStatus> {
    let quantconnect = qc_to_status(poll_qc_once(http.clone()).await);
    vec![quantconnect]
}

async fn poll_qc_once(http: Arc<dyn http_client::HttpClient>) -> QcFacts {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return QcFacts::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<QcFacts> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/quantconnect/connection"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        if !value
            .get("connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(QcFacts::NotConnected);
        }
        let user_id = value
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let projects = value
            .get("project_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        Ok(QcFacts::Connected { user_id, projects })
    }
    .await;
    // Endpoint absent (not deployed), unreachable, or unparseable => honest
    // "off". The secret only ever lived in the Cookie header, never the error.
    attempt.log_err().unwrap_or(QcFacts::NotConnected)
}

impl EventEmitter<()> for ConnectionsRollup {}

impl Render for ConnectionsRollup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The whole chip is decided by the gpui-free rollup reducer; this view
        // only resolves the tone to a dot colour and routes the click.
        let view = rollup_chip_view(&self.members);
        let dot = dot_color(view.tone, cx);
        h_flex()
            .id("auracle-connections-chip")
            .gap_1p5()
            .px_1()
            .child(div().size_1p5().rounded_full().bg(dot))
            .child(
                Label::new(view.label)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .tooltip(Tooltip::text(view.tooltip))
            .on_click(|_, window, cx| {
                window.dispatch_action(
                    Box::new(zed_actions::OpenSettingsAt {
                        path: "connections.account".to_string(),
                        target: None,
                    }),
                    cx,
                );
            })
    }
}

impl StatusItemView for ConnectionsRollup {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<workspace::HideStatusItem> {
        None
    }
}

pub fn register_connections(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let item: Entity<ConnectionsRollup> = cx.new(|cx| ConnectionsRollup::new(cx));
    workspace.status_bar().update(cx, |status_bar, cx| {
        status_bar.add_right_item(item, window, cx);
    });
}
