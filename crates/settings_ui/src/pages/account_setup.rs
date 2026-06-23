//! The native Account page — who's signed in, on what plan, with what license.
//!
//! The page is a thin `match` over [`ViewState`]: a designed skeleton while the
//! `/ui/api/me` fetch is in flight, an honest retryable error when the engine is
//! unreachable, and an identity panel once the profile arrives. The profile
//! itself lives on [`SettingsWindow`] (loaded once when the window opens); this
//! module only maps that fetch outcome to elements. Honesty laws come from the
//! pure helpers it leans on: [`license_summary`] never invents a status or a day
//! count it wasn't given, and nothing here fabricates an identity the engine
//! didn't return.

use auracle_account::{LicenseTone, PersonalSetting, license_summary, personal_settings};
use auracle_connections::Profile;
use auracle_view_state::ViewState;
use gpui::{ScrollHandle, prelude::*};
use ui::{Divider, prelude::*};

use crate::SettingsWindow;
use crate::pages::page_helpers::{render_error_with_retry, render_items_with_dividers};

/// Map a [`LicenseTone`] to the theme colour the license row renders in. Only
/// theme `Color::*` — never a colour literal — so the page tracks the theme.
fn tone_color(tone: LicenseTone) -> Color {
    match tone {
        LicenseTone::Positive => Color::Success,
        LicenseTone::Caution => Color::Warning,
        LicenseTone::Negative => Color::Error,
        LicenseTone::Neutral => Color::Muted,
    }
}

pub(crate) fn render_account_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    // The account payload is never "empty": a successful fetch always carries an
    // identity, so the predicate is constant-false and the empty hint is unused.
    let state = settings_window
        .account_profile
        .clone()
        .into_view(|_profile| false, "");

    let body = match state {
        ViewState::Loading => render_loading(),
        ViewState::Empty { .. } => {
            // Unreachable by construction (the predicate above never reports
            // empty), but handled rather than panicked: treat it as loading.
            render_loading()
        }
        ViewState::Error { message, retryable } => render_error(&message, retryable, cx),
        ViewState::Ready(profile) => render_ready(&profile, cx),
    };

    v_flex()
        .id("account-page")
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

/// A designed skeleton — placeholder rows, not a bare spinner — while the
/// profile fetch is in flight.
fn render_loading() -> AnyElement {
    let skeleton_row = || {
        h_flex().justify_between().py_2().gap_4().child(
            Label::new("Loading…")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
    };

    v_flex()
        .gap_1()
        .child(section_header())
        .child(skeleton_row())
        .child(Divider::horizontal().flex_grow_1())
        .child(skeleton_row())
        .child(Divider::horizontal().flex_grow_1())
        .child(skeleton_row())
        .into_any_element()
}

/// An honest error state — the engine is unreachable or the fetch failed — with
/// a Retry affordance only when the error is retryable.
fn render_error(message: &str, retryable: bool, cx: &mut Context<SettingsWindow>) -> AnyElement {
    render_error_with_retry(
        section_header(),
        "Couldn't load your account",
        message,
        retryable,
        "account-retry",
        |settings_window, cx| settings_window.load_account_profile(cx),
        cx,
    )
}

/// The signed-in identity, plan, and license.
fn render_ready(profile: &Profile, cx: &mut Context<SettingsWindow>) -> AnyElement {
    let email = profile
        .email
        .clone()
        .filter(|email| !email.is_empty())
        .unwrap_or_else(|| "Unknown".to_string());

    let plan = if profile.tier_display.is_empty() {
        // Fall back to the raw tier id rather than fabricating a label, and only
        // if even that is absent show the honest placeholder.
        if profile.tier.is_empty() {
            "Unknown".to_string()
        } else {
            profile.tier.clone()
        }
    } else {
        profile.tier_display.clone()
    };

    let license = license_summary(&profile.license.status, profile.license.days_remaining);
    let license_color = tone_color(license.tone);

    v_flex()
        .gap_1()
        .child(section_header())
        .child(detail_row("Signed in as", email, Color::Default, None))
        .when_some(
            profile.role.clone().filter(|role| !role.is_empty()),
            |this, role| {
                this.child(Divider::horizontal().flex_grow_1())
                    .child(detail_row("Role", role, Color::Default, None))
            },
        )
        .child(Divider::horizontal().flex_grow_1())
        .child(detail_row("Plan", plan, Color::Default, None))
        .child(Divider::horizontal().flex_grow_1())
        .child(detail_row(
            "License",
            license.label,
            license_color,
            license.detail,
        ))
        .child(render_personal_settings(cx))
        .into_any_element()
}

fn section_header() -> impl IntoElement {
    Label::new("Account").size(LabelSize::Large)
}

/// The "Personal settings" section: the settings Zed treats as the user's own
/// (appearance, editor font, keymap), surfaced here as native navigation into
/// the relevant settings page. Identity/plan/license above are engine-owned and
/// read-only; these are the editable, per-user settings — each row jumps to the
/// real native control rather than re-implementing it.
fn render_personal_settings(cx: &mut Context<SettingsWindow>) -> impl IntoElement {
    let section = v_flex()
        .pt_6()
        .gap_1()
        .child(Label::new("Personal settings").size(LabelSize::Large))
        .child(
            Label::new("Your appearance and editor preferences, kept per user.")
                .size(LabelSize::Small)
                .color(Color::Muted),
        );

    let mut rows = Vec::new();
    for (index, setting) in personal_settings().into_iter().enumerate() {
        rows.push(personal_setting_row(index, setting, cx));
    }
    render_items_with_dividers(section, rows)
}

/// One per-user setting row: label + description on the left, a native button on
/// the right that navigates to the setting's own native page. The button carries
/// the `json_path` (a stable settings identifier, not a secret) so the click
/// lands on a real, editable control.
fn personal_setting_row(
    index: usize,
    setting: PersonalSetting,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement + use<> {
    let target = setting.target_json_path;

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .py_3()
        .gap_4()
        .child(
            v_flex()
                .gap_0p5()
                .child(Label::new(setting.title))
                .child(
                    Label::new(setting.description)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
        .child(
            Button::new(("personal-setting", index), "Open")
                .style(ButtonStyle::Outlined)
                .tab_index(0_isize)
                .end_icon(
                    Icon::new(IconName::ChevronRight)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .on_click(cx.listener(move |settings_window, _event, window, cx| {
                    settings_window.navigate_to_setting(target, window, cx);
                })),
        )
}

/// One label/value row, with the value coloured by `value_color` and an optional
/// muted detail beneath it (used for the license's "N days left" line).
fn detail_row(
    label: &'static str,
    value: String,
    value_color: Color,
    detail: Option<String>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .items_start()
        .py_3()
        .gap_4()
        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
        .child(
            v_flex()
                .items_end()
                .gap_0p5()
                .child(Label::new(value).color(value_color))
                .when_some(detail, |this, detail| {
                    this.child(
                        Label::new(detail)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                }),
        )
}
