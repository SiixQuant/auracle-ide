//! Schedules — what's deployed and when it next runs.
//!
//! Polls the engine's schedule feed (`/ui/api/schedules.json`) and shows
//! each deployed strategy with its cron and a live/paused dot. Pausing,
//! resuming, running once, and removing a schedule POST to the engine's
//! mutation routes and refetch so the row reflects the new truth. The panel
//! renders the engine's truth and never invents a schedule.
//!
//! The fetch outcome routes through the shared [`auracle_view_state`] seam, so
//! the list is a thin `match` over [`ViewState`] with a designed loading
//! skeleton, an empty hint, and a retryable error — and a poll failure owns the
//! surface rather than silently retaining stale rows. The row derivation
//! (strategy tail, liveness tone) and the toggle/delete button copy live in the
//! gpui-free [`auracle_schedules`] reducer; the view maps a [`ScheduleTone`] to
//! a theme [`Color`] and nothing more. A *mutation* failure (pause/run/delete)
//! is a row-local concern, so it stays an inline strip, distinct from a fetch
//! failure that owns the surface.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_schedules::{
    ScheduleListItem, ScheduleTone, delete_label, schedule_rows, toggle_label,
};
use auracle_view_state::{Load, ViewState};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use ui::{Divider, Indicator, ListItem, ListItemSpacing};
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

pub struct SchedulesPanel {
    focus_handle: FocusHandle,
    /// `true` until a connection (an engine API key) exists. A pre-fetch state
    /// kept outside the `Load` seam: disconnected offers Connect, never Retry.
    connected: bool,
    /// The schedule list, behind the shared fetch seam.
    schedules: Load<Vec<ScheduleListItem>>,
    _poll: Task<()>,
    /// Holds the in-flight mutation POST so the task isn't dropped (and
    /// cancelled) before it finishes. Mirrors
    /// `auracle_connections::BrokerWizard._task`.
    _action: Option<Task<()>>,
    /// Schedule names whose Delete is armed (first click arms, second
    /// click deletes) — a two-click confirm for a destructive action,
    /// since the repo has no shared confirm dialog for dock panels.
    delete_armed: HashSet<SharedString>,
    /// A short plain message shown when a *mutation* POST fails, cleared on the
    /// next successful refetch so it never lingers as stale. This is row-local
    /// (the list is still live), distinct from a fetch failure that maps to the
    /// surface `ViewState::Error`.
    last_error: Option<SharedString>,
}

