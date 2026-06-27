//! Shared scaffolding for the Auracle dock panels.
//!
//! Each Auracle panel (blotter, incidents, schedules, strategies,
//! validation, …) polls a `/ui/api/*` feed, shows a four-state
//! placeholder while it can't render real rows, and wears a common
//! header. This crate hoists those byte-identical pieces so a panel
//! carries only its own feed shape and row rendering.
//!
//! Honesty is preserved by construction: [`PanelStatus`] has no "green"
//! of its own — a panel only renders rows (and its own success color)
//! after a real engine round-trip flips it to [`PanelStatus::Connected`];
//! the in-flight [`PanelStatus::Loading`] and the
//! [`PanelStatus::Unreachable`] states are neutral.

use std::future::Future;

use gpui::{AnyElement, Context, Task, WeakEntity};
use ui::prelude::*;

/// The four connection states every polled Auracle panel moves through.
/// A panel starts in `Loading` when a key is already saved (it's about to
/// verify) or `NotConnected` when none is; a successful fetch flips it to
/// `Connected`, a failed one to `Unreachable`.
#[derive(Clone, PartialEq)]
pub enum PanelStatus {
    NotConnected,
    Loading,
    Connected,
    Unreachable,
}

impl PanelStatus {
    /// The status a freshly-loaded panel should start in: `Loading` when a
    /// key is already saved (a poll is imminent), else `NotConnected`.
    pub fn initial() -> Self {
        if auracle_connect::load_config().api_key.is_some() {
            PanelStatus::Loading
        } else {
            PanelStatus::NotConnected
        }
    }
}

/// Spawn the standard poll loop: fetch, apply the result to the entity,
/// notify, then sleep `POLL_EVERY` and repeat — stopping cleanly when the
/// entity is dropped. `fetch` produces the in-flight future each tick and
/// `apply` writes its result into the panel (setting status + rows); the
/// helper calls `cx.notify()` after `apply`, so callers needn't.
pub fn spawn_poll<T, R, Fetch, Fut, Apply>(
    cx: &mut Context<T>,
    poll_every: std::time::Duration,
    fetch: Fetch,
    apply: Apply,
) -> Task<()>
where
    T: 'static,
    Fetch: Fn() -> Fut + 'static,
    Fut: Future<Output = R>,
    Apply: Fn(&mut T, R, &mut Context<T>) + 'static,
{
    cx.spawn(async move |this: WeakEntity<T>, cx| {
        loop {
            let fetched = fetch().await;
            let ok = this
                .update(cx, |this, cx| {
                    apply(this, fetched, cx);
                    cx.notify();
                })
                .is_ok();
            if !ok {
                return;
            }
            cx.background_executor().timer(poll_every).await;
        }
    })
}

/// The shared panel header: a bottom-bordered strip carrying the panel's
/// all-caps title. Returns a `Div` so callers can chain more children
/// (provenance notes, status dots, a "Cancel all" button, …).
pub fn panel_header(title: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .px_2()
        .py_1()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().colors().border_variant)
        .child(
            Label::new(title.into())
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
}

/// The labels a panel shows in its three non-connected states plus its
/// connected-but-empty state. Only the loading and empty lines vary across
/// panels in practice; the not-connected and unreachable lines are shared
/// defaults but stay overridable so behavior is byte-identical per panel.
pub struct PlaceholderLabels {
    /// Stable element id for the Connect button (e.g. `"blotter-connect"`).
    pub connect_button_id: &'static str,
    /// Shown while the first poll is in flight (e.g. `"Checking…"`).
    pub loading: SharedString,
    /// Shown when the engine is reachable but the feed is empty.
    pub empty: SharedString,
    /// Shown when no key is saved yet.
    pub not_connected: SharedString,
    /// Shown when a saved key's engine didn't answer.
    pub unreachable: SharedString,
}

impl PlaceholderLabels {
    /// The common defaults: the standard not-connected and unreachable
    /// lines plus the given connect-button id, loading, and empty lines.
    pub fn new(
        connect_button_id: &'static str,
        loading: impl Into<SharedString>,
        empty: impl Into<SharedString>,
    ) -> Self {
        Self {
            connect_button_id,
            loading: loading.into(),
            empty: empty.into(),
            not_connected: "Not connected to your Auracle engine yet.".into(),
            unreachable: "Your engine didn't answer. It may be stopped.".into(),
        }
    }
}

/// Render the four-state placeholder body shared by the polled panels. When
/// the status is `Connected`, returns `None` so the caller renders its own
/// rows; otherwise returns the matching placeholder element (the Connect
/// button dispatches [`auracle_connect::Connect`]). `is_empty` is the
/// caller's own "no rows" check, used only in the `Connected` case to pick
/// between the empty message and yielding to the caller.
pub fn placeholder_body(
    status: &PanelStatus,
    is_empty: bool,
    labels: &PlaceholderLabels,
) -> Option<AnyElement> {
    match status {
        PanelStatus::NotConnected => Some(
            v_flex()
                .p_3()
                .gap_2()
                .child(Label::new(labels.not_connected.clone()))
                .child(
                    Button::new(labels.connect_button_id, "Connect…")
                        .style(ButtonStyle::Filled)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(auracle_connect::Connect), cx);
                        }),
                )
                .into_any_element(),
        ),
        PanelStatus::Loading => Some(
            v_flex()
                .p_3()
                .child(Label::new(labels.loading.clone()).color(Color::Muted))
                .into_any_element(),
        ),
        PanelStatus::Unreachable => Some(
            v_flex()
                .p_3()
                .child(Label::new(labels.unreachable.clone()).color(Color::Muted))
                .into_any_element(),
        ),
        PanelStatus::Connected if is_empty => Some(
            v_flex()
                .p_3()
                .child(Label::new(labels.empty.clone()).color(Color::Muted))
                .into_any_element(),
        ),
        PanelStatus::Connected => None,
    }
}
