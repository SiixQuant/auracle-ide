//! The native, inline Auracle connections surface — a docked panel, not a modal.
//!
//! This is the ONE connect surface. Every connection the operator manages —
//! their account, execution brokers, market-data providers, and the
//! QuantConnect bridge — lives here as a stack of collapsible sections. A
//! section's toggle IS its disclosure: flipping it open/closed never connects
//! or disconnects anything (the honest truth lives in the per-section status
//! summary). Brokers and data sources are master-detail (a list of connectors,
//! each opening an inline credentials detail); the rest are single panels.
//! The AI-model and Git/GitHub setup sections ride along as collapsible
//! sections so first-run setup stays in one place.
//!
//! Honesty laws (mirroring `auracle_connections`):
//!   * a connector reads "connected" ONLY on a real engine status — never a
//!     local guess; Test/Save go through a real round-trip;
//!   * a connector with no engine Test probe shows Save-only (no fake green);
//!   * a tier-gated connector says so and offers no dead credential form;
//!   * sensitive values are never fetched — only a masked preview is shown.
//!
//! Cross-store sync (W5): on load the panel imports the engine's designated
//! AI-provider key into the IDE keychain when that provider isn't yet
//! authenticated locally, and when the operator sets a model here it mirrors
//! the choice back up to the engine so the launcher reflects it.

use std::collections::HashSet;
use std::sync::Arc;

use agent_settings::{AgentSettings, language_model_to_selection};
use anyhow::Result;
use futures::AsyncWriteExt as _;
use gpui::{
    AnyElement, AnyView, App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable,
    Pixels, SharedString, Task, WeakEntity, Window, actions, px,
};
use language_model::{LanguageModelProvider, LanguageModelRegistry, ZED_CLOUD_PROVIDER_ID};
use settings::{Settings as _, update_settings_file};
use ui::Switch;
use ui::prelude::*;
use util::ResultExt as _;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use auracle_connections::{
    Account, Capability, Connector, FieldMeta, SharedSettings, brand_tile, broker_logo_path,
    default_expanded, section_summary,
};

actions!(
    auracle,
    [
        /// Open the native Auracle connections panel (account, brokers, data
        /// sources, QuantConnect, AI model, GitHub).
        OpenConnections
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        // Both the settings deep-link (`OpenConnections`) and the deploy /
        // legacy connect path (`OpenBrokerWizard`, kept for back-compat after
        // the modal's retirement) focus the one docked panel — there is no
        // separate wizard surface anymore.
        workspace.register_action(|workspace, _: &OpenConnections, window, cx| {
            workspace.focus_panel::<AuracleSettingsPanel>(window, cx);
        });
        workspace.register_action(
            |workspace, _: &auracle_connections::OpenBrokerWizard, window, cx| {
                workspace.focus_panel::<AuracleSettingsPanel>(window, cx);
            },
        );
    })
    .detach();
}

// ── Sections ──────────────────────────────────────────────────────────

/// The panel's collapsible sections, top to bottom. `Broker` and `Data` are
/// master-detail connector sections; `QuantConnect` is a single-connector
/// section; the rest are panels.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Account,
    Broker,
    Data,
    QuantConnect,
    AiModel,
    GitHub,
}

impl Section {
    /// A stable element-id key (also the Switch id).
    fn key(self) -> &'static str {
        match self {
            Section::Account => "sec-account",
            Section::Broker => "sec-broker",
            Section::Data => "sec-data",
            Section::QuantConnect => "sec-qc",
            Section::AiModel => "sec-ai",
            Section::GitHub => "sec-github",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Section::Account => "Account",
            Section::Broker => "Broker",
            Section::Data => "Data sources",
            Section::QuantConnect => "QuantConnect",
            Section::AiModel => "AI model",
            Section::GitHub => "Git & GitHub",
        }
    }

    const ALL: [Section; 6] = [
        Section::Account,
        Section::Broker,
        Section::Data,
        Section::QuantConnect,
        Section::AiModel,
        Section::GitHub,
    ];
}

// ── Sub-state ─────────────────────────────────────────────────────────

enum TestState {
    Idle,
    Working,
    Verdict { ok: bool, plain: SharedString },
}

enum GitHubAuthState {
    Unknown,
    Checking,
    SignedIn(SharedString),
    SignedOut,
}

enum ModelStatus {
    Idle,
    /// A message plus whether the underlying default is actually set/usable.
    Verdict {
        ok: bool,
        plain: SharedString,
    },
}

