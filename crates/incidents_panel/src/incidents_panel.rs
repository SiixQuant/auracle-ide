//! Incidents panel — what needs your attention, in plain words.
//!
//! Polls the engine's incident feed (`/ui/api/incidents`) and renders
//! each as an incident card: a severity dot, the engine's plain cause
//! sentence, and the technical detail behind a "show details" toggle.
//! The panel renders truth — it never invents an incident.
//!
//! Each card has a Dismiss action ("mark as seen and hide it"). It
//! posts to the engine's existing protected endpoint using CSRF
//! double-submit — the panel captures the auracle_csrf token from the
//! incidents GET and echoes it as both cookie and X-CSRF-Token — so
//! it clears alerts the same way a browser does, with no weakened
//! server CSRF and no new endpoint.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_feeds::severity_tone;
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
    dismiss_kind: SharedString,
    dismiss_id: i64,
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
    /// The CSRF token captured from the incidents GET response, so a
    /// Dismiss POST can satisfy the engine's double-submit check
    /// without weakening server-side CSRF for browsers.
    csrf: Option<SharedString>,
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
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.status = Status::Loading;
                    this.incidents.clear();
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    incidents: Vec::new(),
                    expanded: HashSet::new(),
                    csrf: None,
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
        auracle_connect::spawn_periodic_fetch(
            cx,
            POLL_EVERY,
            fetch_incidents,
            |this, fetched, _cx| this.apply_fetch(fetched),
        )
    }

    /// Apply a fetch outcome to panel state. Shared by the poll and the manual
    /// `refetch` (Retry), so both reach the same state from the same outcome —
    /// the Retry path can't drift from the poll path.
    fn apply_fetch(&mut self, fetched: FetchResult) {
        match fetched {
            FetchResult::NotConnected => {
                self.status = Status::NotConnected;
                self.incidents.clear();
            }
            FetchResult::Unreachable => {
                self.status = Status::Unreachable;
            }
            FetchResult::Ok(items, csrf) => {
                self.status = Status::Connected;
                self.incidents = items;
                // Only overwrite the captured token when the GET issued a fresh
                // one — a poll that returns no Set-Cookie must not clear it.
                if csrf.is_some() {
                    self.csrf = csrf;
                }
            }
        }
    }

    /// Fetch once now, off the 30s cadence — the Retry affordance on the
    /// unreachable error state. Shows Loading immediately so the click reads as
    /// acted-upon, then applies the outcome through the same `apply_fetch` the
    /// poll uses.
    fn refetch(&mut self, cx: &mut Context<Self>) {
        self.status = Status::Loading;
        cx.notify();
        // The fetch outcome (incl. an unreachable engine) is applied to UI state
        // by `apply_fetch` — never silently dropped — through the same shared
        // fetch helper the poll uses, so Retry can't drift from the poll path.
        auracle_connect::spawn_fetch_once(cx, fetch_incidents, |this, fetched, _cx| {
            this.apply_fetch(fetched)
        })
        .detach();
    }

    fn severity_color(&self, severity: &str, cx: &App) -> Hsla {
        // The severity -> tone decision lives in `auracle_feeds::severity_tone`
        // (gpui-free, unit-tested); this method only maps that tone to a theme
        // colour, so the render path holds no colour literals.
        auracle_connect::tone_to_color(severity_tone(severity), cx)
    }

    fn dismiss(&mut self, incident: &Incident, cx: &mut Context<Self>) {
        let Some(csrf) = self.csrf.clone() else {
            return; // no token yet — a poll will land one shortly
        };
        let row_id = incident.row_id.clone();
        let kind = incident.dismiss_kind.clone();
        let id = incident.dismiss_id;
        // Optimistic: drop the card now; a failed POST is recovered by
        // the next poll (the engine still reports it until dismissed).
        self.incidents.retain(|i| i.row_id != row_id);
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |_this: WeakEntity<Self>, _cx| {
            post_dismiss(http, csrf, kind, id).await.ok();
        })
        .detach();
    }

    fn render_cards(&self, incidents: &[Incident], cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("incidents-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_1()
            .gap_1()
            .children(incidents.iter().map(|incident| {
                let dot = self.severity_color(&incident.severity, cx);
                let is_open = self.expanded.contains(&incident.row_id);
                let row_id = incident.row_id.clone();
                let has_detail = !incident.detail.is_empty();
                // When the CSRF token hasn't landed yet (`csrf` is None), the
                // Dismiss control is omitted rather than shown-but-broken. This
                // is transient: the incidents GET issues the token, so the next
                // 30s poll (or a manual Retry) restores Dismiss within one cycle.
                // A disabled/tooltipped affordance is deliberately out of scope.
                let can_dismiss = self.csrf.is_some();
                let incident_for_dismiss = incident.clone();
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
                            .child(div().mt_1().size_1p5().rounded_full().flex_none().bg(dot))
                            .child(Label::new(incident.cause.clone()).size(LabelSize::Small))
                            .child(div().flex_1())
                            .when(can_dismiss, |row| {
                                row.child(
                                    Button::new(
                                        SharedString::from(format!("dismiss-{row_id}")),
                                        "Dismiss",
                                    )
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::XSmall)
                                    .tooltip(ui::Tooltip::text("Mark as seen and hide it."))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.dismiss(&incident_for_dismiss, cx);
                                        },
                                    )),
                                )
                            }),
                    )
                    .when(has_detail, |card| {
                        let toggle_id: SharedString = format!("toggle-{row_id}").into();
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
            .into_any_element()
    }
}

