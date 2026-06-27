//! Engine connection client + the unified connector data model.
//!
//! The IDE's single connect surface (the docked settings panel) reads and
//! mutates every connection through this crate. Whether a connector trades
//! (kind `broker`), serves prices (kind `data_provider`), or bridges another
//! platform (kind `integration`, e.g. QuantConnect), it arrives as the SAME
//! [`Connector`] JSON shape from the engine's unified `/ui/api/connections`
//! registry — so one generic flow (list → fields → test → save → disconnect)
//! covers all three, and the panel's sections are just `kind`-filtered views.
//!
//! Honesty laws (engine-enforced; mirrored here):
//!   * a connector reads "connected" ONLY when the engine's status says so —
//!     never a local guess (see [`ConnStatus::is_connected`]);
//!   * capability + test results come ONLY from a real engine round-trip;
//!   * sensitive field VALUES are never fetched — the engine returns only a
//!     masked preview + a `has_value` flag, and we never log a request body.
//!
//! Transport + auth reuse [`auracle_connect`] (loopback engine_url + api_key,
//! CSRF double-submit). The reusable functions are `pub` so the settings panel
//! drives them directly without re-deriving the header dance.

use std::sync::Arc;

use anyhow::Result;
use auracle_connect::{AuracleConfig, load_config};
use futures::AsyncReadExt as _;
use gpui::{App, SharedString, actions};
use serde::Deserialize;
use ui::prelude::*;

actions!(
    auracle,
    [
        /// Open the Auracle connections surface. Retained for the deploy path
        /// and the settings deep-link; both now focus the docked settings
        /// panel (the one connect surface) rather than a modal wizard.
        OpenBrokerWizard
    ]
);

pub const MAX_FIELDS: usize = 10;

/// No-op: the connect surface is the docked settings panel, which registers the
/// handlers for [`OpenBrokerWizard`] and `OpenConnections` itself. Kept so the
/// existing `main.rs` init call site stays stable.
pub fn init(_cx: &mut App) {}

// ── Engine JSON shapes (introspection-driven) ────────────────────────

/// One connector's live status, exactly as the engine reports it
/// (`{"state": ..., "detail": ...}`). `state` is the engine's vocabulary:
/// `connected` | `not_configured` | `error` | broker runtime states
/// (`connecting`/`disconnected`/`degraded`/…). `detail` carries the human
/// reason when the state isn't a clean success.
#[derive(Clone, Deserialize, Default)]
pub struct ConnStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
}

impl ConnStatus {
    /// True only when the engine reports a live, configured connection. Every
    /// other state (not_configured, error, disconnected, degraded, …) is honest
    /// about NOT being connected.
    pub fn is_connected(&self) -> bool {
        self.state == "connected"
    }
}

/// One connector in the unified registry. The list endpoint returns these with
/// `fields` empty; the detail endpoint fills `fields` (with `has_value`/preview
/// flags) for the credentials form.
#[derive(Clone, Deserialize, Default)]
pub struct Connector {
    pub id: String,
    #[serde(default)]
    pub display_label: String,
    #[serde(default)]
    pub blurb: String,
    /// `broker` | `data_provider` | `integration`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: ConnStatus,
    /// Credential fields — empty in list responses, populated in detail.
    #[serde(default)]
    pub fields: Vec<FieldMeta>,
    #[serde(default)]
    pub asset_kinds: Vec<String>,
    /// Whether the engine exposes a real Test probe for this connector. When
    /// false the panel shows Save-only (no fake green from a missing tester).
    #[serde(default)]
    pub test_supported: bool,
    /// Whether the operator's tier blocks connecting this connector.
    #[serde(default)]
    pub gated: bool,
    #[serde(default)]
    pub gated_reason: String,
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

// ── Account (read-only identity surface) ──────────────────────────────

/// The IDE-facing license block from `GET /ui/api/account`. `state` is
/// `active` | `expired` | `none`.
#[derive(Clone, Deserialize, Default)]
pub struct LicenseStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub expiry: Option<String>,
}

