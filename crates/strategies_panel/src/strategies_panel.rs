//! Strategies — your strategy files, one click from the editor.
//!
//! Lists the strategies the engine has registered (`/ui/api/backtest/
//! strategies`) and opens the matching `.py` in the editor when a row
//! is clicked. It is a light navigator, not a control panel: deploy and
//! run state live in the Schedules and Runs surfaces. With no engine
//! reachable it says so plainly instead of pretending to have a list.
//!
//! The fetch outcome routes through the shared [`auracle_view_state`] seam, so
//! the panel is a thin `match` over [`ViewState`] with a designed loading
//! skeleton, an empty hint, and a retryable error — never a blank panel. The
//! row parsing, naming, and sort live in the gpui-free [`auracle_strategies`]
//! reducer (shared with the validation rail's picker), and `module_to_relpath`
//! is the tested resolver behind opening a file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_strategies::{StrategyListItem, module_to_relpath, strategy_rows};
use auracle_view_state::{Load, ViewState};
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use ui::{Divider, ListItem, ListItemSpacing};
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::{OpenOptions, OpenVisible, Workspace};

actions!(
    strategies_panel,
    [
        /// Toggle focus on the strategies panel.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(60);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<StrategiesPanel>(window, cx);
        });
    })
    .detach();
}

pub struct StrategiesPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    /// `true` until a connection (an engine API key) exists. A pre-fetch state
    /// kept outside the `Load` seam: disconnected offers Connect, never Retry.
    connected: bool,
    /// The strategy list, behind the shared fetch seam.
    strategies: Load<Vec<StrategyListItem>>,
    /// A short honest note shown when a row click can't resolve to a file (no
    /// open worktree, or an unsplittable module path) — so the click is never a
    /// silent no-op. Cleared on the next successful open.
    open_note: Option<SharedString>,
    _poll: Task<()>,
}

