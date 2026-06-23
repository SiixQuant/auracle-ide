//! Small render helpers shared across the native settings sub-pages, so the
//! pages stay thin and the list/divider and unreachable-error conventions are
//! written once.

use gpui::AnyElement;
use ui::{Divider, prelude::*};

use crate::SettingsWindow;

/// Append `items` to `container`, separating each from the previous with a
/// horizontal divider — never trailing one after the last row, which would leave
/// a dangling rule above the page's bottom padding. The list convention used by
/// the native settings sub-pages, written once.
pub(crate) fn render_items_with_dividers<E: IntoElement>(
    mut container: Div,
    items: impl IntoIterator<Item = E>,
) -> Div {
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            container = container.child(Divider::horizontal().flex_grow_1());
        }
        container = container.child(item);
    }
    container
}

/// An honest unreachable/failed state for a settings sub-page: the section
/// header, a plain `"{prefix}: {message}."` line, and a Retry button (with the
/// page-specific `button_id` and `on_retry`) shown only when the error is
/// retryable. The one place the pages' error state is built.
pub(crate) fn render_error_with_retry(
    header: impl IntoElement,
    prefix: &str,
    message: &str,
    retryable: bool,
    button_id: &'static str,
    on_retry: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    v_flex()
        .gap_2()
        .child(header)
        .child(
            Label::new(format!("{prefix}: {message}."))
                .size(LabelSize::Small)
                .color(Color::Error),
        )
        .when(retryable, |this| {
            this.child(
                Button::new(button_id, "Retry")
                    .tab_index(0_isize)
                    .style(ButtonStyle::Outlined)
                    .start_icon(
                        Icon::new(IconName::RotateCcw)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(move |settings_window, _event, _window, cx| {
                        on_retry(settings_window, cx);
                    })),
            )
        })
        .into_any_element()
}