/// A designed skeleton — placeholder rows, not a bare "Checking…" line — while a
/// fetch is in flight. Mirrors `account_setup::render_loading`.
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
        .gap_1()
        .child(skeleton_row())
        .child(skeleton_row())
        .child(skeleton_row())
        .into_any_element()
}

/// The designed empty state — a successful fetch with no incidents to show.
fn render_empty(hint: &str) -> AnyElement {
    v_flex()
        .p_3()
        .child(Label::new(hint.to_string()).color(Color::Muted))
        .into_any_element()
}

/// An honest, retryable error state — the engine was unreachable — with a Retry
/// that forces an immediate refetch off the 30s poll cadence. Mirrors
/// `account_setup::render_error`.
fn render_error(message: &str, retryable: bool, cx: &mut Context<IncidentsPanel>) -> AnyElement {
    v_flex()
        .p_3()
        .gap_2()
        .child(Label::new(message.to_string()).color(Color::Muted))
        .when(retryable, |this| {
            this.child(
                Button::new("incidents-retry", "Retry")
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
    Ok(Vec<Incident>, Option<SharedString>),
}

/// Pull the value of a Set-Cookie named cookie out of response headers.
fn cookie_from_headers(headers: &http_client::http::HeaderMap, name: &str) -> Option<SharedString> {
    for value in headers
        .get_all(http_client::http::header::SET_COOKIE)
        .iter()
    {
        // Skip a non-UTF-8 header rather than abandoning the rest.
        let Ok(raw) = value.to_str() else { continue };
        for part in raw.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix(name) {
                if let Some(v) = rest.strip_prefix('=') {
                    if !v.is_empty() {
                        return Some(SharedString::from(v.to_string()));
                    }
                }
            }
        }
    }
    None
}

async fn fetch_incidents(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<(Vec<Incident>, Option<SharedString>)> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/incidents"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let csrf = cookie_from_headers(response.headers(), "auracle_csrf");
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let mut out = Vec::new();
        if let Some(items) = value.get("incidents").and_then(|v| v.as_array()) {
            for it in items {
                let kind = it.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let id = it.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                let cause = it.get("cause").and_then(|v| v.as_str()).unwrap_or_default();
                if cause.is_empty() {
                    continue;
                }
                let dismiss = it.get("dismiss");
                let dismiss_kind = dismiss
                    .and_then(|d| d.get("kind"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(kind)
                    .to_string();
                let dismiss_id = dismiss
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(id);
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
                    dismiss_kind: SharedString::from(dismiss_kind),
                    dismiss_id,
                });
            }
        }
        Ok((out, csrf))
    }
    .await;
    match attempt {
        Ok((items, csrf)) => FetchResult::Ok(items, csrf),
        Err(_) => FetchResult::Unreachable,
    }
}

async fn post_dismiss(
    http: Arc<dyn http_client::HttpClient>,
    csrf: SharedString,
    kind: SharedString,
    id: i64,
) -> Result<()> {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        anyhow::bail!("not connected");
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let body = serde_json::to_string(&serde_json::json!({"kind": kind, "id": id}))?;
    // Double-submit CSRF: the token rides as both the auracle_csrf
    // cookie and the X-CSRF-Token header (engine compares the two).
    let request = http_client::http::Request::builder()
        .method(http_client::http::Method::POST)
        .uri(format!("{url}/ui/api/alerts/dismiss"))
        .header(
            "Cookie",
            format!("auracle_session={key}; auracle_csrf={csrf}"),
        )
        .header("X-CSRF-Token", csrf.to_string())
        .header("Content-Type", "application/json")
        .body(http_client::AsyncBody::from(body))?;
    let response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("status {}", response.status());
    }
    Ok(())
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
            )
            .child(
                Label::new("· Monitor")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        // `NotConnected` is the pre-state before the panel ever reaches for the
        // feed — not a fetch outcome — so it's rendered out of band, ahead of the
        // `Load`/`ViewState` seam. Loading / Unreachable / Connected are fetch
        // lifecycle states and flow through the seam, so loading is a designed
        // skeleton, an unreachable engine is a *retryable* error (with Retry),
        // and empty is the designed empty state — none of them hand-rolled.
        let body: AnyElement = if self.status == Status::NotConnected {
            v_flex()
                .p_3()
                .gap_2()
                .child(Label::new("Not connected to your Auracle engine yet."))
                .child(
                    Button::new("incidents-connect", "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element()
        } else {
            let load = match self.status {
                Status::Loading => Load::Pending,
                // An unreachable engine is a transient failure the poll
                // auto-recovers from in ≤30s — but the user gets a manual Retry
                // too, so the state is the designed *retryable* error, not a
                // dead end.
                Status::Unreachable => {
                    Load::Failed("Your engine didn't answer. It may be stopped.".to_string())
                }
                // `Status::NotConnected` is handled above; `Status::Connected`
                // falls here with the current incidents.
                _ => Load::Done(self.incidents.clone()),
            };
            let state = load.into_list_view("Nothing needs your attention right now.");
            match state {
                ViewState::Loading => render_skeleton(),
                ViewState::Empty { hint } => render_empty(&hint),
                ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
                ViewState::Ready(incidents) => self.render_cards(&incidents, cx),
            }
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
