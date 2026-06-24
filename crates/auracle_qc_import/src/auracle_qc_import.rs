//! The Import-from-QuantConnect workspace's view-state — gpui-free so the
//! browse → select → translate → coverage flow is unit-tested without the
//! graphics toolchain.
//!
//! The GPUI editor-tab item owns the http/serde: it fetches the connected
//! account's projects and the per-project translate result, extracts plain facts,
//! and feeds them to the pure transitions here. This module decides what the tab
//! renders — the project list, the coverage `StatGrid`, and whether a loading
//! skeleton is showing — and never fabricates a result it wasn't handed.
//!
//! Honesty rules:
//! - a disconnected account renders an honest empty state, never a fake project
//!   list;
//! - translation is produce-and-warn: unmapped constructs are surfaced, never
//!   silently dropped, and coverage is reported as given (never rounded up to a
//!   reassuring 100%).

/// A QuantConnect project as the import workspace lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QcProject {
    pub id: u64,
    pub name: String,
}

/// One labelled metric cell for the coverage `StatGrid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatCell {
    pub label: String,
    pub value: String,
}

/// A translate coverage report, already extracted from the engine's `/translate`
/// payload by the gpui item (never parsed here).
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageReport {
    /// "framework", "qcalgorithm", or "unknown".
    pub style: String,
    /// Fraction of the source that mapped cleanly, 0.0..=1.0.
    pub coverage: f64,
    /// Mapped component / module names.
    pub mapped: Vec<String>,
    /// Constructs that need hand-finishing (warn-only — never dropped silently).
    pub unmapped: Vec<String>,
    /// Free-form translator notes.
    pub notes: Vec<String>,
}

/// The whole import view-state, advanced by the pure transitions below and
/// rendered by the gpui editor-tab item. No http / serde / gpui here.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportState {
    /// False renders the honest "connect QuantConnect first" empty state.
    pub connected: bool,
    /// A fetch (projects or translate) is in flight → render a loading skeleton.
    pub loading: bool,
    pub projects: Vec<QcProject>,
    /// The selected project id, if any.
    pub selected: Option<u64>,
    pub coverage: Option<CoverageReport>,
    /// The last error, surfaced honestly instead of a blank panel.
    pub error: Option<String>,
}

impl ImportState {
    /// A freshly-opened workspace for a connected account: browsing, with the
    /// first project fetch in flight (so the skeleton shows until it lands).
    pub fn browsing() -> Self {
        Self {
            connected: true,
            loading: true,
            ..Self::default()
        }
    }

    /// A workspace opened without a QuantConnect connection — honest empty state.
    pub fn disconnected() -> Self {
        Self {
            connected: false,
            ..Self::default()
        }
    }

    /// The project list arrived: stop the skeleton and show them. Clears any
    /// prior error.
    pub fn set_projects(&mut self, projects: Vec<QcProject>) {
        self.projects = projects;
        self.loading = false;
        self.error = None;
    }

    /// Select a project to translate. Drops any prior coverage so a stale report
    /// never lingers against a different project.
    pub fn select_project(&mut self, id: u64) {
        self.selected = Some(id);
        self.coverage = None;
        self.error = None;
    }

