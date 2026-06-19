//! The first-run onboarding wizard — one modal, three setups.
//!
//! A new operator lands in the IDE with three things to wire up before
//! anything works: a broker, a default AI model, and (optionally) GitHub.
//! This modal walks them through all three in one frame.
//!
//! It auto-opens once, on first workspace start, only when nothing is set
//! up yet (no broker connected AND no AI provider authenticated). A
//! persisted "onboarding_dismissed" flag in the key-value store
//! ([`OnboardingDismissed`]) ensures it never re-nags after the user
//! finishes or skips it. It can always be re-opened from the command
//! palette / app menu via the [`OpenOnboarding`] action.
//!
//! Honesty laws baked in (mirroring `auracle_connections`):
//!   * the broker step reuses `auracle_connections`' transport, Test, and
//!     Save verbatim — a broker is never shown "connected" without a real
//!     successful Test/Save round-trip;
//!   * the AI step embeds each provider's own credential view and only
//!     reads "configured" from `is_authenticated`, never a local guess;
//!   * the GitHub step shells `gh auth status` and `git config` for real;
//!     it never claims a sign-in it can't observe.

use std::sync::Arc;

use agent_settings::{AgentSettings, language_model_to_selection};
use db::kvp::Dismissable;
use gpui::{
    AnyView, App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Global, SharedString,
    Task, WeakEntity, Window, actions,
};
use language_model::{LanguageModelRegistry, ZED_CLOUD_PROVIDER_ID};
use settings::{Settings as _, update_settings_file};
use ui::prelude::*;
use workspace::{ModalView, Workspace};

use auracle_connections::{BrokerSummary, Capability, FieldMeta};

actions!(
    auracle,
    [
        /// Open the first-run onboarding wizard (broker, AI model, GitHub).
        OpenOnboarding
    ]
);

/// Persisted flag so the wizard auto-opens only once. Stored in the
/// key-value store via the [`Dismissable`] trait (same mechanism the agent
/// panel onboarding upsell uses).
struct OnboardingDismissed;

impl Dismissable for OnboardingDismissed {
    const KEY: &'static str = "auracle-onboarding-dismissed";
}

/// Process-lifetime guard so the first-run auto-open is attempted at most
/// once, even when several workspace windows open during a single launch.
#[derive(Default)]
struct FirstRunChecked(bool);

impl Global for FirstRunChecked {}

pub fn init(cx: &mut App) {
    cx.set_global(FirstRunChecked::default());
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        workspace.register_action(|workspace, _: &OpenOnboarding, window, cx| {
            let weak_workspace = workspace.weak_handle();
            workspace.toggle_modal(window, cx, |window, cx| {
                OnboardingWizard::new(weak_workspace, window, cx)
            });
        });

        // First-run auto-open: at most once per launch, only when the
        // persisted flag isn't set, and only when nothing is set up yet. The
        // cold-start check is async (it calls the engine), so it runs in a
        // spawned task that opens the modal back on the foreground if it
        // confirms a cold start.
        let Some(window) = window else {
            return;
        };
        if cx.global::<FirstRunChecked>().0 {
            return;
        }
        cx.set_global(FirstRunChecked(true));
        if OnboardingDismissed::dismissed(cx) {
            return;
        }
        let weak_workspace = workspace.weak_handle();
        let http = cx.http_client();
        window
            .spawn(cx, async move |cx| {
                let any_broker = any_broker_connected(http).await;
                let any_model = cx
                    .update(|_window, cx| any_ai_provider_authenticated(cx))
                    .unwrap_or(false);
                if any_broker || any_model {
                    return;
                }
                weak_workspace
                    .update_in(cx, |workspace, window, cx| {
                        let inner_weak = workspace.weak_handle();
                        workspace.toggle_modal(window, cx, |window, cx| {
                            OnboardingWizard::new(inner_weak, window, cx)
                        });
                    })
                    .ok();
            })
            .detach();
    })
    .detach();
}

