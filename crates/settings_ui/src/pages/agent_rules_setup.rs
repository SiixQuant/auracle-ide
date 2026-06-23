use std::path::PathBuf;

use auracle_system_prompt::{PromptStoreFacts, RuleRow, RuleSource, RuleTone, RulesFacts};
use gpui::{Action as _, ScrollHandle, prelude::*};
use ui::{Divider, prelude::*};
use util::ResultExt as _;

use crate::SettingsWindow;

pub(crate) fn render_agent_rules_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    // The state resolves asynchronously when the sub-page is pushed (the prompt
    // store loads async; `AGENTS.md` existence is an async stat). Until it lands,
    // show an honest loading state rather than guessing at counts or presence.
    let Some(state) = settings_window.agent_rules() else {
        return v_flex()
            .id("agent-rules-page")
            .size_full()
            .px_8()
            .pt_2()
            .pb_16()
            .items_center()
            .justify_center()
            .child(
                Label::new("Loading agent rules…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .into_any_element();
    };

    let prompt_store = match state.prompt_store.as_ref() {
        Some(store) => {
            let metadata = store.read(cx).all_prompt_metadata();
            // The store seeds built-in prompts (e.g. the commit-message prompt)
            // into its metadata; those aren't user-authored agent rules, so the
            // count and the "default set" check exclude them.
            let user_rules = metadata.iter().filter(|m| !m.id.is_built_in());
            PromptStoreFacts {
                loaded: true,
                rule_count: user_rules.clone().count(),
                has_default: user_rules.clone().any(|m| m.default),
            }
        }
        None => PromptStoreFacts {
            loaded: false,
            rule_count: 0,
            has_default: false,
        },
    };

    let facts = RulesFacts {
        global_file_exists: state.global_file_exists,
        has_project_worktree: state.project_path.is_some(),
        project_file_exists: state.project_file_exists,
        prompt_store,
    };
    let rows = auracle_system_prompt::derive_rows(facts);

    v_flex()
        .id("agent-rules-page")
        .track_scroll(scroll_handle)
        .overflow_y_scroll()
        .size_full()
        .pt_2()
        .pb_16()
        .child(
            v_flex().px_8().pb_3().gap_1().child(
                Label::new(
                    "The agent's standing instructions come from your AGENTS.md rules files and \
                     the reusable rules library. Open any source below to view or edit it.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            ),
        )
        .children(rows.iter().enumerate().flat_map(|(index, row)| {
            let mut elements: Vec<AnyElement> = vec![render_rule_row(row, cx)];
            if index + 1 < rows.len() {
                elements.push(
                    div()
                        .px_8()
                        .child(Divider::horizontal().flex_grow_1())
                        .into_any_element(),
                );
            }
            elements
        }))
        .into_any_element()
}

fn render_rule_row(row: &RuleRow, cx: &mut Context<SettingsWindow>) -> AnyElement {
    let status_color = match row.tone {
        RuleTone::Active => Color::Success,
        RuleTone::Neutral => Color::Muted,
    };
    let source = row.source;
    let open_id = match source {
        RuleSource::GlobalAgentsFile => "open-global-rules",
        RuleSource::ProjectAgentsFile => "open-project-rules",
        RuleSource::PromptStore => "open-rules-library",
    };

    h_flex()
        .w_full()
        .justify_between()
        .py_3()
        .px_8()
        .gap_4()
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .flex_1()
                .child(Label::new(row.title.clone()))
                .child(
                    Label::new(row.status.clone())
                        .size(LabelSize::Small)
                        .color(status_color),
                ),
        )
        .when(row.openable, |this| {
            this.child(
                Button::new(open_id, "Open")
                    .tab_index(0_isize)
                    .style(ButtonStyle::OutlinedGhost)
                    .size(ButtonSize::Medium)
                    .end_icon(
                        Icon::new(IconName::ArrowUpRight)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(move |settings_window, _event, window, cx| {
                        open_rule_source(settings_window, source, window, cx);
                    })),
            )
        })
        .into_any_element()
}

/// Act on an "Open" affordance against the *real* source. File sources open the
/// `AGENTS.md` in an editor buffer (creating it on first open, so editing
/// round-trips through the agent's own file); the store source opens the native
/// skills manager, which hosts the reusable rules library — there is no fake
/// in-page editor for it.
fn open_rule_source(
    settings_window: &mut SettingsWindow,
    source: RuleSource,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) {
    match source {
        RuleSource::GlobalAgentsFile => {
            open_abs_path_in_workspace(settings_window, paths::agents_file().clone(), window, cx);
        }
        RuleSource::ProjectAgentsFile => {
            if let Some(path) = settings_window
                .agent_rules()
                .and_then(|state| state.project_path.clone())
            {
                open_abs_path_in_workspace(settings_window, path, window, cx);
            }
        }
        RuleSource::PromptStore => {
            // In this fork the reusable rules library is surfaced through the
            // native Skills manager (rules were migrated to skills); open it
            // rather than presenting a fake in-page editor.
            window.dispatch_action(zed_actions::assistant::ManageSkills.boxed_clone(), cx);
        }
    }
}

/// Open an absolute path as an editor buffer in the underlying workspace, then
/// close the settings window — mirroring how the Skills page opens a skill file.
fn open_abs_path_in_workspace(
    settings_window: &SettingsWindow,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) {
    let Some(original_window) = settings_window.original_window else {
        return;
    };
    original_window
        .update(cx, |multi_workspace, original_window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(
                        path,
                        workspace::OpenOptions {
                            focus: Some(true),
                            ..Default::default()
                        },
                        original_window,
                        cx,
                    )
                    .detach_and_log_err(cx);
            });
        })
        .log_err();
    window.remove_window();
}
