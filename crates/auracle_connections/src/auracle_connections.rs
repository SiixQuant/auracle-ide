//! The guided broker-connect wizard — one reusable component.
//!
//! A three-step flow (choose broker → enter credentials → confirm
//! capabilities) rendered natively in GPUI. It is introspection-driven:
//! the broker list, the credential FIELDS, and the data/paper/live +
//! asset-kind capability chips all arrive as plain JSON from the engine,
//! so adding a broker engine-side needs no change here.
//!
//! Honesty laws baked in:
//!   * capability chips come ONLY from the engine's unified capability
//!     endpoint — a broker the engine hasn't verified shows a
//!     "not verified yet" banner, never blank chips that read as
//!     "anything goes";
//!   * a tri-state "unknown" renders as "not verified yet", never green;
//!   * sensitive field VALUES are never fetched or shown — the engine
//!     returns only a masked preview, and the wizard never logs a body.
//!
//! The same component serves Settings (Browse) and, later, the deploy
//! gate (Scoped to a strategy's broker) via [`WizardScope`]. Transport +
//! auth reuse [`auracle_connect`] (loopback engine_url + api_key).

use std::sync::Arc;

use anyhow::Result;
use auracle_connect::{AuracleConfig, ConnectGeneration, load_config};
use futures::AsyncReadExt as _;
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Task, Window,
    actions,
};
use serde::Deserialize;
use ui::prelude::*;
use workspace::{ModalView, Workspace};

actions!(
    auracle,
    [
        /// Open the broker connection wizard.
        OpenBrokerWizard
    ]
);

const MAX_FIELDS: usize = 10;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenBrokerWizard, window, cx| {
            workspace.toggle_modal(window, cx, |window, cx| BrokerWizard::new(window, cx));
        });
    })
    .detach();
}

// ── Wizard scope: one component, two mount points ────────────────────

/// Where the wizard is mounted. `Browse` is the Settings home (full
/// broker grid); `Scoped` is the deploy-time gate, opened straight to the
/// broker a strategy needs with the asset kinds it must satisfy.
/// `Scoped` is wired by the deploy-mount increment.
#[allow(dead_code)]
#[derive(Clone)]
pub enum WizardScope {
    Browse,
    Scoped {
        broker: String,
        required_kinds: Vec<String>,
    },
}

// ── Engine JSON shapes (introspection-driven) ────────────────────────

#[derive(Clone, Deserialize, Default)]
struct BrokerSummary {
    id: String,
    #[serde(default)]
    display_label: String,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    status: String,
}

#[derive(Clone, Deserialize, Default)]
struct FieldMeta {
    name: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    has_value: bool,
    #[serde(default)]
    preview: String,
}

#[derive(Clone, Deserialize, Default)]
struct Capability {
    #[serde(default)]
    capabilities: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    asset_kinds: Vec<String>,
    #[serde(default)]
    unsupported: Vec<String>,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    error: Option<String>,
}

// ── Wizard entity ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Step {
    ChooseBroker,
    Credentials,
    Confirm,
}

enum TestState {
    Idle,
    Testing,
    Verdict { ok: bool, plain: SharedString },
}

pub struct BrokerWizard {
    focus_handle: FocusHandle,
    scope: WizardScope,
    step: Step,
    brokers: Vec<BrokerSummary>,
    selected: Option<String>,
    fields: Vec<FieldMeta>,
    /// Fixed pool of single-line editors bound to `fields` by index, so
    /// no entity is created after `new()` (which would need a `Window`
    /// the async loaders don't carry). Editors past `fields.len()` go
    /// unrendered.
    editors: Vec<Entity<editor::Editor>>,
    capability: Option<Capability>,
    state: TestState,
    _task: Option<Task<()>>,
}

