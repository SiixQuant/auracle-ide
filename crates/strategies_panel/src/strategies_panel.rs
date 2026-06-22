//! Strategies — your strategy files, one click from the editor.
//!
//! Lists the strategies the engine has registered (`/ui/api/backtest/
//! strategies`) and opens the matching `.py` in the editor when a row
//! is clicked. It is a light navigator, not a control panel: deploy and
//! run state live in the Schedules and Runs surfaces. With no engine
//! reachable it says so plainly instead of pretending to have a list.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, SharedString,
    Task, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
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

#[derive(Clone)]
struct StrategyRow {
    /// The class or function name — the last segment of the module path.
    name: SharedString,
    /// The full dotted module path (e.g. `strategies.example_ma.MACrossover`).
    path: SharedString,
    /// First line of the docstring, if any.
    doc: SharedString,
    bundled: bool,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

pub struct StrategiesPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    strategies: Vec<StrategyRow>,
    status: Status,
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
                cx.observe_global::<auracle_connect::ConnectGeneration>(
                    |this: &mut Self, cx| {
                        this.status = Status::Loading;
                        this.strategies.clear();
                        cx.notify();
                    },
                )
                .detach();
                let poll = Self::spawn_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    workspace: handle,
                    strategies: Vec::new(),
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
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_strategies(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            FetchResult::NotConnected => {
                                this.status = Status::NotConnected;
                                this.strategies.clear();
                            }
                            FetchResult::Unreachable => {
                                this.status = Status::Unreachable;
                            }
                            FetchResult::Ok(items) => {
                                this.status = Status::Connected;
                                this.strategies = items;
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
            return;
        };
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
}

/// `strategies.example_ma.MACrossover` -> `strategies/example_ma.py`.
fn module_to_relpath(module_path: &str) -> Option<String> {
    let mut parts: Vec<&str> = module_path.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    parts.pop(); // drop the class / function name, leaving the module
    Some(format!("{}.py", parts.join("/")))
}

enum FetchResult {
    NotConnected,
    Unreachable,
    Ok(Vec<StrategyRow>),
}

async fn fetch_strategies(http: Arc<dyn http_client::HttpClient>) -> FetchResult {
    let config = auracle_connect::load_config();
    let Some(key) = config.api_key.filter(|k| !k.trim().is_empty()) else {
        return FetchResult::NotConnected;
    };
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let attempt: Result<Vec<StrategyRow>> = async {
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
        let mut out = Vec::new();
        if let Some(items) = value.get("strategies").and_then(|v| v.as_array()) {
            for it in items {
                let path = it.get("path").and_then(|v| v.as_str()).unwrap_or_default();
                if path.is_empty() {
                    continue;
                }
                let name = path.rsplit('.').next().unwrap_or(path);
                let doc = it
                    .get("doc")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default();
                out.push(StrategyRow {
                    name: SharedString::from(name.to_string()),
                    path: SharedString::from(path.to_string()),
                    doc: SharedString::from(doc.to_string()),
                    bundled: it.get("bundled").and_then(|v| v.as_bool()).unwrap_or(false),
                });
            }
        }
        // User-written strategies first, bundled examples after, then by name.
        out.sort_by(|a, b| a.bundled.cmp(&b.bundled).then(a.name.cmp(&b.name)));
        Ok(out)
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

        let body: AnyElement = match self.status {
            Status::NotConnected => v_flex()
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
                .into_any_element(),
            Status::Loading => v_flex()
                .p_3()
                .child(Label::new("Loading…").color(Color::Muted))
                .into_any_element(),
            Status::Unreachable => v_flex()
                .p_3()
                .child(
                    Label::new("Your engine didn't answer. It may be stopped.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected if self.strategies.is_empty() => v_flex()
                .p_3()
                .child(
                    Label::new("No strategies yet. Create one under strategies/ and it shows up here.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Status::Connected => v_flex()
                .id("strategies-scroll")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_0p5()
                .children(self.strategies.iter().enumerate().map(|(ix, row)| {
                    let path = row.path.clone();
                    let doc = row.doc.clone();
                    h_flex()
                        .id(("strategy-row", ix))
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_start()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(cx.theme().colors().element_hover))
                        .on_click(cx.listener(move |this, _ev, window, cx| {
                            this.open_strategy(path.clone(), window, cx);
                        }))
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .child(Label::new(row.name.clone()).size(LabelSize::Small))
                                        .when(row.bundled, |s| {
                                            s.child(
                                                Label::new("example")
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                )
                                .when(!doc.is_empty(), |s| {
                                    s.child(
                                        Label::new(doc)
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        )
                }))
                .into_any_element(),
        };

        v_flex()
            .key_context("StrategiesPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}
