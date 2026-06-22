//! Connect to your Auracle engine — the paste-key surface (B-10).
//!
//! A zero-background user pastes the engine address and their key
//! into two fields, presses Test, reads a plain verdict, and saves.
//! Configuration lives in the fork's own config file
//! (`<config dir>/auracle.json`); environment variables override it
//! for development. The key stays local to this machine (per-seat
//! token rotation is engine roadmap, tracked there).
//!
//! The honesty rules of the verdict (an in-flight test is never green, an
//! unreachable AI agent never reads as fully ready, nothing is fabricated when
//! the engine omits a field) live in the gpui-free `auracle_connect_state`
//! reducer so they are unit-tested without rendering. This module keeps only the
//! HTTP/JSON I/O and the gpui render: `test_connection` builds exactly one
//! `ConnectProbe` from already-decoded fields, and the render is a thin match
//! over `connect_view`.

use std::sync::Arc;

use anyhow::Result;
use auracle_connect_state::{
    ConnectProbe, ConnectView, DEFAULT_ENGINE_URL, VerdictTone, classify_status, connect_view,
};
use futures::AsyncReadExt as _;
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Global, SharedString, Task,
    Window, actions,
};
use serde::{Deserialize, Serialize};
use ui::{CommonAnimationExt as _, Modal, ModalFooter, ModalHeader, prelude::*};
use util::ResultExt as _;
use workspace::{ModalView, Workspace};

actions!(
    auracle,
    [
        /// Open the Connect-to-Auracle dialog.
        Connect
    ]
);

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct AuracleConfig {
    pub engine_url: Option<String>,
    pub api_key: Option<String>,
}

fn config_path() -> std::path::PathBuf {
    paths::config_dir().join("auracle.json")
}

/// Effective configuration: environment overrides file; defaults last.
pub fn load_config() -> AuracleConfig {
    let mut config: AuracleConfig = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if let Ok(url) = std::env::var("AURACLE_ENGINE_URL") {
        config.engine_url = Some(url);
    }
    if let Ok(key) = std::env::var("AURACLE_API_KEY") {
        config.api_key = Some(key);
    }
    if config.engine_url.is_none() {
        config.engine_url = Some(DEFAULT_ENGINE_URL.to_string());
    }
    config
}

pub fn save_config(config: &AuracleConfig) -> Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

/// Bumped on every saved connection so live panels can reconnect.
#[derive(Default)]
pub struct ConnectGeneration(pub u64);

impl Global for ConnectGeneration {}

pub fn init(cx: &mut App) {
    cx.set_global(ConnectGeneration::default());
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &Connect, window, cx| {
            workspace.toggle_modal(window, cx, |window, cx| ConnectModal::new(window, cx));
        });
    })
    .detach();

    // Auto-connect on startup. Without this the IDE only ever connects when
    // the user re-opens the Connect modal and saves, so a saved connection
    // never takes effect on launch. If a saved key exists, verify it in the
    // background and, on success, bump ConnectGeneration so live panels
    // connect just as a manual save would. A missing key or an unreachable
    // engine is intentionally silent here — the user resolves it via the
    // Connect modal — so launch never blocks or spams an error.
    let config = load_config();
    if config.api_key.as_deref().unwrap_or_default().is_empty() {
        return;
    }
    let http = cx.http_client();
    cx.spawn(async move |cx| {
        // Best-effort: a missing key or unreachable engine on startup is
        // expected (the user resolves it via the Connect modal), so only act
        // on a confirmed-Ok probe and otherwise leave the IDE disconnected.
        let probe = test_connection(http, &config).await;
        if matches!(probe, ConnectProbe::Ok { .. }) {
            // `AsyncApp::update` is infallible here, so there is no error to
            // propagate or log — bumping the generation just notifies panels.
            cx.update(|cx| {
                let generation = cx.global::<ConnectGeneration>().0 + 1;
                cx.set_global(ConnectGeneration(generation));
            });
        }
    })
    .detach();
}

pub struct ConnectModal {
    focus_handle: FocusHandle,
    url_editor: Entity<editor::Editor>,
    key_editor: Entity<editor::Editor>,
    // The render decision is derived from these two facts via the reducer's
    // `connect_view`: `testing` is true while a probe task is in flight, and
    // `probe` holds the last classified result once a test returns. Keeping the
    // decision in the reducer means "in-flight is never green" is enforced in
    // one tested place, not in the render path.
    testing: bool,
    probe: Option<ConnectProbe>,
    _test: Option<Task<()>>,
}

