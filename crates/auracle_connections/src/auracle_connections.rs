//! The native "Connect a broker" settings sub-page.
//!
//! A three-step flow (choose broker → enter credentials → confirm
//! capabilities) rendered natively in GPUI as an inline settings sub-page —
//! it lives inside Zed's own settings chrome, alongside the Account and Data
//! sources sub-pages, not in a foreign workspace modal. It is
//! introspection-driven: the broker list, the credential FIELDS, and the
//! data/paper/live + asset-kind capability chips all arrive as plain JSON from
//! the engine, so adding a broker engine-side needs no change here.
//!
//! Honesty laws baked in:
//!   * capability chips come ONLY from the engine's unified capability
//!     endpoint — a broker the engine hasn't verified shows a
//!     "not verified yet" banner, never blank chips that read as
//!     "anything goes";
//!   * a tri-state "unknown" renders as "not verified yet", never green;
//!   * sensitive field VALUES are never fetched or shown — the engine
//!     returns only a masked preview, and the page never logs a body.
//!
//! The page is hosted by the settings window (see [`BrokerWizard::new`]) and
//! reached either by navigating Connections → "Connect a broker" or via the
//! [`OpenBrokerWizard`] deep-link action (e.g. the deploy gate). Transport +
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
use workspace::Workspace;

actions!(
    auracle,
    [
        /// Open the broker connection page in Settings.
        OpenBrokerWizard,
        /// Open the Import-from-QuantConnect workspace tab.
        OpenQuantConnectImport
    ]
);

/// The `OpenSettingsAt` path of the native "Connect a broker" sub-page on the
/// Connections settings page. Shared so the deep-link action and the page
/// definition can't drift apart.
pub const BROKER_CONNECT_SETTINGS_PATH: &str = "connections.broker";

pub const MAX_FIELDS: usize = 10;

/// `OpenBrokerWizard` is kept as a stable deep-link entry point (the deploy
/// gate and the command palette both dispatch it), but it no longer opens a
/// foreign workspace modal. Instead it opens Settings deep-linked to the native
/// "Connect a broker" sub-page, so broker-connect lives entirely inside Zed's
/// own settings chrome like the Account and Data sources sub-pages beside it.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|_workspace, _: &OpenBrokerWizard, window, cx| {
            window.dispatch_action(
                Box::new(zed_actions::OpenSettingsAt {
                    path: BROKER_CONNECT_SETTINGS_PATH.to_string(),
                    target: None,
                }),
                cx,
            );
        });
    })
    .detach();
}

// ── Engine JSON shapes (introspection-driven) ────────────────────────

#[derive(Clone, Deserialize, Default)]
pub struct BrokerSummary {
    pub id: String,
    #[serde(default)]
    pub display_label: String,
    #[serde(default)]
    pub blurb: String,
    // The engine returns `status` as a NESTED object (ConnectionStatus.to_dict),
    // not a flat string: `{"state": "connected"|..., "detail": ..., "paper_mode": ...}`.
    // Modeling it as a struct keeps `serde_json::from_value` from failing on every
    // broker (which silently dropped them all in `list_brokers`).
    #[serde(default)]
    pub status: ConnStatus,
}

/// The nested `status` object the engine emits for each broker, mirroring
/// `ConnectionStatus.to_dict()` (auracle/brokers/base.py). Only the fields the
/// IDE consumes are modeled; the rest of the payload is ignored. The connected
/// sentinel is the inner `state == "connected"`.
#[derive(Clone, Deserialize, Default)]
pub struct ConnStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub paper_mode: Option<bool>,
}

#[derive(Clone, Deserialize, Default)]
pub struct FieldMeta {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub has_value: bool,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Deserialize, Default)]
pub struct Capability {
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub asset_kinds: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub error: Option<String>,
}

// ── Shared settings (cross-store sync source of truth) ───────────────

/// The engine's view of the shared settings, read over loopback from
/// `GET /ui/api/settings`. Only the fields the IDE consumes are modeled; the
/// rest of the payload is ignored. These are the "shared truths" the launcher
/// and IDE both reflect: which data keys are configured, which AI model the
/// operator designated, and the account tier.
#[derive(Clone, Deserialize, Default)]
pub struct SharedSettings {
    #[serde(default)]
    pub data_keys: std::collections::BTreeMap<String, DataKeyState>,
    #[serde(default)]
    pub ai_model: AiModelState,
    #[serde(default)]
    pub tier: String,
}

/// The signed-in operator's identity + tier/license, read over loopback from
/// `GET /ui/api/me`. Backs the Settings "Profile" section. Only the fields the
/// IDE renders are modeled; the rest of the payload is ignored.
#[derive(Clone, Deserialize, Default)]
pub struct Profile {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub tier_display: String,
    #[serde(default)]
    pub license: LicenseInfo,
}

/// License status for the Profile section — `status` is one of the engine's
/// honest states: community | active | expired | perpetual. `expires_at` /
/// `days_remaining` are null for community + perpetual (no billing cycle).
#[derive(Clone, Deserialize, Default)]
pub struct LicenseInfo {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub days_remaining: Option<i64>,
}

#[derive(Clone, Deserialize, Default)]
pub struct DataKeyState {
    #[serde(default)]
    pub configured: bool,
}