pub struct AuracleSettingsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,

    // Section disclosure
    expanded: HashSet<Section>,
    /// One-shot guard so the first connector load can auto-open sections that
    /// need attention without ever fighting the operator's later toggles.
    did_auto_expand: bool,

    // Connectors per kind
    brokers: Vec<Connector>,
    data_providers: Vec<Connector>,
    integrations: Vec<Connector>,

    // Active connector detail (the master-detail "detail" pane). The section
    // disambiguates connectors that share an id across kinds (e.g. Alpaca).
    selected_connector: Option<(Section, String)>,
    fields: Vec<FieldMeta>,
    /// Whether the engine's field list for the open connector has arrived, so an
    /// honestly field-less connector (e.g. yfinance) reads "ready to use" rather
    /// than "Loading…" forever.
    fields_loaded: bool,
    /// Fixed pool of single-line editors bound to `fields` by index, so no
    /// entity is created after construction. Cleared when a connector opens so
    /// one connector's input never bleeds into another's.
    editors: Vec<Entity<editor::Editor>>,
    selections: std::collections::HashMap<String, String>,
    capability: Option<Capability>,
    connector_saved: bool,
    test_state: TestState,

    // Account
    account: Option<Account>,

    // AI model section
    provider_view: Option<(SharedString, AnyView)>,
    model_id_editor: Entity<editor::Editor>,
    model_status: ModelStatus,

    // GitHub section
    git_name_editor: Entity<editor::Editor>,
    git_email_editor: Entity<editor::Editor>,
    git_identity_saved: bool,
    github_state: GitHubAuthState,

    // Read-only shared truths from the engine (drives the AI section summary +
    // the cross-store AI-key import).
    shared: Option<SharedSettings>,

    /// Single-flight slot for the connector sub-flow (list/select/test/save).
    _connector_task: Option<Task<()>>,
    /// Single-flight slot for the account read.
    _account_task: Option<Task<()>>,
    /// Single-flight slot for the shared-truths read + AI-key import.
    _shared_task: Option<Task<()>>,
    /// Single-flight slot for the GitHub probe / sign-in.
    _github_task: Option<Task<()>>,
    /// Best-effort side task for the model MIRROR PUT / github sign-in, kept off
    /// the other slots so a follow-up action doesn't cancel it.
    _mirror_task: Option<Task<()>>,
}

