//! The native, inline Auracle settings surface — a docked panel, not a modal.
//!
//! Everything the first-run wizard walked an operator through (connect a
//! broker, pick an AI model + key, sign in to GitHub) lives here as a
//! persistent, non-blocking panel. The wizard's blocking modal is demoted to a
//! dismissible first-run banner (see `auracle_onboarding.rs`) that deep-links
//! here via the [`OpenConnections`] action.
//!
//! Honesty laws (mirroring `auracle_connections` and the wizard):
//!   * the broker section reuses `auracle_connections`' transport, Test, and
//!     Save verbatim — a broker is never shown "connected" without a real
//!     successful round-trip;
//!   * the AI section reads "configured" only from `is_authenticated`, never a
//!     local guess; if a provider the engine designated can't be authenticated
//!     even after a key import, it says so rather than faking a ready state;
//!   * the GitHub section probes the OS git credential helper for real
//!     (`git credential fill`) and never claims a sign-in it can't observe;
//!   * the read-only "shared truths" come straight from `GET /ui/api/settings`.
//!
//! Cross-store sync (W5): on load the panel imports the engine's designated
//! AI-provider key into the IDE keychain when that provider isn't yet
//! authenticated locally, and when the operator sets a model here it mirrors
//! the choice back up to the engine so the launcher reflects it.

use std::sync::Arc;

use agent_settings::{AgentSettings, language_model_to_selection};
use anyhow::Result;
use futures::AsyncWriteExt as _;
use gpui::{
    AnyView, App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use language_model::{LanguageModelProvider, LanguageModelRegistry, ZED_CLOUD_PROVIDER_ID};
use settings::{Settings as _, update_settings_file};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use auracle_connections::{
    AiModelState, BrokerSummary, Capability, FieldMeta, SharedSettings,
};

actions!(
    auracle,
    [
        /// Open the native Auracle settings panel (connections, AI model, GitHub).
        OpenConnections
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        // Mirror `blotter_panel::init` (blotter_panel.rs:33-40): register the
        // toggle action so the panel is reachable from the command palette and
        // the first-run banner's deep-link.
        workspace.register_action(|workspace, _: &OpenConnections, window, cx| {
            workspace.focus_panel::<AuracleSettingsPanel>(window, cx);
        });
    })
    .detach();
}

// ── Broker sub-state (reuses auracle_connections transport) ───────────

#[derive(Clone, Copy, PartialEq)]
enum BrokerPhase {
    Choose,
    Credentials,
}

enum TestState {
    Idle,
    Working,
    Verdict { ok: bool, plain: SharedString },
}

// ── GitHub sub-state ──────────────────────────────────────────────────

enum GitHubAuthState {
    Unknown,
    Checking,
    SignedIn(SharedString),
    SignedOut,
}

// ── AI-model sub-state ────────────────────────────────────────────────

enum ModelStatus {
    Idle,
    Working,
    /// A message plus whether the underlying default is actually set/usable.
    Verdict { ok: bool, plain: SharedString },
}

pub struct AuracleSettingsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,

    // Broker section
    broker_phase: BrokerPhase,
    brokers: Vec<BrokerSummary>,
    selected_broker: Option<String>,
    fields: Vec<FieldMeta>,
    /// Fixed pool of single-line editors bound to `fields` by index, so no
    /// entity is created after construction. Mirrors `BrokerWizard`'s pool
    /// (auracle_connections.rs:152-157).
    editors: Vec<Entity<editor::Editor>>,
    selections: std::collections::HashMap<String, String>,
    capability: Option<Capability>,
    broker_saved: bool,
    test_state: TestState,

    // AI model section
    provider_view: Option<(SharedString, AnyView)>,
    model_id_editor: Entity<editor::Editor>,
    model_status: ModelStatus,

    // GitHub section
    git_name_editor: Entity<editor::Editor>,
    git_email_editor: Entity<editor::Editor>,
    git_identity_saved: bool,
    github_state: GitHubAuthState,

    // Read-only shared truths from the engine.
    shared: Option<SharedSettings>,

    /// Single-flight slot for the broker sub-flow (list/select/test/save). A new
    /// broker action cancels the prior one — matching the wizard's `_task`.
    _broker_task: Option<Task<()>>,
    /// Single-flight slot for the shared-truths read + AI-key import. Re-run on
    /// reconnect (ConnectGeneration), so a new read cancels the prior.
    _shared_task: Option<Task<()>>,
    /// Single-flight slot for the GitHub probe / sign-in.
    _github_task: Option<Task<()>>,
    /// Best-effort side task for the model MIRROR PUT, kept off the other slots
    /// so a follow-up broker/github action doesn't cancel an in-flight mirror.
    _mirror_task: Option<Task<()>>,
}