/// The engine's designated default AI model. `provider`/`model_id` name the
/// selection; `configured` reports whether the engine itself holds a usable
/// key for that provider. The plaintext key is NEVER in this payload — it is
/// fetched separately and only over loopback via [`fetch_ai_key`].
#[derive(Clone, Deserialize, Default)]
pub struct AiModelState {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub configured: bool,
}

// ── Wizard entity ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    ChooseBroker,
    Credentials,
    Confirm,
}

impl Step {
    /// The step reached by the "Next" affordance. `Confirm` is terminal — its
    /// forward action is "Connect", not another step — so it stays put.
    fn next(self) -> Step {
        match self {
            Step::ChooseBroker => Step::Credentials,
            Step::Credentials => Step::Confirm,
            Step::Confirm => Step::Confirm,
        }
    }

    /// The step reached by the "Back" affordance. `ChooseBroker` is the root and
    /// has no predecessor, so it stays put (Back is hidden there anyway).
    fn previous(self) -> Step {
        match self {
            Step::ChooseBroker => Step::ChooseBroker,
            Step::Credentials => Step::ChooseBroker,
            Step::Confirm => Step::Credentials,
        }
    }
}

/// The default selection for each `select` field: its first option. A paper/live
/// mode picker must always carry an explicit, valid choice — never an empty
/// value that the engine would have to guess at — so every `select` field is
/// seeded with its first listed option and every non-`select` field is omitted.
/// Pure (no editors, no `cx`) so the seeding rule is unit-tested directly.
pub fn default_selections(fields: &[FieldMeta]) -> std::collections::HashMap<String, String> {
    let mut selections = std::collections::HashMap::new();
    for field in fields {
        if field.kind == "select"
            && let Some(first) = field.options.first()
        {
            selections.insert(field.name.clone(), first.clone());
        }
    }
    selections
}

/// Build the JSON request body sent to the engine's `test`/`save` routes from the
/// field metadata, the chosen `select` options, and the plain text the user typed
/// into the non-`select` fields (keyed by field name).
///
/// The honesty/safety rules live here, gpui-free and tested:
///   * a `select` field contributes its chosen option (or nothing if, somehow,
///     no option was chosen) — never free text;
///   * a non-`select` field contributes its typed value ONLY when non-empty, so
///     leaving a saved sensitive field blank keeps the stored value rather than
///     overwriting it with an empty string;
///   * field order is irrelevant and unknown text entries (no matching field)
///     are ignored, so the body can only ever contain real, declared fields.
pub fn build_connection_body(
    fields: &[FieldMeta],
    selections: &std::collections::HashMap<String, String>,
    text_entries: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for field in fields {
        if field.kind == "select" {
            if let Some(choice) = selections.get(&field.name) {
                map.insert(
                    field.name.clone(),
                    serde_json::Value::String(choice.clone()),
                );
            }
            continue;
        }
        if let Some(value) = text_entries.get(&field.name)
            && !value.is_empty()
        {
            map.insert(field.name.clone(), serde_json::Value::String(value.clone()));
        }
    }
    serde_json::Value::Object(map)
}

enum TestState {
    Idle,
    Testing,
    Verdict { ok: bool, plain: SharedString },
}

/// The native "Connect a broker" page: an introspection-driven, three-step flow
/// (choose broker → enter credentials → confirm capabilities) rendered inline
/// inside Zed's settings chrome. It is hosted as a sub-page entity by the
/// settings window (mirroring the "Model providers" page), so its credential
/// editors keep focus across re-renders rather than living in a workspace modal.
pub struct BrokerWizard {
    focus_handle: FocusHandle,
    step: Step,
    brokers: Vec<BrokerSummary>,
    selected: Option<String>,
    fields: Vec<FieldMeta>,
    /// Fixed pool of single-line editors bound to `fields` by index, so
    /// no entity is created after `new()` (which would need a `Window`
    /// the async loaders don't carry). Editors past `fields.len()` go
    /// unrendered.
    editors: Vec<Entity<editor::Editor>>,
    /// Chosen option per `select` field (field name -> option). Select
    /// fields render as a segmented control, not a free-text editor — the
    /// paper/live mode picker must be a constrained choice, not freetext.
    selections: std::collections::HashMap<String, String>,
    capability: Option<Capability>,
    state: TestState,
    scroll_handle: gpui::ScrollHandle,
    _task: Option<Task<()>>,
}

