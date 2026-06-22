//! Runs dock — the engine's live event feed as a panel.
//!
//! Consumes the engine's event stream (`/ui/api/events/stream`, SSE)
//! with a `/recent` snapshot for initial fill. Every row is the
//! engine's own plain sentence — the panel renders truth, it never
//! invents it. Connection settings come from the environment
//! (`AURACLE_ENGINE_URL`, `AURACLE_API_KEY`); without them the panel
//! says so in plain words instead of pretending.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::Result;
use auracle_feeds::{FeedTone, event_tone};
use auracle_view_state::{Load, ViewState};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Pixels,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    runs_dock,
    [
        /// Toggle focus on the runs dock.
        ToggleFocus
    ]
);

const MAX_ROWS: usize = 200;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<RunsDock>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone)]
struct EventRow {
    kind: SharedString,
    plain: SharedString,
    ts: SharedString,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConfigured,
    Connecting,
    Connected,
    Reconnecting,
}

struct Config {
    base_url: String,
    api_key: Option<String>,
}

fn read_config() -> Config {
    let c = auracle_connect::load_config();
    Config {
        base_url: c
            .engine_url
            .unwrap_or_else(|| "http://127.0.0.1:1969".to_string()),
        api_key: c.api_key.filter(|k| !k.trim().is_empty()),
    }
}

pub struct RunsDock {
    focus_handle: FocusHandle,
    rows: VecDeque<EventRow>,
    status: Status,
    _stream: Option<Task<()>>,
}

impl RunsDock {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                let config = read_config();
                let status = if config.api_key.is_none() {
                    Status::NotConfigured
                } else {
                    Status::Connecting
                };
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.reconnect(cx)
                })
                .detach();
                let mut this = Self {
                    focus_handle: cx.focus_handle(),
                    rows: VecDeque::new(),
                    status,
                    _stream: None,
                };
                if config.api_key.is_some() {
                    this._stream = Some(Self::spawn_stream(config, cx));
                }
                this
            })
        })
    }

    fn spawn_stream(config: Config, cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this, cx| {
            let cookie = format!(
                "auracle_session={}",
                config.api_key.as_deref().unwrap_or_default()
            );
            let mut backoff = Duration::from_secs(2);
            loop {
                let attempt =
                    run_stream_once(http.clone(), &config.base_url, &cookie, &this, cx).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        this.status = Status::Reconnecting;
                        cx.notify();
                    })
                    .is_ok();
                if !keep_going {
                    return;
                }
                if attempt.is_err() {
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                } else {
                    backoff = Duration::from_secs(2);
                }
                cx.background_executor().timer(backoff).await;
            }
        })
    }

    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        let config = read_config();
        self.rows.clear();
        if config.api_key.is_some() {
            self.status = Status::Connecting;
            self._stream = Some(Self::spawn_stream(config, cx));
        } else {
            self.status = Status::NotConfigured;
            self._stream = None;
        }
        cx.notify();
    }

    fn push_event(&mut self, value: &serde_json::Value) {
        let kind = value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("event");
        let plain = value
            .get("plain")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let ts = value.get("ts").and_then(|v| v.as_str()).unwrap_or_default();
        if plain.is_empty() {
            return;
        }
        let short_ts: String = ts.chars().take(16).collect();
        self.rows.push_front(EventRow {
            kind: SharedString::from(kind.to_string()),
            plain: SharedString::from(plain.to_string()),
            ts: SharedString::from(short_ts),
        });
        while self.rows.len() > MAX_ROWS {
            self.rows.pop_back();
        }
    }

    fn kind_color(&self, kind: &str, cx: &App) -> Hsla {
        // The kind -> tone decision lives in `auracle_feeds::event_tone` (gpui-free,
        // unit-tested); this method only maps that tone to a theme colour, so the
        // render path holds no colour literals and the decision stays testable.
        tone_color(event_tone(kind), cx)
    }

    fn render_rows(&self, rows: &[EventRow], cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("runs-scroll")
            .size_full()
            .overflow_y_scroll()
            .children(rows.iter().enumerate().map(|(ix, row)| {
                let color = self.kind_color(&row.kind, cx);
                // The colored dot's meaning, on hover, for the power
                // user — the plain sentence is the novice layer.
                let kind = row.kind.clone();
                h_flex()
                    .id(("run-row", ix))
                    .px_2()
                    .py_0p5()
                    .gap_2()
                    .tooltip(move |_, cx| ui::Tooltip::simple(kind.clone(), cx))
                    .child(div().size_1p5().rounded_full().flex_none().bg(color))
                    .child(Label::new(row.plain.clone()).size(LabelSize::Small))
                    .child(div().flex_1())
                    .child(
                        Label::new(row.ts.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
            }))
            .into_any_element()
    }
}

/// A designed skeleton — placeholder rows, not a blank panel — while the first
/// `/recent` snapshot is in flight. Mirrors `account_setup::render_loading`:
/// muted "Loading…" placeholder rows, so an in-flight first fetch reads as
/// loading rather than as the empty state.
fn render_skeleton() -> AnyElement {
    let skeleton_row = || {
        h_flex().px_2().py_0p5().child(
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

/// The designed empty state — a successful fetch with nothing yet to show.
fn render_empty(hint: &str) -> AnyElement {
    v_flex()
        .p_3()
        .child(Label::new(hint.to_string()).color(Color::Muted))
        .into_any_element()
}

/// Map a feed tone to the theme colour the row dot renders in. Only theme tokens
/// — never a colour literal — so the panel tracks the active theme. Mirrors the
/// `account_setup::tone_color` shape, resolved against `StatusColors` here because
/// the runs dot is drawn directly (`bg(Hsla)`) rather than via `Color`.
fn tone_color(tone: FeedTone, cx: &App) -> Hsla {
    let status = cx.theme().status();
    match tone {
        FeedTone::Negative => status.error,
        FeedTone::Info => status.info,
        FeedTone::Caution => status.warning,
        FeedTone::Positive => status.success,
        FeedTone::Ignored => status.ignored,
        FeedTone::Neutral => cx.theme().colors().text_muted,
    }
}

async fn run_stream_once(
    http: std::sync::Arc<dyn http_client::HttpClient>,
    base_url: &str,
    cookie: &str,
    this: &WeakEntity<RunsDock>,
    cx: &mut gpui::AsyncApp,
) -> Result<()> {
    // Snapshot first so the panel is useful immediately.
    let recent_req = http_client::http::Request::builder()
        .uri(format!("{base_url}/ui/api/events/recent?limit=50"))
        .header("Cookie", cookie)
        .body(http_client::AsyncBody::default())?;
    let mut recent = http.send(recent_req).await?;
    let mut body = String::new();
    recent.body_mut().read_to_string(&mut body).await?;
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(events) = value.get("events").and_then(|v| v.as_array()) {
            this.update(cx, |this, cx| {
                this.rows.clear();
                // /recent is newest-first; insert oldest-first so
                // push_front leaves newest on top.
                for ev in events.iter().rev() {
                    this.push_event(ev);
                }
                this.status = Status::Connected;
                cx.notify();
            })?;
        }
    }

    let stream_req = http_client::http::Request::builder()
        .uri(format!("{base_url}/ui/api/events/stream"))
        .header("Cookie", cookie)
        .header("Accept", "text/event-stream")
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(stream_req).await?;
    if !response.status().is_success() {
        anyhow::bail!("stream returned {}", response.status());
    }
    this.update(cx, |this, cx| {
        this.status = Status::Connected;
        cx.notify();
    })?;

    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    loop {
        let n = response.body_mut().read(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("stream ended");
        }
        pending.push_str(&String::from_utf8_lossy(&buf[..n]));
        while let Some(idx) = pending.find("\n\n") {
            let frame: String = pending.drain(..idx + 2).collect();
            for line in frame.lines() {
                if let Some(json) = line.strip_prefix("data: ") {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                        this.update(cx, |this, cx| {
                            this.push_event(&value);
                            cx.notify();
                        })?;
                    }
                }
            }
        }
    }
}

impl EventEmitter<PanelEvent> for RunsDock {}

impl Focusable for RunsDock {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for RunsDock {
    fn persistent_name() -> &'static str {
        "RunsDock"
    }

    fn panel_key() -> &'static str {
        "RunsDock"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ListX)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Runs")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        11
    }
}

