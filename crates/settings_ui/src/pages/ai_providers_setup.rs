//! The native "Model providers" sub-page on the AI settings page.
//!
//! It does two honest things, both decided by the gpui-free `auracle_ai_settings`
//! crate so the logic is unit-tested without rendering:
//!
//!   1. An engine-default header — what AI model the *engine* designated as the
//!      shared default, and whether the engine holds a usable key for it. This is
//!      read from the [`SettingsWindow`]'s `shared_settings` snapshot (loaded over
//!      loopback when the window opens) and translated through
//!      [`engine_default_summary`], which never invents a model or key state.
//!
//!   2. A provider list — every visible provider in the language-model registry,
//!      each marked with whether it is authenticated and whether it is the engine
//!      default ([`derive_provider_rows`]). Selecting a provider embeds that
//!      provider's own live `configuration_view` so the user can enter/save its
//!      key (or sign in) in place.
//!
//! Why a dedicated [`Render`] entity (not a plain render-fn over `&SettingsWindow`
//! like most sub-pages): the embedded `configuration_view` AnyViews must persist
//! across renders, otherwise an editor inside one loses focus the moment the user
//! types a character and the page re-renders. We build the views once on `new()`
//! (mirroring `agent_configuration::build_provider_configuration_views`) and cache
//! them by provider id, exactly as the agent configuration panel does.
//!
//! Honesty laws (see CLAUDE.md): a provider is shown authenticated only when
//! `is_authenticated(cx)` is true; a default is never set for an unauthenticated
//! provider; no key is ever logged or rendered.

use std::collections::HashMap;
use std::sync::Arc;

use agent_settings::{AgentSettings, language_model_to_selection};
use auracle_ai_settings::{
    AiProviderRow, EngineDefaultStatus, ProviderDescriptor, StatusTone, derive_provider_rows,
    engine_default_summary,
};
use auracle_view_state::ViewState;
use fs::Fs;
use gpui::{AnyView, Entity, FocusHandle, Focusable, ScrollHandle, Task, WeakEntity, prelude::*};
use language_model::{
    ConfigurationViewTargetAgent, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelRegistry,
};
use settings::Settings as _;
use ui::{Divider, prelude::*};
use util::ResultExt as _;

use crate::SettingsWindow;

/// Map a [`StatusTone`] to the theme colour the engine-default header renders in.
/// Only theme `Color::*` — never a literal — so the page tracks the theme.
fn tone_color(tone: StatusTone) -> Color {
    match tone {
        StatusTone::Positive => Color::Success,
        StatusTone::Caution => Color::Warning,
        StatusTone::Neutral => Color::Muted,
    }
}

/// Renders the "Model providers" sub-page by deferring to the backing
/// [`AiProvidersPage`] entity. The entity is created with its cached configuration
/// views when the sub-page is pushed (see `SettingsWindow::push_sub_page`), so the
/// views persist across renders. If for any reason it isn't there yet (it should
/// always be by the time this renders), show a designed loading hint rather than
/// a blank panel.
pub(crate) fn render_ai_providers_page(
    settings_window: &SettingsWindow,
    _scroll_handle: &ScrollHandle,
    _window: &mut Window,
    _cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Some(page) = settings_window.ai_providers_page() else {
        return Label::new("Loading model providers…")
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element();
    };
    page.into_any_element()
}

