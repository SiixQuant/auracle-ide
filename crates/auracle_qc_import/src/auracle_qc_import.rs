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

/// One side-by-side row of the divergence read-out: the same metric from the
/// QuantConnect backtest and from the Auracle backtest of the translated
/// strategy, plus the signed difference. All three are display-ready strings
/// exactly as the engine handed them — never recomputed here — so a missing side
/// reads as an em dash rather than a fabricated zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaMetric {
    pub label: String,
    pub quantconnect: String,
    pub auracle: String,
    pub delta: String,
}

/// The divergence between the original QuantConnect run and Auracle's run of the
/// translated strategy. `Unavailable` is the honest state when one or both sides
/// haven't been produced (no QC backtest on record, or the engine comparison
/// route isn't deployed) — surfaced with its reason instead of an all-zero table
/// that would imply a perfect, verified match.
#[derive(Debug, Clone, PartialEq)]
pub enum Comparison {
    Unavailable { reason: String },
    Rows(Vec<DeltaMetric>),
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
    /// The divergence read-out, once a translated strategy has been run and
    /// compared against its QuantConnect original. `None` until then.
    pub comparison: Option<Comparison>,
    /// Set once the translated scaffold has been saved as an Auracle strategy and
    /// handed off to the cockpit — gates the Land verb so it can't double-land.
    pub landed: bool,
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

    /// Select a project to translate. Drops any prior coverage, comparison, and
    /// landed flag so nothing from the previously-selected project lingers
    /// against this one.
    pub fn select_project(&mut self, id: u64) {
        self.selected = Some(id);
        self.coverage = None;
        self.comparison = None;
        self.landed = false;
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

    /// Record the divergence read-out between the QuantConnect and Auracle runs.
    pub fn set_comparison(&mut self, comparison: Comparison) {
        self.comparison = Some(comparison);
        self.error = None;
    }

    /// Mark the translated strategy as landed in Auracle (saved + handed off to
    /// the cockpit). Idempotent.
    pub fn mark_landed(&mut self) {
        self.landed = true;
    }

    /// The divergence rows to render, or empty when there is no comparison yet or
    /// it is unavailable (use [`Self::comparison_note`] for the why in that case).
    pub fn comparison_rows(&self) -> Vec<DeltaMetric> {
        match &self.comparison {
            Some(Comparison::Rows(rows)) => rows.clone(),
            _ => Vec::new(),
        }
    }

    /// The honest reason the divergence can't be shown, when the comparison is
    /// unavailable; `None` once real rows exist or before any comparison is run.
    pub fn comparison_note(&self) -> Option<String> {
        match &self.comparison {
            Some(Comparison::Unavailable { reason }) => Some(reason.clone()),
            _ => None,
        }
    }

    /// Whether the translated project can be landed as an Auracle strategy: a
    /// translation has been produced and it hasn't already been landed. Coverage
    /// need not be 100% — a partial scaffold is still a useful starting point, and
    /// the unmapped TODOs travel with it.
    pub fn can_land(&self) -> bool {
        self.coverage.is_some() && !self.landed
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
        state.set_comparison(Comparison::Rows(Vec::new()));
        state.mark_landed();
        state.select_project(7);
        assert_eq!(state.selected, Some(7));
        assert!(state.coverage.is_none());
        // The previous project's comparison and landed flag must not linger.
        assert!(state.comparison.is_none());
        assert!(!state.landed);
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

    #[test]
    fn can_land_requires_a_translation_and_only_once() {
        let mut state = ImportState::browsing();
        assert!(!state.can_land()); // nothing translated yet
        state.select_project(7);
        state.set_coverage(report("framework", 0.5, &["AlphaModule"], &["CustomThing"]));
        assert!(state.can_land()); // partial coverage is still landable
        state.mark_landed();
        assert!(!state.can_land()); // never double-land
    }

    #[test]
    fn unavailable_comparison_shows_a_note_not_rows() {
        let mut state = ImportState::browsing();
        state.set_comparison(Comparison::Unavailable {
            reason: "Run a backtest on both sides to compare.".to_string(),
        });
        assert!(state.comparison_rows().is_empty());
        assert_eq!(
            state.comparison_note().as_deref(),
            Some("Run a backtest on both sides to compare.")
        );
    }

    #[test]
    fn populated_comparison_shows_rows_not_a_note() {
        let mut state = ImportState::browsing();
        state.set_comparison(Comparison::Rows(vec![DeltaMetric {
            label: "Sharpe".to_string(),
            quantconnect: "1.80".to_string(),
            auracle: "1.74".to_string(),
            delta: "-0.06".to_string(),
        }]));
        assert_eq!(state.comparison_rows().len(), 1);
        assert!(state.comparison_note().is_none());
    }
}
