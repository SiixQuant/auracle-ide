//! The native "Data sources" section on the Connections settings page.
//!
//! A read-only view of which market-data vendor keys the engine holds. The IDE
//! never edits these (they live in the launcher/engine) and never claims a vendor
//! is configured unless the engine reported it. The page is a thin `match` over
//! the [`SettingsWindow`]'s `shared_settings` snapshot: a designed skeleton while
//! the `/ui/api/settings` fetch is in flight, an honest retryable error when the
//! engine is unreachable, and the vendor list once it arrives. The vendor labels
//! and configured flags come from the gpui-free [`auracle_data_sources`] crate,
//! which humanizes the engine's keys without inventing anything.

use auracle_data_sources::{DataSourceRow, data_source_rows};
use auracle_view_state::ViewState;
use gpui::{ScrollHandle, prelude::*};
use ui::{Divider, prelude::*};

use crate::SettingsWindow;

pub(crate) fn render_data_sources_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    // A successful settings fetch always carries a (possibly empty) data-key map,
    // so the payload is never "empty" at the fetch level; an empty *map* is a
    // real, honest state handled inside `render_ready`.
    let state = settings_window
        .shared_settings
        .clone()
        .into_view(|_settings| false, "");

    let body = match state {
        ViewState::Loading | ViewState::Empty { .. } => render_loading(),
        ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
        ViewState::Ready(settings) => {
            let rows = data_source_rows(
                settings
                    .data_keys
                    .iter()
                    .map(|(vendor, state)| (vendor.as_str(), state.configured)),
            );
            render_ready(&rows)
        }
    };

    v_flex()
        .id("data-sources-page")
        .size_full()
        .pt_2()
        .px_8()
        .pb_16()
        .gap_4()
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .child(body)
        .into_any_element()
}

fn render_loading() -> AnyElement {
    v_flex()
        .gap_2()
        .child(section_header())
        .child(
            Label::new("Checking your data sources…")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
        .into_any_element()
}

fn render_error(message: &str, retryable: bool, cx: &mut Context<SettingsWindow>) -> AnyElement {
    v_flex()
        .gap_2()
        .child(section_header())
        .child(
            Label::new(format!("Couldn't read your data sources: {message}."))
                .size(LabelSize::Small)
                .color(Color::Error),
        )
        .when(retryable, |this| {
            this.child(
                Button::new("data-sources-retry", "Retry")
                    .tab_index(0_isize)
                    .style(ButtonStyle::Outlined)
                    .start_icon(
                        Icon::new(IconName::RotateCcw)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(|settings_window, _event, _window, cx| {
                        settings_window.load_shared_settings(cx);
                    })),
            )
        })
        .into_any_element()
}

fn render_ready(rows: &[DataSourceRow]) -> AnyElement {
    if rows.is_empty() {
        return v_flex()
            .gap_2()
            .child(section_header())
            .child(
                Label::new("The engine isn't reporting any market-data sources yet.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .into_any_element();
    }

    let mut list = v_flex().gap_1().child(section_header()).child(
        Label::new("Market-data vendors the engine holds keys for. Manage these in the launcher.")
            .size(LabelSize::Small)
            .color(Color::Muted),
    );

    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            list = list.child(Divider::horizontal().flex_grow_1());
        }
        list = list.child(data_source_row(row));
    }

    list.into_any_element()
}

fn section_header() -> impl IntoElement {
    Label::new("Data sources").size(LabelSize::Large)
}

/// One vendor row: the humanized name on the left, an honest configured/not
/// status on the right (the engine's `configured` flag is the only thing that
/// decides the colour and text).
fn data_source_row(row: &DataSourceRow) -> impl IntoElement {
    let (status, color) = if row.configured {
        ("Key configured", Color::Success)
    } else {
        ("No key yet", Color::Muted)
    };

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .py_3()
        .gap_4()
        .child(Label::new(row.name.clone()))
        .child(Label::new(status).size(LabelSize::Small).color(color))
}