    /// Begin a translate of the selected project. No-op (returns false) when no
    /// project is selected — the item should never translate "nothing".
    pub fn begin_translate(&mut self) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.loading = true;
        self.error = None;
        true
    }

    /// The translate result arrived: stop the skeleton and hold the report.
    pub fn set_coverage(&mut self, coverage: CoverageReport) {
        self.coverage = Some(coverage);
        self.loading = false;
        self.error = None;
    }

    /// A fetch failed: stop the skeleton and surface the message honestly.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.loading = false;
        self.error = Some(message.into());
    }

    /// Whether a translate can be triggered right now (a project is selected and
    /// nothing is already in flight).
    pub fn can_translate(&self) -> bool {
        self.selected.is_some() && !self.loading
    }

    /// Whether the loading skeleton should render.
    pub fn show_skeleton(&self) -> bool {
        self.loading
    }

    /// The coverage `StatGrid` cells, or empty when there is no report yet. The
    /// coverage percentage is reported as given — never rounded up.
    pub fn coverage_stats(&self) -> Vec<StatCell> {
        let Some(report) = &self.coverage else {
            return Vec::new();
        };
        let pct = (report.coverage * 100.0).round() as i64;
        vec![
            StatCell {
                label: "Style".to_string(),
                value: report.style.clone(),
            },
            StatCell {
                label: "Coverage".to_string(),
                value: format!("{pct}%"),
            },
            StatCell {
                label: "Mapped".to_string(),
                value: report.mapped.len().to_string(),
            },
            StatCell {
                label: "Unmapped".to_string(),
                value: report.unmapped.len().to_string(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(style: &str, coverage: f64, mapped: &[&str], unmapped: &[&str]) -> CoverageReport {
        CoverageReport {
            style: style.to_string(),
            coverage,
            mapped: mapped.iter().map(|s| s.to_string()).collect(),
            unmapped: unmapped.iter().map(|s| s.to_string()).collect(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn browsing_starts_connected_and_loading() {
        let state = ImportState::browsing();
        assert!(state.connected);
        assert!(state.show_skeleton());
        assert!(state.projects.is_empty());
    }

    #[test]
    fn disconnected_is_honest_empty_not_loading() {
        let state = ImportState::disconnected();
        assert!(!state.connected);
        assert!(!state.show_skeleton());
    }

    #[test]
    fn set_projects_stops_skeleton_and_populates() {
        let mut state = ImportState::browsing();
        state.set_projects(vec![QcProject {
            id: 7,
            name: "Momentum".to_string(),
        }]);
        assert!(!state.show_skeleton());
        assert_eq!(state.projects.len(), 1);
    }

    #[test]
    fn select_clears_stale_coverage() {
        let mut state = ImportState::browsing();
        state.set_coverage(report("framework", 1.0, &["AlphaModule"], &[]));
        state.select_project(7);
        assert_eq!(state.selected, Some(7));
        assert!(state.coverage.is_none());
    }

    #[test]
    fn translate_requires_a_selection() {
        let mut state = ImportState::browsing();
        state.set_projects(vec![]);
        assert!(!state.begin_translate()); // nothing selected -> no-op
        assert!(!state.show_skeleton());
        state.select_project(7);
        assert!(state.begin_translate());
        assert!(state.show_skeleton());
    }

    #[test]
    fn can_translate_only_with_selection_and_idle() {
        let mut state = ImportState::browsing();
        state.set_projects(vec![]);
        assert!(!state.can_translate()); // no selection, still loading
        state.select_project(7);
        assert!(state.can_translate());
        state.begin_translate();
        assert!(!state.can_translate()); // in flight
    }

    #[test]
    fn coverage_stats_reports_given_percentage_and_counts() {
        let mut state = ImportState::browsing();
        state.select_project(7);
        state.set_coverage(report(
            "framework",
            0.75,
            &["AlphaModule", "PortfolioModule"],
            &["CustomThing"],
        ));
        let stats = state.coverage_stats();
        assert!(stats.contains(&StatCell {
            label: "Coverage".to_string(),
            value: "75%".to_string()
        }));
        assert!(stats.contains(&StatCell {
            label: "Mapped".to_string(),
            value: "2".to_string()
        }));
        assert!(stats.contains(&StatCell {
            label: "Unmapped".to_string(),
            value: "1".to_string()
        }));
    }

    #[test]
    fn coverage_stats_empty_without_report() {
        assert!(ImportState::browsing().coverage_stats().is_empty());
    }

    #[test]
    fn fail_surfaces_error_and_stops_skeleton() {
        let mut state = ImportState::browsing();
        state.fail("engine unreachable");
        assert!(!state.show_skeleton());
        assert_eq!(state.error.as_deref(), Some("engine unreachable"));
    }
}