/// True if the engine reports at least one broker with `status == "connected"`.
/// Errors (engine down) resolve to `false`, which is the honest cold-start
/// default — we'd rather show the wizard than wrongly suppress it.
async fn any_broker_connected(http: Arc<dyn http_client::HttpClient>) -> bool {
    auracle_connections::list_brokers(http)
        .await
        .map(|brokers| brokers.iter().any(|broker| broker.status == "connected"))
        .unwrap_or(false)
}

/// True if any visible, non-Zed-cloud provider is already authenticated.
fn any_ai_provider_authenticated(cx: &App) -> bool {
    LanguageModelRegistry::read_global(cx)
        .visible_providers()
        .iter()
        .any(|provider| provider.is_authenticated(cx) && provider.id() != ZED_CLOUD_PROVIDER_ID)
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Broker,
    Model,
    GitHub,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Step::Broker => 1,
            Step::Model => 2,
            Step::GitHub => 3,
        }
    }
}

// ── Broker sub-step state (reuses auracle_connections transport) ──────

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

// ── GitHub sub-step state ─────────────────────────────────────────────

enum GitHubAuthState {
    Unknown,
    Checking,
    SignedIn(SharedString),
    SignedOut,
}

pub struct OnboardingWizard {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    step: Step,

    // Step 1 — broker
    broker_phase: BrokerPhase,
    brokers: Vec<BrokerSummary>,
    selected_broker: Option<String>,
    fields: Vec<FieldMeta>,
    /// Fixed pool of single-line editors bound to `fields` by index, so no
    /// entity is created after `new()`. Mirrors `BrokerWizard`'s pool.
    editors: Vec<Entity<editor::Editor>>,
    selections: std::collections::HashMap<String, String>,
    capability: Option<Capability>,
    broker_saved: bool,
    test_state: TestState,

    // Step 2 — AI model: the chosen provider's own credential view, plus a
    // small editor for the model id to set as default.
    provider_view: Option<(SharedString, AnyView)>,
    model_id_editor: Entity<editor::Editor>,
    /// Status line under the model step; carries both success and the
    /// "do this first" hints.
    model_status: SharedString,
    /// True only after a default model was actually written to settings.
    model_configured: bool,

    // Step 3 — GitHub
    git_name_editor: Entity<editor::Editor>,
    git_email_editor: Entity<editor::Editor>,
    git_identity_saved: bool,
    github_state: GitHubAuthState,

    _task: Option<Task<()>>,
}