impl AuracleSettingsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let weak = workspace.weak_handle();
            cx.new(|cx| {
                // Reconnecting (a new saved connection, anywhere) reloads the
                // connectors, the account, and the shared truths, and re-runs
                // the AI-key import — mirrors how the blotter reloads on a
                // ConnectGeneration bump.
                cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut Self, cx| {
                    this.load_connectors(cx);
                    this.load_account(cx);
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
                    // Account + Broker open by default; the first connector load
                    // opens any other section that needs attention.
                    expanded: HashSet::from([Section::Account, Section::Broker]),
                    did_auto_expand: false,
                    brokers: Vec::new(),
                    data_providers: Vec::new(),
                    integrations: Vec::new(),
                    selected_connector: None,
                    fields: Vec::new(),
                    fields_loaded: false,
                    editors,
                    selections: std::collections::HashMap::new(),
                    capability: None,
                    connector_saved: false,
                    test_state: TestState::Idle,
                    account: None,
                    provider_view: None,
                    model_id_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    model_status: ModelStatus::Idle,
                    git_name_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    git_email_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
                    git_identity_saved: false,
                    github_state: GitHubAuthState::Unknown,
                    shared: None,
                    _connector_task: None,
                    _account_task: None,
                    _shared_task: None,
                    _github_task: None,
                    _mirror_task: None,
                };
                this.load_connectors(cx);
                this.load_account(cx);
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

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.expanded.contains(&section) {
            self.expanded.remove(&section);
        } else {
            self.expanded.insert(section);
        }
        cx.notify();
    }

    // ── Account ────────────────────────────────────────────────────────

    fn load_account(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._account_task = Some(cx.spawn(async move |this, cx| {
            let account = auracle_connections::get_account(http).await.ok();
            this.update(cx, |this, cx| {
                this.account = account;
                cx.notify();
            })
            .ok();
        }));
    }

    // ── Shared truths + AI-key import (cross-store sync) ───────────────

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

            let Some(provider) = cx.update(|cx| find_provider(&provider_id, cx)) else {
                return;
            };

            cx.update(|cx| provider.authenticate(cx)).await.log_err();
            if cx.update(|cx| provider.is_authenticated(cx)) {
                return;
            }

            let Ok((_provider, key)) = auracle_connections::fetch_ai_key(http, &provider_id).await
            else {
                return;
            };
            cx.update(|cx| provider.set_api_key(Some(key), cx))
                .await
                .log_err();
            cx.update(|cx| provider.authenticate(cx)).await.log_err();
            this.update(cx, |_this, cx| cx.notify()).ok();
        }));
    }

    // ── Connectors (transport reused from auracle_connections) ─────────

    fn load_connectors(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._connector_task = Some(cx.spawn(async move |this, cx| {
            let brokers = auracle_connections::list_connectors(http.clone(), "broker")
                .await
                .unwrap_or_default();
            let data = auracle_connections::list_connectors(http.clone(), "data_provider")
                .await
                .unwrap_or_default();
            let integrations = auracle_connections::list_connectors(http, "integration")
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.brokers = brokers;
                this.data_providers = data;
                this.integrations = integrations;
                if !this.did_auto_expand {
                    this.did_auto_expand = true;
                    if default_expanded(&this.brokers) {
                        this.expanded.insert(Section::Broker);
                    }
                    if default_expanded(&this.data_providers) {
                        this.expanded.insert(Section::Data);
                    }
                    if default_expanded(&this.integrations) {
                        this.expanded.insert(Section::QuantConnect);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn connectors_for(&self, section: Section) -> &[Connector] {
        match section {
            Section::Broker => &self.brokers,
            Section::Data => &self.data_providers,
            Section::QuantConnect => &self.integrations,
            _ => &[],
        }
    }

    fn select_connector(
        &mut self,
        section: Section,
        connector: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_connector = Some((section, connector.clone()));
        self.test_state = TestState::Idle;
        self.fields = Vec::new();
        self.fields_loaded = false;
        self.capability = None;
        self.connector_saved = false;
        self.selections.clear();
        // Clear the editor pool so a previously-opened connector's typed input
        // never bleeds into this one's fields.
        let editors = self.editors.clone();
        for editor in &editors {
            editor.update(cx, |editor, cx| editor.set_text("", window, cx));
        }
        cx.notify();
        let http = cx.http_client();
        self._connector_task = Some(cx.spawn(async move |this, cx| {
            let fields = auracle_connections::get_fields(http.clone(), &connector)
                .await
                .unwrap_or_default();
            let capability = auracle_connections::get_capability(http, &connector)
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
                this.fields_loaded = true;
                this.capability = capability;
                cx.notify();
            })
            .ok();
        }));
    }

    fn back_to_list(&mut self, cx: &mut Context<Self>) {
        self.selected_connector = None;
        self.test_state = TestState::Idle;
        cx.notify();
    }

    fn active_connector_id(&self) -> Option<String> {
        self.selected_connector.as_ref().map(|(_, id)| id.clone())
    }

    fn current_body(&self, cx: &App) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (index, field) in self.fields.iter().enumerate() {
            if field.kind == "select" {
                if let Some(choice) = self.selections.get(&field.name) {
                    map.insert(
                        field.name.clone(),
                        serde_json::Value::String(choice.clone()),
                    );
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

    fn run_connector_test(&mut self, cx: &mut Context<Self>) {
        let Some(connector) = self.active_connector_id() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._connector_task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::test_connection(http, &connector, body).await;
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

    fn save_connector(&mut self, cx: &mut Context<Self>) {
        let Some(connector) = self.active_connector_id() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._connector_task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::save_connection(http, &connector, body).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.connector_saved = true;
                        this.test_state = TestState::Verdict {
                            ok: true,
                            plain: "Saved — this connection is set up.".into(),
                        };
                        // Bump the generation so this and other live panels
                        // reload their connector lists + statuses.
                        let generation = cx.global::<auracle_connect::ConnectGeneration>().0 + 1;
                        cx.set_global(auracle_connect::ConnectGeneration(generation));
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

    fn disconnect_connector(&mut self, cx: &mut Context<Self>) {
        let Some(connector) = self.active_connector_id() else {
            return;
        };
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._connector_task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::disconnect_connection(http, &connector).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.connector_saved = false;
                        this.test_state = TestState::Verdict {
                            ok: true,
                            plain: "Disconnected — credentials cleared.".into(),
                        };
                        let generation = cx.global::<auracle_connect::ConnectGeneration>().0 + 1;
                        cx.set_global(auracle_connect::ConnectGeneration(generation));
                    }
                    Err(error) => {
                        this.test_state = TestState::Verdict {
                            ok: false,
                            plain: SharedString::from(format!("Couldn't disconnect: {error}.")),
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
        let label = SharedString::from(format!(
            "Default model set to {provider_name} · {model_id}."
        ));
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
        // same model. The IDE keychain key isn't readable back through the
        // provider trait, so we mirror the selection without a key; the engine
        // authenticates with the operator's engine-side key.
        let http = cx.http_client();
        self._mirror_task = Some(cx.spawn(async move |_this, _cx| {
            auracle_connections::put_ai_model(http, &provider_id, &model_id, None)
                .await
                .log_err();
        }));
    }

    // ── GitHub section ────────────────────────────────────────────────

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

/// Probe `git credential fill` for a github.com login over stdin. Returns the
/// username on success, `None` otherwise.
async fn probe_github_credential() -> Option<String> {
    let mut command = util::command::new_command("git");
    command
        .args(["credential", "fill"])
        .stdin(util::command::Stdio::piped())
        .stdout(util::command::Stdio::piped())
        .stderr(util::command::Stdio::null());
    let mut child = command.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
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
    if has_password {
        Some(username.unwrap_or_else(|| "your GitHub account".to_string()))
    } else {
        None
    }
}

/// Capitalize the first character (for tier names like "institutional").
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
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
        Some("Auracle connections — account, brokers, data, QuantConnect")
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
    /// One collapsible section: a clickable header (title + honest summary +
    /// toggle) and, when expanded, its body.
    fn render_section(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.expanded.contains(&section);
        let (summary, summary_color) = self.section_summary_line(section);
        let surface = cx.theme().colors().elevated_surface_background;
        let border = cx.theme().colors().border;
        let header_hover = cx.theme().colors().element_hover;
        let panel = cx.weak_entity();

        // The title block and the Switch are sibling click targets (the parent
        // row has no handler), so each toggles exactly once — a single handler
        // on the row plus one on the Switch would double-fire and net to a
        // no-op when the toggle itself is clicked.
        let header = h_flex()
            .w_full()
            .px_3()
            .py_2p5()
            .gap_3()
            .items_center()
            .rounded_t_lg()
            .child(
                v_flex()
                    .id(section.key())
                    .flex_1()
                    .gap_0p5()
                    .cursor_pointer()
                    .hover(move |style| style.bg(header_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
                    .child(Label::new(section.title()))
                    .child(
                        Label::new(summary)
                            .size(LabelSize::Small)
                            .color(summary_color),
                    ),
            )
            .child(
                Switch::new(
                    SharedString::from(format!("{}-toggle", section.key())),
                    expanded.into(),
                )
                .on_click(move |_, _, cx| {
                    panel
                        .update(cx, |this, cx| this.toggle_section(section, cx))
                        .ok();
                }),
            );

        let mut card = v_flex()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(surface)
            .child(header);
        if expanded {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .pt_1()
                    .child(self.render_section_body(section, cx)),
            );
        }
        card.into_any_element()
    }

    fn section_summary_line(&self, section: Section) -> (String, Color) {
        match section {
            Section::Account => match &self.account {
                None => ("Loading…".into(), Color::Muted),
                Some(account) => {
                    let tier = if account.tier.is_empty() {
                        "—".to_string()
                    } else {
                        capitalize(&account.tier)
                    };
                    let email = if account.email.is_empty() {
                        "no email on file".to_string()
                    } else {
                        account.email.clone()
                    };
                    (format!("{tier} · {email}"), Color::Muted)
                }
            },
            Section::Broker => connector_summary_line(&self.brokers),
            Section::Data => connector_summary_line(&self.data_providers),
            Section::QuantConnect => connector_summary_line(&self.integrations),
            Section::AiModel => match self.shared.as_ref().map(|s| &s.ai_model) {
                Some(ai) if !ai.provider.is_empty() => {
                    (format!("Engine default · {}", ai.provider), Color::Default)
                }
                _ => ("Pick a provider and add a key".into(), Color::Muted),
            },
            Section::GitHub => match &self.github_state {
                GitHubAuthState::SignedIn(_) => ("Signed in".into(), Color::Success),
                GitHubAuthState::Checking => ("Checking…".into(), Color::Muted),
                GitHubAuthState::Unknown => ("Not checked yet".into(), Color::Muted),
                GitHubAuthState::SignedOut => ("Not signed in".into(), Color::Muted),
            },
        }
    }

    fn render_section_body(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        match section {
            Section::Account => self.render_account(cx),
            Section::Broker | Section::Data | Section::QuantConnect => {
                self.render_connector_section(section, cx)
            }
            Section::AiModel => self.render_model(cx).into_any_element(),
            Section::GitHub => self.render_github(cx).into_any_element(),
        }
    }

    fn render_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(account) = &self.account else {
            return Label::new("Reading your account from the engine…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element();
        };
        let row = |label: &str, value: String, value_color: Color| {
            h_flex()
                .justify_between()
                .gap_4()
                .child(
                    Label::new(label.to_string())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(value).size(LabelSize::Small).color(value_color))
        };
        let tier = if account.tier.is_empty() {
            "—".to_string()
        } else {
            capitalize(&account.tier)
        };
        let email = if account.email.is_empty() {
            "—".to_string()
        } else {
            account.email.clone()
        };
        let (license_text, license_color) = match account.license_status.state.as_str() {
            "active" => match &account.license_status.expiry {
                Some(expiry) if !expiry.is_empty() => {
                    (format!("Active · renews {expiry}"), Color::Success)
                }
                _ => ("Active".to_string(), Color::Success),
            },
            "expired" => ("Expired".to_string(), Color::Error),
            _ => ("No active license".to_string(), Color::Warning),
        };

        let mut body = v_flex()
            .gap_2()
            .child(row("Email", email, Color::Default))
            .child(row("Plan", tier, Color::Default))
            .child(row("License", license_text, license_color));

        if let Some(url) = account.manage_url.clone() {
            body = body.child(
                Button::new("account-manage", "Manage billing")
                    .style(ButtonStyle::Filled)
                    .full_width()
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
            );
        }
        body = body.child(
            Label::new("Sign out from the launcher — it shares this engine session.")
                .size(LabelSize::Small)
                .color(Color::Muted),
        );
        body.into_any_element()
    }

    fn render_connector_section(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        let connectors = self.connectors_for(section);
        if let Some((selected_section, id)) = &self.selected_connector {
            if *selected_section == section {
                let id = id.clone();
                return self.render_connector_detail(section, &id, connectors, cx);
            }
        }
        self.render_connector_list(section, connectors, cx)
    }

    fn render_connector_list(
        &self,
        section: Section,
        connectors: &[Connector],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if connectors.is_empty() {
            return Label::new("Loading…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element();
        }
        let border = cx.theme().colors().border;
        let border_focused = cx.theme().colors().border_focused;
        let row_bg = cx.theme().colors().ghost_element_background;
        let row_selected = cx.theme().colors().element_selected;
        let row_hover = cx.theme().colors().element_hover;

        let mut list = v_flex().gap_2();
        for connector in connectors {
            let id = connector.id.clone();
            let title = if connector.display_label.is_empty() {
                connector.id.clone()
            } else {
                connector.display_label.clone()
            };
            let connected = connector.status.is_connected();
            let gated = connector.gated;
            let blurb = connector.blurb.clone();
            let has_logo = broker_logo_path(&connector.id).is_some();
            let (status_text, status_color) = if gated {
                ("Upgrade to use", Color::Warning)
            } else if connected {
                ("Connected", Color::Success)
            } else {
                ("Not connected", Color::Muted)
            };
            let select_id = id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("{}-{}", section.key(), id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if connected { border_focused } else { border })
                    .bg(if connected { row_selected } else { row_bg })
                    .hover(move |style| style.bg(row_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_connector(section, select_id.clone(), window, cx);
                    }))
                    .child(brand_tile(&connector.id, &title))
                    .child(
                        v_flex()
                            .flex_1()
                            .when(!has_logo, |column| column.child(Label::new(title.clone())))
                            .when(!blurb.is_empty(), |column| {
                                column.child(
                                    Label::new(blurb.clone())
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .child(
                        Label::new(status_text)
                            .size(LabelSize::Small)
                            .color(status_color),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_connector_detail(
        &self,
        _section: Section,
        id: &str,
        connectors: &[Connector],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connector = connectors.iter().find(|connector| connector.id == id);
        let title = connector
            .map(|connector| {
                if connector.display_label.is_empty() {
                    connector.id.clone()
                } else {
                    connector.display_label.clone()
                }
            })
            .unwrap_or_else(|| id.to_string());
        let test_supported = connector
            .map(|connector| connector.test_supported)
            .unwrap_or(false);
        let connected = connector
            .map(|connector| connector.status.is_connected())
            .unwrap_or(false);
        let gated = connector.map(|connector| connector.gated).unwrap_or(false);
        let gated_reason = connector
            .map(|connector| connector.gated_reason.clone())
            .unwrap_or_default();

        let mut body = v_flex().gap_3().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("conn-back", "‹ All")
                        .style(ButtonStyle::Subtle)
                        .on_click(cx.listener(|this, _, _, cx| this.back_to_list(cx))),
                )
                .child(brand_tile(id, &title))
                .child(Label::new(title.clone())),
        );

        // Tier-gated: be honest and offer no dead credential form.
        if gated {
            body = body.child(
                Label::new(if gated_reason.is_empty() {
                    "Your plan doesn't include this connector. Upgrade to connect it.".to_string()
                } else {
                    gated_reason
                })
                .size(LabelSize::Small)
                .color(Color::Warning),
            );
            return body.into_any_element();
        }

        if id == "ibkr" {
            body = body.child(
                Label::new(
                    "IBKR needs the IB Gateway or Client Portal running and logged in before \
                     these credentials will verify.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            );
        }

        // Credential fields
        let mut form = v_flex().gap_3();
        if !self.fields_loaded {
            form = form.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        } else if self.fields.is_empty() {
            // An honestly credential-less connector (e.g. yfinance) — not a
            // forever-"Loading…", and no dead Connect button below.
            form = form.child(
                Label::new("No credentials needed — this connector is ready to use.")
                    .size(LabelSize::Small)
                    .color(Color::Success),
            );
        }
        for (index, field) in self.fields.iter().enumerate() {
            let mut label = if field.label.is_empty() {
                field.name.clone()
            } else {
                field.label.clone()
            };
            if field.has_value {
                if field.preview.is_empty() {
                    label = format!("{label}  (saved — leave blank to keep)");
                } else {
                    label = format!("{label}  (saved: {} — leave blank to keep)", field.preview);
                }
            } else if !field.required {
                label = format!("{label}  (optional)");
            }
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
            } else if index < self.editors.len() {
                div()
                    .w_full()
                    .py_1()
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().editor_background)
                    .child(self.editors[index].clone())
                    .into_any_element()
            } else {
                Label::new("Set this field from the launcher for now.")
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
        body = body.child(form);

        // Capability chips — honesty: chips come ONLY from the engine's verified
        // capability; an "unknown" leg reads as "not verified yet", never green.
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

        // Actions. A credential-less connector has nothing to save, so it shows
        // no Connect button (no dead control) — only Disconnect if it's somehow
        // connected.
        let has_fields = !self.fields.is_empty();
        let mut actions_row = h_flex().gap_2();
        if test_supported {
            actions_row = actions_row.child(
                Button::new("conn-test", "Test")
                    .style(ButtonStyle::Outlined)
                    .on_click(cx.listener(|this, _, _, cx| this.run_connector_test(cx))),
            );
        }
        if has_fields {
            actions_row = actions_row.child(
                Button::new("conn-save", "Connect")
                    .style(ButtonStyle::Filled)
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| this.save_connector(cx))),
            );
        }
        if connected {
            actions_row = actions_row.child(
                Button::new("conn-disconnect", "Disconnect")
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_connector(cx))),
            );
        }
        body = body.child(actions_row);

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
        .into_any_element()
    }

    fn render_model(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_3().child(
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
                        .full_width()
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
            .child(Label::new(status.1).size(LabelSize::Small).color(status.0))
    }
}

/// A section's connector summary as `(text, color)`: green once anything is
/// connected, muted otherwise.
fn connector_summary_line(connectors: &[Connector]) -> (String, Color) {
    let tone = if connectors.iter().any(|c| c.status.is_connected()) {
        Color::Success
    } else {
        Color::Muted
    };
    (section_summary(connectors), tone)
}

impl Render for AuracleSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = v_flex()
            .id("auracle-settings-panel")
            .key_context("AuracleSettingsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .gap_3()
            .overflow_y_scroll()
            .bg(cx.theme().colors().panel_background)
            .child(Label::new("Connections").size(LabelSize::Large));
        for section in Section::ALL {
            column = column.child(self.render_section(section, cx));
        }
        column
    }
}
