//! Deploy wizard — the ⚡ front door, opened as an editor tab.
//!
//! Renders the gpui-free [`auracle_live::DeployWizard`] form: Mode (Paper/Live),
//! Brokerage, the **required** Starting Capital (AUM) field, Compute target,
//! and the auto-restart toggle. The Deploy button is gated by the reducer's
//! `validate()` — the same rules the engine preflight enforces — so the user
//! sees why deploy is blocked (most importantly: live requires AUM > 0) before
//! they click, not as a surprise 400 after.
//!
//! Selection fields are buttons (no popover dependency); only the two genuine
//! free-text fields (Name + AUM) use single-line editors. On Deploy the form
//! POSTs `to_request()` to `/ui/api/deploy/live` and reports the outcome; the
//! new deployment then appears in the Live Algorithms panel.

use std::sync::Arc;

use auracle_live::{Compute, DeployWizard, Mode};
use editor::Editor;
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Task, WeakEntity, Window, actions,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::item::Item;

actions!(
    auracle_live_ui,
    [
        /// Open the Deploy wizard as an editor tab.
        OpenDeployWizard
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenDeployWizard, window, cx| {
            let item = cx.new(|cx| DeployWizardItem::new(window, cx));
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
        });
    })
    .detach();
}

pub struct DeployWizardItem {
    focus_handle: FocusHandle,
    wizard: DeployWizard,
    name_editor: Entity<Editor>,
    aum_editor: Entity<Editor>,
    status: Option<SharedString>,
    _task: Option<Task<()>>,
}

const BROKERS: &[(&str, &str)] = &[
    ("clearstreet", "ClearStreet"),
    ("alpaca", "Alpaca"),
    ("ibkr", "Interactive Brokers"),
];

impl DeployWizardItem {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_editor = cx.new(|cx| {
            let mut e = Editor::single_line(window, cx);
            e.set_placeholder_text("e.g. Momentum — Tech", window, cx);
            e
        });
        let aum_editor = cx.new(|cx| {
            let mut e = Editor::single_line(window, cx);
            e.set_placeholder_text("Starting capital, e.g. 100000", window, cx);
            e
        });
        Self {
            focus_handle: cx.focus_handle(),
            wizard: DeployWizard::new(),
            name_editor,
            aum_editor,
            status: None,
            _task: None,
        }
    }

    /// Pull the free-text fields out of the editors into the reducer so
    /// `validate()` / `to_request()` see the latest values.
    fn sync(&mut self, cx: &App) {
        self.wizard.name = self.name_editor.read(cx).text(cx).trim().to_string();
        let aum_text = self.aum_editor.read(cx).text(cx);
        self.wizard.aum = aum_text.trim().replace(['$', ','], "").parse::<f64>().ok();
    }

    fn deploy(&mut self, cx: &mut Context<Self>) {
        self.sync(cx);
        if !self.wizard.can_deploy() {
            return;
        }
        let body = self.wizard.to_request();
        let http = cx.http_client();
        self.status = Some("Deploying…".into());
        cx.notify();
        self._task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = auracle_connections::post_json(http, "/ui/api/deploy/live", body).await;
            this.update(cx, |this, cx| {
                this.status = Some(match result {
                    Ok(_) => "Deployed. Open Live Algorithms to watch it.".into(),
                    Err(error) => SharedString::from(format!("Deploy failed: {error}")),
                });
                cx.notify();
            })
            .ok();
        }));
    }

    fn pill(
        &self,
        id: &str,
        label: &str,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new(SharedString::from(id.to_string()), label.to_string())
            .style(if selected {
                ButtonStyle::Filled
            } else {
                ButtonStyle::Subtle
            })
            .label_size(LabelSize::Small)
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
    }

    fn section(&self, title: &str, body: impl IntoElement) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1p5()
            .child(Label::new(title.to_string()).size(LabelSize::Small).color(Color::Muted))
            .child(body)
    }
}

impl Focusable for DeployWizardItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for DeployWizardItem {}

impl Item for DeployWizardItem {
    type Event = ();

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<ui::Icon> {
        Some(ui::Icon::new(IconName::PlayOutlined))
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        SharedString::from("Deploy")
    }
}

impl Render for DeployWizardItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        let errors = self.wizard.validate();
        let can_deploy = errors.is_empty();
        let border = cx.theme().colors().border;
        let field_bg = cx.theme().colors().editor_background;

        // Mode
        let mode = self.wizard.mode;
        let mode_row = h_flex()
            .gap_1()
            .child(self.pill("mode-paper", "Paper", mode == Mode::Paper, |t, _, _| t.wizard.mode = Mode::Paper, cx))
            .child(self.pill("mode-live", "Live", mode == Mode::Live, |t, _, _| t.wizard.mode = Mode::Live, cx));

        // Brokerage
        let selected_broker = self.wizard.broker.clone();
        let broker_row = h_flex().gap_1().flex_wrap().children(BROKERS.iter().map(|(id, label)| {
            let bid = id.to_string();
            let is_sel = selected_broker.as_deref() == Some(*id);
            self.pill(
                &format!("broker-{id}"),
                label,
                is_sel,
                move |t, _, _| t.wizard.broker = Some(bid.clone()),
                cx,
            )
        }));

        // Compute
        let compute = self.wizard.compute;
        let compute_row = h_flex()
            .gap_1()
            .child(self.pill("compute-local", "This machine", compute == Compute::Local, |t, _, _| t.wizard.compute = Compute::Local, cx))
            .child(self.pill("compute-oci", "Oracle Cloud", compute == Compute::Oci, |t, _, _| t.wizard.compute = Compute::Oci, cx))
            .child(self.pill("compute-aws", "AWS", compute == Compute::Aws, |t, _, _| t.wizard.compute = Compute::Aws, cx));

        let auto_restart = self.wizard.auto_restart;

        let field = |child: gpui::AnyElement| {
            div()
                .w_full()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(field_bg)
                .child(child)
        };

        v_flex()
            .id("deploy-wizard")
            .key_context("DeployWizardItem")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .max_w(px(640.))
            .child(Label::new("Deploy a strategy").size(LabelSize::Large))
            .child(self.section("Name", field(self.name_editor.clone().into_any_element())))
            .child(self.section("Mode", mode_row))
            .child(self.section("Brokerage", broker_row))
            .child(self.section(
                "Starting capital (AUM) — required for live",
                field(self.aum_editor.clone().into_any_element()),
            ))
            .child(self.section("Compute", compute_row))
            .child(
                self.section(
                    "Resilience",
                    self.pill(
                        "auto-restart",
                        if auto_restart { "Auto-restart: on" } else { "Auto-restart: off" },
                        auto_restart,
                        |t, _, _| t.wizard.auto_restart = !t.wizard.auto_restart,
                        cx,
                    ),
                ),
            )
            .when(!errors.is_empty(), |this| {
                this.child(
                    v_flex().gap_0p5().children(
                        errors
                            .iter()
                            .map(|e| Label::new(e.clone()).size(LabelSize::Small).color(Color::Warning)),
                    ),
                )
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(Label::new(status).size(LabelSize::Small).color(Color::Accent))
            })
            .child(
                Button::new("deploy-submit", "Deploy")
                    .style(ButtonStyle::Filled)
                    .disabled(!can_deploy)
                    .tooltip(ui::Tooltip::text(if can_deploy {
                        "Send the deployment to the engine"
                    } else {
                        "Complete the required fields first"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.deploy(cx))),
            )
    }
}