impl BrokerWizard {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut editors = Vec::with_capacity(MAX_FIELDS);
        for _ in 0..MAX_FIELDS {
            editors.push(cx.new(|cx| editor::Editor::single_line(window, cx)));
        }
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            scope: WizardScope::Browse,
            step: Step::ChooseBroker,
            brokers: Vec::new(),
            selected: None,
            fields: Vec::new(),
            editors,
            capability: None,
            state: TestState::Idle,
            _task: None,
        };
        this.load_brokers(cx);
        this
    }

    fn load_brokers(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._task = Some(cx.spawn(async move |this, cx| {
            let brokers = list_brokers(http).await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.brokers = brokers;
                cx.notify();
            })
            .ok();
        }));
    }

    fn select_broker(&mut self, broker: String, cx: &mut Context<Self>) {
        self.selected = Some(broker.clone());
        self.step = Step::Credentials;
        self.state = TestState::Idle;
        self.fields = Vec::new();
        self.capability = None;
        cx.notify();
        let http = cx.http_client();
        self._task = Some(cx.spawn(async move |this, cx| {
            let fields = get_fields(http.clone(), &broker).await.unwrap_or_default();
            let capability = get_capability(http, &broker).await.ok();
            this.update(cx, |this, cx| {
                this.fields = fields;
                this.capability = capability;
                cx.notify();
            })
            .ok();
        }));
    }

    fn current_body(&self, cx: &App) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (i, field) in self.fields.iter().enumerate() {
            if i >= self.editors.len() {
                break;
            }
            let value = self.editors[i].read(cx).text(cx);
            // Skip empty inputs so an unchanged "(saved)" sensitive field
            // isn't overwritten with blank.
            if !value.is_empty() {
                map.insert(field.name.clone(), serde_json::Value::String(value));
            }
        }
        serde_json::Value::Object(map)
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let Some(broker) = self.selected.clone() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.state = TestState::Testing;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = test_connection(http, &broker, body).await;
            this.update(cx, |this, cx| {
                this.state = TestState::Verdict {
                    ok: result.is_ok(),
                    plain: SharedString::from(match result {
                        Ok(message) => message,
                        Err(error) => format!("Couldn't connect: {error}."),
                    }),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(broker) = self.selected.clone() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.state = TestState::Testing;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = save_connection(http, &broker, body).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let generation = cx.global::<ConnectGeneration>().0 + 1;
                        cx.set_global(ConnectGeneration(generation));
                        cx.emit(DismissEvent);
                    }
                    Err(error) => {
                        this.state = TestState::Verdict {
                            ok: false,
                            plain: SharedString::from(format!("Couldn't save: {error}.")),
                        };
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }
}

// ── Engine client (loopback; dual auth headers; CSRF double-submit) ───

fn config() -> AuracleConfig {
    load_config()
}

fn base_url(config: &AuracleConfig) -> String {
    config
        .engine_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:1969".into())
}

async fn get_json(
    http: Arc<dyn http_client::HttpClient>,
    path: &str,
) -> Result<serde_json::Value> {
    let config = config();
    let key = config.api_key.clone().unwrap_or_default();
    let request = http_client::http::Request::builder()
        .uri(format!("{}{path}", base_url(&config)))
        .header("X-API-Key", key.clone())
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("engine answered with status {}", response.status());
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text)?)
}