/// The operator's account as the engine reports it. Carries no secret — the
/// engine never returns a key here (PRD invariant I3). `manage_url` is the
/// billing-portal URL when one is configured, else `None` (the panel hides the
/// button rather than show a dead control).
#[derive(Clone, Deserialize, Default)]
pub struct Account {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub license_status: LicenseStatus,
    #[serde(default)]
    pub manage_url: Option<String>,
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

// ── Pure reducers (engine-free; unit-tested) ──────────────────────────

/// A one-line, honest status summary for a section of connectors. Pure so the
/// section headers stay testable without the engine.
pub fn section_summary(connectors: &[Connector]) -> String {
    if connectors.is_empty() {
        return "None available".to_string();
    }
    let connected = connectors
        .iter()
        .filter(|connector| connector.status.is_connected())
        .count();
    let total = connectors.len();
    match connected {
        0 => "None connected".to_string(),
        n if n == total && total == 1 => "Connected".to_string(),
        n if n == total => format!("All {total} connected"),
        n => format!("{n} of {total} connected"),
    }
}

/// Whether a section should start expanded: a section with nothing connected is
/// opened on first load to invite the operator to connect; an already-set-up
/// section starts collapsed to stay out of the way.
pub fn default_expanded(connectors: &[Connector]) -> bool {
    !connectors.is_empty()
        && !connectors
            .iter()
            .any(|connector| connector.status.is_connected())
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
        anyhow::bail!("engine answered with status {}", response.status());
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text)?)
}

