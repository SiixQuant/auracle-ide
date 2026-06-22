//! Validation rail — does this idea hold up, in plain words.
//!
//! Two views. First you pick a strategy from the engine's list; then
//! the rail shows the overfit checks as traffic lights — green
//! "looks healthy", red "needs attention", or an honest "couldn't be
//! checked this run". Expanding a row reveals the engine's plain
//! "what this means" and "what usually fixes it". The rail renders the
//! engine's verdict; it never invents a tier or a number.
//!
//! Both lists route their fetch outcome through the shared
//! [`auracle_view_state`] seam: the strategy picker and the verdict each
//! become a thin `match` over [`ViewState`], so neither can silently skip a
//! designed loading skeleton, empty hint, or retryable error. The severity
//! and verdict decisions live in the gpui-free [`auracle_validation`] reducer;
//! this module maps a [`Severity`] to a theme [`Color`] and nothing more.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use auracle_strategies::{StrategyListItem, strategy_rows};
use auracle_validation::{Severity, SignalRow, Verdict, signal_row, verdict, verdict_is_empty};
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
    validation_rail,
    [
        /// Toggle focus on the validation rail.
        ToggleFocus
    ]
);

const POLL_EVERY: Duration = Duration::from_secs(60);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ValidationRail>(window, cx);
        });
    })
    .detach();
}

pub struct ValidationRail {
    focus_handle: FocusHandle,
    /// `true` until a connection (an engine API key) exists. This is a pre-fetch
    /// state, not an engine failure, so it is kept outside the `Load` seam: a
    /// disconnected rail must offer Connect, never a false "retry the engine".
    connected: bool,
    /// The strategy picker list, behind the shared fetch seam.
    strategies: Load<Vec<StrategyListItem>>,
    selected: Option<SharedString>,
    /// The verdict for the selected strategy, behind the shared fetch seam.
    verdict: Load<Verdict>,
    /// Signal rows whose plain/fix detail is expanded.
    expanded: HashSet<SharedString>,
    _poll: Task<()>,
}

