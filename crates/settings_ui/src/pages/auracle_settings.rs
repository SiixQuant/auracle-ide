//! The Auracle pages hosted in the native settings window: each block embeds
//! a single-section `AuracleSettingsPanel` instance (the engine-backed
//! Account / Connections / agent-model / git-identity bodies), so the window
//! is the one settings home while `auracle_onboarding` keeps owning the
//! section logic.

use auracle_onboarding::{AuracleSettingsPanel, EmbedScope};
use gpui::{
    AnyElement, Context, ElementId, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div,
};

use crate::SettingsWindow;

fn render_embedded(
    scope: EmbedScope,
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    // Best-effort workspace handle from the window that opened settings —
    // only the git-identity save needs it (project fs); everything else in
    // the embedded sections talks to the engine over loopback.
    let workspace = settings_window.original_window.as_ref().and_then(|handle| {
        handle
            .read_with(cx, |multi_workspace, _| {
                multi_workspace.workspace().downgrade()
            })
            .ok()
    });
    let panel = window.use_keyed_state(
        ElementId::Name(SharedString::new_static(scope.key())),
        cx,
        move |window, cx| AuracleSettingsPanel::embedded(scope, workspace, window, cx),
    );
    div().w_full().min_w_0().child(panel).into_any_element()
}

pub(crate) fn render_auracle_account(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::Account, settings_window, window, cx)
}

pub(crate) fn render_auracle_brokers(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::Brokers, settings_window, window, cx)
}

pub(crate) fn render_auracle_data_sources(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::Data, settings_window, window, cx)
}

pub(crate) fn render_auracle_integrations(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::Integrations, settings_window, window, cx)
}

pub(crate) fn render_auracle_ai_model(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::AiModel, settings_window, window, cx)
}

pub(crate) fn render_auracle_github(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_embedded(EmbedScope::GitHub, settings_window, window, cx)
}