/// List connectors of a given `kind` (`broker` | `data_provider` |
/// `integration` | `all`). An empty `kind` is the back-compatible
/// brokers-only default the engine ships.
pub async fn list_connectors(
    http: Arc<dyn http_client::HttpClient>,
    kind: &str,
) -> Result<Vec<Connector>> {
    let path = if kind.is_empty() {
        "/ui/api/connections".to_string()
    } else {
        format!("/ui/api/connections?kind={kind}")
    };
    let value = get_json(http, &path).await?;
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

/// Brokers only — the execution connectors. Retained for the first-run wizard;
/// thin wrapper over [`list_connectors`].
pub async fn list_brokers(http: Arc<dyn http_client::HttpClient>) -> Result<Vec<Connector>> {
    list_connectors(http, "broker").await
}

/// One connector's full detail, including its credential `fields` (with
/// `has_value`/preview flags). The engine returns the connector object at the
/// top level, so it deserializes straight into [`Connector`].
pub async fn get_connector(
    http: Arc<dyn http_client::HttpClient>,
    connector: &str,
) -> Result<Connector> {
    let value = get_json(http, &format!("/ui/api/connections/{connector}")).await?;
    Ok(serde_json::from_value(value)?)
}

pub async fn get_fields(
    http: Arc<dyn http_client::HttpClient>,
    connector: &str,
) -> Result<Vec<FieldMeta>> {
    let value = get_json(http, &format!("/ui/api/connections/{connector}")).await?;
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
    connector: &str,
) -> Result<Capability> {
    let value = get_json(http, &format!("/ui/api/connections/{connector}/capability")).await?;
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
        anyhow::bail!("engine answered with status {}", response.status());
    }
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

pub async fn test_connection(
    http: Arc<dyn http_client::HttpClient>,
    connector: &str,
    body: serde_json::Value,
) -> Result<String> {
    let value = post_json(http, &format!("/ui/api/connections/{connector}/test"), body).await?;
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
    connector: &str,
    body: serde_json::Value,
) -> Result<()> {
    post_json(http, &format!("/ui/api/connections/{connector}/save"), body).await?;
    Ok(())
}

/// Clear a connector's saved credentials. The engine wipes the vault/pref
/// entries and the connector drops back to `not_configured`.
pub async fn disconnect_connection(
    http: Arc<dyn http_client::HttpClient>,
    connector: &str,
) -> Result<()> {
    post_json(
        http,
        &format!("/ui/api/connections/{connector}/disconnect"),
        serde_json::json!({}),
    )
    .await?;
    Ok(())
}

/// Read the operator's account (email, tier, license, billing-portal URL).
/// Owner-scoped on the engine; on this loopback transport the operator's own
/// key authenticates. Never carries a secret.
pub async fn get_account(http: Arc<dyn http_client::HttpClient>) -> Result<Account> {
    let value = get_json(http, "/ui/api/account").await?;
    Ok(serde_json::from_value(value)?)
}

/// Read the shared settings the launcher and IDE both reflect. Owner-scoped on
/// the engine; on this loopback transport the operator's own key authenticates.
pub async fn get_settings(http: Arc<dyn http_client::HttpClient>) -> Result<SharedSettings> {
    let value = get_json(http, "/ui/api/settings").await?;
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

// ── Connector brand marks (logo chip / monogram fallback) ─────────────

/// The bundled official logo for a connection, rendered full-colour via `img`
/// (gpui rasterises SVG). Returns the asset path when we ship a logo for this
/// connector; unknown ones fall back to a brand-colour monogram tile. To add
/// one: drop `assets/icons/brokers/<file>.svg` and map the id here.
pub fn broker_logo_path(id: &str) -> Option<&'static str> {
    match id {
        "ibkr" | "ibkr_cp" => Some("icons/brokers/interactive-brokers.svg"),
        "alpaca" => Some("icons/brokers/alpaca.svg"),
        "quantconnect" => Some("icons/brokers/quantconnect.svg"),
        _ => None,
    }
}

/// Brand colour for the monogram-tile fallback (connectors without a bundled
/// logo).
fn brand_rgb(id: &str) -> gpui::Rgba {
    match id {
        "clearstreet" => gpui::rgb(0x1466FF),
        "hyperliquid" => gpui::rgb(0x0E9C84),
        "polygon" => gpui::rgb(0x5B3DF5),
        _ => gpui::rgb(0x6B7280),
    }
}

/// A short, stable monogram for the fallback tile: a curated mark for known
/// connectors, else the first two alphanumeric characters of the display label.
fn brand_monogram(id: &str, label: &str) -> SharedString {
    let curated = match id {
        "clearstreet" => Some("CS"),
        "hyperliquid" => Some("HL"),
        "polygon" => Some("PG"),
        _ => None,
    };
    if let Some(mark) = curated {
        return mark.into();
    }
    let source = if label.is_empty() { id } else { label };
    let mono: String = source
        .chars()
        .filter(|character| character.is_alphanumeric())
        .take(2)
        .collect();
    if mono.is_empty() {
        "?".into()
    } else {
        mono.to_uppercase().into()
    }
}

/// A fixed-height connection mark: the official logo on a clean light chip (so
/// brand colours read in any theme) when we ship one, otherwise a brand-colour
/// monogram tile. Sized so every row reads as a deliberate, consistent mark.
pub fn brand_tile(id: &str, label: &str) -> AnyElement {
    if let Some(path) = broker_logo_path(id) {
        return div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h_8()
            .px_2()
            .rounded_md()
            .bg(gpui::white())
            .child(gpui::img(path).h_5())
            .into_any_element();
    }
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size_8()
        .rounded_md()
        .bg(brand_rgb(id))
        .child(
            Label::new(brand_monogram(id, label))
                .size(LabelSize::Small)
                .color(Color::Custom(gpui::white())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(state: &str) -> Connector {
        Connector {
            status: ConnStatus {
                state: state.into(),
                detail: None,
            },
            ..Default::default()
        }
    }

    #[test]
    fn is_connected_only_on_connected_state() {
        assert!(
            ConnStatus {
                state: "connected".into(),
                detail: None
            }
            .is_connected()
        );
        for state in ["not_configured", "error", "disconnected", "degraded", ""] {
            assert!(
                !ConnStatus {
                    state: state.into(),
                    detail: None
                }
                .is_connected(),
                "state {state:?} must not read as connected"
            );
        }
    }

    #[test]
    fn section_summary_is_honest_about_counts() {
        assert_eq!(section_summary(&[]), "None available");
        assert_eq!(
            section_summary(&[conn("not_configured"), conn("error")]),
            "None connected"
        );
        assert_eq!(section_summary(&[conn("connected")]), "Connected");
        assert_eq!(
            section_summary(&[conn("connected"), conn("connected")]),
            "All 2 connected"
        );
        assert_eq!(
            section_summary(&[conn("connected"), conn("not_configured")]),
            "1 of 2 connected"
        );
    }

    #[test]
    fn default_expanded_opens_only_when_nothing_connected() {
        assert!(!default_expanded(&[]));
        assert!(default_expanded(&[conn("not_configured")]));
        assert!(!default_expanded(&[conn("connected")]));
        assert!(!default_expanded(&[
            conn("connected"),
            conn("not_configured")
        ]));
    }

    #[test]
    fn connector_deserializes_status_object() {
        let json = serde_json::json!({
            "id": "ibkr",
            "display_label": "Interactive Brokers",
            "kind": "broker",
            "status": {"state": "connected"},
            "test_supported": true,
            "gated": false
        });
        let connector: Connector = serde_json::from_value(json).unwrap();
        assert_eq!(connector.id, "ibkr");
        assert_eq!(connector.kind, "broker");
        assert!(connector.status.is_connected());
        assert!(connector.test_supported);
        assert!(!connector.gated);
    }

    #[test]
    fn account_deserializes_without_secret() {
        let json = serde_json::json!({
            "email": "ops@example.com",
            "tier": "institutional",
            "license_status": {"state": "active", "expiry": "2027-01-01"},
            "manage_url": null
        });
        let account: Account = serde_json::from_value(json).unwrap();
        assert_eq!(account.email, "ops@example.com");
        assert_eq!(account.license_status.state, "active");
        assert!(account.manage_url.is_none());
    }
}