/// Build the page entity (and its cached configuration views). Called by
/// `SettingsWindow` when the "Model providers" sub-page is pushed, where a
/// `&mut SettingsWindow` is available to store the result.
pub(crate) fn build_ai_providers_page(
    settings_window: WeakEntity<SettingsWindow>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AiProvidersPage> {
    cx.new(|cx| AiProvidersPage::new(settings_window, window, cx))
}

pub(crate) struct AiProvidersPage {
    focus_handle: FocusHandle,
    fs: Arc<dyn Fs>,
    settings_window: WeakEntity<SettingsWindow>,
    /// The provider whose configuration view is currently expanded, if any.
    selected_provider: Option<LanguageModelProviderId>,
    /// Live configuration views, one per visible provider, built once and cached
    /// so editors inside them keep focus across re-renders.
    configuration_views: HashMap<LanguageModelProviderId, AnyView>,
    /// Transient feedback from the most recent "Set as default" attempt.
    set_default_feedback: Option<SetDefaultFeedback>,
    scroll_handle: ScrollHandle,
    /// Best-effort engine→IDE key import kicked off when the page opens. Held so
    /// the task isn't dropped (and cancelled) before it can authenticate the
    /// engine-designated provider. Kept separate from `_mirror_task` so a "Set
    /// as default" mirror never cancels an in-flight import, or vice versa.
    _import_task: Option<Task<()>>,
    /// Best-effort IDE→engine mirror of the most recent "Set as default" choice.
    /// Held separately from `_import_task` for the same reason.
    _mirror_task: Option<Task<()>>,
}

/// The outcome of the last "Set as default" click, shown inline. Held as a small
/// honest value (never a fabricated success) so the page can state both the
/// happy path and the refusal to set a default for an unauthenticated provider.
struct SetDefaultFeedback {
    ok: bool,
    message: SharedString,
}

impl AiProvidersPage {
    fn new(
        settings_window: WeakEntity<SettingsWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fs = <dyn Fs>::global(cx);
        let mut page = Self {
            focus_handle: cx.focus_handle(),
            fs,
            settings_window,
            selected_provider: None,
            configuration_views: HashMap::default(),
            set_default_feedback: None,
            scroll_handle: ScrollHandle::new(),
            _import_task: None,
            _mirror_task: None,
        };
        page.build_configuration_views(window, cx);
        page.import_engine_key(cx);
        page
    }

    /// Engine→IDE key import (the `load_shared_and_import` half of cross-store
    /// sync): if the engine designated an AI provider the IDE hasn't yet
    /// authenticated, pull that provider's plaintext key over loopback and
    /// import it into the IDE keychain so the provider's "Configured" tick and
    /// "Set as default" control light up without the user re-pasting a key the
    /// launcher already holds.
    ///
    /// Best-effort by contract: if the shared-settings snapshot is still
    /// `Pending` (it loads asynchronously when the window opens) we skip rather
    /// than block — the user can still configure any provider manually. Honesty:
    /// a key is imported (and the provider marked configured) ONLY when the
    /// engine actually returns one; a 404/error is treated as "nothing to
    /// import", never a fake-authenticated state, and the key is never logged.
    fn import_engine_key(&mut self, cx: &mut Context<Self>) {
        // Capture the http client and a weak handle to the window OUTSIDE the
        // spawn (they need the sync `cx`). We must NOT read the window
        // synchronously here: `new()` runs from `cx.new(...)` inside
        // `SettingsWindow::push_sub_page`, where the SettingsWindow entity is
        // mid-update (leased out of the entity map). A synchronous
        // `read_with`/`read` of a leased entity is a re-entrant borrow that GPUI
        // aborts (SIGABRT) — the `.ok()` cannot catch it because the lease
        // violation panics before any `Result` exists. Reading the window
        // *inside* the spawned task is safe: by the time the task is polled,
        // `push_sub_page`'s update has returned and the entity is back in the map.
        let http = cx.http_client();
        let settings_window = self.settings_window.clone();
        self._import_task = Some(cx.spawn(async move |this, cx| {
            // Read the engine-designated provider name now that the window is no
            // longer leased. If the shared-settings snapshot is still loading, or
            // the engine named no provider, there is nothing to import.
            let Some(engine_provider) = settings_window
                .read_with(cx, |settings_window, _| {
                    match &settings_window.shared_settings {
                        auracle_view_state::Load::Done(settings) => {
                            let provider = settings.ai_model.provider.trim();
                            (!provider.is_empty()).then(|| provider.to_string())
                        }
                        _ => None,
                    }
                })
                .ok()
                .flatten()
            else {
                return;
            };

            // The engine names the provider by its vault-key (e.g.
            // `deepseek_api_key`) while the IDE registry keys by id (e.g.
            // `auracle-agent`). Translate so the registry lookup hits; an engine
            // name the IDE has no provider for returns `None` and we leave it
            // alone.
            let Some(ide_provider_id) =
                auracle_connections::engine_provider_to_ide(&engine_provider)
            else {
                return;
            };
            // Re-find the provider from the registry INSIDE the task on every
            // `cx.update`, rather than holding an `Arc<dyn LanguageModelProvider>`
            // across an `.await`. `AsyncApp::update` returns the closure value
            // directly, so each lookup comes back as a plain `Option`.
            // `None` means the provider is no longer in the registry — nothing to
            // import.
            let Some(authenticated) = cx.update(|cx| {
                find_visible_provider(ide_provider_id, cx)
                    .map(|provider| provider.is_authenticated(cx))
            }) else {
                return;
            };
            // Already authenticated locally — nothing to import.
            if authenticated {
                return;
            }

            // Pull the engine's key (loopback-only handoff). A 404 (engine holds
            // no key) surfaces as an error we treat as "nothing to import". The
            // engine vault-key name is what `fetch_ai_key` expects. Never log the
            // returned key.
            let Ok((_provider, key)) =
                auracle_connections::fetch_ai_key(http, &engine_provider).await
            else {
                return;
            };

            // Import the key, then authenticate so the freshly-imported key is
            // loaded. Both go through `cx.update` with a fresh registry lookup so
            // no provider handle is held across the awaits.
            if let Some(set) = cx.update(|cx| {
                find_visible_provider(ide_provider_id, cx)
                    .map(|provider| provider.set_api_key(Some(key), cx))
            }) {
                set.await.log_err();
            }
            if let Some(authenticate) = cx.update(|cx| {
                find_visible_provider(ide_provider_id, cx).map(|provider| provider.authenticate(cx))
            }) {
                authenticate.await.log_err();
            }

            // Nudge a redraw so the provider row's "Configured" tick reflects the
            // import.
            this.update(cx, |_this, cx| cx.notify()).ok();
        }));
    }

    /// Build (or rebuild) one configuration view per visible provider. The
    /// Zed-cloud provider is skipped — this fork has no hosted-cloud sign-in to
    /// configure (matching the bespoke panel this replaces). The `auracle-agent`
    /// provider needs no special-casing: its key lives in the engine vault and
    /// its own `configuration_view` handles that, so we just embed it.
    fn build_configuration_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for provider in visible_providers(cx) {
            let view =
                provider.configuration_view(ConfigurationViewTargetAgent::ZedAgent, window, cx);
            self.configuration_views.insert(provider.id(), view);
        }
    }

    fn select_provider(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        // Toggle: clicking the selected provider again collapses its view.
        if self.selected_provider.as_ref() == Some(&id) {
            self.selected_provider = None;
        } else {
            self.selected_provider = Some(id);
        }
        self.set_default_feedback = None;
        cx.notify();
    }

    /// Write the local default agent model from `provider`, refusing for an
    /// unauthenticated provider. Mirrors `auracle_onboarding`'s `set_default_model`
    /// (resolve a model → build a [`LanguageModelSelection`] → write
    /// `agent.default_model`), then mirrors the choice up to the engine so the
    /// launcher reflects the same default.
    fn set_as_default(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        let Some(provider) = visible_providers(cx)
            .into_iter()
            .find(|provider| provider.id() == id)
        else {
            self.set_default_feedback = Some(SetDefaultFeedback {
                ok: false,
                message: "That provider is no longer available.".into(),
            });
            cx.notify();
            return;
        };

        // Honesty: never set a default for a provider that can't authenticate.
        if !provider.is_authenticated(cx) {
            self.set_default_feedback = Some(SetDefaultFeedback {
                ok: false,
                message: "Add this provider's key before making it the default.".into(),
            });
            cx.notify();
            return;
        }

        let model = provider
            .default_model(cx)
            .or_else(|| provider.recommended_models(cx).first().cloned())
            .or_else(|| provider.provided_models(cx).first().cloned());
        let Some(model) = model else {
            self.set_default_feedback = Some(SetDefaultFeedback {
                ok: false,
                message: "This provider has no model to set as the default.".into(),
            });
            cx.notify();
            return;
        };

        let current = AgentSettings::get_global(cx).default_model.clone();
        let selection = language_model_to_selection(&model, current.as_ref());
        let provider_display = provider.name().0.to_string();
        let provider_id = provider.id().0.to_string();
        let model_id = model.id().0.to_string();
        let fs = self.fs.clone();
        settings::update_settings_file(fs, cx, move |settings, _cx| {
            settings.agent.get_or_insert_default().default_model = Some(selection);
        });
        self.set_default_feedback = Some(SetDefaultFeedback {
            ok: true,
            message: format!("Default model set to {provider_display} · {model_id}.").into(),
        });
        cx.notify();

        // Mirror (IDE→engine): best-effort PUT so the launcher reflects the same
        // default. Non-fatal on failure — the local write above already
        // succeeded. No key is sent: the IDE keychain key is not readable back
        // through the provider trait, so we mirror only the selection
        // (`{provider, model_id}`); the engine authenticates with its own
        // operator-side key. Translate the IDE registry id to the engine
        // vault-key name first, or the engine's whitelist rejects the raw id.
        let engine_provider = auracle_connections::ide_provider_to_engine(&provider_id).to_string();
        let http = cx.http_client();
        self._mirror_task = Some(cx.spawn(async move |_this, _cx| {
            auracle_connections::put_ai_model(http, &engine_provider, &model_id, None)
                .await
                .log_err();
        }));
    }

    /// The engine-default header, mapped from the window's shared-settings
    /// snapshot through [`engine_default_summary`]. Designs every state — loading
    /// skeleton, retryable error, and the honest summary — so the row is never
    /// blank.
    fn render_engine_default(
        &self,
        descriptors: &[ProviderDescriptor],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let load = self
            .settings_window
            .read_with(cx, |settings_window, _| {
                settings_window.shared_settings.clone()
            })
            .unwrap_or(auracle_view_state::Load::Pending);

        // A successful settings fetch always carries an `ai_model` object (it
        // defaults), so the payload is never "empty": the predicate is
        // constant-false and the empty hint is unused.
        let state = load.into_view(|_| false, "");

        let body = match state {
            ViewState::Loading | ViewState::Empty { .. } => Label::new("Checking engine default…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
            ViewState::Error { message, retryable } => v_flex()
                .gap_1()
                .child(
                    Label::new(format!("Couldn't read the engine default: {message}."))
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .when(retryable, |this| {
                    this.child(
                        Button::new("ai-providers-retry", "Retry")
                            .tab_index(0_isize)
                            .style(ButtonStyle::Outlined)
                            .start_icon(
                                Icon::new(IconName::RotateCcw)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.reload_shared_settings(cx);
                            })),
                    )
                })
                .into_any_element(),
            ViewState::Ready(settings) => {
                let engine_default_ide_id =
                    auracle_connections::engine_provider_to_ide(&settings.ai_model.provider);
                let summary: EngineDefaultStatus = engine_default_summary(
                    &descriptors,
                    engine_default_ide_id,
                    Some(settings.ai_model.model_id.as_str()),
                    settings.ai_model.configured,
                );
                self.render_engine_default_summary(&summary)
            }
        };

        v_flex()
            .gap_1()
            .child(Label::new("Engine default").size(LabelSize::Large))
            .child(
                Label::new(
                    "The AI model the engine uses as the shared default across the launcher and IDE.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(body)
            .into_any_element()
    }

    fn render_engine_default_summary(&self, summary: &EngineDefaultStatus) -> AnyElement {
        v_flex()
            .pt_1()
            .gap_0p5()
            .child(Label::new(summary.label.clone()).color(tone_color(summary.tone)))
            .when_some(summary.detail.clone(), |this, detail| {
                this.child(
                    Label::new(detail)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }

    fn render_provider_rows(&self, rows: &[AiProviderRow], cx: &mut Context<Self>) -> AnyElement {
        if rows.is_empty() {
            return Label::new("No model providers are available.")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element();
        }

        let last = rows.len().saturating_sub(1);
        v_flex()
            .gap_1()
            .child(Label::new("Providers").size(LabelSize::Large))
            .children(rows.iter().enumerate().map(|(index, row)| {
                // Match the native list convention (see the Skills sub-page):
                // separate rows with a divider, but never trail one after the
                // last row, which would leave a dangling rule above the page's
                // bottom padding.
                self.render_provider_row(row, index < last, cx)
            }))
            .into_any_element()
    }

    fn render_provider_row(
        &self,
        row: &AiProviderRow,
        with_divider: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = LanguageModelProviderId::from(row.id.clone());
        let is_selected = self.selected_provider.as_ref() == Some(&id);
        let authenticated = row.authenticated;
        let configuration_view = self.configuration_views.get(&id).cloned();

        let header = h_flex()
            .id(SharedString::from(format!("ai-provider-{}", row.id)))
            .w_full()
            .justify_between()
            .items_center()
            .py_3()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Label::new(row.display.clone()))
                    .when(row.is_engine_default, |this| {
                        this.child(
                            Label::new("Engine default")
                                .size(LabelSize::Small)
                                .color(Color::Accent),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    // Honest authentication indicator: the checkmark shows ONLY
                    // when `is_authenticated(cx)` was true at descriptor time.
                    .child(if authenticated {
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Icon::new(IconName::Check)
                                    .size(IconSize::Small)
                                    .color(Color::Success),
                            )
                            .child(
                                Label::new("Configured")
                                    .size(LabelSize::Small)
                                    .color(Color::Success),
                            )
                            .into_any_element()
                    } else {
                        Label::new("Not configured")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .into_any_element()
                    })
                    .child(
                        Button::new(
                            SharedString::from(format!("ai-provider-toggle-{}", row.id)),
                            if is_selected { "Hide" } else { "Configure" },
                        )
                        .tab_index(0_isize)
                        .style(ButtonStyle::OutlinedGhost)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _event, _window, cx| {
                                this.select_provider(id.clone(), cx);
                            }
                        })),
                    ),
            );

        v_flex()
            .child(header)
            .when(is_selected, |this| {
                this.child(
                    v_flex()
                        .pb_2()
                        .gap_2()
                        .when_some(configuration_view, |this, view| this.child(view))
                        // Honesty: only offer "Set as default" once the provider
                        // is authenticated; otherwise the click would refuse and
                        // surface a hint, so we don't show a dead control.
                        .when(authenticated, |this| {
                            this.child(
                                Button::new(
                                    SharedString::from(format!("ai-provider-default-{}", row.id)),
                                    "Set as default",
                                )
                                .tab_index(0_isize)
                                .style(ButtonStyle::Outlined)
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _event, _window, cx| {
                                        this.set_as_default(id.clone(), cx);
                                    }
                                })),
                            )
                        })
                        .when_some(self.set_default_feedback_for(&id), |this, feedback| {
                            let color = if feedback.ok {
                                Color::Success
                            } else {
                                Color::Warning
                            };
                            this.child(
                                Label::new(feedback.message.clone())
                                    .size(LabelSize::Small)
                                    .color(color),
                            )
                        }),
                )
            })
            .when(with_divider, |this| {
                this.child(Divider::horizontal().flex_grow_1())
            })
            .into_any_element()
    }

    /// The most recent "Set as default" feedback, shown only under the provider
    /// it referred to (the currently selected one).
    fn set_default_feedback_for(
        &self,
        id: &LanguageModelProviderId,
    ) -> Option<&SetDefaultFeedback> {
        if self.selected_provider.as_ref() == Some(id) {
            self.set_default_feedback.as_ref()
        } else {
            None
        }
    }

    fn reload_shared_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_window
            .update(cx, |settings_window, cx| {
                settings_window.load_shared_settings(cx);
            })
            .log_err();
    }
}