impl Render for RunsDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, label): (Hsla, &str) = match self.status {
            Status::NotConfigured => (cx.theme().colors().text_muted, "not connected"),
            Status::Connecting => (cx.theme().status().warning, "connecting…"),
            Status::Connected => (cx.theme().status().success, "live"),
            Status::Reconnecting => (cx.theme().status().warning, "reconnecting…"),
        };

        // `NotConfigured` is not a fetch outcome — it's the pre-state before the
        // panel ever reaches for the stream — so it's rendered out of band, ahead
        // of the `Load`/`ViewState` seam. Everything else (Connecting / Reconnecting
        // / Connected) is a fetch lifecycle and flows through the seam, so empty /
        // loading / ready are the *designed* states, not hand-rolled branches that
        // can silently disagree.
        let body: AnyElement = if self.status == Status::NotConfigured {
            v_flex()
                .p_3()
                .gap_2()
                .child(Label::new(
                    "Not connected to your Auracle engine yet — normal on \
                     first start, nothing is broken.",
                ))
                .child(
                    Button::new("runs-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element()
        } else {
            // Connecting / Reconnecting with nothing buffered yet is genuinely
            // loading the first snapshot — render a skeleton, not the empty hint
            // (which would dishonestly claim "nothing has happened" during the
            // in-flight first fetch). Once rows exist, show them even while
            // reconnecting; the header dot already carries the "reconnecting…"
            // truth, so a transient reconnect must not blank a populated feed.
            let load = match self.status {
                Status::Connecting | Status::Reconnecting if self.rows.is_empty() => Load::Pending,
                // `EventRow` is `Clone` (its fields are `Arc`-backed `SharedString`s),
                // so collecting the ring buffer to a `Vec` for the seam is a cheap
                // refcount bump per row; the seam takes `Load<Vec<T>>`.
                _ => Load::Done(self.rows.iter().cloned().collect::<Vec<EventRow>>()),
            };
            let state = load.into_list_view("Runs appear here the moment something executes.");
            match state {
                // The stream loop reconnects forever and never surfaces a hard
                // error to the panel, so the only states here are Loading / Empty
                // / Ready — there is no retryable Error and so, honestly, no Retry
                // button (unlike blotter/incidents, which can hit a dead poll).
                ViewState::Loading => render_skeleton(),
                ViewState::Empty { hint } => render_empty(&hint),
                ViewState::Error { message, .. } => {
                    // Unreachable by construction (no failed `Load` is built here),
                    // but handled rather than panicked: show it honestly.
                    render_empty(&message)
                }
                ViewState::Ready(rows) => self.render_rows(&rows, cx),
            }
        };

        v_flex()
            .key_context("RunsDock")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(
                        Label::new("RUNS")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new("· Monitor")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(div().size_1p5().rounded_full().bg(dot))
                    .child(
                        Label::new(label)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(body)
    }
}