impl AuracleSettingsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        // Mirror `BlotterPanel::load` (blotter_panel.rs:78-107): build the
        // entity inside `workspace.update_in` so editors get a real `Window`.
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let weak = workspace.weak_handle();
            cx.new(|cx| {
                // Reconnecting (a new saved engine key) reloads the shared
                // truths and re-runs the import, just as the blotter reloads
                // on `ConnectGeneration` (blotter_panel.rs:84-91).
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.load_shared_and_import(cx);
                })
                .detach();

                let mut editors = Vec::with_capacity(auracle_connections::MAX_FIELDS);
                for _ in 0..auracle_connections::MAX_FIELDS {
                    editors.push(cx.new(|cx| editor::Editor::single_line(window, cx)));
                }
                let mut this = Self {
                    focus_handle: cx.focus_handle(),
                    workspace: weak,
                    broker_phase: BrokerPhase::Choose,
                    brokers: Vec::new(),
                    selected_broker: None,
                    fields: Vec::new(),
                    editors,
                    selections: std::collections::HashMap::new(),
                    capability: None,
                    broker_saved: false,
                    test_state: TestState::Idle,
                    provider_view: None,
                    model_id_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    model_status: ModelStatus::Idle,
                    git_name_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    git_email_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    git_identity_saved: false,
                    github_state: GitHubAuthState::Unknown,
                    shared: None,
                    _broker_task: None,
                    _shared_task: None,
                    _github_task: None,
                    _mirror_task: None,
                };
                this.load_brokers(cx);
                this.load_shared_and_import(cx);
                this.check_github(cx);
                this
            })
        })
    }

    fn fs(&self, cx: &App) -> Option<Arc<dyn fs::Fs>> {
        let workspace = self.workspace.upgrade()?;
        Some(workspace.read(cx).project().read(cx).fs().clone())
    }

    // ── Shared truths + AI-key import (cross-store sync) ───────────────

    /// Read `GET /ui/api/settings` to refresh the read-only truths, then, if
    /// the engine designated an AI provider that the IDE hasn't authenticated,
    /// pull that provider's plaintext key over loopback and import it into the
    /// IDE keychain via `provider.set_api_key(Some(key), cx)`. This is the
    /// engine→IDE half of true cross-store sync (W5 step 5).
    fn load_shared_and_import(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._shared_task = Some(cx.spawn(async move |this, cx| {
            let shared = auracle_connections::get_settings(http.clone()).await.ok();
            this.update(cx, |this, cx| {
                this.shared = shared.clone();
                cx.notify();
            })
            .ok();

            let Some(shared) = shared else {
                return;
            };
            let provider_id = shared.ai_model.provider.trim().to_string();
            if provider_id.is_empty() {
                return;
            }

            // Find the matching visible provider. `AsyncApp::update` returns
            // the closure value directly (async_context.rs:163), so the lookup
            // comes back unwrapped.
            let Some(provider) = cx.update(|cx| find_provider(&provider_id, cx)).flatten() else {
                return;
            };

            // Drive the keychain load before reading `is_authenticated`, to
            // avoid the cold-start race (mirrors `resolve_seed_model` in
            // auracle_onboarding.rs:214: `cx.update(|cx| provider.authenticate
            // (cx)).await.ok()`). If already authenticated, nothing to import.
            cx.update(|cx| provider.authenticate(cx)).await.log_err();
            if cx.update(|cx| provider.is_authenticated(cx)) {
                return;
            }

            // Pull the engine's key (loopback-only handoff) and import it. A
            // 404 (engine holds no key) surfaces as an error we treat as
            // "nothing to import" — never a fake-authenticated state.
            let Ok((_provider, key)) =
                auracle_connections::fetch_ai_key(http, &provider_id).await
            else {
                return;
            };
            cx.update(|cx| provider.set_api_key(Some(key), cx))
                .await
                .log_err();
            // Re-authenticate so the freshly-imported key is loaded, then nudge
            // a redraw so the provider's "configured" tick reflects it.
            cx.update(|cx| provider.authenticate(cx)).await.log_err();
            this.update(cx, |_this, cx| cx.notify()).ok();
        }));
    }

    // ── Broker section (transport reused from auracle_connections) ─────

    fn load_brokers(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._broker_task = Some(cx.spawn(async move |this, cx| {
            let brokers = auracle_connections::list_brokers(http)
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.brokers = brokers;
                cx.notify();
            })
            .ok();
        }));
    }

    fn select_broker(&mut self, broker: String, cx: &mut Context<Self>) {
        self.selected_broker = Some(broker.clone());
        self.broker_phase = BrokerPhase::Credentials;
        self.test_state = TestState::Idle;
        self.fields = Vec::new();
        self.capability = None;
        self.broker_saved = false;
        cx.notify();
        let http = cx.http_client();
        self._broker_task = Some(cx.spawn(async move |this, cx| {
            let fields = auracle_connections::get_fields(http.clone(), &broker)
                .await
                .unwrap_or_default();
            let capability = auracle_connections::get_capability(http, &broker)
                .await
                .ok();
            this.update(cx, |this, cx| {
                this.selections.clear();
                for field in &fields {
                    if field.kind == "select" {
                        if let Some(first) = field.options.first() {
                            this.selections.insert(field.name.clone(), first.clone());
                        }
                    }
                }
                this.fields = fields;
                this.capability = capability;
                cx.notify();
            })
            .ok();
        }));
    }

    fn current_body(&self, cx: &App) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (index, field) in self.fields.iter().enumerate() {
            if field.kind == "select" {
                if let Some(choice) = self.selections.get(&field.name) {
                    map.insert(field.name.clone(), serde_json::Value::String(choice.clone()));
                }
                continue;
            }
            if index >= self.editors.len() {
                continue;
            }
            let value = self.editors[index].read(cx).text(cx);
            if !value.is_empty() {
                map.insert(field.name.clone(), serde_json::Value::String(value));
            }
        }
        serde_json::Value::Object(map)
    }

    fn run_broker_test(&mut self, cx: &mut Context<Self>) {
        let Some(broker) = self.selected_broker.clone() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._broker_task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::test_connection(http, &broker, body).await;
            this.update(cx, |this, cx| {
                this.test_state = TestState::Verdict {
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

    fn save_broker(&mut self, cx: &mut Context<Self>) {
        let Some(broker) = self.selected_broker.clone() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._broker_task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::save_connection(http, &broker, body).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        // Bump the generation just like the wizard's save
                        // (auracle_connections.rs:278-281) so live panels
                        // reconnect against the new connection.
                        let generation = cx.global::<auracle_connect::ConnectGeneration>().0 + 1;
                        cx.set_global(auracle_connect::ConnectGeneration(generation));
                        this.broker_saved = true;
                        this.test_state = TestState::Verdict {
                            ok: true,
                            plain: "Saved — this broker is connected.".into(),
                        };
                    }
                    Err(error) => {
                        this.test_state = TestState::Verdict {
                            ok: false,
                            plain: SharedString::from(format!("Couldn't save: {error}.")),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ── AI model section ──────────────────────────────────────────────

    fn select_provider(
        &mut self,
        provider: Arc<dyn LanguageModelProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = provider.configuration_view(
            language_model::ConfigurationViewTargetAgent::ZedAgent,
            window,
            cx,
        );
        self.provider_view = Some((provider.name().0, view));
        cx.notify();
    }

    /// Set the IDE default agent model from the selected provider, then mirror
    /// the choice up to the engine (W5 step 7). Mirrors the agent settings UI's
    /// `update_settings_file` write of `default_model` and the wizard's
    /// `set_default_model`.
    fn set_default_model(&mut self, cx: &mut Context<Self>) {
        let Some((provider_name, _)) = self.provider_view.clone() else {
            self.model_status = ModelStatus::Verdict {
                ok: false,
                plain: "Pick a provider first.".into(),
            };
            cx.notify();
            return;
        };
        let Some(provider) = LanguageModelRegistry::read_global(cx)
            .visible_providers()
            .into_iter()
            .find(|provider| provider.name().0 == provider_name)
        else {
            self.model_status = ModelStatus::Verdict {
                ok: false,
                plain: "That provider is no longer available.".into(),
            };
            cx.notify();
            return;
        };
        if !provider.is_authenticated(cx) {
            // Honesty (mirrors auracle_onboarding.rs:419 hint): never claim a
            // default is set when the provider can't authenticate.
            self.model_status = ModelStatus::Verdict {
                ok: false,
                plain: "Add this provider's API key to use the shared default.".into(),
            };
            cx.notify();
            return;
        }
        let typed = self.model_id_editor.read(cx).text(cx);
        let model = if typed.is_empty() {
            provider
                .default_model(cx)
                .or_else(|| provider.recommended_models(cx).first().cloned())
                .or_else(|| provider.provided_models(cx).first().cloned())
        } else {
            provider
                .provided_models(cx)
                .into_iter()
                .find(|model| model.id().0.as_ref() == typed.as_str())
        };
        let Some(model) = model else {
            self.model_status = ModelStatus::Verdict {
                ok: false,
                plain: "Couldn't find that model for this provider.".into(),
            };
            cx.notify();
            return;
        };
        let Some(fs) = self.fs(cx) else {
            self.model_status = ModelStatus::Verdict {
                ok: false,
                plain: "Couldn't reach the settings file.".into(),
            };
            cx.notify();
            return;
        };
        let current = AgentSettings::get_global(cx).default_model.clone();
        let selection = language_model_to_selection(&model, current.as_ref());
        let provider_id = provider.id().0.to_string();
        let model_id = model.id().0.to_string();
        let label = SharedString::from(format!("Default model set to {provider_name} · {model_id}."));
        update_settings_file(fs, cx, move |settings, _cx| {
            let agent = settings.agent.get_or_insert_default();
            agent.default_model = Some(selection);
        });
        self.model_status = ModelStatus::Verdict {
            ok: true,
            plain: label,
        };
        cx.notify();

        // MIRROR (IDE→engine): best-effort PUT so the launcher reflects the
        // same model. Non-fatal on failure — the local write already succeeded.
        // The IDE keychain key is not readable back through the provider trait,
        // so we mirror the *selection* (`{provider, model_id}`) without a key;
        // the engine authenticates with the operator's engine-side key (the
        // same one the SEED/import path uses), so the cross-store default stays
        // consistent without round-tripping a secret the IDE can't read.
        let http = cx.http_client();
        self._mirror_task = Some(cx.spawn(async move |_this, _cx| {
            auracle_connections::put_ai_model(http, &provider_id, &model_id, None)
                .await
                .log_err();
        }));
    }

    // ── GitHub section ────────────────────────────────────────────────

    /// Probe the OS git credential helper for a github.com login. The launcher's
    /// device-flow writes the token there and the IDE's git inherits it, so this
    /// is the shared store. We shell `git credential fill` (mirrors how the repo
    /// shells git via `util::command::new_command`, e.g. git/repository.rs) and
    /// read whether a username comes back. Honest: a present credential reads as
    /// signed-in, an absent one as signed-out, never a fake "connected".
    fn check_github(&mut self, cx: &mut Context<Self>) {
        self.github_state = GitHubAuthState::Checking;
        cx.notify();
        self._github_task = Some(cx.spawn(async move |this, cx| {
            let signed_in = probe_github_credential().await;
            this.update(cx, |this, cx| {
                this.github_state = match signed_in {
                    Some(user) => GitHubAuthState::SignedIn(SharedString::from(format!(
                        "Signed in to GitHub as {user}."
                    ))),
                    None => GitHubAuthState::SignedOut,
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_git_identity(&mut self, cx: &mut Context<Self>) {
        let name = self.git_name_editor.read(cx).text(cx);
        let email = self.git_email_editor.read(cx).text(cx);
        if name.is_empty() && email.is_empty() {
            return;
        }
        self._github_task = Some(cx.spawn(async move |this, cx| {
            if !name.is_empty() {
                util::command::new_command("git")
                    .args(["config", "--global", "user.name", &name])
                    .output()
                    .await
                    .log_err();
            }
            if !email.is_empty() {
                util::command::new_command("git")
                    .args(["config", "--global", "user.email", &email])
                    .output()
                    .await
                    .log_err();
            }
            this.update(cx, |this, cx| {
                this.git_identity_saved = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn sign_in_github(&mut self, cx: &mut Context<Self>) {
        // Prefer the `gh` device flow if the CLI is present; otherwise open the
        // device-code page directly. Either way the launcher's sign-in also
        // works since both write the same OS credential. We never fake the
        // session — "Check status" reflects the real probe afterward. Kept on
        // `_mirror_task` (not `_github_task`) so the trailing `check_github`,
        // which writes `_github_task`, doesn't drop this still-running task.
        self._mirror_task = Some(cx.spawn(async move |this, cx| {
            let gh_available = util::command::new_command("gh")
                .args(["--version"])
                .output()
                .await
                .map(|output| output.status.success())
                .unwrap_or(false);
            if gh_available {
                util::command::new_command("gh")
                    .args(["auth", "login", "--web", "--git-protocol", "https"])
                    .output()
                    .await
                    .log_err();
                this.update(cx, |this, cx| this.check_github(cx)).ok();
            } else {
                cx.update(|cx| cx.open_url("https://github.com/login/device"));
            }
        }));
    }

    fn back_to_broker_choice(&mut self, cx: &mut Context<Self>) {
        self.broker_phase = BrokerPhase::Choose;
        self.selected_broker = None;
        self.test_state = TestState::Idle;
        cx.notify();
    }
}

/// Find a visible, non-cloud provider by its provider id (e.g. "anthropic").
fn find_provider(provider_id: &str, cx: &App) -> Option<Arc<dyn LanguageModelProvider>> {
    LanguageModelRegistry::read_global(cx)
        .visible_providers()
        .into_iter()
        .find(|provider| {
            provider.id().0.as_ref() == provider_id && provider.id() != ZED_CLOUD_PROVIDER_ID
        })
}

/// Probe `git credential fill` for github.com over stdin. Mirrors the
/// stdin-piping pattern in `util::command` (command/darwin.rs:648-669:
/// `.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()`, then
/// `write_all`/`close`). Returns the username on success, `None` otherwise.
async fn probe_github_credential() -> Option<String> {
    let mut command = util::command::new_command("git");
    command
        .args(["credential", "fill"])
        .stdin(util::command::Stdio::piped())
        .stdout(util::command::Stdio::piped())
        .stderr(util::command::Stdio::null());
    let mut child = command.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        // The blank line terminates the credential request.
        stdin
            .write_all(b"protocol=https\nhost=github.com\n\n")
            .await
            .log_err();
        stdin.close().await.log_err();
    }
    let output = child.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut username = None;
    let mut has_password = false;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("username=") {
            username = Some(value.trim().to_string());
        } else if line.starts_with("password=") {
            has_password = true;
        }
    }
    // Only report signed-in when a real credential (a password/token) came
    // back, not merely an echoed host.
    if has_password {
        Some(username.unwrap_or_else(|| "your GitHub account".to_string()))
    } else {
        None
    }
}

// ── Panel trait impls (mirror BlotterPanel) ──────────────────────────

impl EventEmitter<PanelEvent> for AuracleSettingsPanel {}

impl Focusable for AuracleSettingsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AuracleSettingsPanel {
    fn persistent_name() -> &'static str {
        "AuracleSettingsPanel"
    }

    fn panel_key() -> &'static str {
        "AuracleSettingsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
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
        px(420.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Settings)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Auracle setup — connections, AI model, GitHub")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(OpenConnections)
    }

    fn activation_priority(&self) -> u32 {
        14
    }
}

// ── Render ────────────────────────────────────────────────────────────

impl AuracleSettingsPanel {
    fn render_broker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_3()
            .child(Label::new("Connect a broker").size(LabelSize::Large));
        match self.broker_phase {
            BrokerPhase::Choose => {
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
                body = body.child(list);
            }
            BrokerPhase::Credentials => {
                let broker = self.selected_broker.clone().unwrap_or_default();
                body = body.child(Label::new(format!("Connect {broker}")));
                if broker == "ibkr" {
                    body = body.child(
                        Label::new(
                            "IBKR needs the IB Gateway or Client Portal running and logged in \
                             before these credentials will verify.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    );
                }
                let mut form = v_flex().gap_3();
                if self.fields.is_empty() {
                    form = form.child(
                        Label::new("Loading…")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                }
                for (index, field) in self.fields.iter().enumerate() {
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
                    let input = if field.kind == "select" {
                        let selected =
                            self.selections.get(&field.name).cloned().unwrap_or_default();
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
                    } else if index < self.editors.len() {
                        self.editors[index].clone().into_any_element()
                    } else {
                        Label::new("Set this field from the web console for now.")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .into_any_element()
                    };
                    form = form.child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(input),
                    );
                }
                body = body.child(form);
                // Capability chips — honesty mirrors `BrokerWizard::render_confirm`
                // (auracle_connections.rs:575-633): chips come ONLY from the
                // engine's verified capability; an "unknown" leg reads as "not
                // verified yet", never green.
                if let Some(capability) = &self.capability {
                    let mut chips = h_flex().gap_2();
                    for leg in ["data", "paper", "live"] {
                        let state = capability
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
                    if !capability.asset_kinds.is_empty() {
                        body = body.child(
                            Label::new(format!("Trades: {}", capability.asset_kinds.join(", ")))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        );
                    }
                }
                body = body.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("settings-broker-test", "Test")
                                .style(ButtonStyle::Outlined)
                                .on_click(cx.listener(|this, _, _, cx| this.run_broker_test(cx))),
                        )
                        .child(
                            Button::new("settings-broker-save", "Connect")
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(|this, _, _, cx| this.save_broker(cx))),
                        )
                        .child(
                            Button::new("settings-broker-other", "Pick another broker")
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.back_to_broker_choice(cx);
                                })),
                        ),
                );
            }
        }
        let verdict: Option<(Color, SharedString)> = match &self.test_state {
            TestState::Idle => None,
            TestState::Working => Some((Color::Muted, "Working…".into())),
            TestState::Verdict { ok, plain } => Some((
                if *ok { Color::Success } else { Color::Error },
                plain.clone(),
            )),
        };
        body.when_some(verdict, |this, (color, plain)| {
            this.child(Label::new(plain).size(LabelSize::Small).color(color))
        })
    }

    fn render_model(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_3()
            .child(Label::new("AI model").size(LabelSize::Large))
            .child(
                Label::new("Pick a provider and add its API key")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

        let mut providers_row = h_flex().gap_2().flex_wrap();
        let providers = LanguageModelRegistry::read_global(cx).visible_providers();
        for provider in providers {
            if provider.id() == ZED_CLOUD_PROVIDER_ID {
                continue;
            }
            let name = provider.name().0;
            let authenticated = provider.is_authenticated(cx);
            let is_selected = self
                .provider_view
                .as_ref()
                .is_some_and(|(selected, _)| selected == &name);
            let label = if authenticated {
                format!("{name} ✓")
            } else {
                name.to_string()
            };
            let provider_for_click = provider.clone();
            providers_row = providers_row.child(
                Button::new(SharedString::from(format!("settings-prov-{name}")), label)
                    .style(if is_selected {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Outlined
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_provider(provider_for_click.clone(), window, cx);
                    })),
            );
        }
        body = body.child(providers_row);

        if let Some((name, view)) = self.provider_view.clone() {
            body = body
                .child(
                    Label::new(format!("Configure {name}"))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(view)
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            Label::new("Model id to use as default (optional)")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(self.model_id_editor.clone()),
                )
                .child(
                    Button::new("settings-set-default-model", "Set as default model")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| this.set_default_model(cx))),
                );
        } else {
            body = body.child(
                Label::new("Select a provider above to add a key.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }

        let verdict: Option<(Color, SharedString)> = match &self.model_status {
            ModelStatus::Idle => None,
            ModelStatus::Working => Some((Color::Muted, "Working…".into())),
            ModelStatus::Verdict { ok, plain } => Some((
                if *ok { Color::Success } else { Color::Warning },
                plain.clone(),
            )),
        };
        body.when_some(verdict, |this, (color, plain)| {
            this.child(Label::new(plain).size(LabelSize::Small).color(color))
        })
    }

    fn render_github(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status: (Color, SharedString) = match &self.github_state {
            GitHubAuthState::Unknown => (Color::Muted, "GitHub status not checked yet.".into()),
            GitHubAuthState::Checking => (Color::Muted, "Checking…".into()),
            GitHubAuthState::SignedIn(line) => (Color::Success, line.clone()),
            GitHubAuthState::SignedOut => (
                Color::Warning,
                "Not signed in to GitHub (no credential for github.com).".into(),
            ),
        };
        v_flex()
            .gap_3()
            .child(Label::new("Git identity and GitHub").size(LabelSize::Large))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Your name")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(self.git_name_editor.clone()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Your email")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(self.git_email_editor.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("settings-git-save", "Save git identity")
                            .style(ButtonStyle::Outlined)
                            .on_click(cx.listener(|this, _, _, cx| this.save_git_identity(cx))),
                    )
                    .when(self.git_identity_saved, |row| {
                        row.child(
                            Label::new("Saved with git config.")
                                .size(LabelSize::Small)
                                .color(Color::Success),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("settings-git-signin", "Sign in to GitHub")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, _, cx| this.sign_in_github(cx))),
                    )
                    .child(
                        Button::new("settings-git-check", "Check status")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| this.check_github(cx))),
                    ),
            )
            .child(
                Label::new(
                    "Signing in via the launcher also works — it shares the same git \
                     credential.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(Label::new(status.1).size(LabelSize::Small).color(status.0))
    }

    fn render_shared_truths(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_2()
            .child(Label::new("Shared with the launcher").size(LabelSize::Large));
        match &self.shared {
            None => {
                body = body.child(
                    Label::new("Reading shared settings from the engine…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
            }
            Some(shared) => {
                if !shared.tier.is_empty() {
                    body = body.child(
                        Label::new(format!("Tier: {}", shared.tier))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                }
                if let Some((text, color)) = ai_truth_row(&shared.ai_model) {
                    body = body.child(Label::new(text).size(LabelSize::Small).color(color));
                }
                if shared.data_keys.is_empty() {
                    body = body.child(
                        Label::new("No data-source keys configured yet.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                } else {
                    for (provider, state) in &shared.data_keys {
                        let (text, color) = if state.configured {
                            (format!("{provider}: key configured"), Color::Success)
                        } else {
                            (format!("{provider}: no key yet"), Color::Muted)
                        };
                        body = body.child(Label::new(text).size(LabelSize::Small).color(color));
                    }
                }
            }
        }
        body
    }
}

/// The engine's designated AI model as a read-only truth row, or `None` when
/// the engine has no AI model set.
fn ai_truth_row(ai_model: &AiModelState) -> Option<(String, Color)> {
    if ai_model.provider.is_empty() {
        return None;
    }
    Some(if ai_model.configured {
        (
            format!(
                "Engine default AI: {} · {} (key configured)",
                ai_model.provider, ai_model.model_id
            ),
            Color::Success,
        )
    } else {
        (
            format!(
                "Engine default AI: {} · {} (no key — add one above)",
                ai_model.provider, ai_model.model_id
            ),
            Color::Warning,
        )
    })
}

impl Render for AuracleSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("AuracleSettingsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .gap_4()
            .overflow_y_scroll()
            .bg(cx.theme().colors().panel_background)
            .child(self.render_broker(cx))
            .child(self.render_model(cx))
            .child(self.render_github(cx))
            .child(self.render_shared_truths(cx))
    }
}
