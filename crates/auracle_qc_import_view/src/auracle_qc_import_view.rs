//! The Import-from-QuantConnect workspace tab.
//!
//! Opened as a workspace `Item` (an editor tab) via the `OpenQuantConnectImport`
//! action — from the command palette or handed off from the "Connect
//! QuantConnect" settings page once an account is connected. The tab walks the
//! browse → select → translate → coverage flow:
//!
//! 1. it lists the connected account's QuantConnect projects,
//! 2. the user selects one,
//! 3. on Translate it fetches that project's LEAN files and asks the engine's
//!    structural translator for a coverage report,
//! 4. it renders the report honestly — the coverage percentage is shown exactly
//!    as the engine returned it, and constructs the translator could not map are
//!    surfaced as warnings, never silently dropped.
//!
//! This file is only the render + async-I/O shell. Every decision about what the
//! tab shows (the project list, the coverage `StatGrid`, whether a loading
//! skeleton is up, the honest empty/error states) lives in the gpui-free
//! `auracle_qc_import` reducer so it is unit-tested without the graphics
//! toolchain.

use auracle_connections::{OpenQuantConnectImport, get_json, post_json};
use auracle_qc_import::{CoverageReport, ImportState, QcProject, StatCell};
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Task, Window};
use serde::Deserialize;
use ui::{Banner, Callout, Severity, prelude::*};
use workspace::{Workspace, item::Item};

/// Register the `OpenQuantConnectImport` workspace action so the command palette
/// and the settings hand-off can open the import tab.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenQuantConnectImport, window, cx| {
            open_qc_import(workspace, window, cx);
        });
    })
    .detach();
}

/// Open (or re-open) the QuantConnect import tab in the workspace's active pane.
pub fn open_qc_import(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = cx.new(QcImportView::new);
    workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
}

// ── Engine JSON shapes ───────────────────────────────────────────────

#[derive(Deserialize)]
struct ProjectsResponse {
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    projects: Vec<RawProject>,
}