impl ConnectModal {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let existing = load_config();
        let url_editor = cx.new(|cx| {
            let mut editor = editor::Editor::single_line(window, cx);
            // Show the default address as placeholder rather than a committed
            // value: an untouched field reads honestly empty, not as a
            // fabricated entry the user never typed. A blank URL resolves to
            // the default at I/O time (`current_input`).
            editor.set_placeholder_text(DEFAULT_ENGINE_URL, window, cx);
            if let Some(url) = existing
                .engine_url
                .clone()
                .filter(|url| !url.is_empty() && url != DEFAULT_ENGINE_URL)
            {
                editor.set_text(url, window, cx);
            }
            editor
        });
        let key_editor = cx.new(|cx| {
            let mut editor = editor::Editor::single_line(window, cx);
            editor.set_placeholder_text("paste your key here", window, cx);
            if let Some(key) = existing.api_key.clone() {
                editor.set_text(key, window, cx);
            }
            editor
        });
        url_editor.update(cx, |editor, cx| editor.focus_handle(cx).focus(window, cx));
        Self {
            focus_handle: cx.focus_handle(),
            url_editor,
            key_editor,
            testing: false,
            probe: None,
            _test: None,
        }
    }

    fn current_input(&self, cx: &App) -> AuracleConfig {
        // A blank URL field means "use the default": resolve it here, at I/O
        // time, so a saved config persists the real address rather than an
        // empty string. The key field is taken verbatim.
        let typed_url = self.url_editor.read(cx).text(cx);
        let engine_url = if typed_url.trim().is_empty() {
            DEFAULT_ENGINE_URL.to_string()
        } else {
            typed_url
        };
        AuracleConfig {
            engine_url: Some(engine_url),
            api_key: Some(self.key_editor.read(cx).text(cx)),
        }
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let input = self.current_input(cx);
        let http = cx.http_client();
        self.testing = true;
        cx.notify();
        self._test = Some(cx.spawn(async move |this, cx| {
            let probe = test_connection(http, &input).await;
            this.update(cx, |this, cx| {
                this.testing = false;
                this.probe = Some(probe);
                cx.notify();
            })
            .log_err();
        }));
    }

    fn save_and_close(&mut self, cx: &mut Context<Self>) {
        let input = self.current_input(cx);
        if save_config(&input).is_ok() {
            let generation = cx.global::<ConnectGeneration>().0 + 1;
            cx.set_global(ConnectGeneration(generation));
        }
        cx.emit(DismissEvent);
    }
}

/// Translate a transport error into a short plain phrase for the user. We never
/// surface the raw `anyhow` chain — it leaks internals and reads as noise to a
/// zero-background user — and it never carries the key or URL, so nothing
/// sensitive escapes here.
fn unreachable_detail(error: &anyhow::Error) -> String {
    // The error's own root cause is usually a one-line transport phrase
    // ("connection refused", "dns error", …); take just that line.
    error
        .root_cause()
        .to_string()
        .lines()
        .next()
        .unwrap_or("the engine did not respond")
        .to_string()
}

/// Probe the engine's connect-check endpoint and classify the outcome into
/// exactly one `ConnectProbe`. All HTTP/JSON I/O lives here; the honesty mapping
/// (probe → verdict text + tone) lives in the reducer.
async fn test_connection(
    http: Arc<dyn http_client::HttpClient>,
    input: &AuracleConfig,
) -> ConnectProbe {
    match probe_engine(http, input).await {
        Ok(probe) => probe,
        // A transport failure means the engine couldn't be reached at all.
        Err(error) => ConnectProbe::Unreachable {
            detail: unreachable_detail(&error),
        },
    }
}

