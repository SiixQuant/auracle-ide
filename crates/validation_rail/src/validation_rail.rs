//! Validation rail — does this idea hold up, in plain words.
//!
//! Two views. First you pick a strategy from the engine's list; then
//! the rail shows the seven overfit checks as traffic lights — green
//! "looks healthy", red "needs attention", or an honest "couldn't be
//! checked this run". Expanding a row reveals the engine's plain
//! "what this means" and "what usually fixes it". The rail renders the
//! engine's verdict; it never invents a tier or a number.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Pixels,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use std::collections::HashSet;
use ui::prelude::*;
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

#[derive(Clone)]
struct Strategy {
    path: SharedString,
    doc: SharedString,
}

#[derive(Clone)]
struct Signal {
    name: SharedString,
    tier: SharedString,
    plain: SharedString,
    fix: SharedString,
}

#[derive(Clone)]
struct Verdict {
    summary: SharedString,
    signals: Vec<Signal>,
}

#[derive(Clone, PartialEq)]
enum Conn {
    NotConnected,
    Loading,
    Ready,
    Unreachable,
}

pub struct ValidationRail {
    focus_handle: FocusHandle,
    conn: Conn,
    strategies: Vec<Strategy>,
    selected: Option<SharedString>,
    verdict: Option<Verdict>,
    measuring: bool,
    measure_error: Option<SharedString>,
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
                cx.observe_global::<auracle_connect::ConnectGeneration>(
                    |this: &mut Self, cx| {
                        this.conn = Conn::Loading;
                        this.strategies.clear();
                        this.selected = None;
                        this.verdict = None;
                        this.measure_error = None;
                        cx.notify();
                    },
                )
                .detach();
                let poll = Self::spawn_list_poll(cx);
                Self {
                    focus_handle: cx.focus_handle(),
                    conn: if auracle_connect::load_config().api_key.is_some() {
                        Conn::Loading
                    } else {
                        Conn::NotConnected
                    },
                    strategies: Vec::new(),
                    selected: None,
                    verdict: None,
                    measuring: false,
                    measure_error: None,
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
                                this.conn = Conn::NotConnected;
                                this.strategies.clear();
                            }
                            ListResult::Unreachable => {
                                if this.strategies.is_empty() {
                                    this.conn = Conn::Unreachable;
                                }
                            }
                            ListResult::Ok(items) => {
                                this.conn = Conn::Ready;
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

    fn select(&mut self, path: SharedString, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        self.verdict = None;
        self.measure_error = None;
        self.measuring = true;
        self.expanded.clear();
        cx.notify();
        let http = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = fetch_validation(http, path.clone()).await;
            this.update(cx, |this, cx| {
                if this.selected.as_ref() != Some(&path) {
                    return; // selection changed mid-flight
                }
                this.measuring = false;
                match result {
                    Ok(v) => {
                        this.verdict = Some(v);
                        this.measure_error = None;
                    }
                    Err(msg) => {
                        this.verdict = None;
                        this.measure_error = Some(msg);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn tier_color(&self, tier: &str, cx: &App) -> Hsla {
        let s = cx.theme().status();
        match tier {
            "green" => s.success,
            "red" => s.error,
            "amber" | "warning" => s.warning,
            _ => s.ignored, // unknown — couldn't be checked
        }
    }

    /// A one-word tier marker so each row reads without relying on the
    /// dot's color — and so an unknown row says "not checked" instead
    /// of looking broken (the same glance-legibility law as the
    /// runway rail).
    fn tier_word(&self, tier: &str) -> (&'static str, Color) {
        match tier {
            "green" => ("ok", Color::Success),
            "red" => ("needs attention", Color::Error),
            "amber" | "warning" => ("caution", Color::Warning),
            _ => ("not checked", Color::Muted),
        }
    }
}

enum ListResult {
    NotConnected,
    Unreachable,
    Ok(Vec<Strategy>),
}

fn engine() -> Option<(String, String)> {
    let config = auracle_connect::load_config();
    let key = config.api_key.filter(|k| !k.trim().is_empty())?;
    let url = config
        .engine_url
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    Some((url.to_string(), key.to_string()))
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
            let mut out = Vec::new();
            if let Some(items) = v.get("strategies").and_then(|x| x.as_array()) {
                for it in items {
                    if let Some(path) = it.get("path").and_then(|p| p.as_str()) {
                        out.push(Strategy {
                            path: SharedString::from(path.to_string()),
                            doc: SharedString::from(
                                it.get("doc")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                        });
                    }
                }
            }
            ListResult::Ok(out)
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
            let summary = v
                .get("plain")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string();
            let mut signals = Vec::new();
            if let Some(items) = v.get("signals").and_then(|x| x.as_array()) {
                for it in items {
                    signals.push(Signal {
                        name: field(it, "name"),
                        tier: field(it, "tier"),
                        plain: field(it, "plain"),
                        fix: field(it, "what_usually_fixes_it"),
                    });
                }
            }
            Ok(Verdict {
                summary: SharedString::from(summary),
                signals,
            })
        }
        Err(e) => Err(SharedString::from(format!("{e}"))),
    }
}

fn field(v: &serde_json::Value, key: &str) -> SharedString {
    SharedString::from(v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_string())
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

        let body: AnyElement = match self.conn {
            Conn::NotConnected => v_flex()
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
                .into_any_element(),
            Conn::Loading => v_flex()
                .p_3()
                .child(Label::new("Checking…").color(Color::Muted))
                .into_any_element(),
            Conn::Unreachable => v_flex()
                .p_3()
                .child(
                    Label::new("Your engine didn't answer. It may be stopped.")
                        .color(Color::Muted),
                )
                .into_any_element(),
            Conn::Ready => {
                if let Some(selected) = self.selected.clone() {
                    self.render_verdict(selected, cx).into_any_element()
                } else {
                    self.render_picker(cx).into_any_element()
                }
            }
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

impl ValidationRail {
    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.strategies.is_empty() {
            return v_flex()
                .p_3()
                .child(
                    Label::new(
                        "No strategies yet. Build one first, then come back to check it.",
                    )
                    .color(Color::Muted),
                )
                .into_any_element();
        }
        v_flex()
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
            .children(self.strategies.iter().map(|s| {
                let path = s.path.clone();
                v_flex()
                    .id(SharedString::from(format!("pick-{}", s.path)))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .hover(|st| st.bg(cx.theme().colors().ghost_element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(path.clone(), cx);
                    }))
                    .child(Label::new(s.path.clone()).size(LabelSize::Small))
                    .when(!s.doc.is_empty(), |row| {
                        row.child(
                            Label::new(s.doc.clone())
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_verdict(&self, selected: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let back = h_flex().px_2().py_1().gap_2().child(
            Button::new("validation-back", "← change strategy")
                .style(ButtonStyle::Subtle)
                .label_size(LabelSize::XSmall)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selected = None;
                    this.verdict = None;
                    this.measure_error = None;
                    cx.notify();
                })),
        );

        let mut col = v_flex().size_full().child(back).child(
            div().px_2().child(
                Label::new(selected.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            ),
        );

        if self.measuring {
            return col.child(
                div()
                    .p_3()
                    .child(Label::new("Checking this strategy…").color(Color::Muted)),
            );
        }
        if let Some(err) = &self.measure_error {
            return col.child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .child(Label::new("Couldn't check this strategy.").color(Color::Warning))
                    .child(Label::new(err.clone()).size(LabelSize::XSmall).color(Color::Muted)),
            );
        }
        if let Some(verdict) = &self.verdict {
            col = col.child(
                div().px_2().py_1().child(
                    Label::new(verdict.summary.clone()).size(LabelSize::Small),
                ),
            );
            let rows = v_flex()
                .id("validation-signals")
                .size_full()
                .overflow_y_scroll()
                .p_1()
                .gap_0p5()
                .children(verdict.signals.iter().map(|sig| {
                    let dot = self.tier_color(&sig.tier, cx);
                    let key = sig.name.clone();
                    let is_open = self.expanded.contains(&key);
                    v_flex()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .hover(|st| st.bg(cx.theme().colors().ghost_element_hover))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .id(SharedString::from(format!("sig-{}", sig.name)))
                                .on_click(cx.listener({
                                    let key = key.clone();
                                    move |this, _, _, cx| {
                                        if !this.expanded.remove(&key) {
                                            this.expanded.insert(key.clone());
                                        }
                                        cx.notify();
                                    }
                                }))
                                .child(div().size_2().rounded_full().flex_none().bg(dot))
                                .child(Label::new(sig.name.clone()).size(LabelSize::Small))
                                .child(div().flex_1())
                                .child({
                                    let (word, color) = self.tier_word(&sig.tier);
                                    Label::new(word).size(LabelSize::XSmall).color(color)
                                }),
                        )
                        .when(is_open, |row| {
                            row.child(
                                v_flex()
                                    .pl_4()
                                    .gap_1()
                                    .child(
                                        Label::new(sig.plain.clone())
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(!sig.fix.is_empty(), |c| {
                                        c.child(
                                            Label::new(format!(
                                                "Usually fixed by: {}",
                                                sig.fix
                                            ))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                        )
                                    }),
                            )
                        })
                }));
            return col.child(rows);
        }
        col.child(div().p_3().child(Label::new("…").color(Color::Muted)))
    }
}