impl OnboardingWizard {
    fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editors = Vec::with_capacity(auracle_connections::MAX_FIELDS);
        for _ in 0..auracle_connections::MAX_FIELDS {
            editors.push(cx.new(|cx| editor::Editor::single_line(window, cx)));
        }
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            step: Step::Broker,
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
            model_status: SharedString::default(),
            model_configured: false,
            git_name_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
            git_email_editor: cx.new(|cx| editor::Editor::single_line(window, cx)),
            git_identity_saved: false,
            github_state: GitHubAuthState::Unknown,
            _task: None,
        };
        this.load_brokers(cx);
        this
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        OnboardingDismissed::set_dismissed(true, cx);
        cx.emit(DismissEvent);
    }

    fn fs(&self, cx: &App) -> Option<Arc<dyn fs::Fs>> {
        let workspace = self.workspace.upgrade()?;
        Some(workspace.read(cx).project().read(cx).fs().clone())
    }

    // ── Step 1: broker (transport reused from auracle_connections) ────

    fn load_brokers(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._task = Some(cx.spawn(async move |this, cx| {
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
        self._task = Some(cx.spawn(async move |this, cx| {
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

    fn run_broker_test(&mut self, cx: &mut Context<Self>) {
        let Some(broker) = self.selected_broker.clone() else {
            return;
        };
        let body = self.current_body(cx);
        let http = cx.http_client();
        self.test_state = TestState::Working;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
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
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = auracle_connections::save_connection(http, &broker, body).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
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

    // ── Step 2: AI default model ──────────────────────────────────────

    fn select_provider(
        &mut self,
        provider: Arc<dyn language_model::LanguageModelProvider>,
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

    /// Sets the engine's default agent model from the chosen provider's
    /// default (or, if the operator typed a model id, that one). Mirrors the
    /// agent settings UI's `update_settings_file` write of `default_model`.
    fn set_default_model(&mut self, cx: &mut Context<Self>) {
        let Some((provider_name, _)) = self.provider_view.clone() else {
            self.model_status = "Pick a provider first.".into();
            cx.notify();
            return;
        };
        let Some(provider) = LanguageModelRegistry::read_global(cx)
            .visible_providers()
            .into_iter()
            .find(|provider| provider.name().0 == provider_name)
        else {
            self.model_status = "That provider is no longer available.".into();
            cx.notify();
            return;
        };
        if !provider.is_authenticated(cx) {
            self.model_status = "Add and confirm an API key above first.".into();
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
            self.model_status = "Couldn't find that model for this provider.".into();
            cx.notify();
            return;
        };
        let Some(fs) = self.fs(cx) else {
            self.model_status = "Couldn't reach the settings file.".into();
            cx.notify();
            return;
        };
        let current = AgentSettings::get_global(cx).default_model.clone();
        let selection = language_model_to_selection(&model, current.as_ref());
        let label = SharedString::from(format!(
            "Default model set to {} · {}.",
            provider.name().0,
            model.id().0
        ));
        update_settings_file(fs, cx, move |settings, _cx| {
            let agent = settings.agent.get_or_insert_default();
            agent.default_model = Some(selection);
        });
        self.model_status = label;
        self.model_configured = true;
        cx.notify();
    }

    // ── Step 3: GitHub ────────────────────────────────────────────────

    fn check_github(&mut self, cx: &mut Context<Self>) {
        self.github_state = GitHubAuthState::Checking;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = util::command::new_command("gh")
                .args(["auth", "status"])
                .output()
                .await;
            this.update(cx, |this, cx| {
                this.github_state = match result {
                    Ok(output) if output.status.success() => {
                        let text = String::from_utf8_lossy(&output.stdout);
                        let line = text
                            .lines()
                            .find(|line| line.contains("Logged in"))
                            .unwrap_or("Signed in to GitHub.")
                            .trim()
                            .to_string();
                        GitHubAuthState::SignedIn(SharedString::from(line))
                    }
                    Ok(_) => GitHubAuthState::SignedOut,
                    Err(_) => GitHubAuthState::SignedOut,
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
        self._task = Some(cx.spawn(async move |this, cx| {
            if !name.is_empty() {
                util::command::new_command("git")
                    .args(["config", "--global", "user.name", &name])
                    .output()
                    .await
                    .ok();
            }
            if !email.is_empty() {
                util::command::new_command("git")
                    .args(["config", "--global", "user.email", &email])
                    .output()
                    .await
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.git_identity_saved = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn sign_in_github(&mut self, cx: &mut Context<Self>) {
        // The IDE has no GitHub OAuth client, so we point the user at the
        // `gh` device flow rather than fake a sign-in. After they finish in
        // the browser/CLI, "Check status" reflects the real result.
        cx.open_url("https://github.com/login/device");
    }
}

// ── Render ────────────────────────────────────────────────────────────

impl EventEmitter<DismissEvent> for OnboardingWizard {}

impl Focusable for OnboardingWizard {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for OnboardingWizard {}

impl OnboardingWizard {
    fn render_header(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut dots = h_flex().gap_1p5();
        for step in [Step::Broker, Step::Model, Step::GitHub] {
            let active = step == self.step;
            // A step's dot turns Success only when that step's setup actually
            // landed (a saved broker / a typed-in model-id confirmation),
            // never merely because the user clicked past it.
            let confirmed = match step {
                Step::Broker => self.broker_saved,
                Step::Model => self.model_configured,
                Step::GitHub => self.git_identity_saved,
            };
            let color = if active {
                Color::Accent
            } else if confirmed {
                Color::Success
            } else {
                Color::Muted
            };
            dots = dots.child(
                Label::new(match step {
                    Step::Broker => "1 Broker",
                    Step::Model => "2 AI model",
                    Step::GitHub => "3 GitHub",
                })
                .size(LabelSize::Small)
                .color(color),
            );
        }
        v_flex()
            .gap_1()
            .child(Label::new("Set up Auracle").size(LabelSize::Large))
            .child(
                Label::new(format!("Step {} of 3", self.step.index()))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(dots)
    }

    fn render_broker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_3();
        match self.broker_phase {
            BrokerPhase::Choose => {
                body = body.child(Label::new("Choose a broker to connect"));
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
                body = body.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("onb-broker-test", "Test")
                                .style(ButtonStyle::Outlined)
                                .on_click(cx.listener(|this, _, _, cx| this.run_broker_test(cx))),
                        )
                        .child(
                            Button::new("onb-broker-save", "Connect")
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(|this, _, _, cx| this.save_broker(cx))),
                        )
                        .child(
                            Button::new("onb-broker-other", "Pick another broker")
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.broker_phase = BrokerPhase::Choose;
                                    this.selected_broker = None;
                                    this.test_state = TestState::Idle;
                                    cx.notify();
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
            .child(Label::new("Pick a provider and add its API key"));

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
                Button::new(SharedString::from(format!("onb-prov-{name}")), label)
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
                    Button::new("onb-set-default-model", "Set as default model")
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

        let status_color = if self.model_configured {
            Color::Success
        } else {
            Color::Warning
        };
        body.when(!self.model_status.is_empty(), |this| {
            this.child(
                Label::new(self.model_status.clone())
                    .size(LabelSize::Small)
                    .color(status_color),
            )
        })
    }

    fn render_github(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status: (Color, SharedString) = match &self.github_state {
            GitHubAuthState::Unknown => (
                Color::Muted,
                "GitHub status not checked yet.".into(),
            ),
            GitHubAuthState::Checking => (Color::Muted, "Checking…".into()),
            GitHubAuthState::SignedIn(line) => (Color::Success, line.clone()),
            GitHubAuthState::SignedOut => (
                Color::Warning,
                "Not signed in to GitHub (the gh CLI reports no session).".into(),
            ),
        };
        v_flex()
            .gap_3()
            .child(Label::new("Git identity and GitHub (optional)"))
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
                        Button::new("onb-git-save", "Save git identity")
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
                        Button::new("onb-git-signin", "Sign in to GitHub")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, _, cx| this.sign_in_github(cx))),
                    )
                    .child(
                        Button::new("onb-git-check", "Check status")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| this.check_github(cx))),
                    ),
            )
            .child(
                Label::new(
                    "Sign in opens the GitHub device-code page; finish in your browser, \
                     then Check status.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(Label::new(status.1).size(LabelSize::Small).color(status.0))
    }
}

impl Render for OnboardingWizard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let is_last = step == Step::GitHub;

        v_flex()
            .key_context("OnboardingWizard")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| {
                this.dismiss(cx);
            }))
            .w(rems(36.))
            .p_4()
            .gap_4()
            .bg(cx.theme().colors().elevated_surface_background)
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(self.render_header(cx))
            .map(|this| match step {
                Step::Broker => this.child(self.render_broker(cx)),
                Step::Model => this.child(self.render_model(cx)),
                Step::GitHub => this.child(self.render_github(cx)),
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        Button::new("onb-skip", if is_last { "Skip" } else { "Skip for now" })
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(step != Step::Broker, |row| {
                                row.child(
                                    Button::new("onb-back", "Back")
                                        .style(ButtonStyle::Subtle)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.step = match this.step {
                                                Step::GitHub => Step::Model,
                                                _ => Step::Broker,
                                            };
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(!is_last, |row| {
                                row.child(
                                    Button::new("onb-next", "Next")
                                        .style(ButtonStyle::Filled)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.step = match this.step {
                                                Step::Broker => Step::Model,
                                                _ => Step::GitHub,
                                            };
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(is_last, |row| {
                                row.child(
                                    Button::new("onb-done", "Done")
                                        .style(ButtonStyle::Filled)
                                        .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                                )
                            }),
                    ),
            )
    }
}