async fn list_brokers(http: Arc<dyn http_client::HttpClient>) -> Result<Vec<BrokerSummary>> {
    let value = get_json(http, "/ui/api/connections").await?;
    let list = value
        .get("connections")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

async fn get_fields(http: Arc<dyn http_client::HttpClient>, broker: &str) -> Result<Vec<FieldMeta>> {
    let value = get_json(http, &format!("/ui/api/connections/{broker}")).await?;
    let list = value
        .get("fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

async fn get_capability(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
) -> Result<Capability> {
    let value = get_json(http, &format!("/ui/api/connections/{broker}/capability")).await?;
    Ok(serde_json::from_value(value)?)
}

/// Fetch the double-submit CSRF token: GET a /ui page so the engine
/// issues an `auracle_csrf` cookie, then return its value to echo back as
/// the `X-CSRF-Token` header on the mutation (the engine's CSRF
/// middleware requires the cookie and header to match).
async fn fetch_csrf(http: Arc<dyn http_client::HttpClient>, config: &AuracleConfig) -> String {
    let key = config.api_key.clone().unwrap_or_default();
    let Ok(request) = http_client::http::Request::builder()
        .uri(format!("{}/ui/account", base_url(config)))
        .header("X-API-Key", key.clone())
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

async fn post_json(
    http: Arc<dyn http_client::HttpClient>,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    let config = config();
    let key = config.api_key.clone().unwrap_or_default();
    let csrf = fetch_csrf(http.clone(), &config).await;
    let payload = serde_json::to_string(&body)?;
    let request = http_client::http::Request::builder()
        .method("POST")
        .uri(format!("{}{path}", base_url(&config)))
        .header("Content-Type", "application/json")
        .header("X-API-Key", key.clone())
        .header("X-CSRF-Token", csrf.clone())
        .header("Cookie", format!("auracle_session={key}; auracle_csrf={csrf}"))
        .body(http_client::AsyncBody::from(payload))?;
    let mut response = http.send(request).await?;
    if !response.status().is_success() {
        anyhow::bail!("engine answered with status {}", response.status());
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

async fn test_connection(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
    body: serde_json::Value,
) -> Result<String> {
    let value = post_json(http, &format!("/ui/api/connections/{broker}/test"), body).await?;
    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if ok {
        Ok(if message.is_empty() {
            "Connected — the engine accepted these credentials.".into()
        } else {
            message
        })
    } else {
        anyhow::bail!(if message.is_empty() {
            "the engine did not accept these credentials".into()
        } else {
            message
        })
    }
}

async fn save_connection(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
    body: serde_json::Value,
) -> Result<()> {
    post_json(http, &format!("/ui/api/connections/{broker}/save"), body).await?;
    Ok(())
}

// ── Render ───────────────────────────────────────────────────────────

impl EventEmitter<DismissEvent> for BrokerWizard {}

impl Focusable for BrokerWizard {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for BrokerWizard {}

impl BrokerWizard {
    fn render_choose(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex().gap_2();
        if self.brokers.is_empty() {
            list = list.child(
                Label::new("Loading brokers…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }
        for broker in self.brokers.clone() {
            let id = broker.id.clone();
            let title = if broker.display_label.is_empty() {
                broker.id.clone()
            } else {
                broker.display_label.clone()
            };
            let connected = broker.status == "connected";
            list = list.child(
                Button::new(SharedString::from(broker.id.clone()), title)
                    .style(if connected {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Outlined
                    })
                    .full_width()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_broker(id.clone(), cx);
                    })),
            );
        }
        v_flex()
            .gap_2()
            .child(Label::new("Choose a broker to connect"))
            .child(list)
    }

    fn render_credentials(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut form = v_flex().gap_3();
        if self.fields.is_empty() {
            form = form.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }
        for (i, field) in self.fields.iter().enumerate() {
            if i >= self.editors.len() {
                break;
            }
            let mut label = if field.label.is_empty() {
                field.name.clone()
            } else {
                field.label.clone()
            };
            if field.has_value {
                label = format!("{label}  (saved — leave blank to keep)");
            } else if !field.required {
                label = format!("{label}  (optional)");
            }
            form = form.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(label)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(self.editors[i].clone()),
            );
        }
        form
    }

    fn render_confirm(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_2().child(Label::new("What this broker can do"));
        match &self.capability {
            None => {
                body = body.child(
                    Label::new("Checking capabilities…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
            }
            Some(cap) if cap.capabilities.is_empty() => {
                // Honesty: no verified capability row — never blank chips
                // that read as "anything goes".
                body = body.child(
                    Label::new(if cap.reason.is_empty() {
                        "Capabilities aren't verified for this broker yet.".to_string()
                    } else {
                        cap.reason.clone()
                    })
                    .size(LabelSize::Small)
                    .color(Color::Warning),
                );
            }
            Some(cap) => {
                let mut chips = h_flex().gap_2();
                for leg in ["data", "paper", "live"] {
                    let state = cap.capabilities.get(leg).map(String::as_str).unwrap_or("unknown");
                    let (text, color) = match state {
                        "yes" => (format!("{leg}: yes"), Color::Success),
                        "no" => (format!("{leg}: no"), Color::Muted),
                        _ => (format!("{leg}: not verified yet"), Color::Warning),
                    };
                    chips = chips.child(Label::new(text).size(LabelSize::Small).color(color));
                }
                body = body.child(chips);
                if !cap.asset_kinds.is_empty() {
                    body = body.child(
                        Label::new(format!("Trades: {}", cap.asset_kinds.join(", ")))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                }
                if let Some(error) = &cap.error {
                    body = body.child(
                        Label::new(format!("Couldn't fully verify: {error}"))
                            .size(LabelSize::Small)
                            .color(Color::Warning),
                    );
                } else if !cap.ok && !cap.reason.is_empty() {
                    body = body.child(
                        Label::new(cap.reason.clone())
                            .size(LabelSize::Small)
                            .color(Color::Warning),
                    );
                }
            }
        }
        body
    }
}

impl Render for BrokerWizard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let verdict: Option<(Color, SharedString)> = match &self.state {
            TestState::Idle => None,
            TestState::Testing => Some((Color::Muted, "Working…".into())),
            TestState::Verdict { ok, plain } => Some((
                if *ok { Color::Success } else { Color::Error },
                plain.clone(),
            )),
        };
        let step = self.step;
        let has_selection = self.selected.is_some();

        v_flex()
            .key_context("BrokerWizard")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| {
                cx.emit(DismissEvent);
            }))
            .w(rems(34.))
            .p_4()
            .gap_3()
            .bg(cx.theme().colors().elevated_surface_background)
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new("Connect a broker").size(LabelSize::Large))
            .map(|this| match step {
                Step::ChooseBroker => this.child(self.render_choose(cx)),
                Step::Credentials => this.child(self.render_credentials(cx)),
                Step::Confirm => this.child(self.render_confirm(cx)),
            })
            .when_some(verdict, |this, (color, plain)| {
                this.child(Label::new(plain).size(LabelSize::Small).color(color))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(step != Step::ChooseBroker, |row| {
                        row.child(
                            Button::new("wiz-back", "Back")
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step = match this.step {
                                        Step::Confirm => Step::Credentials,
                                        _ => Step::ChooseBroker,
                                    };
                                    cx.notify();
                                })),
                        )
                    })
                    .when(step == Step::Credentials && has_selection, |row| {
                        row.child(
                            Button::new("wiz-test", "Test")
                                .style(ButtonStyle::Outlined)
                                .on_click(cx.listener(|this, _, _, cx| this.run_test(cx))),
                        )
                        .child(
                            Button::new("wiz-next", "Next")
                                .style(ButtonStyle::Outlined)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step = Step::Confirm;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(step == Step::Confirm && has_selection, |row| {
                        row.child(
                            Button::new("wiz-save", "Connect")
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                    }),
            )
    }
}