impl ValidationRail {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| {
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.connected = auracle_connect::load_config().api_key.is_some();
                    this.strategies = Load::Pending;
                    this.selected = None;
                    this.verdict = Load::Pending;
                    this.expanded.clear();
                    cx.notify();
                })
                .detach();
                let poll = Self::spawn_list_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    connected: auracle_connect::load_config().api_key.is_some(),
                    strategies: Load::Pending,
                    selected: None,
                    verdict: Load::Pending,
                    expanded: HashSet::new(),
                    _poll: poll,
                }
            })
        })
    }

    fn spawn_list_poll(cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = fetch_strategies(http.clone()).await;
                let ok = this
                    .update(cx, |this, cx| {
                        match fetched {
                            ListResult::NotConnected => {
                                this.connected = false;
                                this.strategies = Load::Pending;
                            }
                            // A poll failure replaces the picker with a designed,
                            // retryable error rather than silently holding stale
                            // rows — staleness must be honest (rubric item 3).
                            ListResult::Unreachable => {
                                this.connected = true;
                                this.strategies = Load::Failed(
                                    "Your engine didn't answer. It may be stopped.".into(),
                                );
                            }
                            ListResult::Ok(items) => {
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

    fn select(&mut self, path: SharedString, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        self.verdict = Load::Pending;
        self.expanded.clear();
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = fetch_validation(http, path.clone()).await;
            this.update(cx, |this, cx| {
                if this.selected.as_ref() != Some(&path) {
                    return; // selection changed mid-flight
                }
                this.verdict = match result {
                    Ok(v) => Load::Done(v),
                    Err(message) => Load::Failed(message.to_string()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Map a reducer [`Severity`] to the theme colour the rail renders it in. Only
/// theme `Color::*` — never a colour literal — mirroring
/// `account_setup::tone_color` so the rail tracks the theme.
fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Ok => Color::Success,
        Severity::Caution => Color::Warning,
        Severity::NeedsAttention => Color::Error,
        // Unknown / blank tier — couldn't be checked, said honestly.
        Severity::NotChecked => Color::Muted,
    }
}

/// A one-word severity marker so each row reads without relying on the dot's
/// colour — and so an unchecked row says "not checked" instead of looking
/// broken (the same glance-legibility law as the runway rail). Pairs the word
/// with the theme colour from [`severity_color`].
fn severity_word(severity: Severity) -> (&'static str, Color) {
    let word = match severity {
        Severity::Ok => "ok",
        Severity::Caution => "caution",
        Severity::NeedsAttention => "needs attention",
        Severity::NotChecked => "not checked",
    };
    (word, severity_color(severity))
}

enum ListResult {
    NotConnected,
    Unreachable,
    Ok(Vec<StrategyListItem>),
}

fn engine() -> Option<(String, String)> {
    let config = auracle_connect::load_config();
    let key = config.api_key.filter(|k| !k.trim().is_empty())?;
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    Some((url, key))
}

async fn get_json(
    http: Arc<dyn http_client::HttpClient>,
    path: String,
) -> Result<serde_json::Value> {
    let Some((url, key)) = engine() else {
        anyhow::bail!("not connected");
    };
    let request = http_client::http::Request::builder()
        .uri(format!("{url}{path}"))
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(request).await?;
    let status = response.status();
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    if !status.is_success() {
        // surface the engine's plain detail when present
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(detail) = v.get("detail").and_then(|d| d.as_str()) {
                anyhow::bail!("{detail}");
            }
        }
        anyhow::bail!("status {status}");
    }
    Ok(serde_json::from_str(&body)?)
}

async fn fetch_strategies(http: Arc<dyn http_client::HttpClient>) -> ListResult {
    if engine().is_none() {
        return ListResult::NotConnected;
    }
    match get_json(http, "/ui/api/backtest/strategies".into()).await {
        Ok(v) => {
            // The picker reads the same engine route as the strategies navigator,
            // so it builds rows with the same reducer (parse, derive name + first
            // doc line, drop empty paths, sort). The rail only needs `path`/`doc`
            // and ignores the extra fields.
            let raw: Vec<(String, String, bool)> = v
                .get("strategies")
                .and_then(|x| x.as_array())
                .map(|items| {
                    items
                        .iter()
                        .map(|it| {
                            (
                                it.get("path")
                                    .and_then(|p| p.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                it.get("doc")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                it.get("bundled").and_then(|b| b.as_bool()).unwrap_or(false),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            ListResult::Ok(strategy_rows(
                raw.iter().map(|(p, d, b)| (p.as_str(), d.as_str(), *b)),
            ))
        }
        Err(_) => ListResult::Unreachable,
    }
}

async fn fetch_validation(
    http: Arc<dyn http_client::HttpClient>,
    strategy_path: SharedString,
) -> Result<Verdict, SharedString> {
    let path = format!(
        "/ui/api/validation?strategy_path={}",
        urlencode(&strategy_path)
    );
    match get_json(http, path).await {
        Ok(v) => {
            let summary = v.get("plain").and_then(|p| p.as_str()).unwrap_or_default();
            let signals = v
                .get("signals")
                .and_then(|x| x.as_array())
                .map(|items| {
                    items
                        .iter()
                        .map(|it| {
                            signal_row(
                                str_field(it, "name"),
                                str_field(it, "tier"),
                                str_field(it, "plain"),
                                str_field(it, "what_usually_fixes_it"),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            // The reducer normalises a blank summary to `None` and never
            // fabricates text.
            Ok(verdict(summary, signals))
        }
        Err(e) => Err(SharedString::from(format!("{e}"))),
    }
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default()
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

impl EventEmitter<PanelEvent> for ValidationRail {}

impl Focusable for ValidationRail {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ValidationRail {
    fn persistent_name() -> &'static str {
        "ValidationRail"
    }
    fn panel_key() -> &'static str {
        "ValidationRail"
    }
    fn position(&self, _w: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }
    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right | DockPosition::Left)
    }
    fn set_position(&mut self, _p: DockPosition, _w: &mut Window, _cx: &mut Context<Self>) {}
    fn default_size(&self, _w: &Window, _cx: &App) -> Pixels {
        px(320.)
    }
    fn icon(&self, _w: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Check)
    }
    fn icon_tooltip(&self, _w: &Window, _cx: &App) -> Option<&'static str> {
        Some("Validation — does this idea hold up")
    }
    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }
    fn activation_priority(&self) -> u32 {
        14
    }
}

impl Render for ValidationRail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Label::new("VALIDATION")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("· Validate")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );

        // Disconnected is a pre-fetch state, kept outside the `Load` seam: it
        // isn't an engine failure, so it offers Connect rather than a Retry.
        let body: AnyElement = if !self.connected {
            render_not_connected()
        } else if let Some(selected) = self.selected.clone() {
            self.render_verdict(selected, cx)
        } else {
            self.render_picker(cx)
        };

        v_flex()
            .key_context("ValidationRail")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(header)
            .child(body)
    }
}

/// The disconnected pre-state: no engine key yet, so offer Connect (never a
/// false retryable error).
fn render_not_connected() -> AnyElement {
    v_flex()
        .p_3()
        .gap_2()
        .child(Label::new("Not connected to your Auracle engine yet."))
        .child(
            Button::new("validation-connect", "Connect…")
                .style(ButtonStyle::Filled)
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                }),
        )
        .into_any_element()
}

/// A designed skeleton — muted placeholder rows, not a bare "Checking…" label —
/// while a fetch is in flight (mirrors `account_setup::render_loading`).
fn render_loading() -> AnyElement {
    let skeleton_row = || {
        ListItem::new("validation-skeleton")
            .spacing(ListItemSpacing::Sparse)
            .child(
                Label::new("Checking…")
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

impl ValidationRail {
    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        // The picker is a thin match over the shared seam: a designed loading
        // skeleton, an honest empty hint, a retryable error, then the rows.
        let state = self
            .strategies
            .clone()
            .into_list_view("No strategies yet. Build one first, then come back to check it.");

        match state {
            ViewState::Loading => render_loading(),
            ViewState::Empty { hint } => render_empty(&hint),
            ViewState::Error { message, retryable } => {
                self.render_picker_error(&message, retryable, cx)
            }
            ViewState::Ready(strategies) => v_flex()
                .id("validation-picker")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_0p5()
                .child(
                    Label::new("Pick a strategy to check:")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .children(strategies.into_iter().enumerate().map(|(index, item)| {
                    let path = SharedString::from(item.path);
                    let click_path = path.clone();
                    let doc = item.doc;
                    ListItem::new(("validation-pick", index))
                        .spacing(ListItemSpacing::Sparse)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select(click_path.clone(), cx);
                        }))
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(Label::new(path).size(LabelSize::Small))
                                .when(!doc.is_empty(), |row| {
                                    row.child(
                                        Label::new(SharedString::from(doc))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        )
                }))
                .into_any_element(),
        }
    }

    /// An honest, retryable error for the picker fetch — re-runs the next poll
    /// by re-fetching the list once (mirrors `account_setup::render_error`).
    fn render_picker_error(
        &self,
        message: &str,
        retryable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                    Button::new("validation-picker-retry", "Retry")
                        .style(ButtonStyle::Outlined)
                        .start_icon(
                            Icon::new(IconName::RotateCcw)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.strategies = Load::Pending;
                            cx.notify();
                            let http = cx.http_client();
                            cx.spawn(async move |this: WeakEntity<Self>, cx| {
                                let fetched = fetch_strategies(http).await;
                                this.update(cx, |this, cx| {
                                    this.strategies = match fetched {
                                        ListResult::NotConnected => {
                                            this.connected = false;
                                            Load::Pending
                                        }
                                        ListResult::Unreachable => Load::Failed(
                                            "Your engine didn't answer. It may be stopped.".into(),
                                        ),
                                        ListResult::Ok(items) => Load::Done(items),
                                    };
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        })),
                )
            })
            .into_any_element()
    }

    fn render_verdict(&self, selected: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let back = h_flex().px_2().py_1().gap_2().child(
            Button::new("validation-back", "← change strategy")
                .style(ButtonStyle::Subtle)
                .label_size(LabelSize::XSmall)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selected = None;
                    this.verdict = Load::Pending;
                    this.expanded.clear();
                    cx.notify();
                })),
        );

        let head = div().px_2().child(
            Label::new(selected.clone())
                .size(LabelSize::Small)
                .color(Color::Muted),
        );

        // The verdict is a thin match over the shared seam. A verdict with no
        // signals is "empty" regardless of summary (`verdict_is_empty`), so the
        // empty hint covers the "no checks for this strategy" case.
        let state = self.verdict.clone().into_view(
            verdict_is_empty,
            "The engine returned no checks for this strategy yet.",
        );

        let body = match state {
            ViewState::Loading => render_loading(),
            ViewState::Empty { hint } => render_empty(&hint),
            ViewState::Error { message, retryable } => {
                self.render_verdict_error(selected, &message, retryable, cx)
            }
            ViewState::Ready(verdict) => self.render_verdict_rows(&verdict, cx),
        };

        v_flex()
            .size_full()
            .child(back)
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// An honest, retryable error for the verdict fetch — re-runs the same
    /// measure by re-invoking `select(path)`.
    fn render_verdict_error(
        &self,
        selected: SharedString,
        message: &str,
        retryable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .p_3()
            .gap_2()
            .child(Label::new("Couldn't check this strategy.").color(Color::Error))
            .child(
                Label::new(message.to_string())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .when(retryable, |this| {
                let retry_path = selected.clone();
                this.child(
                    Button::new("validation-verdict-retry", "Retry")
                        .style(ButtonStyle::Outlined)
                        .start_icon(
                            Icon::new(IconName::RotateCcw)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.select(retry_path.clone(), cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn render_verdict_rows(&self, verdict: &Verdict, cx: &mut Context<Self>) -> AnyElement {
        let mut col = v_flex().size_full();

        // The reducer omits a blank summary, so a present summary is real text.
        if let Some(summary) = &verdict.summary {
            col = col.child(
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new(summary.clone()).size(LabelSize::Small)),
            );
        }

        let rows = v_flex()
            .id("validation-signals")
            .size_full()
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .children(
                verdict
                    .signals
                    .iter()
                    .enumerate()
                    .map(|(index, signal)| self.render_signal_row(index, signal, cx)),
            );

        col.child(rows).into_any_element()
    }

    /// One signal as a native `ListItem`: a status dot in the start slot, the
    /// engine's check name, the one-word severity marker, and a disclosure
    /// chevron. Clicking the row (or its chevron) expands the engine's plain /
    /// fix text beneath it.
    fn render_signal_row(
        &self,
        index: usize,
        signal: &SignalRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = SharedString::from(signal.name.clone());
        let is_open = self.expanded.contains(&key);
        let dot_color = severity_color(signal.severity);
        let (word, word_color) = severity_word(signal.severity);
        let toggle_key = key.clone();

        let row = ListItem::new(("validation-signal", index))
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .toggle(Some(is_open))
            .on_toggle(cx.listener({
                let key = toggle_key.clone();
                move |this, _, _, cx| {
                    if !this.expanded.remove(&key) {
                        this.expanded.insert(key.clone());
                    }
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&toggle_key) {
                    this.expanded.insert(toggle_key.clone());
                }
                cx.notify();
            }))
            .start_slot(Indicator::dot().color(dot_color))
            .child(Label::new(key).size(LabelSize::Small))
            .end_slot(Label::new(word).size(LabelSize::XSmall).color(word_color));

        let detail = is_open.then(|| {
            v_flex()
                .pl_4()
                .pb_1()
                .gap_1()
                .when(!signal.plain.is_empty(), |c| {
                    c.child(
                        Label::new(SharedString::from(signal.plain.clone()))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                })
                .when(!signal.fix.is_empty(), |c| {
                    c.child(
                        Label::new(format!("Usually fixed by: {}", signal.fix))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                })
        });

        v_flex()
            .child(row)
            .when_some(detail, |this, detail| this.child(detail))
            .into_any_element()
    }
}
