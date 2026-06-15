//! Connect to your Auracle engine — the paste-key surface (B-10).
//!
//! A zero-background user pastes the engine address and their key
//! into two fields, presses Test, reads a plain verdict, and saves.
//! Configuration lives in the fork's own config file
//! (`<config dir>/auracle.json`); environment variables override it
//! for development. The key stays local to this machine (per-seat
//! token rotation is engine roadmap, tracked there).

use std::sync::Arc;

use anyhow::Result;
use futures::AsyncReadExt as _;
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Global, SharedString, Task,
    Window, actions,
};
use serde::{Deserialize, Serialize};
use ui::prelude::*;
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
        config.engine_url = Some("http://127.0.0.1:1969".to_string());
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
            workspace.toggle_modal(window, cx, |window, cx| {
                ConnectModal::new(window, cx)
            });
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
        // on a successful verify and otherwise leave the IDE disconnected.
        if test_connection(http, &config).await.is_ok() {
            cx.update(|cx| {
                let generation = cx.global::<ConnectGeneration>().0 + 1;
                cx.set_global(ConnectGeneration(generation));
            });
        }
    })
    .detach();
}

enum TestState {
    Idle,
    Testing,
    Verdict { ok: bool, plain: SharedString },
}

pub struct ConnectModal {
    focus_handle: FocusHandle,
    url_editor: Entity<editor::Editor>,
    key_editor: Entity<editor::Editor>,
    state: TestState,
    _test: Option<Task<()>>,
}

impl ConnectModal {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let existing = load_config();
        let url_editor = cx.new(|cx| {
            let mut e = editor::Editor::single_line(window, cx);
            e.set_text(
                existing.engine_url.clone().unwrap_or_default(),
                window,
                cx,
            );
            e
        });
        let key_editor = cx.new(|cx| {
            let mut e = editor::Editor::single_line(window, cx);
            e.set_placeholder_text("paste your key here", window, cx);
            if let Some(key) = existing.api_key.clone() {
                e.set_text(key, window, cx);
            }
            e
        });
        url_editor.update(cx, |e, cx| e.focus_handle(cx).focus(window, cx));
        Self {
            focus_handle: cx.focus_handle(),
            url_editor,
            key_editor,
            state: TestState::Idle,
            _test: None,
        }
    }

    fn current_input(&self, cx: &App) -> AuracleConfig {
        AuracleConfig {
            engine_url: Some(self.url_editor.read(cx).text(cx)),
            api_key: Some(self.key_editor.read(cx).text(cx)),
        }
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let input = self.current_input(cx);
        let http = cx.http_client();
        self.state = TestState::Testing;
        cx.notify();
        self._test = Some(cx.spawn(async move |this, cx| {
            let plain = test_connection(http, &input).await;
            this.update(cx, |this, cx| {
                this.state = TestState::Verdict {
                    ok: plain.is_ok(),
                    plain: SharedString::from(match plain {
                        Ok(p) => p,
                        Err(e) => format!(
                            "Couldn't connect: {e}. Check the address, the \
                             key, and that your engine is running."
                        ),
                    }),
                };
                cx.notify();
            })
            .ok();
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

async fn test_connection(
    http: Arc<dyn http_client::HttpClient>,
    input: &AuracleConfig,
) -> Result<String> {
    let url = input
        .engine_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:1969".into());
    let key = input.api_key.clone().unwrap_or_default();
    // Hit the engine's purpose-built IDE connect-check endpoint, which
    // reports not just engine+key health but also whether the AI agent
    // (MCP) leg is reachable. Send the key via the proper `X-API-Key`
    // header, and also as the `auracle_session` cookie so this still
    // works against an older engine that only honors the cookie — both
    // headers carry the same key.
    let request = http_client::http::Request::builder()
        .uri(format!("{url}/ui/api/ide/connect-check"))
        .header("X-API-Key", key.clone())
        .header("Cookie", format!("auracle_session={key}"))
        .body(http_client::AsyncBody::default())?;
    let mut response = http.send(request).await?;
    if response.status().as_u16() == 401 || response.status().as_u16() == 302 {
        anyhow::bail!("the engine answered, but the key was not accepted");
    }
    if !response.status().is_success() {
        anyhow::bail!("the engine answered with status {}", response.status());
    }
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    // Read defensively: against an unexpected engine any field may be
    // absent, so fall back rather than unwrap.
    let version = value
        .get("engine")
        .and_then(|engine| engine.get("version"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let active = value
        .get("active_broker")
        .and_then(|value| value.as_str())
        .unwrap_or("none yet");
    let agent_reachable = value
        .get("agent")
        .and_then(|agent| agent.get("reachable"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let agent_detail = value
        .get("agent")
        .and_then(|agent| agent.get("detail"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    // Surface BOTH legs honestly: the engine+key verdict and a separate
    // line of truth about the AI agent, so an unreachable agent never
    // reads as a fully-ready setup.
    let mut verdict = format!(
        "Connected — engine v{version} is up and your key works \
         (active broker: {active})."
    );
    if agent_reachable {
        verdict.push_str(" Your AI agent is reachable.");
    } else {
        let detail = if agent_detail.is_empty() {
            "the engine couldn't reach the MCP agent server"
        } else {
            agent_detail
        };
        verdict.push_str(&format!(
            " Note: the AI agent isn't reachable yet ({detail})."
        ));
    }
    Ok(verdict)
}

impl EventEmitter<DismissEvent> for ConnectModal {}

impl Focusable for ConnectModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for ConnectModal {}

impl Render for ConnectModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Color the verdict honestly: an in-flight test is NEUTRAL, not
        // green — green is reserved for an engine-confirmed success.
        let verdict: Option<(Color, SharedString)> = match &self.state {
            TestState::Idle => None,
            TestState::Testing => Some((Color::Muted, "Testing…".into())),
            TestState::Verdict { ok, plain } => Some((
                if *ok { Color::Success } else { Color::Error },
                plain.clone(),
            )),
        };

        v_flex()
            .key_context("ConnectModal")
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
            .child(Label::new("Connect to your Auracle engine"))
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
                            "Your key — open http://127.0.0.1:1969/ui/account \
                             in your browser and copy the key shown there",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .child(self.key_editor.clone()),
            )
            .when_some(verdict, |this, (color, plain)| {
                this.child(Label::new(plain).size(LabelSize::Small).color(color))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("connect-test", "Test")
                            .style(ButtonStyle::Outlined)
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
            )
    }
}
