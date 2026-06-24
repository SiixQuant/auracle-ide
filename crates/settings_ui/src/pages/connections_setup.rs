//! The native "Connect a broker" sub-page on the Connections settings page.
//!
//! Like the "Model providers" sub-page, the interactive part is a dedicated
//! [`Render`] entity (`auracle_connections::BrokerWizard`) rather than a plain
//! render-fn over `&SettingsWindow`: the broker-connect flow embeds credential
//! editors whose focus must survive re-renders, so the entity is built once when
//! the sub-page is pushed (see `SettingsWindow::push_sub_page`) and dropped when
//! it is popped. This module only defers to that entity, showing a designed
//! loading hint in the (transient) window before it exists.

use gpui::{ScrollHandle, prelude::*};
use ui::prelude::*;

use crate::SettingsWindow;

pub(crate) fn render_connections_page(
    settings_window: &SettingsWindow,
    _scroll_handle: &ScrollHandle,
    _window: &mut Window,
    _cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Some(page) = settings_window.broker_connect_page() else {
        return Label::new("Loading broker connections…")
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element();
    };
    page.into_any_element()
}

/// The native "Connect QuantConnect" sub-page. Like the broker page, it defers to
/// a dedicated entity (`auracle_connections::QuantConnectConnect`) whose credential
/// editors must survive re-renders, showing a loading hint until it exists.
pub(crate) fn render_quantconnect_page(
    settings_window: &SettingsWindow,
    _scroll_handle: &ScrollHandle,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Some(page) = settings_window.quantconnect_connect_page() else {
        return Label::new("Loading QuantConnect…")
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element();
    };
    // The connect form owns its own scroll; pin the import hand-off below it. The
    // import tab lives in the workspace window, so opening it from this (separate)
    // settings window dispatches across to the original workspace — the same
    // cross-window hand-off the "Manage Trust" banner uses.
    let original_window = settings_window.original_window;
    v_flex()
        .size_full()
        .child(div().flex_1().min_h_0().child(page))
        .child(
            h_flex().px_8().pb_4().justify_end().child(
                Button::new("qc-open-import", "Import projects →")
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(move |_settings_window, _, _window, cx| {
                        if let Some(original_window) = original_window {
                            original_window
                                .update(cx, |multi_workspace, window, cx| {
                                    multi_workspace.workspace().update(cx, |workspace, cx| {
                                        auracle_qc_import_view::open_qc_import(
                                            workspace, window, cx,
                                        );
                                    });
                                })
                                .ok();
                        }
                    })),
            ),
        )
        .into_any_element()
}