impl Focusable for AiProvidersPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AiProvidersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Recompute the rows every render: authentication state can change while
        // the page is open (the user just entered a key), and `derive_provider_rows`
        // is cheap and pure. The expensive, focus-sensitive part — the embedded
        // configuration views — is the cached `HashMap`, not recomputed here.
        let engine_default_ide_id = self
            .settings_window
            .read_with(cx, |settings_window, _| {
                match &settings_window.shared_settings {
                    auracle_view_state::Load::Done(settings) => {
                        auracle_connections::engine_provider_to_ide(&settings.ai_model.provider)
                            .map(str::to_string)
                    }
                    _ => None,
                }
            })
            .ok()
            .flatten();

        let descriptors = current_provider_descriptors(cx);
        let rows = derive_provider_rows(&descriptors, engine_default_ide_id.as_deref());

        let engine_default = self.render_engine_default(&descriptors, cx);
        let providers = self.render_provider_rows(&rows, cx);

        v_flex()
            .id("ai-providers-page")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                v_flex()
                    .id("ai-providers-scroll")
                    .size_full()
                    .px_8()
                    .pt_2()
                    .pb_16()
                    .gap_6()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .child(engine_default)
                    .child(Divider::horizontal().flex_grow_1())
                    .child(providers),
            )
            .into_element()
    }
}

