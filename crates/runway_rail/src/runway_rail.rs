//! The Quant Runway rail — the persistent six-stage spine of the
//! Auracle IDE (Research → Build → Validate → Paper → Go live →
//! Monitor).
//!
//! This is the geometry-reserving placeholder demanded by the design
//! council: every stage renders locked, with plain-word hints, until
//! the engine exposes its stage/gate truth API. The rail never
//! invents progress — a stage lights up only when the engine says so.

use anyhow::Result;
use gpui::{
    App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, Pixels, WeakEntity,
    Window, actions, px,
};
use ui::Tooltip;
use ui::prelude::*;
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    runway_rail,
    [
        /// Toggle focus on the runway rail.
        ToggleFocus
    ]
);

const STAGES: [(&str, &str); 6] = [
    ("Research", "Look at markets, data, and ideas."),
    ("Build", "Shape an idea into a strategy."),
    ("Validate", "Test it against the past, honestly."),
    ("Paper", "Practice with pretend money."),
    ("Go live", "Real money — only after every gate is green."),
    ("Monitor", "Watch everything that runs."),
];

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<RunwayRail>(window, cx);
        });
    })
    .detach();
}

pub struct RunwayRail {
    focus_handle: FocusHandle,
}

impl RunwayRail {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_workspace, _window, cx| {
            cx.new(|cx| Self {
                focus_handle: cx.focus_handle(),
            })
        })
    }
}

impl EventEmitter<PanelEvent> for RunwayRail {}

impl Focusable for RunwayRail {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for RunwayRail {
    fn persistent_name() -> &'static str {
        "RunwayRail"
    }

    fn panel_key() -> &'static str {
        "RunwayRail"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(192.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::PlayOutlined)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Runway")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}

impl Render for RunwayRail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("RunwayRail")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .p_2()
            .gap_1()
            .child(
                h_flex().px_1().pb_1().child(
                    Label::new("RUNWAY")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .children(STAGES.iter().enumerate().map(|(ix, (name, hint))| {
                let tooltip_text: SharedString = format!(
                    "{hint} Locked — this stage lights up when your Auracle engine \
                     starts tracking the runway."
                )
                .into();
                h_flex()
                    .id(ix)
                    .px_1()
                    .py_0p5()
                    .gap_2()
                    .rounded_sm()
                    .hover(|style| style.bg(cx.theme().colors().ghost_element_hover))
                    .child(
                        Icon::new(IconName::LockOutlined)
                            .size(IconSize::XSmall)
                            .color(Color::Disabled),
                    )
                    .child(Label::new(*name).color(Color::Disabled))
                    .tooltip(Tooltip::text(tooltip_text))
            }))
            .child(div().flex_1())
            .child(
                Label::new("Not connected to the engine's runway yet.")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
    }
}