#[derive(Deserialize)]
struct RawProject {
    #[serde(rename = "projectId", alias = "id", default)]
    id: u64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct FilesResponse {
    #[serde(default)]
    files: Vec<RawFile>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct TranslateResponse {
    #[serde(default)]
    style: String,
    #[serde(default)]
    coverage: f64,
    #[serde(default)]
    components: Vec<RawComponent>,
    #[serde(default)]
    unmapped: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Deserialize)]
struct RawComponent {
    #[serde(default)]
    auracle_module: String,
}

/// The import tab. Holds the pure [`ImportState`] plus the in-flight fetch task.
pub struct QcImportView {
    focus_handle: FocusHandle,
    state: ImportState,
    _task: Option<Task<()>>,
}

impl QcImportView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            state: ImportState::browsing(),
            _task: None,
        };
        view.fetch_projects(cx);
        view
    }

    /// List the connected account's QuantConnect projects. A `connected: false`
    /// payload resets the tab to its honest "connect first" empty state rather
    /// than showing an empty list that looks like a connected account with no
    /// projects.
    fn fetch_projects(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = get_json(http, "/ui/api/quantconnect/projects").await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(value) => match serde_json::from_value::<ProjectsResponse>(value) {
                        Ok(response) if !response.connected => {
                            this.state = ImportState::disconnected();
                        }
                        Ok(response) => {
                            this.state.set_projects(
                                response
                                    .projects
                                    .into_iter()
                                    .map(|project| QcProject {
                                        id: project.id,
                                        name: project.name,
                                    })
                                    .collect(),
                            );
                        }
                        Err(error) => this.state.fail(format!("Couldn't read projects: {error}.")),
                    },
                    Err(error) => this.state.fail(format!("Couldn't load projects: {error}.")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        self.state.select_project(id);
        cx.notify();
    }

    /// Translate the selected project: fetch its LEAN files, join their source,
    /// and ask the engine's structural translator for a coverage report. A
    /// project with no readable source fails honestly instead of reporting a
    /// fabricated 0%/100% against nothing.
    fn translate(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.state.selected else {
            return;
        };
        if !self.state.begin_translate() {
            return;
        }
        cx.notify();
        let http = cx.http_client();
        self._task = Some(cx.spawn(async move |this, cx| {
            let files = get_json(
                http.clone(),
                &format!("/ui/api/quantconnect/projects/{project_id}/files"),
            )
            .await;
            let source = match files {
                Ok(value) => match serde_json::from_value::<FilesResponse>(value) {
                    Ok(response) => join_python_source(response.files),
                    Err(error) => {
                        this.update(cx, |this, cx| {
                            this.state
                                .fail(format!("Couldn't read project files: {error}."));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                },
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.state
                            .fail(format!("Couldn't load project files: {error}."));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            if source.trim().is_empty() {
                this.update(cx, |this, cx| {
                    this.state
                        .fail("This project has no LEAN Python source to translate.");
                    cx.notify();
                })
                .ok();
                return;
            }
            let translated = post_json(
                http,
                "/ui/api/quantconnect/translate",
                serde_json::json!({ "source": source }),
            )
            .await;
            this.update(cx, |this, cx| {
                match translated {
                    Ok(value) => match serde_json::from_value::<TranslateResponse>(value) {
                        Ok(response) => this.state.set_coverage(coverage_from(response)),
                        Err(error) => this
                            .state
                            .fail(format!("Couldn't read translation: {error}.")),
                    },
                    Err(error) => this.state.fail(format!("Translation failed: {error}.")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .children(self.state.projects.iter().map(|project| {
                let selected = self.state.selected == Some(project.id);
                let id = project.id;
                Button::new(
                    SharedString::from(format!("qc-project-{}", project.id)),
                    project.name.clone(),
                )
                .style(if selected {
                    ButtonStyle::Filled
                } else {
                    ButtonStyle::Subtle
                })
                .full_width()
                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            }))
    }

    fn render_coverage(&self) -> impl IntoElement {
        let Some(report) = &self.state.coverage else {
            return div().into_any_element();
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_6()
                    .flex_wrap()
                    .children(self.state.coverage_stats().into_iter().map(render_stat)),
            )
            .when(!report.unmapped.is_empty(), |this| {
                this.child(
                    Callout::new()
                        .severity(Severity::Warning)
                        .icon(IconName::Warning)
                        .title("Needs hand-finishing")
                        .description(format!(
                            "The translator couldn't map {} construct(s): {}. They are kept as TODOs in the scaffold, never dropped.",
                            report.unmapped.len(),
                            report.unmapped.join(", ")
                        )),
                )
            })
            .when(!report.notes.is_empty(), |this| {
                this.child(
                    Label::new(report.notes.join("  •  "))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }
}

/// Join the Python source of a project's files so the translator sees every
/// top-level class. Non-`.py` files (data, config) are skipped.
fn join_python_source(files: Vec<RawFile>) -> String {
    files
        .into_iter()
        .filter(|file| file.name.ends_with(".py"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build the reducer's [`CoverageReport`] from the engine's translate payload.
/// Coverage is carried through exactly as given — never rounded toward a
/// reassuring 100%.
fn coverage_from(response: TranslateResponse) -> CoverageReport {
    CoverageReport {
        style: response.style,
        coverage: response.coverage,
        mapped: response
            .components
            .into_iter()
            .map(|component| component.auracle_module)
            .filter(|module| !module.is_empty())
            .collect(),
        unmapped: response.unmapped,
        notes: response.notes,
    }
}

/// One stat strip cell: a large value over a muted label.
fn render_stat(cell: StatCell) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(Label::new(cell.value).size(LabelSize::Large))
        .child(
            Label::new(cell.label)
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
}

impl Focusable for QcImportView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for QcImportView {}

impl Item for QcImportView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Import from QuantConnect".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Download))
    }
}

impl Render for QcImportView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if !self.state.connected {
            Callout::new()
                .severity(Severity::Info)
                .icon(IconName::Info)
                .title("Connect QuantConnect first")
                .description(
                    "Open Settings → Connections → QuantConnect to connect your account, \
                     then reopen this tab to import your LEAN strategies.",
                )
                .into_any_element()
        } else if self.state.show_skeleton() {
            Label::new("Loading your QuantConnect projects…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element()
        } else {
            v_flex()
                .gap_4()
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(self.render_projects(cx))
                        .child(
                            Button::new("qc-translate", "Translate")
                                .style(ButtonStyle::Filled)
                                .disabled(!self.state.can_translate())
                                .on_click(cx.listener(|this, _, _, cx| this.translate(cx))),
                        ),
                )
                .when_some(self.state.error.clone(), |this, message| {
                    this.child(
                        Banner::new()
                            .severity(Severity::Warning)
                            .child(Label::new(message).size(LabelSize::Small)),
                    )
                })
                .child(self.render_coverage())
                .into_any_element()
        };

        v_flex()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .gap_4()
            .p_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Import from QuantConnect").size(LabelSize::Large))
                    .child(
                        Label::new(
                            "Browse a project, then translate its LEAN source into an Auracle scaffold.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
            )
            .child(body)
    }
}