impl BrokerWizard {
    /// Build the page entity (and its pool of credential editors). Called by the
    /// settings window when the "Connect a broker" sub-page is pushed, where a
    /// `Window` + `&mut App` are available, so the editors persist across renders.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut editors = Vec::with_capacity(MAX_FIELDS);
        for _ in 0..MAX_FIELDS {
            editors.push(cx.new(|cx| editor::Editor::single_line(window, cx)));
        }
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            step: Step::ChooseBroker,
            brokers: Vec::new(),
            selected: None,
            fields: Vec::new(),
            editors,
            selections: std::collections::HashMap::new(),
            capability: None,
            state: TestState::Idle,
            scroll_handle: gpui::ScrollHandle::new(),
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
                // Default each select field to its first option so a paper/
                // live choice is always explicit, never empty (pure rule).
                this.selections = default_selections(&fields);
                this.fields = fields;
                this.capability = capability;
                cx.notify();
            })
            .ok();
        }));
    }

    fn current_body(&self, cx: &App) -> serde_json::Value {
        // Read the per-field typed text out of the editors here (the only place
        // that needs `cx`), then hand the plain values to the pure body builder
        // so the skip-empty / select-only rules are unit-tested without gpui.
        let mut text_entries = std::collections::HashMap::new();
        for (i, field) in self.fields.iter().enumerate() {
            if field.kind == "select" {
                continue;
            }
            if let Some(editor) = self.editors.get(i) {
                text_entries.insert(field.name.clone(), editor.read(cx).text(cx));
            }
        }
        build_connection_body(&self.fields, &self.selections, &text_entries)
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
            this.update(cx, |this, cx| match result {
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

/// Build the failure message for a non-success engine response. The engine is
/// FastAPI, whose errors carry a user-actionable reason in a `{"detail": ...}`
/// body (e.g. a 404 "no DeepSeek key configured — set it in the launcher" or a
/// 409 VaultFailClosed message). We surface that detail so callers can show the
/// engine's own words instead of a generic "couldn't reach the engine"; when no
/// parseable detail is present we fall back to the bare status.
fn engine_error_message(status: http_client::StatusCode, body: &str) -> String {
    match engine_detail(body) {
        Some(detail) => format!("engine answered with status {status}: {detail}"),
        None => format!("engine answered with status {status}"),
    }
}

/// Pull FastAPI's `detail` out of an error body. A string detail is surfaced
/// verbatim; a structured detail (422 validation errors arrive as a list) is
/// rendered compactly rather than dropped. Returns `None` when the body isn't
/// JSON or carries no usable detail, so the caller falls back to the status.
fn engine_detail(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    match value.get("detail")? {
        serde_json::Value::String(detail) => {
            let detail = detail.trim();
            (!detail.is_empty()).then(|| detail.to_string())
        }
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Authenticated GET against a `/ui/api` route, returning parsed JSON. Made
/// `pub` so sibling surfaces (the native settings panel) can read engine
/// truths through the same loopback transport.
pub async fn get_json(
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
        let status = response.status();
        let mut body = String::new();
        // Best-effort read of the engine's error body so its `detail` reaches
        // the caller; a read failure still leaves the status to report.
        match response.body_mut().read_to_string(&mut body).await {
            Ok(_) => anyhow::bail!("{}", engine_error_message(status, &body)),
            Err(error) => {
                anyhow::bail!("engine answered with status {status} (could not read body: {error})")
            }
        }
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text)?)
}

pub async fn list_brokers(http: Arc<dyn http_client::HttpClient>) -> Result<Vec<BrokerSummary>> {
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

pub async fn get_fields(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
) -> Result<Vec<FieldMeta>> {
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

pub async fn get_capability(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
) -> Result<Capability> {
    let value = get_json(http, &format!("/ui/api/connections/{broker}/capability")).await?;
    Ok(serde_json::from_value(value)?)
}

/// Fetch the double-submit CSRF token: GET a `/ui/api` route so the engine
/// issues an `auracle_csrf` cookie, then return its value to echo back as
/// the `X-CSRF-Token` header on the mutation (the engine's CSRF middleware
/// requires the cookie and header to match). We hit `/ui/api/status` rather
/// than an HTML page so the cookie still flows under the headless web
/// profile, where portal pages 404 but the `/ui/api` surface stays served.
///
/// Made `pub` so the native settings surface can reuse the same authenticated,
/// CSRF-correct transport for the `/ui/api/settings` calls rather than
/// re-implementing the header dance.
pub async fn fetch_csrf(http: Arc<dyn http_client::HttpClient>, config: &AuracleConfig) -> String {
    let key = config.api_key.clone().unwrap_or_default();
    let Ok(request) = http_client::http::Request::builder()
        .uri(format!("{}/ui/api/status", base_url(config)))
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

/// Authenticated, CSRF-correct POST/PUT-style mutation against a `/ui/api`
/// route. Defaults to POST; callers that need PUT use [`send_json`]. Made
/// `pub` so the native settings panel can mirror the AI model up to the engine
/// through the same transport rather than re-deriving the header dance.
pub async fn post_json(
    http: Arc<dyn http_client::HttpClient>,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    send_json(http, "POST", path, body).await
}

/// The transport shared by [`post_json`] and the PUT-based settings writes.
/// `method` is the HTTP verb ("POST" or "PUT"); the engine's settings writes
/// require PUT, while connection saves/tests use POST.
pub async fn send_json(
    http: Arc<dyn http_client::HttpClient>,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    let config = config();
    let key = config.api_key.clone().unwrap_or_default();
    let csrf = fetch_csrf(http.clone(), &config).await;
    let payload = serde_json::to_string(&body)?;
    let request = http_client::http::Request::builder()
        .method(method)
        .uri(format!("{}{path}", base_url(&config)))
        .header("Content-Type", "application/json")
        .header("X-API-Key", key.clone())
        .header("X-CSRF-Token", csrf.clone())
        .header(
            "Cookie",
            format!("auracle_session={key}; auracle_csrf={csrf}"),
        )
        .body(http_client::AsyncBody::from(payload))?;
    let mut response = http.send(request).await?;
    if !response.status().is_success() {
        let status = response.status();
        let mut body = String::new();
        // Best-effort read of the engine's error body so its `detail` reaches
        // the caller; a read failure still leaves the status to report.
        match response.body_mut().read_to_string(&mut body).await {
            Ok(_) => anyhow::bail!("{}", engine_error_message(status, &body)),
            Err(error) => {
                anyhow::bail!("engine answered with status {status} (could not read body: {error})")
            }
        }
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

pub async fn test_connection(
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

pub async fn save_connection(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
    body: serde_json::Value,
) -> Result<()> {
    post_json(http, &format!("/ui/api/connections/{broker}/save"), body).await?;
    Ok(())
}

/// Disconnect a broker by clearing its stored credentials (the toggle-OFF path on
/// the inline Connections rows). The engine clears the vault entry and the broker
/// then reports back as not connected on the next `list_brokers` poll.
pub async fn disconnect_connection(
    http: Arc<dyn http_client::HttpClient>,
    broker: &str,
) -> Result<()> {
    post_json(
        http,
        &format!("/ui/api/connections/{broker}/disconnect"),
        serde_json::Value::Null,
    )
    .await?;
    Ok(())
}

/// Write QuantConnect credentials through to the engine vault and report whether
/// they authenticated. The token only ever lives in the request body; the engine
/// stores it in Key Master and returns just `{connected, user_id}`.
pub async fn save_quantconnect_credentials(
    http: Arc<dyn http_client::HttpClient>,
    user_id: &str,
    api_token: &str,
) -> Result<bool> {
    let value = post_json(
        http,
        "/ui/api/quantconnect/credentials",
        serde_json::json!({ "user_id": user_id, "api_token": api_token }),
    )
    .await?;
    Ok(value
        .get("connected")
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Disconnect QuantConnect by clearing the stored credentials (token rotation or
/// account switch).
pub async fn clear_quantconnect_credentials(http: Arc<dyn http_client::HttpClient>) -> Result<()> {
    send_json(
        http,
        "DELETE",
        "/ui/api/quantconnect/credentials",
        serde_json::Value::Null,
    )
    .await?;
    Ok(())
}

/// Read the shared settings the launcher and IDE both reflect. Owner-scoped on
/// the engine; on this loopback transport the operator's own key authenticates.
pub async fn get_settings(http: Arc<dyn http_client::HttpClient>) -> Result<SharedSettings> {
    let value = get_json(http, "/ui/api/settings").await?;
    Ok(serde_json::from_value(value)?)
}

/// Read the signed-in operator's profile (identity + tier/license) from
/// `GET /ui/api/me`. Backs the Settings "Profile" section. Mirrors
/// `get_settings`'s loopback read.
pub async fn get_profile(http: Arc<dyn http_client::HttpClient>) -> Result<Profile> {
    let value = get_json(http, "/ui/api/me").await?;
    Ok(serde_json::from_value(value)?)
}

/// Mirror the IDE's chosen AI model up to the engine so the launcher reflects
/// it (`PUT /ui/api/settings {ai_model:{provider, model_id, key}}`). The key is
/// included so the engine can authenticate the same provider the IDE just
/// authenticated — this is the IDE→engine half of true cross-store sync.
/// Best-effort by contract: callers treat a failure as non-fatal.
pub async fn put_ai_model(
    http: Arc<dyn http_client::HttpClient>,
    provider: &str,
    model_id: &str,
    key: Option<&str>,
) -> Result<()> {
    let mut ai_model = serde_json::Map::new();
    ai_model.insert("provider".into(), provider.into());
    ai_model.insert("model_id".into(), model_id.into());
    if let Some(key) = key {
        ai_model.insert("key".into(), key.into());
    }
    let body = serde_json::json!({ "ai_model": serde_json::Value::Object(ai_model) });
    send_json(http, "PUT", "/ui/api/settings", body).await?;
    Ok(())
}

/// Pull the plaintext AI-provider key the engine holds, so the IDE can import
/// it into its own keychain (engine→IDE half of cross-store sync). This is the
/// `POST /ui/api/settings/ai-key {provider}` handoff: loopback-only and
/// never-logged engine-side, gated to the AI-providers whitelist. Returns the
/// `(provider, key)` pair the engine returns; the engine 404s when it has no
/// key for that provider, which surfaces here as an error the caller treats as
/// "nothing to import".
pub async fn fetch_ai_key(
    http: Arc<dyn http_client::HttpClient>,
    provider: &str,
) -> Result<(String, String)> {
    let body = serde_json::json!({ "provider": provider });
    let value = post_json(http, "/ui/api/settings/ai-key", body).await?;
    let provider = value
        .get("provider")
        .and_then(|value| value.as_str())
        .unwrap_or(provider)
        .to_string();
    let key = value
        .get("key")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the engine returned no key for this provider"))?;
    Ok((provider, key))
}

// ── Provider-name reconciliation (engine vault-key ↔ IDE registry id) ──
//
// The engine and the IDE name the same AI provider differently and both
// names are already shipped, so the IDE owns the translation. The engine
// stores `ai_model.provider` as the vault-key name from
// `auracle.keys.PROVIDERS` (whitelisted by `_AI_PROVIDERS` in
// `auracle/houston/web/views/settings.py`): `anthropic`, `openai_api_key`,
// `deepseek_api_key`, `ollama_host`. The IDE's `LanguageModelRegistry`
// keys providers by registry id: `anthropic`, `openai`, `ollama`, and
// `auracle-agent` (the engine-backed DeepSeek gateway — see
// `language_models/src/provider/auracle.rs`'s `PROVIDER_ID`).
//
// The DeepSeek pairing is the load-bearing one: the launcher's "default
// agent" writes `deepseek_api_key`, which the IDE surfaces as the
// `auracle-agent` provider (not the bare `deepseek` provider, which would
// require a pasted key the IDE deliberately never stores).
//
// These are intentionally a closed table of the four shipped pairs, not a
// heuristic: an unknown engine name returns `None` (the caller falls back
// to its default ordering) and an unknown IDE id passes through unchanged
// (so a future BYO provider still mirrors *something* the engine can
// reject honestly rather than being silently rewritten). No Google/Gemini
// engine slot exists, so it is deliberately absent.

/// The four shipped (engine vault-key name, IDE registry provider id) pairs.
const PROVIDER_NAME_PAIRS: &[(&str, &str)] = &[
    ("deepseek_api_key", "auracle-agent"),
    ("anthropic", "anthropic"),
    ("openai_api_key", "openai"),
    ("ollama_host", "ollama"),
];

/// Map an engine vault-key provider name to the IDE registry provider id.
/// Returns `None` for any provider the engine could store but the IDE has
/// no registry provider for, so the caller keeps its own fallback ordering
/// rather than seeding a provider that does not exist in the IDE.
pub fn engine_provider_to_ide(engine_provider: &str) -> Option<&'static str> {
    PROVIDER_NAME_PAIRS
        .iter()
        .find(|(engine, _)| *engine == engine_provider)
        .map(|(_, ide)| *ide)
}

/// Map an IDE registry provider id to the engine vault-key provider name.
/// An unrecognized id (e.g. a BYO frontier provider the engine doesn't
/// model) passes through unchanged, so the engine's own `_AI_PROVIDERS`
/// whitelist remains the single authority on what it will accept — the IDE
/// never silently rewrites an id the engine might legitimately reject.
pub fn ide_provider_to_engine(ide_provider: &str) -> &str {
    PROVIDER_NAME_PAIRS
        .iter()
        .find(|(_, ide)| *ide == ide_provider)
        .map(|(engine, _)| *engine)
        .unwrap_or(ide_provider)
}

// ── Render ───────────────────────────────────────────────────────────

impl EventEmitter<DismissEvent> for BrokerWizard {}

impl Focusable for BrokerWizard {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

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
            let connected = broker.status.state == "connected";
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
            .gap_1()
            .child(Label::new("Broker connections").size(LabelSize::Large))
            .child(
                Label::new(
                    "Connect a broker to route orders and pull market data. \
                     Each broker shows what it can do before you connect.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(list.pt_2())
    }

    fn render_credentials(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut form = v_flex().gap_3();
        if self.fields.is_empty() {
            form = form.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }
        for (i, field) in self.fields.iter().enumerate() {
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
            // A `select` (e.g. the paper/live mode) renders as a segmented
            // control so the value is always a valid option — never a typo
            // in a free-text box. Other kinds use the text/password editor.
            let input = if field.kind == "select" {
                let selected = self
                    .selections
                    .get(&field.name)
                    .cloned()
                    .unwrap_or_default();
                let mut segmented = h_flex().gap_1();
                for option in field.options.clone() {
                    let field_name = field.name.clone();
                    let chosen = option.clone();
                    let is_selected = option == selected;
                    segmented = segmented.child(
                        Button::new(
                            SharedString::from(format!("sel-{}-{}", field.name, option)),
                            option,
                        )
                        .style(if is_selected {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Outlined
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selections.insert(field_name.clone(), chosen.clone());
                            cx.notify();
                        })),
                    );
                }
                segmented.into_any_element()
            } else if i < self.editors.len() {
                self.editors[i].clone().into_any_element()
            } else {
                Label::new("Set this field from the web console for now.")
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .into_any_element()
            };
            form = form.child(
                v_flex()
                    .gap_1()
                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                    .child(input),
            );
        }
        form
    }

    fn render_confirm(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_2()
            .child(Label::new("What this broker can do"));
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
                    let state = cap
                        .capabilities
                        .get(leg)
                        .map(String::as_str)
                        .unwrap_or("unknown");
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

        // An inline settings sub-page — no modal card chrome, no surface
        // background, no border. It fills the sub-page content area and scrolls
        // like the Account and Data sources sub-pages beside it, matching their
        // padding so the three Connections pages read as one native surface.
        v_flex()
            .id("broker-connect-page")
            .track_focus(&self.focus_handle)
            .size_full()
            .pt_2()
            .px_8()
            .pb_16()
            .gap_3()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
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
                                .tab_index(0_isize)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step = this.step.previous();
                                    cx.notify();
                                })),
                        )
                    })
                    .when(step == Step::Credentials && has_selection, |row| {
                        row.child(
                            Button::new("wiz-test", "Test")
                                .style(ButtonStyle::Outlined)
                                .tab_index(0_isize)
                                .on_click(cx.listener(|this, _, _, cx| this.run_test(cx))),
                        )
                        .child(
                            Button::new("wiz-next", "Next")
                                .style(ButtonStyle::Outlined)
                                .tab_index(0_isize)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step = this.step.next();
                                    cx.notify();
                                })),
                        )
                    })
                    .when(step == Step::Confirm && has_selection, |row| {
                        row.child(
                            Button::new("wiz-save", "Connect")
                                .style(ButtonStyle::Filled)
                                .tab_index(0_isize)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                    }),
            )
    }
}

/// The native "Connect QuantConnect" sub-page on the Connections settings page.
///
/// A two-field credential form (user ID + API token) that mirrors the broker
/// connect flow's hosting model: a dedicated [`Render`] entity whose editors are
/// built once on push so their focus survives re-renders. Saving writes the
/// credentials *through* the engine into the Key Master vault (the token never
/// touches IDE disk) and bumps [`ConnectGeneration`] so the connections status
/// chip re-polls; Disconnect clears the stored credentials.
pub struct QuantConnectConnect {
    focus_handle: FocusHandle,
    user_id_editor: Entity<editor::Editor>,
    token_editor: Entity<editor::Editor>,
    state: TestState,
    scroll_handle: gpui::ScrollHandle,
    _task: Option<Task<()>>,
}

impl QuantConnectConnect {
    /// Build the page entity and its two credential editors. Called by the
    /// settings window when the "Connect QuantConnect" sub-page is pushed, where
    /// a `Window` + `&mut App` are available so the editors persist across renders.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            user_id_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
            token_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
            state: TestState::Idle,
            scroll_handle: gpui::ScrollHandle::new(),
            _task: None,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let user_id = self.user_id_editor.read(cx).text(cx).trim().to_string();
        let api_token = self.token_editor.read(cx).text(cx).trim().to_string();
        if user_id.is_empty() || api_token.is_empty() {
            self.state = TestState::Verdict {
                ok: false,
                plain: "Enter both your QuantConnect user ID and API token.".into(),
            };
            cx.notify();
            return;
        }
        let http = cx.http_client();
        self.state = TestState::Testing;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = save_quantconnect_credentials(http, &user_id, &api_token).await;
            this.update(cx, |this, cx| match result {
                // Authenticated: the connections chip should re-poll, and the
                // sub-page can close — the card now reads connected.
                Ok(true) => {
                    let generation = cx.global::<ConnectGeneration>().0 + 1;
                    cx.set_global(ConnectGeneration(generation));
                    cx.emit(DismissEvent);
                }
                // Stored but rejected — honest feedback, never a fake success.
                Ok(false) => {
                    this.state = TestState::Verdict {
                        ok: false,
                        plain: "Saved, but QuantConnect didn't accept these credentials. \
                                Check your user ID and API token, then try again."
                            .into(),
                    };
                    cx.notify();
                }
                Err(error) => {
                    this.state = TestState::Verdict {
                        ok: false,
                        plain: SharedString::from(format!("Couldn't save: {error}.")),
                    };
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self.state = TestState::Testing;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = clear_quantconnect_credentials(http).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    let generation = cx.global::<ConnectGeneration>().0 + 1;
                    cx.set_global(ConnectGeneration(generation));
                    cx.emit(DismissEvent);
                }
                Err(error) => {
                    this.state = TestState::Verdict {
                        ok: false,
                        plain: SharedString::from(format!("Couldn't disconnect: {error}.")),
                    };
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

impl EventEmitter<DismissEvent> for QuantConnectConnect {}

impl Focusable for QuantConnectConnect {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for QuantConnectConnect {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let verdict: Option<(Color, SharedString)> = match &self.state {
            TestState::Idle => None,
            TestState::Testing => Some((Color::Muted, "Working…".into())),
            TestState::Verdict { ok, plain } => Some((
                if *ok { Color::Success } else { Color::Error },
                plain.clone(),
            )),
        };
        // Resolve the field border once so the per-field builder doesn't reborrow
        // the context while the element tree is under construction.
        let border = cx.theme().colors().border;
        let field = |label: &str, hint: &str, editor: Entity<editor::Editor>| {
            v_flex()
                .gap_1()
                .child(Label::new(label.to_string()).size(LabelSize::Small))
                .child(
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .child(editor),
                )
                .child(
                    Label::new(hint.to_string())
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
        };

        v_flex()
            .id("quantconnect-connect-page")
            .track_focus(&self.focus_handle)
            .size_full()
            .pt_2()
            .px_8()
            .pb_16()
            .gap_3()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .child(
                Label::new("Connect QuantConnect to import your LEAN strategies into Auracle.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(field(
                "User ID",
                "Your numeric QuantConnect user ID (from quantconnect.com/account).",
                self.user_id_editor.clone(),
            ))
            .child(field(
                "API token",
                "Stored in your engine's vault — never written to this app's disk.",
                self.token_editor.clone(),
            ))
            .when_some(verdict, |this, (color, plain)| {
                this.child(Label::new(plain).size(LabelSize::Small).color(color))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("qc-disconnect", "Disconnect")
                            .style(ButtonStyle::Subtle)
                            .tab_index(0_isize)
                            .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                    )
                    .child(
                        Button::new("qc-save", "Save")
                            .style(ButtonStyle::Filled)
                            .tab_index(0_isize)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BrokerSummary, FieldMeta, Step, build_connection_body, default_selections, engine_detail,
        engine_error_message, engine_provider_to_ide, ide_provider_to_engine,
    };
    use http_client::StatusCode;
    use std::collections::HashMap;

    fn text_field(name: &str) -> FieldMeta {
        FieldMeta {
            name: name.to_string(),
            kind: "text".to_string(),
            ..Default::default()
        }
    }

    fn select_field(name: &str, options: &[&str]) -> FieldMeta {
        FieldMeta {
            name: name.to_string(),
            kind: "select".to_string(),
            options: options.iter().map(|option| option.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn step_next_advances_then_settles_on_confirm() {
        assert_eq!(Step::ChooseBroker.next(), Step::Credentials);
        assert_eq!(Step::Credentials.next(), Step::Confirm);
        // Confirm is terminal — its forward action is "Connect", not a step.
        assert_eq!(Step::Confirm.next(), Step::Confirm);
    }

    #[test]
    fn step_previous_retreats_then_settles_on_root() {
        assert_eq!(Step::Confirm.previous(), Step::Credentials);
        assert_eq!(Step::Credentials.previous(), Step::ChooseBroker);
        // ChooseBroker is the root — Back has nowhere to go.
        assert_eq!(Step::ChooseBroker.previous(), Step::ChooseBroker);
    }

    #[test]
    fn default_selections_seeds_each_select_with_its_first_option() {
        let fields = vec![
            text_field("api_key"),
            select_field("mode", &["paper", "live"]),
            select_field("venue", &["spot", "margin"]),
        ];
        let selections = default_selections(&fields);
        // Exactly the select fields are seeded — text fields are never seeded —
        // and each gets its first listed option.
        assert_eq!(selections.len(), 2);
        assert_eq!(selections.get("mode").map(String::as_str), Some("paper"));
        assert_eq!(selections.get("venue").map(String::as_str), Some("spot"));
        assert!(!selections.contains_key("api_key"));
    }

    #[test]
    fn default_selections_skips_a_select_with_no_options() {
        // A select that arrives without options can't be seeded with a real
        // choice, so it is left unset rather than seeded with an empty string.
        let fields = vec![select_field("mode", &[])];
        assert!(default_selections(&fields).is_empty());
    }

    #[test]
    fn build_body_uses_chosen_select_and_skips_empty_text() {
        let fields = vec![
            text_field("api_key"),
            text_field("api_secret"),
            select_field("mode", &["paper", "live"]),
        ];
        let mut selections = HashMap::new();
        selections.insert("mode".to_string(), "live".to_string());
        let mut text = HashMap::new();
        text.insert("api_key".to_string(), "abc123".to_string());
        // api_secret is left blank: a saved sensitive field the user didn't
        // re-type must NOT be overwritten with an empty string.
        text.insert("api_secret".to_string(), String::new());

        let body = build_connection_body(&fields, &selections, &text);
        let object = body.as_object().expect("body is a json object");
        assert_eq!(
            object.get("api_key").and_then(|v| v.as_str()),
            Some("abc123")
        );
        assert_eq!(object.get("mode").and_then(|v| v.as_str()), Some("live"));
        // The blank secret is absent — kept, not cleared.
        assert!(!object.contains_key("api_secret"));
    }

    #[test]
    fn build_body_ignores_unknown_text_and_unchosen_selects() {
        let fields = vec![
            text_field("api_key"),
            select_field("mode", &["paper", "live"]),
        ];
        let selections = HashMap::new(); // mode never chosen
        let mut text = HashMap::new();
        text.insert("api_key".to_string(), "k".to_string());
        // A stray entry with no matching field must never leak into the body.
        text.insert("ghost".to_string(), "value".to_string());
        // Free text under a select field's name must never be treated as its value.
        text.insert("mode".to_string(), "typo".to_string());

        let body = build_connection_body(&fields, &selections, &text);
        let object = body.as_object().expect("body is a json object");
        assert_eq!(object.len(), 1);
        assert_eq!(object.get("api_key").and_then(|v| v.as_str()), Some("k"));
        assert!(!object.contains_key("mode"));
        assert!(!object.contains_key("ghost"));
    }

    #[test]
    fn engine_to_ide_maps_the_four_shipped_pairs() {
        // The load-bearing pair: the launcher's "default agent" stores
        // `deepseek_api_key`; the IDE surfaces it as the engine-backed
        // `auracle-agent` provider, never the bare `deepseek` provider.
        assert_eq!(
            engine_provider_to_ide("deepseek_api_key"),
            Some("auracle-agent")
        );
        assert_eq!(engine_provider_to_ide("anthropic"), Some("anthropic"));
        assert_eq!(engine_provider_to_ide("openai_api_key"), Some("openai"));
        assert_eq!(engine_provider_to_ide("ollama_host"), Some("ollama"));
    }

    #[test]
    fn engine_to_ide_returns_none_for_unmodeled_provider() {
        // An engine provider the IDE has no registry entry for must not be
        // coerced — the seed keeps its own fallback ordering instead.
        assert_eq!(engine_provider_to_ide(""), None);
        assert_eq!(engine_provider_to_ide("polygon"), None);
        // The bare IDE id is NOT a valid engine name; the inverse direction
        // owns that pairing, so the forward lookup must miss.
        assert_eq!(engine_provider_to_ide("auracle-agent"), None);
    }

    #[test]
    fn ide_to_engine_maps_the_four_shipped_pairs() {
        assert_eq!(ide_provider_to_engine("auracle-agent"), "deepseek_api_key");
        assert_eq!(ide_provider_to_engine("anthropic"), "anthropic");
        assert_eq!(ide_provider_to_engine("openai"), "openai_api_key");
        assert_eq!(ide_provider_to_engine("ollama"), "ollama_host");
    }

    #[test]
    fn ide_to_engine_passes_unknown_id_through_unchanged() {
        // A BYO provider the engine doesn't model is forwarded verbatim so
        // the engine's `_AI_PROVIDERS` whitelist stays the single authority
        // on acceptance — the IDE never silently rewrites it.
        assert_eq!(ide_provider_to_engine("x_ai"), "x_ai");
        assert_eq!(ide_provider_to_engine(""), "");
    }

    #[test]
    fn round_trip_is_stable_for_shipped_pairs() {
        for ide in ["auracle-agent", "anthropic", "openai", "ollama"] {
            let engine = ide_provider_to_engine(ide);
            assert_eq!(engine_provider_to_ide(engine), Some(ide));
        }
    }

    #[test]
    fn engine_error_surfaces_fastapi_string_detail() {
        // The load-bearing case: a 404 whose body carries the engine's own
        // user-actionable reason must reach the caller, not be flattened to the
        // bare status — otherwise the chat falls back to "couldn't reach your
        // engine" even though the engine answered precisely.
        let body = r#"{"detail":"no DeepSeek key configured — set it in the launcher"}"#;
        assert_eq!(
            engine_error_message(StatusCode::NOT_FOUND, body),
            "engine answered with status 404 Not Found: \
             no DeepSeek key configured — set it in the launcher"
        );
        assert_eq!(
            engine_detail(body).as_deref(),
            Some("no DeepSeek key configured — set it in the launcher")
        );
    }

    #[test]
    fn engine_error_renders_structured_detail_compactly() {
        // FastAPI's 422 validation errors arrive as a list under `detail`; we
        // render it compactly rather than drop it, so the reason still reaches
        // the caller.
        let body = r#"{"detail":[{"loc":["body","mode"],"msg":"field required"}]}"#;
        assert_eq!(
            engine_detail(body).as_deref(),
            Some(r#"[{"loc":["body","mode"],"msg":"field required"}]"#)
        );
        assert_eq!(
            engine_error_message(StatusCode::UNPROCESSABLE_ENTITY, body),
            r#"engine answered with status 422 Unprocessable Entity: [{"loc":["body","mode"],"msg":"field required"}]"#
        );
    }

    #[test]
    fn engine_error_falls_back_to_status_without_usable_detail() {
        // Non-JSON, JSON without `detail`, an empty/whitespace detail, and a
        // null detail all carry nothing actionable — the message stays the
        // bare status it was before this fix, so behavior never regresses.
        for body in [
            "",
            "<html>502 Bad Gateway</html>",
            r#"{"error":"boom"}"#,
            r#"{"detail":""}"#,
            r#"{"detail":"   "}"#,
            r#"{"detail":null}"#,
        ] {
            assert_eq!(
                engine_detail(body),
                None,
                "body should yield no detail: {body:?}"
            );
            assert_eq!(
                engine_error_message(StatusCode::BAD_GATEWAY, body),
                "engine answered with status 502 Bad Gateway",
                "body should fall back to status: {body:?}"
            );
        }
    }

    #[test]
    fn broker_summary_deserializes_nested_status_object() {
        // Pins the live GET /ui/api/connections contract: each broker's `status`
        // is the NESTED ConnectionStatus.to_dict() object, not a flat string.
        // Modeling it as a string made serde fail and silently drop every broker
        // (Loading brokers… forever, toggles stuck OFF, disconnect a no-op).
        let payload = serde_json::json!({
            "connections": [{
                "id": "ibkr",
                "display_label": "Interactive Brokers",
                "blurb": "Route orders + market data",
                "status": {
                    "state": "connected",
                    "detail": "gateway up",
                    "last_activity_at": null,
                    "paper_mode": true,
                    "account_id": "DU123",
                    "latency_ms": 12,
                    "checked_at": "2026-06-24T00:00:00+00:00"
                }
            }]
        });
        let list = payload["connections"].as_array().unwrap().clone();
        let brokers: Vec<BrokerSummary> = list
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect();

        assert_eq!(brokers.len(), 1, "the broker must not be silently dropped");
        let ibkr = &brokers[0];
        assert_eq!(ibkr.id, "ibkr");
        assert_eq!(ibkr.status.state, "connected");
        assert_eq!(ibkr.status.detail.as_deref(), Some("gateway up"));
        assert_eq!(ibkr.status.paper_mode, Some(true));
    }
}