/// Find a single visible provider by its registry id. Used inside the
/// engine→IDE import task, which must re-fetch the provider from the registry on
/// each `cx.update` rather than hold an `Arc<dyn LanguageModelProvider>` across
/// an `.await`.
fn find_visible_provider(provider_id: &str, cx: &App) -> Option<Arc<dyn LanguageModelProvider>> {
    visible_providers(cx)
        .into_iter()
        .find(|provider| provider.id().0.as_ref() == provider_id)
}

/// All visible language-model providers except the hosted Zed-cloud one, which
/// this fork does not surface for configuration.
fn visible_providers(cx: &App) -> Vec<Arc<dyn LanguageModelProvider>> {
    LanguageModelRegistry::read_global(cx)
        .visible_providers()
        .into_iter()
        .filter(|provider| provider.id() != language_model::ZED_CLOUD_PROVIDER_ID)
        .collect()
}

/// Extract the gpui-free [`ProviderDescriptor`]s for the current registry state,
/// reading `is_authenticated(cx)` honestly for each.
fn current_provider_descriptors(cx: &App) -> Vec<ProviderDescriptor> {
    visible_providers(cx)
        .into_iter()
        .map(|provider| ProviderDescriptor {
            id: provider.id().0.to_string(),
            display: provider.name().0.to_string(),
            authenticated: provider.is_authenticated(cx),
        })
        .collect()
}