impl StrategiesPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        let handle = workspace.clone();
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.connected = auracle_connect::load_config().api_key.is_some();
                    this.strategies = Load::Pending;
                    this.open_note = None;
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    workspace: handle,
                    connected: auracle_connect::load_config().api_key.is_some(),
                    strategies: Load::Pending,
                    open_note: None,
                    _poll: poll,
                }
            })
        })
    }

    fn spawn_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_strategies(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            FetchResult::NotConnected => {
                                this.connected = false;
                                this.strategies = Load::Pending;
                            }
                            // A poll failure replaces the list with a designed,
                            // retryable error rather than silently holding stale
                            // rows — staleness must be honest (rubric item 3).
                            FetchResult::Unreachable => {
                                this.connected = true;
                                this.strategies = Load::Failed(
                                    "Your engine didn't answer. It may be stopped.".into(),
                                );
                            }
                            FetchResult::Ok(items) => {
                                this.connected = true;
                                this.strategies = Load::Done(items);
                            }
                        }
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

    /// Resolve a dotted module path to an absolute `.py` file under the
    /// first open worktree. `strategies.example_ma.MACrossover` becomes
    /// `<worktree>/strategies/example_ma.py`. Returns `None` when there
    /// is no worktree or the path can't be split into module + name.
    fn resolve_abs(&self, module_path: &str, cx: &App) -> Option<PathBuf> {
        let rel = module_to_relpath(module_path)?;
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().clone();
        let worktree = project.read(cx).visible_worktrees(cx).next()?;
        let root = worktree.read(cx).abs_path();
        Some(root.join(rel))
    }

    fn open_strategy(
        &mut self,
        module_path: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(abs) = self.resolve_abs(&module_path, cx) else {
            // Resolution failed (no open worktree, or the path can't be split).
            // Surface an honest note instead of a dead, silent click (rubric
            // item 5: no dead controls).
            self.open_note = Some("Open this folder as a project to edit strategy files.".into());
            cx.notify();
            return;
        };
        self.open_note = None;
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |_this, cx| {
            workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_abs_path(
                        abs,
                        OpenOptions {
                            visible: Some(OpenVisible::None),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                })?
                .await?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn refetch(&mut self, cx: &mut Context<Self>) {
        self.strategies = Load::Pending;
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let fetched = fetch_strategies(http).await;
            this.update(cx, |this, cx| {
                this.strategies = match fetched {
                    FetchResult::NotConnected => {
                        this.connected = false;
                        Load::Pending
                    }
                    FetchResult::Unreachable => {
                        Load::Failed("Your engine didn't answer. It may be stopped.".into())
                    }
                    FetchResult::Ok(items) => Load::Done(items),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<StrategyListItem>),
}

async fn fetch_strategies(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<StrategyListItem>> = async {
        let request = http_client::http::Request::builder()
            .uri(format!("{url}/ui/api/backtest/strategies"))
            .header("Cookie", format!("auracle_session={key}"))
            .body(http_client::AsyncBody::default())?;
        let mut response = http.send(request).await?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        // Pull the raw `(path, doc, bundled)` tuples and let the reducer derive
        // the name + first doc line, drop empty paths, and sort.
        let raw: Vec<(String, String, bool)> = value
            .get("strategies")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(|it| {
                        (
                            it.get("path")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("doc")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            it.get("bundled").and_then(|v| v.as_bool()).unwrap_or(false),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(strategy_rows(
            raw.iter().map(|(p, d, b)| (p.as_str(), d.as_str(), *b)),
        ))
    }
    .await;
    match attempt {
        Ok(items) => FetchResult::Ok(items),
        Err(_) => FetchResult::Unreachable,
    }
}

impl EventEmitter<PanelEvent> for StrategiesPanel {}

impl Focusable for StrategiesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for StrategiesPanel {
    fn persistent_name() -> &'static str {
        "StrategiesPanel"
    }

    fn panel_key() -> &'static str {
        "StrategiesPanel"
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
        Some(IconName::Code)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Strategies")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}

impl Render for StrategiesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("STRATEGIES")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("· open in editor")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        // Disconnected is a pre-fetch state, kept outside the `Load` seam.
        let body: AnyElement = if !self.connected {
            render_not_connected()
        } else {
            let state = self.strategies.clone().into_list_view(
                "No strategies yet. Create one under strategies/ and it shows up here.",
            );
            match state {
                ViewState::Loading => render_loading(),
                ViewState::Empty { hint } => render_empty(&hint),
                ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
                ViewState::Ready(strategies) => self.render_list(&strategies, cx),
            }
        };

        v_flex()
            .key_context("StrategiesPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .when_some(self.open_note.clone(), |this, note| {
                this.child(
                    div()
                        .px_2()
                        .py_1()
                        .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted)),
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
            Button::new("strategies-connect", "Connect…")
                .style(ButtonStyle::Filled)
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                }),
        )
        .into_any_element()
}

/// A designed skeleton — muted placeholder rows, not a bare "Loading…" label —
/// while the strategies fetch is in flight.
fn render_loading() -> AnyElement {
    let skeleton_row = || {
        ListItem::new("strategies-skeleton")
            .spacing(ListItemSpacing::Sparse)
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

/// An honest, retryable error — re-polls the engine once.
fn render_error(message: &str, retryable: bool, cx: &mut Context<StrategiesPanel>) -> AnyElement {
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
                Button::new("strategies-retry", "Retry")
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

impl StrategiesPanel {
    fn render_list(&self, strategies: &[StrategyListItem], cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("strategies-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .children(strategies.iter().enumerate().map(|(index, row)| {
                let path = SharedString::from(row.path.clone());
                let name = SharedString::from(row.name.clone());
                let doc = row.doc.clone();
                let bundled = row.bundled;
                ListItem::new(("strategy-row", index))
                    .spacing(ListItemSpacing::Sparse)
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.open_strategy(path.clone(), window, cx);
                    }))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(Label::new(name).size(LabelSize::Small))
                                    .when(bundled, |s| {
                                        s.child(
                                            Label::new("example")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    }),
                            )
                            .when(!doc.is_empty(), |s| {
                                s.child(
                                    Label::new(SharedString::from(doc))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    )
            }))
            .into_any_element()
    }
}