/// The fallible inner probe: build and send the request, classify the status,
/// and on success decode the body defensively. Errors here are transport/decode
/// failures, mapped to `Unreachable` by the caller.
async fn probe_engine(
    http: Arc<dyn http_client::HttpClient>,
    input: &AuracleConfig,
) -> Result<ConnectProbe> {
    let url = input
        .engine_url
        .clone()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENGINE_URL.to_string());
    let key = input.api_key.clone().unwrap_or_default();
    // Hit the engine's purpose-built IDE connect-check endpoint, which reports
    // not just engine+key health but also whether the AI agent (MCP) leg is
    // reachable. Send the key only via the proper `X-API-Key` header: the
    // endpoint honors it standalone, so we never also place the secret in a
    // `Cookie` header where proxies and access logs commonly capture it. The
    // key is never put in the URL or logged.
    let request = http_client::http::Request::builder()
        .uri(format!("{url}/ui/api/ide/connect-check"))
        .header("X-API-Key", key)
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(request).await?;
    // Classify the HTTP status first: a non-success status is a key/engine
    // problem, not an Ok body to decode.
    if let Some(probe) = classify_status(response.status().as_u16()) {
        return Ok(probe);
    }
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    // Read defensively: against an unexpected engine any field may be absent, so
    // hand the reducer an `Option` per field rather than a fabricated fallback —
    // the reducer renders "unknown"/"none yet"/a generic phrase for `None`.
    let engine_version = value
        .get("engine")
        .and_then(|engine| engine.get("version"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let active_broker = value
        .get("active_broker")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let agent_reachable = value
        .get("agent")
        .and_then(|agent| agent.get("reachable"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let agent_detail = value
        .get("agent")
        .and_then(|agent| agent.get("detail"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Ok(ConnectProbe::Ok {
        engine_version,
        active_broker,
        agent_reachable,
        agent_detail,
    })
}

impl EventEmitter<DismissEvent> for ConnectModal {}

impl Focusable for ConnectModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for ConnectModal {}

/// Map a verdict's `VerdictTone` to the theme colour it renders in — only theme
/// `Color::*`, never a colour literal — so text and colour come from the same
/// (tested) verdict and the surface tracks the theme. Mirrors
/// `account_setup::tone_color`.
fn tone_color(tone: VerdictTone) -> Color {
    match tone {
        VerdictTone::Positive => Color::Success,
        VerdictTone::Caution => Color::Warning,
        VerdictTone::Negative => Color::Error,
        VerdictTone::Neutral => Color::Muted,
    }
}

impl Render for ConnectModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = connect_view(self.testing, self.probe.as_ref());

        // While a test is in flight the only honest affordance is to wait, so
        // the test button is disabled. Once a verdict comes back, a failure is
        // re-runnable: re-label the same button "Retry" so a failure is honestly
        // recoverable, and keep "Test" for the idle/success cases.
        let (action_label, action_enabled) = match &view {
            ConnectView::Testing => ("Test", false),
            ConnectView::Done(verdict) if verdict.retryable => ("Retry", true),
            _ => ("Test", true),
        };

        let verdict_row = match view {
            ConnectView::Idle => None,
            // An in-flight test is NEUTRAL, never green: a spinner plus a muted
            // label, not a bare word and never a success colour.
            ConnectView::Testing => Some(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::LoadCircle)
                            .size(IconSize::Small)
                            .color(Color::Muted)
                            .with_rotate_animation(2),
                    )
                    .child(
                        Label::new("Testing the connection…")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ),
            // Text and colour come from the same verdict, so they can never
            // disagree: the reducer decided both.
            ConnectView::Done(verdict) => Some(
                Label::new(SharedString::from(verdict.message))
                    .size(LabelSize::Small)
                    .color(tone_color(verdict.tone))
                    .into_any_element(),
            ),
        };

        v_flex()
            .id("connect-modal")
            .key_context("ConnectModal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| {
                cx.emit(DismissEvent);
            }))
            .w(rems(34.))
            .elevation_3(cx)
            .child(
                Modal::new("connect", None)
                    .header(ModalHeader::new().headline("Connect to your Auracle engine"))
                    .child(
                        v_flex()
                            .px_3()
                            .pb_2()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Label::new("Engine address")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(self.url_editor.clone()),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            "Your key is set up automatically by the desktop \
                                             launcher. To enter it by hand, fetch it from your \
                                             engine's /ui/api/me/credentials page.",
                                        )
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                    )
                                    .child(self.key_editor.clone()),
                            )
                            .when_some(verdict_row, |this, verdict_row| this.child(verdict_row)),
                    )
                    .footer(
                        ModalFooter::new().end_slot(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("connect-test", action_label)
                                        .style(ButtonStyle::Outlined)
                                        .disabled(!action_enabled)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_test(cx);
                                        })),
                                )
                                .child(
                                    Button::new("connect-save", "Save")
                                        .style(ButtonStyle::Filled)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_and_close(cx);
                                        })),
                                ),
                        ),
                    ),
            )
    }
}