impl SchedulesPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.connected = auracle_connect::load_config().api_key.is_some();
                    this.schedules = Load::Pending;
                    this.last_error = None;
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    connected: auracle_connect::load_config().api_key.is_some(),
                    schedules: Load::Pending,
                    _poll: poll,
                    _action: None,
                    delete_armed: HashSet::new(),
                    last_error: None,
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

    /// Fold a fetch outcome into the list state. A poll failure replaces the
    /// list with a designed, retryable error rather than silently holding stale
    /// rows — a dead engine must not look live (rubric item 3).
    fn apply_fetch(&mut self, fetched: FetchResult) {
        match fetched {
            FetchResult::NotConnected => {
                self.connected = false;
                self.schedules = Load::Pending;
            }
            FetchResult::Unreachable => {
                self.connected = true;
                self.schedules =
                    Load::Failed("Your engine didn't answer. It may be stopped.".into());
            }
            FetchResult::Ok(items) => {
                self.connected = true;
                self.schedules = Load::Done(items);
            }
        }
    }

    fn refetch(&mut self, cx: &mut Context<Self>) {
        self.schedules = Load::Pending;
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let fetched = fetch_schedules(http).await;
            this.update(cx, |this, cx| {
                this.apply_fetch(fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// POST a schedule mutation, then refetch so the row reflects the new
    /// truth (paused/resumed, a fresh run, or a removed row) without waiting
    /// for the next poll tick. `failure_prefix` shapes the inline error when
    /// the POST fails.
    fn mutate_and_refetch(
        &mut self,
        path: String,
        failure_prefix: &'static str,
        cx: &mut Context<Self>,
    ) {
        let http = cx.http_client();
        self._action = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if let Err(error) = post_mutation(http.clone(), &path).await {
                this.update(cx, |this, cx| {
                    this.last_error =
                        Some(SharedString::from(format!("{failure_prefix}: {error}.")));
                    cx.notify();
                })
                .ok();
            }
            let fetched = fetch_schedules(http).await;
            this.update(cx, |this, cx| {
                if let FetchResult::Ok(items) = fetched {
                    this.connected = true;
                    this.schedules = Load::Done(items);
                    this.last_error = None;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.mutate_and_refetch(
            format!("/ui/api/schedules/{name}/toggle"),
            "Couldn't pause/resume",
            cx,
        );
    }

    fn run_now(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.mutate_and_refetch(format!("/ui/api/schedules/{name}/run"), "Couldn't run", cx);
    }

    /// First Delete click arms the confirm (the button relabels to "Confirm
    /// delete"); the second click within the armed state actually deletes.
    fn delete(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if self.delete_armed.remove(&name) {
            self.mutate_and_refetch(
                format!("/ui/api/schedules/{name}/delete"),
                "Couldn't delete",
                cx,
            );
        } else {
            self.delete_armed.insert(name);
            cx.notify();
        }
    }
}

/// Map a reducer [`ScheduleTone`] to the theme colour the liveness dot renders
/// in. Only theme `Color::*` — never a colour literal — so the row tracks the
/// theme (mirrors `account_setup::tone_color`).
fn tone_color(tone: ScheduleTone) -> Color {
    match tone {
        ScheduleTone::Running => Color::Success,
        ScheduleTone::Paused => Color::Muted,
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<ScheduleListItem>),
}

async fn fetch_schedules(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<ScheduleListItem>> = async {
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
        // Pull the raw `(name, strategy_path, cron, enabled)` tuples and let the
        // reducer derive the strategy tail + liveness tone and drop empty names.
        let raw: Vec<(String, String, String, bool)> = value
            .get("schedules")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(|it| {
                        (
                            it.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("strategy_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("cron")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(schedule_rows(raw.iter().map(|(n, s, c, e)| {
            (n.as_str(), s.as_str(), c.as_str(), *e)
        })))
    }
    .await;
    match attempt {
        Ok(items) => FetchResult::Ok(items),
        Err(_) => FetchResult::Unreachable,
    }
}

/// Fetch the double-submit CSRF token: GET `/ui/api/status` so the engine
/// issues an `auracle_csrf` cookie, then return its value to echo back as
/// the `X-CSRF-Token` header on the mutation. Mirrors
/// `auracle_connections::fetch_csrf` — we hit `/ui/api/status` (not an HTML
/// page) so the cookie still flows under the headless web profile.
async fn fetch_csrf(http: Arc<dyn http_client::HttpClient>, base_url: &str, key: &str) -> String {
    let Ok(request) = http_client::http::Request::builder()
        .uri(format!("{base_url}/ui/api/status"))
        .header("X-API-Key", key)
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())
    else {
        return String::new();
    };
    let Ok(response) = http.send(request).await else {
        return String::new();
    };
    for value in response.headers().get_all("set-cookie") {
        let Ok(cookie) = value.to_str() else { continue };
        if let Some(rest) = cookie.strip_prefix("auracle_csrf=") {
            return rest.split(';').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// POST a `/ui/api` mutation with the dual auth headers + the double-submit
/// CSRF token, over loopback. Mirrors `auracle_connections::post_json`; these
/// schedule routes take an empty body. Returns the result so the caller can
/// react — never logs the request (the session key in the headers must not
/// reach the logs).
async fn post_mutation(http: Arc<dyn http_client::HttpClient>, path: &str) -> Result<()> {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        anyhow::bail!("not connected");
    };
    let base_url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let csrf = fetch_csrf(http.clone(), &base_url, &key).await;
    let request = http_client::http::Request::builder()
        .method("POST")
        .uri(format!("{base_url}{path}"))
        .header("Content-Type", "application/json")
        .header("X-API-Key", key.clone())
        .header("X-CSRF-Token", csrf.clone())
        .header(
            "Cookie",
            format!("auracle_session={key}; auracle_csrf={csrf}"),
        )
        .body(http_client::AsyncBody::default())?;
    let response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("status {}", response.status());
    }
    Ok(())
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
        9
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

        // Disconnected is a pre-fetch state, kept outside the `Load` seam.
        let body: AnyElement = if !self.connected {
            render_not_connected()
        } else {
            let state = self
                .schedules
                .clone()
                .into_list_view("Nothing deployed yet. Deploy a strategy and it appears here.");
            match state {
                ViewState::Loading => render_loading(),
                ViewState::Empty { hint } => render_empty(&hint),
                ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
                ViewState::Ready(schedules) => self.render_list(&schedules, cx),
            }
        };

        v_flex()
            .key_context("SchedulesPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            // A mutation failure is row-local (the list is still live), so it
            // stays an inline strip above the body rather than owning the
            // surface as a fetch error would.
            .when_some(self.last_error.clone(), |this, error| {
                this.child(
                    div().px_2().py_1().child(
                        Label::new(error)
                            .size(LabelSize::XSmall)
                            .color(Color::Error),
                    ),
                )
            })
            .child(body)
    }
}

/// The disconnected pre-state: offer Connect (never a false retryable error).
fn render_not_connected() -> AnyElement {
    v_flex()
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
        .into_any_element()
}

/// A designed skeleton — muted placeholder rows, not a bare "Loading…" label —
/// while the schedules fetch is in flight.
fn render_loading() -> AnyElement {
    let skeleton_row = || {
        ListItem::new("schedules-skeleton")
            .spacing(ListItemSpacing::Sparse)
            .start_slot(Indicator::dot().color(Color::Muted))
            .child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    };
    v_flex()
        .p_1()
        .child(skeleton_row())
        .child(Divider::horizontal().flex_grow_1())
        .child(skeleton_row())
        .child(Divider::horizontal().flex_grow_1())
        .child(skeleton_row())
        .into_any_element()
}

/// A designed empty state carrying the hint about what would appear here.
fn render_empty(hint: &str) -> AnyElement {
    v_flex()
        .p_3()
        .child(Label::new(hint.to_string()).color(Color::Muted))
        .into_any_element()
}

/// An honest, retryable error — a poll failure owns the surface (no stale
/// rows) and offers a single re-poll.
fn render_error(message: &str, retryable: bool, cx: &mut Context<SchedulesPanel>) -> AnyElement {
    v_flex()
        .p_3()
        .gap_2()
        .child(
            Label::new(message.to_string())
                .size(LabelSize::Small)
                .color(Color::Error),
        )
        .when(retryable, |this| {
            this.child(
                Button::new("schedules-retry", "Retry")
                    .style(ButtonStyle::Outlined)
                    .start_icon(
                        Icon::new(IconName::RotateCcw)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refetch(cx);
                    })),
            )
        })
        .into_any_element()
}

impl SchedulesPanel {
    fn render_list(&self, schedules: &[ScheduleListItem], cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("schedules-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .children(
                schedules
                    .iter()
                    .enumerate()
                    .map(|(index, row)| self.render_row(index, row, cx)),
            )
            .into_any_element()
    }

    /// One schedule as a native `ListItem`: a liveness dot in the start slot,
    /// the name with a strategy · cron sub-line, and the Pause/Resume · Run ·
    /// Delete cluster in the end slot. All labels come from the reducer.
    fn render_row(
        &self,
        index: usize,
        row: &ScheduleListItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = SharedString::from(row.name.clone());
        let strategy = SharedString::from(row.strategy.clone());
        let cron = SharedString::from(row.cron.clone());
        let enabled = row.enabled;
        let dot_color = tone_color(row.tone);
        let toggle_text = toggle_label(enabled);
        let delete_armed = self.delete_armed.contains(&name);
        let delete_text = delete_label(delete_armed);

        let toggle_name = name.clone();
        let run_name = name.clone();
        let delete_name = name.clone();

        let actions = h_flex()
            .gap_1()
            .flex_none()
            .child(
                Button::new(
                    SharedString::from(format!("sched-toggle-{name}")),
                    toggle_text,
                )
                .style(ButtonStyle::Subtle)
                .label_size(LabelSize::XSmall)
                .tooltip(ui::Tooltip::text(if enabled {
                    "Pause this schedule (keeps it, stops the cron)"
                } else {
                    "Resume this schedule"
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle(toggle_name.clone(), cx);
                })),
            )
            .child(
                Button::new(SharedString::from(format!("sched-run-{name}")), "Run now")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::XSmall)
                    .tooltip(ui::Tooltip::text("Run this schedule once now"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_now(run_name.clone(), cx);
                    })),
            )
            .child(
                Button::new(
                    SharedString::from(format!("sched-delete-{name}")),
                    delete_text,
                )
                .style(ButtonStyle::Subtle)
                .label_size(LabelSize::XSmall)
                .color(Color::Error)
                .tooltip(ui::Tooltip::text(if delete_armed {
                    "Click again to remove this schedule"
                } else {
                    "Remove this schedule (click twice to confirm)"
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.delete(delete_name.clone(), cx);
                })),
            );

        ListItem::new(("schedule-row", index))
            .spacing(ListItemSpacing::Sparse)
            .start_slot(Indicator::dot().color(dot_color))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(Label::new(name).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                Label::new(strategy)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(cron).size(LabelSize::XSmall).color(Color::Muted))
                            .when(!enabled, |s| {
                                s.child(
                                    Label::new("paused")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    ),
            )
            .end_slot(actions)
            .into_any_element()
    }
}
