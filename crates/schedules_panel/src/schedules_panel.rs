//! Schedules — what's deployed and when it next runs.
//!
//! Polls the engine's schedule feed (`/ui/api/schedules.json`) and shows
//! each deployed strategy with its cron and a live/paused dot. It is a
//! light read-only surface: starting, pausing, and stopping a schedule
//! happen through the agent ("pause the momentum schedule") which calls
//! the engine's mutation tools. The panel renders the engine's truth and
//! never invents a schedule.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_connect::post_mutation;
use auracle_panel_common::{
    PanelStatus as Status, PlaceholderLabels, panel_header, placeholder_body,
};
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

pub struct SchedulesPanel {
    focus_handle: FocusHandle,
    schedules: Vec<ScheduleRow>,
    status: Status,
    _poll: Task<()>,
    /// Holds the in-flight mutation POST so the task isn't dropped (and
    /// cancelled) before it finishes. Mirrors
    /// `auracle_connections::BrokerWizard._task`.
    _action: Option<Task<()>>,
    /// Schedule names whose Delete is armed (first click arms, second
    /// click deletes) — a two-click confirm for a destructive action,
    /// since the repo has no shared confirm dialog for dock panels.
    /// Mirrors `incidents_panel`'s `expanded: HashSet<SharedString>`.
    delete_armed: HashSet<SharedString>,
    /// A short plain message shown when a mutation POST fails, cleared on
    /// the next successful refetch so it never lingers as stale.
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
                    this.status = Status::Loading;
                    this.schedules.clear();
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    schedules: Vec::new(),
                    status: Status::initial(),
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
        auracle_panel_common::spawn_poll(
            cx,
            POLL_EVERY,
            move || fetch_schedules(http.clone()),
            |this, fetched, _cx| match fetched {
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
                    this.last_error = None;
                }
            },
        )
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
                    this.status = Status::Connected;
                    this.schedules = items;
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
        let header = panel_header("SCHEDULES", cx).child(
            Label::new("· what's deployed")
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        );

        let labels = PlaceholderLabels::new(
            "schedules-connect",
            "Loading…",
            "Nothing deployed yet. Deploy a strategy and it appears here.",
        );
        let body: AnyElement =
            match placeholder_body(&self.status, self.schedules.is_empty(), &labels) {
                Some(placeholder) => placeholder,
                None => v_flex()
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
                        let name = row.name.clone();
                        // Label Pause/Resume from the row's current paused state
                        // (enabled == running → "Pause"; paused → "Resume").
                        let toggle_label = if row.enabled { "Pause" } else { "Resume" };
                        let toggle_name = name.clone();
                        let run_name = name.clone();
                        let delete_name = name.clone();
                        let delete_armed = self.delete_armed.contains(&name);
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
                            .child(div().flex_1())
                            .child(
                                h_flex()
                                    .gap_1()
                                    .flex_none()
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("sched-toggle-{name}")),
                                            toggle_label,
                                        )
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .tooltip(ui::Tooltip::text(if row.enabled {
                                            "Pause this schedule (keeps it, stops the cron)"
                                        } else {
                                            "Resume this schedule"
                                        }))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle(toggle_name.clone(), cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("sched-run-{name}")),
                                            "Run now",
                                        )
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .tooltip(ui::Tooltip::text("Run this schedule once now"))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.run_now(run_name.clone(), cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("sched-delete-{name}")),
                                            if delete_armed {
                                                "Confirm delete"
                                            } else {
                                                "Delete"
                                            },
                                        )
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .color(Color::Error)
                                        .tooltip(ui::Tooltip::text(if delete_armed {
                                            "Click again to remove this schedule"
                                        } else {
                                            "Remove this schedule (click twice to confirm)"
                                        }))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.delete(delete_name.clone(), cx);
                                            }),
                                        ),
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
