//! Honest row-building + label decisions for the native Schedules panel.
//!
//! The engine lists deployed schedules as `(name, strategy_path, cron, enabled)`
//! entries. The panel's decision logic is mechanical: drop entries with no name,
//! show only the strategy's last dotted segment, and derive a liveness tone
//! (`Running` iff `enabled`) plus the pause/resume and delete button labels. The
//! tone is tied strictly to `enabled` — it is never inferred from anything else —
//! and the cron string is passed through verbatim, never reformatted.
//!
//! Centralising the toggle/delete copy here means the view holds no inline string
//! decision (it maps tone → `Color` and nothing more). Kept gpui-free so the
//! tone contract and the label copy are unit-tested without rendering. The panel
//! wraps the returned rows in `auracle_view_state::Load` at the call site; this
//! crate stays dependency-free.

/// Liveness tone of a schedule row, for the theme to colour at render time.
///
/// The view maps this (and only this) to a `Color`; the reducer never picks a
/// color literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleTone {
    Running,
    Paused,
}

/// One schedule row, derived from the engine's schedule entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleListItem {
    pub name: String,
    /// Last dotted segment of `strategy_path`, for a scannable row.
    pub strategy: String,
    /// Cron expression, verbatim.
    pub cron: String,
    pub enabled: bool,
    /// `Running` iff `enabled` — never inferred from anything else.
    pub tone: ScheduleTone,
}

/// Strategy tail (the last `.`-segment) — the same last-segment rule the
/// strategies navigator uses. Whole path if it has no dot; "" for "".
pub fn strategy_tail(strategy_path: &str) -> String {
    if strategy_path.is_empty() {
        return String::new();
    }
    strategy_path
        .rsplit('.')
        .next()
        .unwrap_or(strategy_path)
        .to_string()
}

/// Build the rows from raw `(name, strategy_path, cron, enabled)` entries.
///
/// - Entries with an empty name are dropped (matches the live fetch guard).
/// - Engine order is preserved (no sort).
/// - `tone` is `Running` iff `enabled`; cron is passed through verbatim.
///
/// Pure → unit-tested.
pub fn schedule_rows<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str, &'a str, bool)>,
) -> Vec<ScheduleListItem> {
    entries
        .into_iter()
        .filter(|(name, _strategy, _cron, _enabled)| !name.is_empty())
        .map(|(name, strategy_path, cron, enabled)| ScheduleListItem {
            name: name.to_string(),
            strategy: strategy_tail(strategy_path),
            cron: cron.to_string(),
            enabled,
            tone: if enabled {
                ScheduleTone::Running
            } else {
                ScheduleTone::Paused
            },
        })
        .collect()
}

/// Button label for the pause/resume toggle, from the row's `enabled` state.
/// An enabled (running) schedule offers "Pause"; a paused one offers "Resume".
pub fn toggle_label(enabled: bool) -> &'static str {
    if enabled { "Pause" } else { "Resume" }
}

/// Delete button label given whether the two-click confirm is armed. Centralises
/// the confirm copy so the view holds no inline string decision.
pub fn delete_label(armed: bool) -> &'static str {
    if armed { "Confirm delete" } else { "Delete" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_rows_derive_tail_drop_empty_name_preserve_order() {
        let rows = schedule_rows([
            (
                "nightly",
                "strategies.example_ma.MACrossover",
                "0 0 * * *",
                true,
            ),
            ("", "strategies.ignored.Ignored", "* * * * *", true),
            ("hourly", "strategies.mean.MeanRevert", "0 * * * *", false),
        ]);
        assert_eq!(rows.len(), 2);
        // Empty-name entry dropped; engine order preserved.
        assert_eq!(rows[0].name, "nightly");
        assert_eq!(rows[0].strategy, "MACrossover");
        assert_eq!(rows[0].cron, "0 0 * * *");
        assert_eq!(rows[1].name, "hourly");
        assert_eq!(rows[1].strategy, "MeanRevert");
    }

    #[test]
    fn tone_is_running_iff_enabled() {
        let rows = schedule_rows([
            ("a", "s.A", "* * * * *", true),
            ("b", "s.B", "* * * * *", false),
        ]);
        assert_eq!(rows[0].tone, ScheduleTone::Running);
        assert_eq!(rows[1].tone, ScheduleTone::Paused);
    }

    #[test]
    fn toggle_label_matches_enabled_state() {
        assert_eq!(toggle_label(true), "Pause");
        assert_eq!(toggle_label(false), "Resume");
    }

    #[test]
    fn delete_label_matches_armed_state() {
        assert_eq!(delete_label(true), "Confirm delete");
        assert_eq!(delete_label(false), "Delete");
    }

    #[test]
    fn strategy_tail_takes_last_segment() {
        assert_eq!(strategy_tail("strategies.x.MACrossover"), "MACrossover");
        assert_eq!(strategy_tail("solo"), "solo");
        assert_eq!(strategy_tail(""), "");
    }
}
